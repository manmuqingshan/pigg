#![no_std]
#![no_main]

use crate::hw_definition::config::HardwareConfigMessage;
use crate::hw_definition::description::{HardwareDescription, HardwareDetails, PinDescriptionSet};
use crate::pin_descriptions::PIN_DESCRIPTIONS;
use crate::ssid::{MARKER_LENGTH, SSID_NAME, SSID_NAME_LENGTH, SSID_PASS, SSID_PASS_LENGTH};
use core::str;
use cyw43_pio::PioSpi;
use defmt::{error, info};
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_rp::bind_interrupts;
use embassy_rp::flash::Async;
use embassy_rp::flash::Flash;
use embassy_rp::gpio::{Level, Output};
#[cfg(feature = "usb")]
use embassy_rp::peripherals::USB;
use embassy_rp::peripherals::{DMA_CH1, FLASH, PIO0};
use embassy_rp::pio::InterruptHandler as PioInterruptHandler;
use embassy_rp::pio::Pio;
#[cfg(feature = "usb")]
use embassy_rp::usb::Driver;
#[cfg(feature = "usb")]
use embassy_rp::usb::InterruptHandler as USBInterruptHandler;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embedded_io_async::Write;
use faster_hex::hex_encode;
use heapless::Vec;
use panic_probe as _;
use static_cell::StaticCell;

#[cfg(all(feature = "usb-tcp", feature = "usb-raw"))]
compile_error!(
    "Features 'usb-raw' and 'usb-tcp' are mutually exclusive and cannot be enabled together"
);

/// The ssid config generated by build.rs in "$OUT_DIR/ssid.rs"
mod ssid {
    include!(concat!(env!("OUT_DIR"), "/ssid.rs"));
}

/// Wi-Fi related functions
mod wifi;

#[cfg(feature = "usb")]
mod usb;

#[cfg(feature = "usb-tcp")]
/// Module for Tcp over USB
mod usb_tcp;

#[cfg(feature = "usb-raw")]
mod usb_raw;

/// TCP related functions
mod tcp;

/// GPIO control related functions
mod gpio;

/// Definition of hardware structs passed back and fore between porky and the GUI
#[path = "../../src/hw_definition/mod.rs"]
mod hw_definition;

/// The Pi Pico GPIO [PinDefinition]s that get passed to the GUI
mod pin_descriptions;

const FLASH_SIZE: usize = 2 * 1024 * 1024;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
    #[cfg(feature = "usb")]
    USBCTRL_IRQ => USBInterruptHandler<USB>;
});

pub static HARDWARE_EVENT_CHANNEL: Channel<ThreadModeRawMutex, HardwareConfigMessage, 1> =
    Channel::new();

/// Read a unique device ID from the Flash and format it as hex into the provided 16 byte array
fn device_id(mut device_id_hex: [u8; 16], flash_pin: FLASH, dma_pin: DMA_CH1) {
    // Get a unique device id - in this case an eight-byte ID from flash rendered as hex string
    let mut flash = Flash::<_, Async, { FLASH_SIZE }>::new(flash_pin, dma_pin);
    let mut device_id = [0; 8];
    flash.blocking_unique_id(&mut device_id).unwrap();

    // convert the device_id to a hex "string"
    hex_encode(&device_id, &mut device_id_hex).unwrap();
}

/// Get the unique serial number from Flash
fn serial_number(flash: FLASH, dma: DMA_CH1) -> &'static str {
    let device_id_hex: [u8; 16] = [0; 16];
    device_id(device_id_hex, flash, dma);
    static ID: StaticCell<[u8; 16]> = StaticCell::new();
    let id = ID.init(device_id_hex);
    str::from_utf8(id).unwrap()
}

/// Create a [HardwareDescription] for this device with the provided serial number
fn hardware_description(serial: &str) -> HardwareDescription {
    let details = HardwareDetails {
        model: "Pi Pico W",
        hardware: "RP2040",
        revision: "",
        serial,
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
    let serial_number = serial_number(peripherals.FLASH, peripherals.DMA_CH1);
    let hw_desc = hardware_description(serial_number);

    #[cfg(feature = "usb")]
    let driver = Driver::new(peripherals.USB, Irqs);

    #[cfg(feature = "usb-tcp")]
    let usb_stack = usb_tcp::start(spawner, driver, serial_number).await;
    #[cfg(feature = "usb-tcp")]
    let mut usb_tx_buffer = [0; 4096];
    #[cfg(feature = "usb-tcp")]
    let mut usb_rx_buffer = [0; 4096];

    #[cfg(feature = "usb-raw")]
    usb_raw::start(spawner, driver, serial_number).await;

    let ssid_name = SSID_NAME[MARKER_LENGTH..(MARKER_LENGTH + SSID_NAME_LENGTH)].trim();
    let ssid_pass = SSID_PASS[MARKER_LENGTH..(MARKER_LENGTH + SSID_PASS_LENGTH)].trim();
    wifi::join(&mut control, wifi_stack, ssid_name, ssid_pass).await;
    let mut wifi_tx_buffer = [0; 4096];
    let mut wifi_rx_buffer = [0; 4096];

    loop {
        match tcp::wait_connection(
            wifi_stack,
            #[cfg(feature = "usb-tcp")]
            usb_stack,
            &mut wifi_tx_buffer,
            &mut wifi_rx_buffer,
            #[cfg(feature = "usb-tcp")]
            &mut usb_tx_buffer,
            #[cfg(feature = "usb-tcp")]
            &mut usb_rx_buffer,
        )
        .await
        {
            Ok(mut socket) => {
                let mut buf = [0; 4096];
                let slice = postcard::to_slice(&hw_desc, &mut buf).unwrap();
                info!("Sending hardware description (length: {})", slice.len());
                socket.write_all(slice).await.unwrap();

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
                            Some(message) => {
                                gpio::apply_config_change(&mut control, &spawner, message).await
                            }
                        },
                        Either::Second(hardware_event) => {
                            let mut buf = [0; 1024];
                            let gui_message =
                                postcard::to_slice(&hardware_event, &mut buf).unwrap();
                            socket.write_all(gui_message).await.unwrap();
                        }
                    }
                }
                info!("Exiting Message Loop");
            }
            Err(_) => error!("TCP accept error"),
        }
    }
}
