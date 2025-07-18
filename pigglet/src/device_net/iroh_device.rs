use crate::config;
use anyhow::{anyhow, bail, Context};
use iroh::endpoint::Connection;
use iroh::Watcher;
use iroh::{Endpoint, NodeId, RelayMode, RelayUrl, SecretKey};
use log::{debug, info, trace};
use pigdef::config::HardwareConfig;
use pigdef::config::HardwareConfigMessage::{IOLevelChanged, NewConfig, NewPinConfig};
use pigdef::config::{HardwareConfigMessage, LevelChange};
use pigdef::description::BCMPinNumber;
use pigdef::description::HardwareDescription;
use pigdef::net_values::PIGGLET_ALPN;
use pigdef::pin_function::PinFunction;
use pigdef::pin_function::PinFunction::Output;
use piggpio::HW;
use rand_core::OsRng;
use std::fmt;
use std::fmt::{Display, Formatter};
use std::path::Path;
use std::str::{FromStr, Lines};

pub struct IrohDevice {
    pub nodeid: NodeId,
    pub relay_url: RelayUrl,
    pub endpoint: Option<Endpoint>,
}

impl IrohDevice {
    /// Don't fail parsing on lack of endpoint data
    pub fn parse(lines: &mut Lines) -> anyhow::Result<Self> {
        let nodeid = lines.next().ok_or_else(|| anyhow!("Missing nodeid"))?;
        let relay = lines.next().ok_or_else(|| anyhow!("Missing relayUrl"))?;
        let _endpoint = lines.next();

        Ok(IrohDevice {
            nodeid: NodeId::from_str(nodeid)?,
            relay_url: RelayUrl::from_str(relay)?,
            endpoint: None, // TODO
        })
    }
}
impl Display for IrohDevice {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "nodeid:{}", self.nodeid)?;
        writeln!(f, "relay URL:{}", self.relay_url)?;
        writeln!(f, "Endpoint:{:?}", self.endpoint)?;
        Ok(())
    }
}

pub async fn get_device() -> anyhow::Result<IrohDevice> {
    let secret_key = SecretKey::generate(OsRng);

    // Build an `Endpoint`, which uses PublicKeys as node identifiers, that uses QUIC for directly
    // connecting to other nodes, and uses the relay protocol and relay servers to holepunch direct
    // connections between nodes when there are NATs or firewalls preventing direct connections.
    // If no direct connection can be made, packets are relayed over the relay servers.
    #[allow(unused_mut)]
    let mut builder = Endpoint::builder()
        // The secret key is used to authenticate with other nodes.
        // The PublicKey portion of this secret key is how we identify nodes, often referred
        // to as the `node_id` in our codebase.
        .secret_key(secret_key)
        // set the ALPN protocols this endpoint will accept on incoming connections
        .alpns(vec![PIGGLET_ALPN.to_vec()])
        // `RelayMode::Default` means that we will use the default relay servers to holepunch and relay.
        // Use `RelayMode::Custom` to pass in a `RelayMap` with custom relay urls.
        // Use `RelayMode::Disable` to disable holepunching and relaying over HTTPS
        // If you want to experiment with relaying using your own relay server,
        // you must pass in the same custom relay url to both the `listen` code AND the `connect` code
        .relay_mode(RelayMode::Default);

    let endpoint = builder.bind().await?;

    let nodeid = endpoint.node_id();
    println!("nodeid: {nodeid}"); // Don't remove - required by integration tests

    let local_addrs = endpoint
        .direct_addresses()
        .initialized()
        .await
        .context("no endpoints")?
        .into_iter()
        .map(|endpoint| endpoint.addr.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    info!("local Addresses: {local_addrs}");

    let relay_url = endpoint.home_relay().initialized().await?;
    println!("Relay URL: {relay_url}"); // Don't remove - required by integration tests

    Ok(IrohDevice {
        nodeid,
        relay_url,
        endpoint: Some(endpoint),
    })
}

/// accept incoming connections, returns a normal QUIC connection
pub async fn accept_connection(
    endpoint: &Endpoint,
    desc: &HardwareDescription,
    hardware_config: HardwareConfig,
) -> anyhow::Result<Connection> {
    debug!("Waiting for connection");
    if let Some(connecting) = endpoint.accept().await {
        let connection = connecting.await?;
        let node_id = Connection::remote_node_id(&connection)?;
        debug!("New connection from nodeid: '{node_id}'",);
        trace!("Sending hardware description");
        let mut gui_sender = connection.open_uni().await?;
        let message = postcard::to_allocvec(&(&desc, hardware_config))?;
        gui_sender.write_all(&message).await?;
        gui_sender.finish()?;
        Ok(connection)
    } else {
        bail!("Could not connect to iroh")
    }
}

/// Process incoming config change messages from the GUI.
/// On the end of the stream exit the loop
pub async fn iroh_message_loop(
    connection: Connection,
    hardware_config: &mut HardwareConfig,
    exec_path: &Path,
    hardware: &mut HW,
) -> anyhow::Result<()> {
    loop {
        let mut config_receiver = connection.accept_uni().await?;
        info!("Waiting for message");
        let payload = config_receiver.read_to_end(4096).await?;

        if payload.is_empty() {
            bail!("End of message stream");
        }

        if let Ok(config_message) = postcard::from_bytes(&payload) {
            if apply_config_change(
                hardware,
                config_message,
                hardware_config,
                connection.clone(),
            )
            .await
            .is_ok()
            {
                let _ = config::store_config(hardware_config, exec_path).await;
            }
        }
    }
}

/// Apply a config change to the hardware
/// NOTE: Initially the callback to Config/PinConfig change was async, and that compiles and runs
/// but wasn't working - so this uses a sync callback again to fix that, and an async version of
/// send_input_level() for use directly from the async context
async fn apply_config_change(
    hardware: &mut HW,
    config_change: HardwareConfigMessage,
    hardware_config: &mut HardwareConfig,
    connection: Connection,
) -> anyhow::Result<()> {
    match config_change {
        NewConfig(config) => {
            info!("New config applied");
            let cc = connection.clone();
            hardware
                .apply_config(&config, move |bcm, level_change| {
                    let _ = send_input_level_sync(connection.clone(), bcm, level_change);
                })
                .await?;

            send_current_input_levels(cc, &config, hardware).await?;
            // replace the entire config with the new one
            *hardware_config = config;
        }
        NewPinConfig(bcm, pin_function) => {
            info!("New pin config for pin #{bcm}: {pin_function:?}");
            let cc = connection.clone();
            hardware
                .apply_pin_config(bcm, &pin_function, move |bcm, level| {
                    let _ = send_input_level_sync(connection.clone(), bcm, level);
                })
                .await?;

            if let Some(function) = pin_function {
                send_current_input_level(&bcm, &function, cc, hardware).await?;
                // add/replace the new pin config to the hardware config
                hardware_config.pin_functions.insert(bcm, function);
            } else {
                hardware_config.pin_functions.remove(&bcm);
            }
        }
        IOLevelChanged(bcm, level_change) => {
            trace!("Pin #{bcm} Output level change: {level_change:?}");
            hardware.set_output_level(bcm, level_change.new_level)?;
            // add/replace the new pin config to the hardware config
            hardware_config
                .pin_functions
                .insert(bcm, Output(Some(level_change.new_level)));
        }
        HardwareConfigMessage::GetConfig => {
            let message = postcard::to_allocvec(&NewConfig(hardware_config.clone()))?;
            send(connection, &message).await?
        }
        HardwareConfigMessage::Disconnect => return Err(anyhow!("Disconnect message received")),
    }

    Ok(())
}

/// Send the current input level for all configured inputs
async fn send_current_input_levels(
    connection: Connection,
    config: &HardwareConfig,
    hardware: &HW,
) -> anyhow::Result<()> {
    for (bcm_pin_number, pin_function) in &config.pin_functions {
        send_current_input_level(bcm_pin_number, pin_function, connection.clone(), hardware)
            .await?;
    }

    Ok(())
}

/// Send the current input level for one input - with a timestamp that will match with future
/// LevelChange timestamps (time since boot)
async fn send_current_input_level(
    bcm_pin_number: &BCMPinNumber,
    pin_function: &PinFunction,
    connection: Connection,
    hardware: &HW,
) -> anyhow::Result<()> {
    let now = hardware.get_time_since_boot();

    // Send initial levels
    if let PinFunction::Input(_pullup) = pin_function {
        if let Ok(initial_level) = hardware.get_input_level(*bcm_pin_number) {
            let level_change = LevelChange::new(initial_level, now);
            trace!("Pin #{bcm_pin_number} Input level change: {level_change:?}");
            let hardware_event = IOLevelChanged(*bcm_pin_number, level_change);
            let message = postcard::to_allocvec(&hardware_event)?;
            send(connection.clone(), &message).await?;
        }
    }

    Ok(())
}

/// Send a detected input level change back to the GUI using `connection` [Connection],
/// timestamping with the current time in Utc
fn send_input_level_sync(
    connection: Connection,
    bcm: BCMPinNumber,
    level_change: LevelChange,
) -> anyhow::Result<()> {
    trace!("Pin #{bcm} Input level change: {level_change:?}");
    let hardware_event = IOLevelChanged(bcm, level_change);
    let message = postcard::to_allocvec(&hardware_event)?;
    // TODO avoid recreating every time?
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(send(connection, &message))
}

/// Send a message to the GUI using `connection` [Connection]
async fn send(connection: Connection, message: &[u8]) -> anyhow::Result<()> {
    let mut gui_sender = connection.open_uni().await?;
    gui_sender.write_all(message).await?;
    gui_sender.finish()?;
    Ok(())
}
