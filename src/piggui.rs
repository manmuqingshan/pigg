use crate::file_helper::{maybe_load_no_picker, pick_and_load, save};
use crate::hw_definition::config::HardwareConfig;
#[cfg(feature = "usb-raw")]
use crate::views::hardware_menu::{DeviceEvent, KnownDevice};
use crate::views::hardware_view::{HardwareTarget, HardwareView, HardwareViewMessage};
use crate::views::info_dialog::{InfoDialog, InfoDialogMessage};
use crate::views::info_row::InfoRow;
use crate::views::layout_menu::{Layout, LayoutSelector};
use crate::views::message_box::MessageMessage::{Error, Info};
use crate::views::message_box::MessageRowMessage;
#[cfg(feature = "usb-raw")]
use crate::views::ssid_dialog::SsidDialog;
use crate::widgets::modal::modal;
use crate::Message::*;
#[cfg(not(target_arch = "wasm32"))]
use clap::{Arg, ArgMatches};
use iced::widget::{container, Column};
use iced::{window, Element, Length, Padding, Pixels, Settings, Subscription, Task, Theme};
#[cfg(feature = "usb-raw")]
use std::collections::HashMap;

#[cfg(any(feature = "iroh", feature = "tcp"))]
use crate::views::connect_dialog::{
    ConnectDialog, ConnectDialogMessage, ConnectDialogMessage::HideConnectDialog,
};
#[cfg(feature = "usb-raw")]
use crate::views::hardware_menu;
use crate::views::hardware_view::HardwareTarget::NoHW;
#[cfg(feature = "usb-raw")]
use crate::views::message_box::MessageRowMessage::ShowStatusMessage;
#[cfg(feature = "usb-raw")]
use crate::views::ssid_dialog::SsidDialogMessage;
#[cfg(feature = "usb-raw")]
use crate::views::ssid_dialog::SsidDialogMessage::HideSsidDialog;
#[cfg(feature = "iroh")]
use iroh_net::NodeId;
#[cfg(any(feature = "iroh", feature = "tcp"))]
use std::str::FromStr;

pub mod event;
#[cfg(not(target_arch = "wasm32"))]
mod file_helper;
mod hardware_subscription;
mod hw;
mod hw_definition;
pub mod local_device;
mod networking;
#[cfg(feature = "usb-raw")]
mod usb_raw;
mod views;
mod widgets;

const PIGGUI_ID: &str = "piggui";

/// These are the messages that Piggui responds to
#[derive(Debug, Clone)]
pub enum Message {
    ConfigLoaded(String, HardwareConfig),
    ConfigSaved,
    ConfigChangesMade,
    Save,
    Load,
    LayoutChanged(Layout),
    Hardware(HardwareViewMessage),
    Modal(InfoDialogMessage),
    InfoRow(MessageRowMessage),
    WindowEvent(iced::Event),
    #[cfg(any(feature = "iroh", feature = "tcp"))]
    ConnectDialog(ConnectDialogMessage),
    ConnectRequest(HardwareTarget),
    Connected,
    Disconnected,
    ConnectionError(String),
    MenuBarButtonClicked,
    #[cfg(feature = "usb-raw")]
    Device(DeviceEvent),
    #[cfg(feature = "usb-raw")]
    SsidDialog(SsidDialogMessage),
    #[cfg(feature = "usb-raw")]
    ResetSsid(String),
    #[cfg(feature = "usb-raw")]
    SsidSpecSent(Result<(), String>),
}

/// [Piggui] Is the struct that holds application state and implements [Application] for Iced
pub struct Piggui {
    config_filename: Option<String>,
    layout_selector: LayoutSelector,
    unsaved_changes: bool,
    info_row: InfoRow,
    modal_handler: InfoDialog,
    hardware_view: HardwareView,
    #[cfg(any(feature = "iroh", feature = "tcp"))]
    connect_dialog: ConnectDialog,
    hardware_target: HardwareTarget,
    #[cfg(feature = "usb-raw")]
    known_devices: HashMap<String, KnownDevice>,
    #[cfg(feature = "usb-raw")]
    ssid_dialog: SsidDialog,
}

fn main() -> iced::Result {
    let settings = Settings {
        id: Some(PIGGUI_ID.into()),
        default_text_size: Pixels(14.0),
        ..Default::default()
    };

    iced::application(Piggui::title, Piggui::update, Piggui::view)
        .subscription(Piggui::subscription)
        .window_size((500.0, 800.0))
        .exit_on_close_request(false)
        .resizable(true)
        .settings(settings)
        .window_size(LayoutSelector::get_default_window_size())
        .theme(|_| Theme::Dark)
        .run_with(Piggui::new)
}

#[cfg(feature = "usb-raw")]
#[allow(unused_variables)]
fn reset_ssid(serial_number: String) -> Task<Message> {
    #[cfg(feature = "usb-raw")]
    return Task::perform(usb_raw::reset_ssid_spec(serial_number), |res| match res {
        Ok(_) => InfoRow(ShowStatusMessage(Info(
            "Wi-Fi Setup reset to Default by USB".into(),
        ))),
        Err(e) => InfoRow(ShowStatusMessage(Error(
            "Error resetting Wi-Fi Setup via USB".into(),
            e,
        ))),
    });
    #[cfg(not(feature = "usb-raw"))]
    Task::none()
}

impl Piggui {
    #[cfg(any(feature = "iroh", feature = "tcp"))]
    /// We have disconnected, or been disconnected from the hardware
    fn disconnected(&mut self) {
        self.info_row
            .add_info_message(Info("Disconnected from hardware".to_string()));
        self.config_filename = None;
        self.unsaved_changes = false;
        self.hardware_target = NoHW;
        self.hardware_view = HardwareView::new()
    }

    /// Connect to hardware - resetting the relevant control variables in the process
    fn connect(&mut self, new_target: HardwareTarget) {
        self.config_filename = None;
        self.unsaved_changes = false;
        self.hardware_target = new_target.clone();
        self.hardware_view.new_target(new_target);

        //        self.hardware_view = HardwareView::new();
    }

    fn new() -> (Self, Task<Message>) {
        #[cfg(not(target_arch = "wasm32"))]
        let matches = get_matches();
        #[cfg(not(target_arch = "wasm32"))]
        let config_filename = matches
            .get_one::<String>("config-file")
            .map(|s| s.to_string());
        #[cfg(target_arch = "wasm32")]
        let config_filename = None;

        (
            Self {
                config_filename: config_filename.clone(),
                layout_selector: LayoutSelector::new(),
                unsaved_changes: false,
                info_row: InfoRow::new(),
                modal_handler: InfoDialog::new(),
                hardware_view: HardwareView::new(),
                #[cfg(any(feature = "iroh", feature = "tcp"))]
                connect_dialog: ConnectDialog::new(),
                hardware_target: get_hardware_target(&matches),
                #[cfg(feature = "usb-raw")]
                known_devices: HashMap::new(),
                #[cfg(feature = "usb-raw")]
                ssid_dialog: SsidDialog::new(),
            },
            maybe_load_no_picker(config_filename),
        )
    }

    fn title(&self) -> String {
        self.config_filename
            .clone()
            .unwrap_or(String::from("piggui"))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            WindowEvent(event) => {
                if let iced::Event::Window(window::Event::CloseRequested) = event {
                    if self.unsaved_changes {
                        let _ = self
                            .modal_handler
                            .update(InfoDialogMessage::UnsavedChangesExitModal);
                    } else {
                        return window::get_latest().and_then(window::close);
                    }
                }
            }

            LayoutChanged(layout) => {
                let layout = self.layout_selector.update(layout);
                return window::get_latest().then(move |latest| {
                    if let Some(id) = latest {
                        window::resize(id, layout)
                    } else {
                        Task::none()
                    }
                });
            }

            Save => {
                return save(self.hardware_view.get_config());
            }

            ConfigSaved => {
                self.unsaved_changes = false;
                self.info_row
                    .add_info_message(Info("File saved successfully".to_string()));
            }

            Load => {
                if self.unsaved_changes {
                    let _ = self
                        .modal_handler
                        .update(InfoDialogMessage::UnsavedLoadConfigChangesModal);
                } else {
                    return pick_and_load();
                }
            }

            Modal(modal_message) => {
                return self.modal_handler.update(modal_message);
            }

            #[cfg(any(feature = "iroh", feature = "tcp"))]
            ConnectDialog(connect_dialog_message) => {
                return self.connect_dialog.update(connect_dialog_message);
            }

            InfoRow(msg) => {
                return self.info_row.update(msg);
            }

            Hardware(msg) => {
                return self.hardware_view.update(msg);
            }

            ConfigChangesMade => {
                self.unsaved_changes = true;
            }

            ConfigLoaded(filename, config) => {
                self.config_filename = Some(filename);
                self.unsaved_changes = false;
                self.hardware_view.new_config(config);
            }

            ConnectRequest(new_target) => {
                #[cfg(any(feature = "iroh", feature = "tcp"))]
                self.connect_dialog.disable_widgets_and_load_spinner();
                self.connect(new_target);
            }

            Connected => {
                #[cfg(any(feature = "iroh", feature = "tcp"))]
                self.connect_dialog.enable_widgets_and_hide_spinner();
                #[cfg(any(feature = "iroh", feature = "tcp"))]
                self.connect_dialog.hide_modal();
                self.info_row
                    .add_info_message(Info("Connected to hardware".to_string()));
                #[cfg(debug_assertions)] // Output used in testing - DON'T REMOVE
                println!("Connected to hardware");
            }

            Disconnected => {
                #[cfg(any(feature = "iroh", feature = "tcp"))]
                self.connect_dialog.enable_widgets_and_hide_spinner();
                #[cfg(any(feature = "iroh", feature = "tcp"))]
                self.disconnected();
            }

            ConnectionError(details) => {
                #[cfg(any(feature = "iroh", feature = "tcp"))]
                self.connect_dialog.enable_widgets_and_hide_spinner();
                self.info_row
                    .add_info_message(Error("Connection Error".to_string(), details.clone()));
                #[cfg(any(feature = "iroh", feature = "tcp"))]
                self.connect_dialog.set_error(details);
            }

            MenuBarButtonClicked => { /* Needed for Highlighting on hover to work on menu bar */ }

            #[cfg(feature = "usb-raw")]
            Device(event) => self.device_event(event),

            #[cfg(feature = "usb-raw")]
            SsidDialog(ssid_dialog_message) => {
                return self.ssid_dialog.update(ssid_dialog_message);
            }

            #[cfg(feature = "usb-raw")]
            ResetSsid(serial_number) => {
                return reset_ssid(serial_number);
            }

            #[cfg(feature = "usb-raw")]
            SsidSpecSent(result) => match result {
                Ok(_) => {
                    self.ssid_dialog.hide_modal();
                    self.info_row
                        .add_info_message(Info("Wi-Fi Setup sent via USB".to_string()));
                }
                Err(e) => {
                    self.ssid_dialog.enable_widgets_and_hide_spinner();
                    self.ssid_dialog.set_error(e.clone());
                    self.info_row.add_info_message(Error(
                        "Error sending Wi-Fi Setup via USB".to_string(),
                        e,
                    ));
                }
            },
        }

        Task::none()
    }

    /*
       +-window-------------------------------------------------------------------------------+
       |  +-content(main_col)---------------------------------------------------------------+ |
       |  | +-hardware-view---------------------------------------------------------------+ | |
       |  | |                                                                             | | |
       |  | |                                                                             | | |
       |  | |                                                                             | | |
       |  | +-----------------------------------------------------------------------------+ | |
       |  | +-info-row--------------------------------------------------------------------+ | |
       |  | |                                                                             | | |
       |  | +-----------------------------------------------------------------------------+ | |
       |  +---------------------------------------------------------------------------------+ |
       +--------------------------------------------------------------------------------------+
    */
    fn view(&self) -> Element<Message> {
        let main_col = Column::new()
            .push(
                self.hardware_view
                    .view(self.layout_selector.get(), &self.hardware_target),
            )
            .push(self.info_row.view(
                self.unsaved_changes,
                &self.layout_selector,
                &self.hardware_view,
                &self.hardware_target,
                #[cfg(feature = "usb-raw")]
                &self.known_devices,
            ));

        let content = container(main_col)
            .height(Length::Fill)
            .width(Length::Fill)
            .padding(Padding::new(0.0))
            .align_x(iced::alignment::Horizontal::Center)
            .center_x(Length::Fill)
            .center_y(Length::Fill);

        #[cfg(any(feature = "iroh", feature = "tcp"))]
        if self.connect_dialog.show_modal {
            return modal(
                content,
                self.connect_dialog.view(),
                ConnectDialog(HideConnectDialog),
            );
        }

        #[cfg(feature = "usb-raw")]
        if self.ssid_dialog.show_modal {
            return modal(content, self.ssid_dialog.view(), SsidDialog(HideSsidDialog));
        }

        if self.modal_handler.show_modal {
            return modal(
                content,
                self.modal_handler.view(),
                Modal(InfoDialogMessage::HideModal),
            );
        }

        content.into()
    }

    /// Subscribe to events from Hardware, from Windows and timings for StatusRow
    fn subscription(&self) -> Subscription<Message> {
        #[allow(unused_mut)]
        let mut subscriptions = vec![
            iced::event::listen().map(WindowEvent),
            self.modal_handler.subscription().map(Modal), // Handle Esc key event for modal
            self.info_row.subscription().map(InfoRow),
            self.hardware_view
                .subscription(&self.hardware_target)
                .map(Hardware),
            #[cfg(feature = "usb-raw")]
            hardware_menu::subscription().map(Device),
        ];

        // Handle Keyboard events for ConnectDialog
        #[cfg(any(feature = "iroh", feature = "tcp"))]
        subscriptions.push(self.connect_dialog.subscription().map(ConnectDialog));

        #[cfg(feature = "usb-raw")]
        subscriptions.push(self.ssid_dialog.subscription().map(SsidDialog));

        Subscription::batch(subscriptions)
    }

    #[cfg(feature = "usb-raw")]
    /// Process messages related to USB raw discovery of attached devices
    fn device_event(&mut self, event: DeviceEvent) {
        match event {
            DeviceEvent::DeviceFound(method, hardware_description, wifi_details) => {
                self.info_row
                    .add_info_message(Info("USB Device Found".to_string()));
                self.known_devices.insert(
                    hardware_description.details.serial.clone(),
                    KnownDevice::Porky(method, hardware_description, wifi_details),
                );
            }
            DeviceEvent::DeviceLost(hardware_description) => {
                self.info_row
                    .add_info_message(Info("USB Device Lost".to_string()));
                self.known_devices
                    .remove(&hardware_description.details.serial);
            }
            DeviceEvent::Error(e) => {
                self.info_row
                    .add_info_message(Error("Connection Error".to_string(), e.clone()));
            }
        }
    }
}

/// Determine the hardware target based on command line options
#[allow(unused_variables)]
fn get_hardware_target(matches: &ArgMatches) -> HardwareTarget {
    #[allow(unused_mut)]
    let mut target = HardwareTarget::default();

    #[cfg(feature = "iroh")]
    if let Some(node_str) = matches.get_one::<String>("nodeid") {
        if let Ok(nodeid) = NodeId::from_str(node_str) {
            target = HardwareTarget::Iroh(nodeid, None);
        } else {
            eprintln!("Could not create a NodeId for IrohNet from '{}'", node_str);
        }
    }

    #[cfg(feature = "tcp")]
    if let Some(ip_str) = matches.get_one::<String>("ip") {
        if let Ok(tcp_target) = parse_ip_string(ip_str) {
            target = tcp_target;
        }
    }

    target
}

#[cfg(feature = "tcp")]
fn parse_ip_string(ip_str: &str) -> anyhow::Result<HardwareTarget> {
    let (ip_str, port_str) = ip_str
        .split_once(':')
        .ok_or(anyhow::anyhow!("Could not parse ip:port"))?;
    let ip = std::net::IpAddr::from_str(ip_str)?;
    let port = u16::from_str(port_str)?;
    Ok(HardwareTarget::Tcp(ip, port))
}

#[cfg(not(target_arch = "wasm32"))]
/// Parse the command line arguments using clap
fn get_matches() -> ArgMatches {
    let app = clap::Command::new(env!("CARGO_BIN_NAME")).version(env!("CARGO_PKG_VERSION"));

    let app = app.about("'piggui' - Pi GPIO GUI for interacting with Raspberry Pi GPIO Hardware");

    #[cfg(feature = "iroh")]
    let app = app.arg(
        Arg::new("nodeid")
            .short('n')
            .long("nodeid")
            .num_args(1)
            .number_of_values(1)
            .value_name("NODEID")
            .help("Node Id of a piglet instance to connect to"),
    );

    #[cfg(feature = "tcp")]
    let app = app.arg(
        Arg::new("ip")
            .short('i')
            .long("ip")
            .num_args(1)
            .number_of_values(1)
            .value_name("IP")
            .help("'IP:port' of a piglet instance to connect to"),
    );

    let app = app.arg(
        Arg::new("config-file")
            .num_args(0..)
            .help("Path of a '.pigg' config file to load"),
    );

    app.get_matches()
}
