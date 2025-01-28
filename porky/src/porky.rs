#![no_std]
#![no_main]

use crate::flash::DbFlash;
use crate::gpio::Gpio;
use crate::hw_definition::config::HardwareConfig;
use crate::hw_definition::config::HardwareConfigMessage;
use crate::hw_definition::description::{HardwareDescription, HardwareDetails, PinDescriptionSet};
#[cfg(feature = "wifi")]
use crate::hw_definition::description::{TCP_MDNS_SERVICE_NAME, TCP_MDNS_SERVICE_PROTOCOL};
use crate::pin_descriptions::PIN_DESCRIPTIONS;
#[cfg(feature = "wifi")]
use crate::tcp::TCP_PORT;
use core::str;
#[cfg(feature = "wifi")]
use cyw43::Control;
#[cfg(feature = "wifi")]
use cyw43_pio::{PioSpi, DEFAULT_CLOCK_DIVIDER};
use defmt::error;
#[cfg(any(feature = "pico2", feature = "wifi"))]
use defmt::info;
use defmt_rtt as _;
use ekv::Database;
use embassy_executor::Spawner;
#[cfg(all(feature = "usb", feature = "wifi"))]
use embassy_futures::select::{select, Either};
use embassy_rp::bind_interrupts;
use embassy_rp::flash::{Blocking, Flash};
#[cfg(feature = "wifi")]
use embassy_rp::gpio::{Level, Output};
#[cfg(feature = "usb")]
use embassy_rp::peripherals::USB;
use embassy_rp::peripherals::{FLASH, PIO0};
use embassy_rp::pio::InterruptHandler as PioInterruptHandler;
#[cfg(feature = "wifi")]
use embassy_rp::pio::Pio;
#[cfg(feature = "usb")]
use embassy_rp::usb::Driver;
#[cfg(feature = "usb")]
use embassy_rp::usb::InterruptHandler as USBInterruptHandler;
use embassy_rp::watchdog::Watchdog;
use embassy_sync::blocking_mutex::raw::{NoopRawMutex, ThreadModeRawMutex};
use embassy_sync::channel::Channel;
use panic_probe as _;
use static_cell::StaticCell;

#[cfg(not(any(feature = "usb", feature = "wifi")))]
compile_error!("You must chose a feature from [\"usb\", \"wifi\"] in order to control 'porky'");

#[cfg(feature = "wifi")]
/// The ssid config generated by build.rs in "$OUT_DIR/ssid.rs"
mod ssid {
    include!(concat!(env!("OUT_DIR"), "/ssid.rs"));
}

#[cfg(feature = "wifi")]
/// Wi-Fi related functions
mod wifi;

#[cfg(feature = "usb")]
mod usb;

#[cfg(feature = "wifi")]
/// TCP related functions
mod tcp;

/// GPIO control related functions
mod gpio;
mod gpio_input_monitor;

/// Definition of hardware structs passed back and fore between porky and the GUI
#[path = "../../src/hw_definition/mod.rs"]
mod hw_definition;

/// Functions for interacting with the Flash ROM
mod flash;

/// Persistence layer built on top of flash
mod persistence;

#[cfg(all(feature = "discovery", feature = "wifi"))]
/// Discovery via mDNS
mod mdns;

/// The Pi Pico GPIO [PinDefinition]s that get passed to the GUI
mod pin_descriptions;

#[cfg(feature = "usb")]
bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
    USBCTRL_IRQ => USBInterruptHandler<USB>;
});

#[cfg(not(feature = "usb"))]
bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
});

pub static HARDWARE_EVENT_CHANNEL: Channel<ThreadModeRawMutex, HardwareConfigMessage, 1> =
    Channel::new();

/// Create a [HardwareDescription] for this device with the provided serial number
fn hardware_description(serial: &str) -> HardwareDescription {
    let details = HardwareDetails {
        #[cfg(all(feature = "pico1", not(feature = "wifi")))]
        model: "Pi Pico",
        #[cfg(all(feature = "pico1", feature = "wifi"))]
        model: "Pi Pico W",
        #[cfg(all(feature = "pico2", not(feature = "wifi")))]
        model: "Pi Pico2",
        #[cfg(all(feature = "pico2", feature = "wifi"))]
        model: "Pi Pico2 W",
        #[cfg(feature = "pico1")]
        hardware: "RP2040",
        #[cfg(feature = "pico2")]
        hardware: "RP235XA",
        revision: "",
        serial,
        wifi: cfg!(feature = "wifi"),
        app_name: env!("CARGO_BIN_NAME"),
        app_version: env!("CARGO_PKG_VERSION"),
    };

    HardwareDescription {
        details,
        pins: PinDescriptionSet::new(&PIN_DESCRIPTIONS),
    }
}

#[cfg(feature = "pico2")]
/// Get the unique serial number from Chip OTP
pub fn serial_number() -> &'static str {
    let device_id = embassy_rp::otp::get_chipid().unwrap();
    let device_id_bytes = device_id.to_ne_bytes();

    // convert the device_id to a 16 char hex "string"
    let mut device_id_hex: [u8; 16] = [0; 16];
    faster_hex::hex_encode(&device_id_bytes, &mut device_id_hex).unwrap();

    static ID: StaticCell<[u8; 16]> = StaticCell::new();
    let id = ID.init(device_id_hex);
    let device_id_str = str::from_utf8(id).unwrap();
    info!("device_id: {}", device_id_str);
    device_id_str
}

#[cfg(feature = "usb")]
#[allow(clippy::too_many_arguments)]
async fn usb_only(
    spawner: Spawner,
    driver: Driver<'static, USB>,
    mut gpio: Gpio,
    hw_desc: &'static HardwareDescription<'_>,
    mut hardware_config: HardwareConfig,
    db: &'static Database<
        DbFlash<Flash<'static, FLASH, Blocking, { flash::FLASH_SIZE }>>,
        NoopRawMutex,
    >,
    watchdog: Watchdog,
    #[cfg(feature = "wifi")] mut control: Control<'_>,
) {
    let mut usb_connection = usb::start(spawner, driver, hw_desc, None, db, watchdog).await;

    loop {
        if usb::wait_connection(&mut usb_connection, &hardware_config)
            .await
            .is_err()
        {
            error!("Could not establish USB connection");
        } else {
            let _ = usb::message_loop(
                &mut gpio,
                &mut usb_connection,
                &mut hardware_config,
                &spawner,
                #[cfg(feature = "wifi")]
                &mut control,
                db,
            )
            .await;
        }
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // Get the RPi Pico Peripherals - a number of the PINS are available for GPIO (they are
    // passed to AvailablePins) while others are reserved for internal use and not available for
    // GPIO
    let peripherals = embassy_rp::init(Default::default());

    #[cfg(feature = "wifi")]
    // PIN_25 - OP wireless SPI CS - when high also enables GPIO29 ADC pin to read VSYS
    let cs = Output::new(peripherals.PIN_25, Level::High);
    #[cfg(feature = "wifi")]
    let mut pio = Pio::new(peripherals.PIO0, Irqs);

    #[cfg(feature = "wifi")]
    // Initialize the cyw43 and start the network
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        DEFAULT_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        // PIN_24 - OP/IP wireless SPI data/IRQ
        peripherals.PIN_24,
        // PIN_29 - OP/IP wireless SPI CLK/ADC mode (ADC3) to measure VSYS/3
        peripherals.PIN_29,
        peripherals.DMA_CH0,
    );

    #[cfg(feature = "wifi")]
    // PIN_23 - OP wireless power on signal
    let (mut control, wifi_stack) = wifi::start_net(spawner, peripherals.PIN_23, spi).await;

    let mut gpio = Gpio::new(
        peripherals.PIN_0,
        peripherals.PIN_1,
        peripherals.PIN_2,
        peripherals.PIN_3,
        peripherals.PIN_4,
        peripherals.PIN_5,
        peripherals.PIN_6,
        peripherals.PIN_7,
        peripherals.PIN_8,
        peripherals.PIN_9,
        peripherals.PIN_10,
        peripherals.PIN_11,
        peripherals.PIN_12,
        peripherals.PIN_13,
        peripherals.PIN_14,
        peripherals.PIN_15,
        peripherals.PIN_16,
        peripherals.PIN_17,
        peripherals.PIN_18,
        peripherals.PIN_19,
        peripherals.PIN_20,
        peripherals.PIN_21,
        peripherals.PIN_22,
        peripherals.PIN_26,
        peripherals.PIN_27,
        peripherals.PIN_28,
    );

    // create hardware description
    #[allow(unused_mut)]
    let mut flash = flash::get_flash(peripherals.FLASH);
    #[cfg(feature = "pico1")]
    let serial_number = flash::serial_number(&mut flash);
    #[cfg(feature = "pico2")]
    let serial_number = serial_number();
    static HARDWARE_DESCRIPTION: StaticCell<HardwareDescription> = StaticCell::new();
    let hw_desc = HARDWARE_DESCRIPTION.init(hardware_description(serial_number));

    #[cfg(feature = "usb")]
    let driver = Driver::new(peripherals.USB, Irqs);

    // start the flash database
    static DATABASE: StaticCell<
        Database<DbFlash<Flash<'static, FLASH, Blocking, { flash::FLASH_SIZE }>>, NoopRawMutex>,
    > = StaticCell::new();
    let db = DATABASE.init(flash::db_init(flash).await);

    #[cfg(feature = "wifi")]
    static STATIC_BUF: StaticCell<[u8; 200]> = StaticCell::new();
    #[cfg(feature = "wifi")]
    let static_buf = STATIC_BUF.init([0u8; 200]);

    #[cfg(feature = "usb")]
    let watchdog = Watchdog::new(peripherals.WATCHDOG);

    // Load initial config from flash
    let mut hardware_config = persistence::get_config(db).await;

    // apply the loaded config to the hardware immediately
    gpio.apply_config_change(
        #[cfg(feature = "wifi")]
        &mut control,
        &spawner,
        &HardwareConfigMessage::NewConfig(hardware_config.clone()),
        &mut hardware_config,
    )
    .await;

    #[cfg(all(not(feature = "wifi"), feature = "usb"))]
    usb_only(
        spawner,
        driver,
        gpio,
        hw_desc,
        hardware_config,
        db,
        watchdog,
    )
    .await;

    // If we have a valid SsidSpec, then try and join that network using it
    #[cfg(feature = "wifi")]
    match persistence::get_ssid_spec(db, static_buf).await {
        Some(ssid) => match wifi::join(&mut control, wifi_stack, &ssid).await {
            Ok(ip) => {
                info!("Assigned IP: {}", ip);

                if spawner
                    .spawn(mdns::mdns_responder(
                        wifi_stack,
                        ip,
                        TCP_PORT,
                        serial_number,
                        hw_desc.details.model,
                        TCP_MDNS_SERVICE_NAME,
                        TCP_MDNS_SERVICE_PROTOCOL,
                    ))
                    .is_err()
                {
                    error!("Could not spawn mDNS responder task");
                }

                #[cfg(feature = "usb")]
                let mut usb_connection = usb::start(
                    spawner,
                    driver,
                    hw_desc,
                    #[cfg(feature = "wifi")]
                    Some((ip.octets(), TCP_PORT)),
                    #[cfg(feature = "wifi")]
                    db,
                    #[cfg(feature = "wifi")]
                    watchdog,
                )
                .await;

                #[cfg(feature = "wifi")]
                let mut wifi_tx_buffer = [0; 4096];
                #[cfg(feature = "wifi")]
                let mut wifi_rx_buffer = [0; 4096];

                #[cfg(all(feature = "usb", feature = "wifi"))]
                loop {
                    match select(
                        tcp::wait_connection(wifi_stack, &mut wifi_tx_buffer, &mut wifi_rx_buffer),
                        usb::wait_connection(&mut usb_connection, &hardware_config),
                    )
                    .await
                    {
                        Either::First(socket_select) => match socket_select {
                            Ok(socket) => {
                                tcp::message_loop(
                                    &mut gpio,
                                    socket,
                                    hw_desc,
                                    &mut hardware_config,
                                    &spawner,
                                    &mut control,
                                    db,
                                )
                                .await
                            }
                            Err(_) => error!("TCP accept error"),
                        },
                        Either::Second(_) => {
                            let _ = usb::message_loop(
                                &mut gpio,
                                &mut usb_connection,
                                &mut hardware_config,
                                &spawner,
                                &mut control,
                                db,
                            )
                            .await;
                        }
                    }
                }

                #[cfg(all(not(feature = "usb"), feature = "wifi"))]
                loop {
                    match tcp::wait_connection(wifi_stack, &mut wifi_tx_buffer, &mut wifi_rx_buffer)
                        .await
                    {
                        Ok(socket) => {
                            tcp::message_loop(
                                &mut gpio,
                                socket,
                                hw_desc,
                                &mut hardware_config,
                                &spawner,
                                &mut control,
                                db,
                            )
                            .await
                        }
                        Err(_) => error!("TCP accept error"),
                    }
                }
            }
            Err(e) => {
                #[cfg(feature = "usb")]
                error!("Could not join Wi-Fi network: {}, so starting USB only", e);
                #[cfg(feature = "usb")]
                usb_only(
                    spawner,
                    driver,
                    gpio,
                    hw_desc,
                    hardware_config,
                    db,
                    watchdog,
                    control,
                )
                .await;
            }
        },
        None => {
            #[cfg(feature = "usb")]
            info!("No valid SsidSpec was found, cannot start Wi-Fi, so starting USB only");
            #[cfg(feature = "usb")]
            usb_only(
                spawner,
                driver,
                gpio,
                hw_desc,
                hardware_config,
                db,
                watchdog,
                control,
            )
            .await;
        }
    }
}
