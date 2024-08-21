#![no_std]
#![no_main]

use crate::ssid::{
    MARKER_LENGTH, SSID_NAME, SSID_NAME_LENGTH, SSID_PASS, SSID_PASS_LENGTH, SSID_SECURITY,
};
use cyw43::Control;
use cyw43::NetDriver;
use cyw43_pio::PioSpi;
use defmt::{error, info};
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_net::{
    tcp::client::{TcpClient, TcpClientState},
    Stack, StackResources,
};
use embassy_rp::bind_interrupts;
use embassy_rp::flash::Async;
use embassy_rp::flash::Flash;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::USB;
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::usb::InterruptHandler as USBInterruptHandler;
use embassy_time::{Duration, Timer};
use faster_hex::hex_encode;
use panic_probe as _;
use static_cell::StaticCell;

/// The ssid config generated by build.rs in "$OUT_DIR/ssid.rs"
mod ssid {
    include!(concat!(env!("OUT_DIR"), "/ssid.rs"));
}

const LED: u8 = 0;

const ON: bool = true;
const OFF: bool = false;

const FLASH_SIZE: usize = 2 * 1024 * 1024;

const WIFI_JOIN_RETRY_ATTEMPT_LIMIT: usize = 3;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    USBCTRL_IRQ => USBInterruptHandler<USB>;
});

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

async fn wait_for_dhcp(stack: &Stack<NetDriver<'static>>) {
    info!("Waiting for DHCP...");
    while !stack.is_config_up() {
        Timer::after_millis(100).await;
    }
    info!("DHCP is now up!");
}

async fn message_loop<'a>(stack: &Stack<NetDriver<'static>>, control: &mut Control<'_>) {
    let client_state: TcpClientState<2, 1024, 1024> = TcpClientState::new();
    let _client = TcpClient::new(stack, &client_state);
    // let mut rx_buf = [0; 4096];

    // wait for an incoming tcp connection
    //let tcp = tcp_accept(&mut tcp_listener, &desc);

    // send hardware description

    info!("Starting message loop");
    loop {
        // wait for config message

        control.gpio_set(LED, ON).await;

        Timer::after(Duration::from_secs(1)).await;

        control.gpio_set(LED, OFF).await;
    }
    // info!("Exited message loop");
}

async fn join_wifi(
    control: &mut Control<'_>,
    stack: &Stack<NetDriver<'static>>,
    ssid_name: &str,
    ssid_pass: &str,
) -> bool {
    let mut attempt = 1;
    while attempt <= WIFI_JOIN_RETRY_ATTEMPT_LIMIT {
        info!("Attempt #{} to join wifi network: '{}'", attempt, ssid_name);
        let result = match SSID_SECURITY {
            "open" => control.join_open(ssid_name).await,
            "wpa2" => control.join_wpa2(ssid_name, ssid_pass).await,
            "wpa3" => control.join_wpa3(ssid_name, ssid_pass).await,
            _ => {
                error!("Security '{}' is not supported", SSID_SECURITY);
                return false;
            }
        };

        match result {
            Ok(_) => {
                info!("Joined wifi network: '{}'", ssid_name);
                wait_for_dhcp(stack).await;
                control.gpio_set(0, false).await;
                return true;
            }
            Err(_) => {
                attempt += 1;
                error!("Error joining wifi");
            }
        }
    }

    false
}

fn log_device_id(device_id: [u8; 8]) {
    let mut device_id_hex: [u8; 16] = [0; 16];
    hex_encode(&device_id, &mut device_id_hex).unwrap();
    info!(
        "Device ID = {}",
        core::str::from_utf8(&device_id_hex).unwrap()
    );
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
    let p = embassy_rp::init(Default::default());
    let fw = include_bytes!("../assets/43439A0.bin");
    let clm = include_bytes!("../assets/43439A0_clm.bin");
    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        pio.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        p.DMA_CH0,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;

    spawner.spawn(wifi_task(runner)).unwrap();

    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    // Switch on led to show we are up and running
    control.gpio_set(LED, ON).await;

    // Get a unique device id - in this case an eight-byte ID from flash rendered as hex string
    let mut flash = Flash::<_, Async, { FLASH_SIZE }>::new(p.FLASH, p.DMA_CH1);
    let mut device_id = [0; 8];
    flash.blocking_unique_id(&mut device_id).unwrap();
    log_device_id(device_id);

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

    if join_wifi(&mut control, &stack, ssid_name, ssid_pass).await {
        loop {
            info!("Waiting for TCP connection");
            message_loop(stack, &mut control).await;
            info!("Disconnected");
        }
    }

    info!("Exiting");
    control.gpio_set(LED, OFF).await;
}
