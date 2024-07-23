use crate::hw::HardwareConfigMessage::{IOLevelChanged, NewConfig, NewPinConfig};
use crate::hw::{BCMPinNumber, PinLevel};
use crate::hw::{LevelChange, PIGLET_ALPN};
use anyhow::Context;
use clap::{Arg, ArgMatches, Command};
use futures_lite::StreamExt;
use hw::config::HardwareConfig;
use hw::Hardware;
use iroh_net::endpoint::Connection;
use iroh_net::{key::SecretKey, relay::RelayMode, Endpoint};
use log::error;
use log::{info, trace};
use std::str::FromStr;
use std::{env, io};
use tracing::Level;
use tracing_subscriber::filter::{Directive, LevelFilter};
use tracing_subscriber::EnvFilter;

mod hw;

fn local_init() -> io::Result<impl Hardware> {
    let matches = get_matches();

    setup_logging(&matches);

    let mut hw = hw::get();
    info!("\n{}", hw.description().unwrap().details);
    trace!("Pin Descriptions:");
    for pin_description in hw.description().unwrap().pins.iter() {
        trace!("{pin_description}")
    }

    // Load any config file specified on the command line, or else the default
    let config = match matches.get_one::<String>("config-file") {
        Some(config_filename) => {
            let config = HardwareConfig::load(config_filename).unwrap();
            info!("Config loaded from file: {config_filename}");
            trace!("{config}");
            config
        }
        None => {
            info!("Default Config loaded");
            HardwareConfig::default()
        }
    };

    hw.apply_config(&config, input_level_changed)?;
    trace!("Configuration applied to hardware");

    Ok(hw)
}

/// Piglet will expose the same functionality from the GPIO Hardware Backend used by the GUI
/// in Piggy, but without any GUI or related dependencies, loading a config from file and
/// over the network.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let hw = local_init()?;
    listen(hw).await
}

/// Callback function that is called when an input changes level
async fn input_level_changed(bcm_pin_number: BCMPinNumber, level: PinLevel) {
    info!("Pin #{bcm_pin_number} changed level to '{level}'");
}

/// Setup logging with the requested verbosity level - or default if none specified
fn setup_logging(matches: &ArgMatches) {
    let default: Directive = LevelFilter::from_level(Level::ERROR).into();
    let verbosity_option = matches
        .get_one::<String>("verbosity")
        .and_then(|v| Directive::from_str(v).ok());
    let directive = verbosity_option.unwrap_or(default);
    let env_filter = EnvFilter::builder()
        .with_default_directive(directive)
        .from_env_lossy();
    tracing_subscriber::fmt().with_env_filter(env_filter).init();
}

/// Parse the command line arguments using clap
fn get_matches() -> ArgMatches {
    let app = Command::new(env!("CARGO_BIN_NAME")).version(env!("CARGO_PKG_VERSION"));

    let app = app.about(
        "'piglet' - for making Raspberry Pi GPIO hardware accessible remotely using 'piggui'",
    );

    let app = app.arg(
        Arg::new("verbosity")
            .short('v')
            .long("verbosity")
            .num_args(1)
            .number_of_values(1)
            .value_name("VERBOSITY_LEVEL")
            .help(
                "Set verbosity level for output (trace, debug, info, warn, error (default), off)",
            ),
    );

    let app = app.arg(
        Arg::new("config-file")
            .num_args(0..)
            .help("Path of a '.pigg' config file to load"),
    );

    app.get_matches()
}

/// Listen for an incoming iroh-net connection
async fn listen(mut hardware: impl Hardware) -> anyhow::Result<()> {
    let secret_key = SecretKey::generate();

    // Build a `Endpoint`, which uses PublicKeys as node identifiers, uses QUIC for directly connecting to other nodes, and uses the relay protocol and relay servers to holepunch direct connections between nodes when there are NATs or firewalls preventing direct connections. If no direct connection can be made, packets are relayed over the relay servers.
    let endpoint = Endpoint::builder()
        // The secret key is used to authenticate with other nodes. The PublicKey portion of this secret key is how we identify nodes, often referred to as the `node_id` in our codebase.
        .secret_key(secret_key)
        // set the ALPN protocols this endpoint will accept on incoming connections
        .alpns(vec![PIGLET_ALPN.to_vec()])
        // `RelayMode::Default` means that we will use the default relay servers to holepunch and relay.
        // Use `RelayMode::Custom` to pass in a `RelayMap` with custom relay urls.
        // Use `RelayMode::Disable` to disable holepunching and relaying over HTTPS
        // If you want to experiment with relaying using your own relay server, you must pass in the same custom relay url to both the `listen` code AND the `connect` code
        .relay_mode(RelayMode::Default)
        // you can choose a port to bind to, but passing in `0` will bind the socket to a random available port
        .bind(0)
        .await?;

    let me = endpoint.node_id();
    info!("node id: {me}");

    let local_addrs = endpoint
        .direct_addresses()
        .next()
        .await
        .context("no endpoints")?
        .into_iter()
        .map(|endpoint| endpoint.addr.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    info!("local Addresses: {local_addrs}");

    let relay_url = endpoint
        .home_relay()
        .expect("should be connected to a relay server, try calling `endpoint.local_endpoints()` or `endpoint.connect()` first, to ensure the endpoint has actually attempted a connection before checking for the connected relay server");
    info!("node relay server url: {relay_url}");

    // accept incoming connections, returns a normal QUIC connection
    if let Some(connecting) = endpoint.accept().await {
        let connection = connecting.await?;
        let node_id = iroh_net::endpoint::get_remote_node_id(&connection)?;
        info!("new connection from {node_id}",);

        let mut gui_sender = connection.open_uni().await?;

        trace!("Sending hardware description");
        let desc = hardware.description()?;
        let message = serde_json::to_string(&desc)?;
        gui_sender.write_all(message.as_bytes()).await?;
        gui_sender.finish().await?;

        loop {
            // accept a bidirectional QUIC connection - use the `quinn` APIs to send and recv content
            trace!("waiting for a bi-di connection");
            let mut config_receiver = connection.accept_uni().await?;
            let connection_clone = connection.clone();
            trace!("Connected, waiting for message");
            let message = config_receiver.read_to_end(4096).await?;

            if !message.is_empty() {
                let content = String::from_utf8_lossy(&message);
                match serde_json::from_str(&content) {
                    Ok(NewConfig(config)) => {
                        info!("New config applied");
                        let _ = hardware.apply_config(&config, move |bcm, level| {
                            let cc = connection_clone.clone();
                            async move {
                                let _ = send_input_change(cc, bcm, level).await;
                            }
                        });
                    }
                    Ok(NewPinConfig(bcm, pin_function)) => {
                        info!("New pin config for pin #{bcm}: {pin_function}");
                        let _ = hardware.apply_pin_config(bcm, &pin_function, move |bcm, level| {
                            let cc = connection_clone.clone();
                            async move {
                                let _ = send_input_change(cc, bcm, level).await;
                            }
                        });
                    }
                    Ok(IOLevelChanged(bcm, level_change)) => {
                        info!("Pin #{bcm} Output level change: {level_change:?}");
                        let _ = hardware.set_output_level(bcm, level_change.new_level);
                    }
                    _ => error!("Unknown message: {content}"),
                }
            }
        }
    }

    Ok(())
}

/// Send a detected input level change back to the GUI using the `gui_sender` [SendStream]
async fn send_input_change(
    connection: Connection,
    bcm: BCMPinNumber,
    level: PinLevel,
) -> anyhow::Result<()> {
    let level_change = LevelChange::new(level);
    info!("Pin #{bcm} Input level change: {level_change:?}");
    let hardware_event = IOLevelChanged(bcm, level_change);
    let message = serde_json::to_string(&hardware_event)?;
    let mut gui_sender = connection.open_uni().await?;
    gui_sender.write_all(message.as_bytes()).await?;
    Ok(gui_sender.finish().await?)
}
