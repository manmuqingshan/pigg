#![no_std]
#![no_main]

use crate::ssid::{
    MARKER_LENGTH, SSID_NAME, SSID_NAME_LENGTH, SSID_PASS, SSID_PASS_LENGTH, SSID_SECURITY,
};
use core::str::from_utf8;
use cyw43::Control;
use cyw43::NetDriver;
use cyw43_pio::PioSpi;
use defmt::{debug, error, info, warn};
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_net::Ipv4Address;
use embassy_net::{
    tcp::client::{TcpClient, TcpClientState},
    Stack, StackResources,
};
use embassy_rp::bind_interrupts;
use embassy_rp::flash::Async;
use embassy_rp::flash::Flash;
use embassy_rp::gpio::Flex;
use embassy_rp::gpio::Pull;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::USB;
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::usb::InterruptHandler as USBInterruptHandler;
use embassy_time::Instant;
use embassy_time::Timer;
use embedded_io_async::Write;
use faster_hex::hex_encode;
use heapless::FnvIndexMap;
use heapless::Vec;
use hw_definition::config::HardwareConfigMessage::*;
use hw_definition::config::{HardwareConfig, HardwareConfigMessage, InputPull, LevelChange};
use hw_definition::description::{
    HardwareDescription, HardwareDetails, PinDescriptionSet, PinNumberingScheme,
};
use hw_definition::pin_function::PinFunction;
use hw_definition::{BCMPinNumber, PinLevel};
use panic_probe as _;
use pin_descriptions::PIN_DESCRIPTIONS;
use static_cell::StaticCell;

/// The ssid config generated by build.rs in "$OUT_DIR/ssid.rs"
mod ssid {
    include!(concat!(env!("OUT_DIR"), "/ssid.rs"));
}

#[path = "../../src/hw_definition/mod.rs"]
mod hw_definition;

mod pin_descriptions;

const FLASH_SIZE: usize = 2 * 1024 * 1024;

const WIFI_JOIN_RETRY_ATTEMPT_LIMIT: usize = 3;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    USBCTRL_IRQ => USBInterruptHandler<USB>;
});

//#[derive(PartialOrd, PartialEq)]
enum GPIOPin<'a> {
    Available(Flex<'a>),
    GPIOInput(Flex<'a>),
    CYW43Input,
    CYW43Output,
    GPIOOutput(Flex<'a>),
}

#[embassy_executor::task]
async fn wifi_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<NetDriver<'static>>) -> ! {
    stack.run().await
}

async fn join_wifi(
    control: &mut Control<'_>,
    stack: &Stack<NetDriver<'static>>,
    ssid_name: &str,
    ssid_pass: &str,
) -> Option<Ipv4Address> {
    let mut attempt = 1;
    while attempt <= WIFI_JOIN_RETRY_ATTEMPT_LIMIT {
        info!(
            "Attempt #{} to join wifi network: '{}' with security = '{}'",
            attempt, ssid_name, SSID_SECURITY
        );
        let result = match SSID_SECURITY {
            "open" => control.join_open(ssid_name).await,
            "wpa2" => control.join_wpa2(ssid_name, ssid_pass).await,
            "wpa3" => control.join_wpa3(ssid_name, ssid_pass).await,
            _ => {
                error!("Security '{}' is not supported", SSID_SECURITY);
                return None;
            }
        };

        match result {
            Ok(_) => {
                info!("Joined wifi network: '{}'", ssid_name);
                return wait_for_dhcp(stack).await;
            }
            Err(_) => {
                attempt += 1;
                warn!("Failed to join wifi, retrying");
            }
        }
    }

    error!(
        "Failed to join Wifi after {} reties",
        WIFI_JOIN_RETRY_ATTEMPT_LIMIT
    );
    None
}

/// Wait for the DHCP service to come up and for us to get an IP address
async fn wait_for_dhcp(stack: &Stack<NetDriver<'static>>) -> Option<Ipv4Address> {
    info!("Waiting for DHCP...");
    while !stack.is_config_up() {
        Timer::after_millis(100).await;
    }
    info!("DHCP is now up!");
    if let Some(if_config) = stack.config_v4() {
        Some(if_config.address.address())
    } else {
        None
    }
}

/// Wait until a message in received on the [TcpSocket] then deserialize it and return it
async fn wait_message(socket: &mut TcpSocket<'_>) -> Option<HardwareConfigMessage> {
    let mut buf = [0; 4096]; // TODO needed?

    // wait for hardware config message
    let n = socket.read(&mut buf).await.ok()?;
    if n == 0 {
        return None;
    }

    postcard::from_bytes(&buf[..n]).ok()
}

fn into_level(value: PinLevel) -> Level {
    match value {
        true => Level::High,
        false => Level::Low,
    }
}

/// Set an output's level using the bcm pin number
async fn set_output_level<'a>(
    control: &mut Control<'_>,
    gpio_pins: &mut FnvIndexMap<BCMPinNumber, GPIOPin<'a>, 32>,
    bcm_pin_number: BCMPinNumber,
    pin_level: PinLevel,
) {
    debug!(
        "Pin #{} Output level change: {:?}",
        bcm_pin_number, pin_level
    );

    // GPIO 0 and 1 are connected via cyw43 wifi chip
    match gpio_pins.get_mut(&bcm_pin_number) {
        Some(GPIOPin::CYW43Output) => control.gpio_set(bcm_pin_number, pin_level).await,
        Some(GPIOPin::GPIOOutput(flex)) => flex.set_level(into_level(pin_level)),
        _ => error!("Pin {} is not configured as an Output", bcm_pin_number),
    }
}

/// Send a detected input level change back to the GUI using `writer` [TcpStream],
/// timestamping with the current time in Utc
async fn send_input_level(socket: &mut TcpSocket<'_>, bcm: BCMPinNumber, level: Level) {
    let level_change = LevelChange::new(
        level == Level::High,
        Instant::now().duration_since(Instant::MIN).into(),
    );
    let hardware_event = IOLevelChanged(bcm, level_change);
    let mut buf = [0; 1024];
    let message = postcard::to_slice(&hardware_event, &mut buf).unwrap();
    socket.write_all(&message).await.unwrap();
}

/// Send the current input state for all inputs configured in the config
async fn send_current_input_levels<'a>(
    gpio_pins: &FnvIndexMap<BCMPinNumber, GPIOPin<'a>, 32>,
    socket: &mut TcpSocket<'_>,
) {
    for (bcm_pin_number, pin) in gpio_pins {
        if let GPIOPin::GPIOInput(flex) = pin {
            let _ = send_input_level(socket, *bcm_pin_number, flex.get_level()).await;
        }
    }
}

/// Apply the requested config to one pin, using bcm_pin_number
async fn apply_pin_config<'a>(
    control: &mut Control<'_>,
    gpio_pins: &mut FnvIndexMap<BCMPinNumber, GPIOPin<'a>, 32>,
    bcm_pin_number: BCMPinNumber,
    new_pin_function: &PinFunction,
) {
    let Some(entry) = gpio_pins.remove(&bcm_pin_number) else {
        error!("Could not find pin #{}", bcm_pin_number);
        return;
    };

    let gpio_pin = match entry {
        GPIOPin::Available(flex) | GPIOPin::GPIOInput(flex) | GPIOPin::GPIOOutput(flex) => {
            Some(flex)
        }
        GPIOPin::CYW43Input | GPIOPin::CYW43Output => None,
    };

    match new_pin_function {
        PinFunction::None => {
            // if pin 0, 1 or 2 - then have been removed and so are considered unconfigured
            if let Some(flex) = gpio_pin {
                let _ = gpio_pins.insert(bcm_pin_number, GPIOPin::Available(flex));
            }
        }

        PinFunction::Input(pull) => {
            // GPIO 2 is connected via cyw43 wifi chip
            if bcm_pin_number == 2 {
                let _ = gpio_pins.insert(bcm_pin_number, GPIOPin::CYW43Input);
            } else {
                if let Some(mut flex) = gpio_pin {
                    match pull {
                        None | Some(InputPull::None) => flex.set_pull(Pull::None),
                        Some(InputPull::PullUp) => flex.set_pull(Pull::Up),
                        Some(InputPull::PullDown) => flex.set_pull(Pull::Down),
                    };

                    /*
                    input
                        .set_async_interrupt(
                            Trigger::Both,
                            Some(Duration::from_millis(1)),
                            move |event| {
                                callback(bcm_pin_number, event.trigger == Trigger::RisingEdge);
                            },
                        )
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                        */

                    let _ = gpio_pins.insert(bcm_pin_number, GPIOPin::GPIOInput(flex));
                }
            }
        }

        PinFunction::Output(level) => {
            // GPIO 0 and 1 are connected via cyw43 wifi chip
            if bcm_pin_number == 0 || bcm_pin_number == 1 {
                let _ = gpio_pins.insert(bcm_pin_number, GPIOPin::CYW43Output);
            } else {
                let _ = gpio_pins.insert(bcm_pin_number, GPIOPin::GPIOOutput(gpio_pin.unwrap()));
            }

            if let Some(l) = level {
                set_output_level(control, gpio_pins, bcm_pin_number, *l).await;
            }
        }
    }

    info!("New pin config for pin #{}", bcm_pin_number);
}

/// This takes the GPIOConfig struct and configures all the pins in it
async fn apply_config<'a>(
    control: &mut Control<'_>,
    gpio_pins: &mut FnvIndexMap<BCMPinNumber, GPIOPin<'a>, 32>,
    config: &HardwareConfig,
) {
    // Config only has pins that are configured
    for (bcm_pin_number, pin_function) in &config.pin_functions {
        apply_pin_config(control, gpio_pins, *bcm_pin_number, pin_function).await;
    }
    info!("New config applied");
}

/// Apply a config change to the hardware
/// NOTE: Initially the callback to Config/PinConfig change was async, and that compiles and runs
/// but wasn't working - so this uses a sync callback again to fix that, and an async version of
/// send_input_level() for use directly from the async context
async fn apply_config_change<'a>(
    control: &mut Control<'_>,
    gpio_pins: &mut FnvIndexMap<BCMPinNumber, GPIOPin<'a>, 32>,
    config_change: HardwareConfigMessage,
    socket: &mut TcpSocket<'_>,
) {
    match config_change {
        NewConfig(config) => {
            apply_config(control, gpio_pins, &config).await;
            let _ = send_current_input_levels(gpio_pins, socket).await;
        }
        NewPinConfig(bcm, pin_function) => {
            apply_pin_config(control, gpio_pins, bcm, &pin_function).await;
        }
        IOLevelChanged(bcm, level_change) => {
            set_output_level(control, gpio_pins, bcm, level_change.new_level).await;
        }
    }
}

/// Wait for an incoming TCP connection, then respond to it with the [HardwareDescription]
async fn tcp_accept(socket: &mut TcpSocket<'_>, ip_address: &Ipv4Address, device_id: &[u8; 8]) {
    let mut buf = [0; 4096];

    info!("Listening on TCP {}:1234", ip_address);
    if let Err(e) = socket.accept(1234).await {
        error!("TCP accept error: {:?}", e);
        return;
    }

    info!(
        "Received connection from {:?}",
        socket.remote_endpoint().unwrap()
    );

    let mut device_id_hex: [u8; 16] = [0; 16];
    hex_encode(device_id, &mut device_id_hex).unwrap();

    // send hardware description
    let details = HardwareDetails {
        hardware: "foo",
        revision: "foo",
        serial: from_utf8(&device_id_hex).unwrap(),
        model: "Pi Pico W",
    };

    let hw_desc = HardwareDescription {
        details,
        pins: PinDescriptionSet {
            pin_numbering: PinNumberingScheme::CounterClockwise,
            pins: Vec::from_slice(&PIN_DESCRIPTIONS).unwrap(),
        },
    };

    let slice = postcard::to_slice(&hw_desc, &mut buf).unwrap();
    info!("Sending hardware description (length: {})", slice.len());
    socket.write_all(slice).await.unwrap();
}

/// Enter the message loop, processing config change messages from piggui
async fn message_loop<'a>(
    control: &mut Control<'_>,
    gpio_pins: &mut FnvIndexMap<BCMPinNumber, GPIOPin<'a>, 32>,
    device_id: [u8; 8],
    ip_address: Ipv4Address,
    stack: &Stack<NetDriver<'static>>,
) {
    let client_state: TcpClientState<2, 1024, 1024> = TcpClientState::new();
    let _client = TcpClient::new(stack, &client_state);

    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
    //socket.set_timeout(Some(Duration::from_secs(10)));

    // wait for a connection from `piggui`
    tcp_accept(&mut socket, &ip_address, &device_id).await;

    info!("Entering message loop");
    loop {
        if let Some(config_message) = wait_message(&mut socket).await {
            let _ = apply_config_change(control, gpio_pins, config_message, &mut socket).await;
        }
    }
}

/*
Wifi scanning

We could use this to program the ssid config with a list of ssids, and when
it cannot connect via one, it scans to see if another one it knows is available
and then tries to connect to that.

let mut scanner = control.scan(Default::default()).await;
while let Some(bss) = scanner.next().await {
    if let Ok(ssid_str) = str::from_utf8(&bss.ssid) {
    info!("scanned {} == {:x}", ssid_str, bss.bssid);
    }
}

 */

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let peripherals = embassy_rp::init(Default::default());
    let fw = include_bytes!("../assets/43439A0.bin");
    let clm = include_bytes!("../assets/43439A0_clm.bin");
    let pwr = Output::new(peripherals.PIN_23, Level::Low);
    let cs = Output::new(peripherals.PIN_25, Level::High);
    let mut pio = Pio::new(peripherals.PIO0, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        pio.irq0,
        cs,
        peripherals.PIN_24,
        peripherals.PIN_29,
        peripherals.DMA_CH0,
    );

    let mut gpio_pins = FnvIndexMap::<BCMPinNumber, GPIOPin, 32>::new();
    let _ = gpio_pins.insert(0, GPIOPin::CYW43Output); // GP0 connected to CYW43 chip
    let _ = gpio_pins.insert(1, GPIOPin::CYW43Output); // GP1 connected to CYW43 chip
    let _ = gpio_pins.insert(2, GPIOPin::CYW43Input); // GP2 connected to CYW43 chip
    let _ = gpio_pins.insert(3, GPIOPin::Available(Flex::new(peripherals.PIN_3)));
    let _ = gpio_pins.insert(4, GPIOPin::Available(Flex::new(peripherals.PIN_4)));
    let _ = gpio_pins.insert(5, GPIOPin::Available(Flex::new(peripherals.PIN_5)));
    let _ = gpio_pins.insert(6, GPIOPin::Available(Flex::new(peripherals.PIN_6)));
    let _ = gpio_pins.insert(7, GPIOPin::Available(Flex::new(peripherals.PIN_7)));
    let _ = gpio_pins.insert(8, GPIOPin::Available(Flex::new(peripherals.PIN_8)));
    let _ = gpio_pins.insert(9, GPIOPin::Available(Flex::new(peripherals.PIN_9)));
    let _ = gpio_pins.insert(10, GPIOPin::Available(Flex::new(peripherals.PIN_10)));
    let _ = gpio_pins.insert(11, GPIOPin::Available(Flex::new(peripherals.PIN_11)));
    let _ = gpio_pins.insert(12, GPIOPin::Available(Flex::new(peripherals.PIN_12)));
    let _ = gpio_pins.insert(13, GPIOPin::Available(Flex::new(peripherals.PIN_13)));
    let _ = gpio_pins.insert(14, GPIOPin::Available(Flex::new(peripherals.PIN_14)));
    let _ = gpio_pins.insert(15, GPIOPin::Available(Flex::new(peripherals.PIN_15)));
    let _ = gpio_pins.insert(16, GPIOPin::Available(Flex::new(peripherals.PIN_16)));
    let _ = gpio_pins.insert(17, GPIOPin::Available(Flex::new(peripherals.PIN_17)));
    let _ = gpio_pins.insert(18, GPIOPin::Available(Flex::new(peripherals.PIN_18)));
    let _ = gpio_pins.insert(19, GPIOPin::Available(Flex::new(peripherals.PIN_19)));
    let _ = gpio_pins.insert(20, GPIOPin::Available(Flex::new(peripherals.PIN_20)));
    let _ = gpio_pins.insert(21, GPIOPin::Available(Flex::new(peripherals.PIN_21)));
    let _ = gpio_pins.insert(22, GPIOPin::Available(Flex::new(peripherals.PIN_22)));
    let _ = gpio_pins.insert(26, GPIOPin::Available(Flex::new(peripherals.PIN_26)));
    let _ = gpio_pins.insert(27, GPIOPin::Available(Flex::new(peripherals.PIN_27)));
    let _ = gpio_pins.insert(28, GPIOPin::Available(Flex::new(peripherals.PIN_28)));

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;

    spawner.spawn(wifi_task(runner)).unwrap();

    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    // Get a unique device id - in this case an eight-byte ID from flash rendered as hex string
    let mut flash = Flash::<_, Async, { FLASH_SIZE }>::new(peripherals.FLASH, peripherals.DMA_CH1);
    let mut device_id = [0; 8];
    flash.blocking_unique_id(&mut device_id).unwrap();

    let dhcp_config = embassy_net::Config::dhcpv4(Default::default());

    static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
    let resources = RESOURCES.init(StackResources::new());

    // Generate random seed
    let seed = 0x0123_4567_89ab_cdef;
    static STACK: StaticCell<Stack<NetDriver<'static>>> = StaticCell::new();
    let stack = STACK.init(Stack::new(net_device, dhcp_config, resources, seed));

    spawner.spawn(net_task(stack)).unwrap();

    let ssid_name = SSID_NAME[MARKER_LENGTH..(MARKER_LENGTH + SSID_NAME_LENGTH)].trim();
    let ssid_pass = SSID_PASS[MARKER_LENGTH..(MARKER_LENGTH + SSID_PASS_LENGTH)].trim();

    while let Some(ip_address) = join_wifi(&mut control, &stack, ssid_name, ssid_pass).await {
        loop {
            message_loop(&mut control, &mut gpio_pins, device_id, ip_address, stack).await;
            info!("Disconnected");
        }
    }

    info!("Exiting");
}
