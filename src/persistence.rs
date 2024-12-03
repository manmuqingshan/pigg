use crate::hw_definition::config::HardwareConfig;
#[cfg(any(feature = "iroh", feature = "tcp"))]
use crate::ListenerInfo;
use clap::ArgMatches;
use log::{info, trace};
#[cfg(any(feature = "iroh", feature = "tcp"))]
use std::io::Write;
use std::path::Path;

/// Get the initial [HardwareConfig] determined following:
/// - A config file specified on the command line, or
/// - A config file saved from a previous run
/// - The default (empty) config
pub(crate) async fn get_config(matches: &ArgMatches, exec_path: &Path) -> HardwareConfig {
    // A config file specified on the command line overrides any config file from previous run
    let config_file = matches.get_one::<String>("config-file");

    // Load any config file specified on the command line
    match config_file {
        Some(config_filename) => match HardwareConfig::load(config_filename) {
            Ok(config) => {
                info!("Config loaded from file: {config_filename}");
                trace!("{config}");
                config
            }
            Err(_) => HardwareConfig::default(),
        },
        None => {
            // look for config file from last run
            let last_run_filename = exec_path.join(".piglet_config.json");
            match HardwareConfig::load(&last_run_filename.to_string_lossy()) {
                Ok(config) => {
                    info!("Config loaded from file: {}", last_run_filename.display());
                    trace!("{config}");
                    config
                }
                Err(_) => HardwareConfig::default(),
            }
        }
    }
}
#[cfg(any(feature = "iroh", feature = "tcp"))]
/// Persist the current config in file, for picking up at restart
pub(crate) async fn store_config(exec_path: &Path, config: &HardwareConfig) -> anyhow::Result<()> {
    let last_run_filename = exec_path.join(".piglet_config.json");
    config.save(&last_run_filename.to_string_lossy())?;
    Ok(())
}

#[cfg(any(feature = "iroh", feature = "tcp"))]
/// Write a [ListenerInfo] file that captures information that can be used to connect to piglet
pub(crate) fn write_info_file(
    info_path: &Path,
    listener_info: &ListenerInfo,
) -> anyhow::Result<()> {
    let mut output = std::fs::File::create(info_path)?;
    write!(output, "{}", serde_json::to_string(listener_info)?)?;
    info!("Info file written at: {info_path:?}");
    Ok(())
}

#[cfg(feature = "iroh")]
#[cfg(test)]
mod test {
    use crate::device_net::PIGLET_ALPN;
    use crate::{persistence, ListenerInfo};
    #[cfg(feature = "tcp")]
    use async_std::net::TcpListener;
    use futures::executor::block_on;
    use iroh_net::key::SecretKey;
    use iroh_net::relay::RelayMode;
    use iroh_net::Endpoint;
    use std::fs;
    use std::path::PathBuf;
    use std::str::FromStr;
    use tempfile::tempdir;

    #[cfg(feature = "iroh")]
    async fn test_iroh_endpoint() -> Endpoint {
        let secret_key = SecretKey::generate();
        let builder = Endpoint::builder()
            .secret_key(secret_key)
            .alpns(vec![PIGLET_ALPN.to_vec()])
            .relay_mode(RelayMode::Default);

        builder.bind().await.expect("Could not bind")
    }

    #[cfg(feature = "tcp")]
    async fn test_tcp_listener(address: &str) -> TcpListener {
        TcpListener::bind(&address).await.expect("Could not bind")
    }

    #[cfg(feature = "iroh")]
    #[test]
    fn write_info_file() {
        let output_dir = tempdir().expect("Could not create a tempdir").into_path();
        let test_file = output_dir.join("test.info");
        let nodeid =
            iroh_net::NodeId::from_str("rxci3kuuxljxqej7hau727aaemcjo43zvf2zefnqla4p436sqwhq")
                .expect("Could not create nodeid");
        let local_addrs = "79.154.163.213:58604 192.168.1.77:58604";
        let relay_url = iroh_net::relay::RelayUrl::from_str("https://euw1-1.relay.iroh.network./ ")
            .expect("Could not create Relay URL");

        let info = ListenerInfo {
            iroh_device: crate::iroh_device::IrohDevice {
                nodeid,
                local_addrs: local_addrs.to_string(),
                relay_url,
                alpn: "".to_string(),
                endpoint: block_on(test_iroh_endpoint()),
            },
            #[cfg(feature = "tcp")]
            tcp_device: crate::tcp_device::TcpDevice {
                ip: std::net::IpAddr::from_str("10.0.0.0").expect("Could not parse IpAddr"),
                port: 9001,
                listener: block_on(test_tcp_listener("10.0.0.0:9001")),
            },
        };

        persistence::write_info_file(&test_file, &info).expect("Writing info file failed");
        assert!(test_file.exists(), "File was not created as expected");
        let piglet_info = fs::read_to_string(test_file).expect("Could not read info file");
        assert!(piglet_info.contains(&nodeid.to_string()));
    }

    #[cfg(feature = "iroh")]
    #[test]
    fn write_info_file_non_existent() {
        let output_dir = PathBuf::from("/foo");
        let test_file = output_dir.join("test.info");
        let nodeid =
            iroh_net::NodeId::from_str("rxci3kuuxljxqej7hau727aaemcjo43zvf2zefnqla4p436sqwhq")
                .expect("Could not create nodeid");
        let local_addrs = "79.154.163.213:58604 192.168.1.77:58604";
        let relay_url = iroh_net::relay::RelayUrl::from_str("https://euw1-1.relay.iroh.network./ ")
            .expect("Could not create Relay URL");
        let info = ListenerInfo {
            iroh_device: crate::iroh_device::IrohDevice {
                nodeid,
                local_addrs: local_addrs.to_string(),
                relay_url,
                alpn: "".to_string(),
                endpoint: block_on(test_iroh_endpoint()),
            },
            #[cfg(feature = "tcp")]
            tcp_device: crate::tcp_device::TcpDevice {
                ip: std::net::IpAddr::from_str("10.0.0.0").expect("Could not parse IpAddr"),
                port: 9001,
                listener: block_on(test_tcp_listener("10.0.0.0:9001")),
            },
        };

        assert!(persistence::write_info_file(&test_file, &info).is_err());
        assert!(!test_file.exists(), "File was created!");
    }
}
