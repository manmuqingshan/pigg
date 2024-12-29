#![no_std]
#![no_main]

use crate::flash::DbFlash;
use crate::hw_definition::config::HardwareConfigMessage;
use crate::hw_definition::description::{
    HardwareDescription, HardwareDetails, PinDescriptionSet, TCP_MDNS_SERVICE_NAME,
    TCP_MDNS_SERVICE_PROTOCOL,
};
use crate::pin_descriptions::PIN_DESCRIPTIONS;
use crate::tcp::TCP_PORT;
use core::str;
use cyw43_pio::PioSpi;
use defmt::{error, info};
use defmt_rtt as _;
use ekv::Database;
use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_rp::bind_interrupts;
use embassy_rp::flash::{Blocking, Flash};
use embassy_rp::gpio::{Level, Output};
#[cfg(feature = "usb")]
use embassy_rp::peripherals::USB;
use embassy_rp::peripherals::{FLASH, PIO0};
use embassy_rp::pio::InterruptHandler as PioInterruptHandler;
use embassy_rp::pio::Pio;
#[cfg(feature = "usb")]
use embassy_rp::usb::Driver;
#[cfg(feature = "usb")]
use embassy_rp::usb::InterruptHandler as USBInterruptHandler;
use embassy_rp::watchdog::Watchdog;
use embassy_sync::blocking_mutex::raw::{NoopRawMutex, ThreadModeRawMutex};
use embassy_sync::channel::Channel;
use heapless::Vec;
use panic_probe as _;
use static_cell::StaticCell;

#[cfg(not(any(feature = "pico", feature = "pico_w")))]
compile_error!(
    "You must chose a feature from [\"pico\", \"pico_w\"] to select a device to build for"
);

#[cfg(all(feature = "pico", feature = "pico_w"))]
compile_error!(
    "You must chose a just one of [\"pico\", \"pico_w\"] to select a device to build for"
);

/// The ssid config generated by build.rs in "$OUT_DIR/ssid.rs"
mod ssid {
    include!(concat!(env!("OUT_DIR"), "/ssid.rs"));
}

/// Wi-Fi related functions
mod wifi;

#[cfg(feature = "usb")]
mod usb;

/// TCP related functions
mod tcp;

/// GPIO control related functions
mod gpio;

/// Definition of hardware structs passed back and fore between porky and the GUI
#[path = "../../src/hw_definition/mod.rs"]
mod hw_definition;

/// Functions for interacting with the Flash ROM
mod flash;

/// Persistence layer built on top of flash
mod persistence;

#[cfg(feature = "discovery")]
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
        model: "Pi Pico W",
        hardware: "RP2040",
        revision: "",
        serial,
        wifi: true,
        app_name: env!("CARGO_BIN_NAME"),
        app_version: env!("CARGO_PKG_VERSION"),
    };

    HardwareDescription {
        details,
        pins: PinDescriptionSet {
            pins: Vec::from_slice(&PIN_DESCRIPTIONS).unwrap(),
        },
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // Get the RPi Pico Peripherals - a number of the PINS are available for GPIO (they are
    // passed to AvailablePins) while others are reserved for internal use and not available for
    // GPIO
    let peripherals = embassy_rp::init(Default::default());
    // PIN_25 - OP wireless SPI CS - when high also enables GPIO29 ADC pin to read VSYS
    let cs = Output::new(peripherals.PIN_25, Level::High);
    let mut pio = Pio::new(peripherals.PIO0, Irqs);

    // Initialize the cyw43 and start the network
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        pio.irq0,
        cs,
        // PIN_24 - OP/IP wireless SPI data/IRQ
        peripherals.PIN_24,
        // PIN_29 - OP/IP wireless SPI CLK/ADC mode (ADC3) to measure VSYS/3
        peripherals.PIN_29,
        peripherals.DMA_CH0,
    );
    // PIN_23 - OP wireless power on signal
    let (mut control, wifi_stack) = wifi::start_net(spawner, peripherals.PIN_23, spi).await;

    // Take the following pins out of peripherals for use a GPIO
    let header_pins = gpio::HeaderPins {
        #[cfg(not(feature = "debug-probe"))]
        pin_0: peripherals.PIN_0,
        #[cfg(not(feature = "debug-probe"))]
        pin_1: peripherals.PIN_1,
        pin_2: peripherals.PIN_2,
        pin_3: peripherals.PIN_3,
        pin_4: peripherals.PIN_4,
        pin_5: peripherals.PIN_5,
        pin_6: peripherals.PIN_6,
        pin_7: peripherals.PIN_7,
        pin_8: peripherals.PIN_8,
        pin_9: peripherals.PIN_9,
        pin_10: peripherals.PIN_10,
        pin_11: peripherals.PIN_11,
        pin_12: peripherals.PIN_12,
        pin_13: peripherals.PIN_13,
        pin_14: peripherals.PIN_14,
        pin_15: peripherals.PIN_15,
        pin_16: peripherals.PIN_16,
        pin_17: peripherals.PIN_17,
        pin_18: peripherals.PIN_18,
        pin_19: peripherals.PIN_19,
        pin_20: peripherals.PIN_20,
        pin_21: peripherals.PIN_21,
        pin_22: peripherals.PIN_22,
        pin_26: peripherals.PIN_26,
        pin_27: peripherals.PIN_27,
        pin_28: peripherals.PIN_28,
    };
    gpio::setup_pins(header_pins);

    // create hardware description
    let mut flash = flash::get_flash(peripherals.FLASH);
    let serial_number = flash::serial_number(&mut flash);
    static HARDWARE_DESCRIPTION: StaticCell<HardwareDescription> = StaticCell::new();
    let hw_desc = HARDWARE_DESCRIPTION.init(hardware_description(serial_number));

    #[cfg(feature = "usb")]
    let driver = Driver::new(peripherals.USB, Irqs);

    // start the flash database
    static DATABASE: StaticCell<
        Database<DbFlash<Flash<'static, FLASH, Blocking, { flash::FLASH_SIZE }>>, NoopRawMutex>,
    > = StaticCell::new();
    let db = DATABASE.init(flash::db_init(flash).await);

    static STATIC_BUF: StaticCell<[u8; 200]> = StaticCell::new();
    let static_buf = STATIC_BUF.init([0u8; 200]);

    #[cfg(feature = "usb")]
    let watchdog = Watchdog::new(peripherals.WATCHDOG);

    // Load initial config from flash
    let mut hardware_config = persistence::get_config(db).await;

    // apply the loaded config to the hardware immediately
    gpio::apply_config_change(
        &mut control,
        &spawner,
        &HardwareConfigMessage::NewConfig(hardware_config.clone()),
        &mut hardware_config,
    )
    .await;

    // If we have a valid SsidSpec, then try and join that network using it
    match persistence::get_ssid_spec(db, static_buf).await {
        Some(ssid) => match wifi::join(&mut control, wifi_stack, &ssid).await {
            Ok(ip) => {
                info!("Assigned IP: {}", ip);

                let _ = spawner.spawn(mdns::mdns_responder(
                    wifi_stack,
                    ip,
                    TCP_PORT,
                    serial_number,
                    hw_desc.details.model,
                    TCP_MDNS_SERVICE_NAME,
                    TCP_MDNS_SERVICE_PROTOCOL,
                ));

                let tcp = (ip.octets(), TCP_PORT);

                #[cfg(feature = "usb")]
                let mut usb_connection = usb::start(
                    spawner,
                    driver,
                    hw_desc,
                    hardware_config.clone(),
                    Some(tcp),
                    db,
                    watchdog,
                )
                .await;

                let mut wifi_tx_buffer = [0; 4096];
                let mut wifi_rx_buffer = [0; 4096];

                loop {
                    match tcp::wait_connection(wifi_stack, &mut wifi_tx_buffer, &mut wifi_rx_buffer)
                        .await
                    {
                        Ok(mut socket) => {
                            tcp::send_hardware_description_and_config(
                                &mut socket,
                                hw_desc,
                                &hardware_config,
                            )
                            .await;

                            info!("Entering message loop");
                            loop {
                                match select(
                                    tcp::wait_message(&mut socket),
                                    HARDWARE_EVENT_CHANNEL.receiver().receive(),
                                )
                                .await
                                {
                                    Either::First(config_message) => match config_message {
                                        None => break,
                                        Some(hardware_config_message) => {
                                            gpio::apply_config_change(
                                                &mut control,
                                                &spawner,
                                                &hardware_config_message,
                                                &mut hardware_config,
                                            )
                                            .await;
                                            let _ = persistence::store_config_change(
                                                db,
                                                &hardware_config_message,
                                            )
                                            .await;
                                            if matches!(
                                                hardware_config_message,
                                                HardwareConfigMessage::GetConfig
                                            ) {
                                                tcp::send_hardware_config(
                                                    &mut socket,
                                                    &hardware_config,
                                                )
                                                .await;
                                            }
                                        }
                                    },
                                    Either::Second(hardware_config_message) => {
                                        tcp::send_message(
                                            &mut socket,
                                            hardware_config_message.clone(),
                                        )
                                        .await;
                                        //info!("Sending hw message via USB");
                                        //let _ = usb_connection.send(hardware_config_message).await;
                                    }
                                }
                            }
                            info!("Exiting Message Loop");
                        }
                        Err(_) => error!("TCP accept error"),
                    }
                }
            }
            Err(e) => {
                error!("Could not join Wi-Fi network: {}, so starting USB only", e);
                #[cfg(feature = "usb")]
                usb::start(
                    spawner,
                    driver,
                    hw_desc,
                    hardware_config,
                    None,
                    db,
                    watchdog,
                )
                .await;
            }
        },
        None => {
            info!("No valid SsidSpec was found, cannot start Wi-Fi, so starting USB only");
            #[cfg(feature = "usb")]
            usb::start(
                spawner,
                driver,
                hw_desc,
                hardware_config,
                None,
                db,
                watchdog,
            )
            .await;
        }
    }
}
