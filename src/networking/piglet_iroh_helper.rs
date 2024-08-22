use crate::hw::config::HardwareConfig;
use crate::hw::{Hardware, PIGLET_ALPN};
use anyhow::{bail, Context};
use futures::StreamExt;
use iroh_net::{Endpoint, NodeId};
use log::{debug, info, trace};
use std::fmt;
use std::fmt::{Display, Formatter};

use crate::hw_definition::{
    config_message::BCMPinNumber, config_message::PinLevel, pin_function::PinFunction,
};

use crate::hw_definition::config_message::HardwareConfigMessage::{
    IOLevelChanged, NewConfig, NewPinConfig,
};
use crate::hw_definition::config_message::{HardwareConfigMessage, LevelChange};
use crate::hw_definition::hardware_description::HardwareDescription;
use iroh_net::endpoint::Connection;
use iroh_net::key::SecretKey;
use iroh_net::relay::{RelayMode, RelayUrl};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(crate) struct IrohInfo {
    pub nodeid: NodeId,
    pub local_addrs: String,
    pub relay_url: RelayUrl,
    pub alpn: String,
    #[serde(skip)]
    pub endpoint: Option<Endpoint>,
}

impl Display for IrohInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "nodeid: {}", self.nodeid)?;
        writeln!(f, "relay URL: {}", self.relay_url)?;
        Ok(())
    }
}

pub async fn get_iroh_listener_info() -> anyhow::Result<IrohInfo> {
    let secret_key = SecretKey::generate();

    // Build a `Endpoint`, which uses PublicKeys as node identifiers, uses QUIC for directly
    // connecting to other nodes, and uses the relay protocol and relay servers to holepunch direct
    // connections between nodes when there are NATs or firewalls preventing direct connections.
    // If no direct connection can be made, packets are relayed over the relay servers.
    let endpoint = Endpoint::builder()
        // The secret key is used to authenticate with other nodes.
        // The PublicKey portion of this secret key is how we identify nodes, often referred
        // to as the `node_id` in our codebase.
        .secret_key(secret_key)
        // set the ALPN protocols this endpoint will accept on incoming connections
        .alpns(vec![PIGLET_ALPN.to_vec()])
        // `RelayMode::Default` means that we will use the default relay servers to holepunch and relay.
        // Use `RelayMode::Custom` to pass in a `RelayMap` with custom relay urls.
        // Use `RelayMode::Disable` to disable holepunching and relaying over HTTPS
        // If you want to experiment with relaying using your own relay server,
        // you must pass in the same custom relay url to both the `listen` code AND the `connect` code
        .relay_mode(RelayMode::Default)
        // pass in `0` to bind the socket to a random available port
        .bind(0)
        .await?;

    let nodeid = endpoint.node_id();
    println!("nodeid: {nodeid}");

    let local_addrs = endpoint
        .direct_addresses()
        .next()
        .await
        .context("no endpoints")?
        .into_iter()
        .map(|endpoint| endpoint.addr.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    println!("local Addresses: {local_addrs}");

    let relay_url = endpoint
            .home_relay()
            .expect("should be connected to a relay server, try calling `endpoint.local_endpoints()` or `endpoint.connect()` first, to ensure the endpoint has actually attempted a connection before checking for the connected relay server");
    println!("Relay URL: {relay_url}");

    Ok(IrohInfo {
        nodeid,
        local_addrs,
        relay_url,
        alpn: String::from_utf8_lossy(PIGLET_ALPN).parse()?,
        endpoint: Some(endpoint),
    })
}

/// accept incoming connections, returns a normal QUIC connection
pub async fn iroh_accept(
    endpoint: &Endpoint,
    desc: &HardwareDescription,
) -> anyhow::Result<Connection> {
    debug!("Waiting for connection");
    if let Some(connecting) = endpoint.accept().await {
        let connection = connecting.await?;
        let node_id = iroh_net::endpoint::get_remote_node_id(&connection)?;
        debug!("New connection from nodeid: '{node_id}'",);
        trace!("Sending hardware description");
        let mut gui_sender = connection.open_uni().await?;
        let message = serde_json::to_string(&desc)?;
        gui_sender.write_all(message.as_bytes()).await?;
        gui_sender.finish().await?;
        Ok(connection)
    } else {
        bail!("Could not connect to iroh")
    }
}

/// Process incoming config change messages from the GUI. On end of stream exit the loop
pub async fn iroh_message_loop(
    connection: Connection,
    hardware: &mut impl Hardware,
) -> anyhow::Result<()> {
    loop {
        let mut config_receiver = connection.accept_uni().await?;
        trace!("Receiving config message");
        let payload = config_receiver.read_to_end(4096).await?;

        if payload.is_empty() {
            bail!("End of message stream");
        }

        let config_message = serde_json::from_slice(&payload)?;
        apply_config_change(hardware, config_message, connection.clone()).await?;
    }
}

/// Apply a config change to the hardware
/// NOTE: Initially the callback to Config/PinConfig change was async, and that compiles and runs
/// but wasn't working - so this uses a sync callback again to fix that, and an async version of
/// send_input_level() for use directly from the async context
pub async fn apply_config_change(
    hardware: &mut impl Hardware,
    config_change: HardwareConfigMessage,
    connection: Connection,
) -> anyhow::Result<()> {
    match config_change {
        NewConfig(config) => {
            let cc = connection.clone();
            info!("New config applied");
            hardware.apply_config(&config, move |bcm, level| {
                let cc = connection.clone();
                let _ = send_input_level(cc, bcm, level);
            })?;

            send_current_input_states(cc, &config, hardware).await?;
        }
        NewPinConfig(bcm, pin_function) => {
            info!("New pin config for pin #{bcm}: {pin_function}");
            hardware.apply_pin_config(bcm, &pin_function, move |bcm, level| {
                let cc = connection.clone();
                let _ = send_input_level(cc, bcm, level);
            })?;
        }
        IOLevelChanged(bcm, level_change) => {
            trace!("Pin #{bcm} Output level change: {level_change:?}");
            hardware.set_output_level(bcm, level_change.new_level)?;
        }
    }

    Ok(())
}

/// Send the current input state for all inputs configured in the config
pub async fn send_current_input_states(
    connection: Connection,
    config: &HardwareConfig,
    hardware: &impl Hardware,
) -> anyhow::Result<()> {
    // Send initial levels
    for (bcm_pin_number, pin_function) in &config.pins {
        if let PinFunction::Input(_pullup) = pin_function {
            // Update UI with initial state
            if let Ok(initial_level) = hardware.get_input_level(*bcm_pin_number) {
                let _ = send_input_level_async(connection.clone(), *bcm_pin_number, initial_level)
                    .await;
            }
        }
    }

    Ok(())
}

/// Send a detected input level change back to the GUI using `connection` [Connection],
/// timestamping with the current time in Utc
async fn send_input_level_async(
    connection: Connection,
    bcm: BCMPinNumber,
    level: PinLevel,
) -> anyhow::Result<()> {
    let level_change = LevelChange::new(level);
    trace!("Pin #{bcm} Input level change: {level_change:?}");
    let hardware_event = IOLevelChanged(bcm, level_change);
    let message = serde_json::to_string(&hardware_event)?;
    send(connection, message).await
}

/// Send a detected input level change back to the GUI using `connection` [Connection],
/// timestamping with the current time in Utc
// TODO they are looking for testers of async closures! This is the place!
fn send_input_level(
    connection: Connection,
    bcm: BCMPinNumber,
    level: PinLevel,
) -> anyhow::Result<()> {
    let level_change = LevelChange::new(level);
    trace!("Pin #{bcm} Input level change: {level_change:?}");
    let hardware_event = IOLevelChanged(bcm, level_change);
    let message = serde_json::to_string(&hardware_event)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(send(connection, message))
}

async fn send(connection: Connection, message: String) -> anyhow::Result<()> {
    let mut gui_sender = connection.open_uni().await?;
    gui_sender.write_all(message.as_bytes()).await?;
    gui_sender.finish().await?;
    Ok(())
}
