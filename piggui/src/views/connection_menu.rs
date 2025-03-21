#[cfg(any(feature = "iroh", feature = "tcp"))]
use crate::views::connect_dialog::ConnectDialogMessage;
use crate::views::hardware_view::HardwareView;
use crate::views::info_dialog::InfoDialogMessage::HardwareDetailsModal;
use crate::views::info_row::{menu_bar_button, menu_button_style};
use crate::Message;
use iced::widget::{button, text};
use iced::{Length, Renderer, Theme};
use iced_aw::menu::{Item, Menu};
use pignet::HardwareConnection::*;
use std::collections::HashMap;

/// Create the view that represents the clickable button that shows what hardware is connected
pub fn view<'a>(hardware_view: &'a HardwareView) -> Item<'a, Message, Theme, Renderer> {
    // Conditionally render menu items based on hardware features
    let mut menu_items: Vec<Item<'a, Message, _, _>> = vec![];

    #[cfg(any(feature = "iroh", feature = "tcp", not(target_arch = "wasm32")))]
    let disconnect: Item<'a, Message, _, _> = Item::<Message, Theme, Renderer>::new(
        button("Disconnect")
            .width(Length::Fill)
            .on_press(Message::Disconnect)
            .style(menu_button_style),
    );

    #[cfg(any(feature = "iroh", feature = "tcp"))]
    let connect: Item<'a, Message, _, _> = Item::new(
        button("Connect to remote Pi ...")
            .width(Length::Fill)
            .on_press(Message::ConnectDialog(
                ConnectDialogMessage::ShowConnectDialog,
            ))
            .style(menu_button_style),
    );

    if let Some(hardware_description) = hardware_view.get_description() {
        let show_details = Item::new(
            button("Display Device Details...")
                .on_press(Message::Modal(HardwareDetailsModal(
                    hardware_description.details.clone(),
                    HashMap::default(),
                )))
                .width(Length::Fill)
                .style(menu_button_style),
        );
        menu_items.push(show_details);
    }

    match hardware_view.get_hardware_connection() {
        NoConnection => {
            #[cfg(any(feature = "iroh", feature = "tcp"))]
            menu_items.push(connect);
        }
        #[cfg(not(target_arch = "wasm32"))]
        Local => {
            #[cfg(any(feature = "iroh", feature = "tcp"))]
            menu_items.push(connect);
            menu_items.push(disconnect);
        }
        #[cfg(any(feature = "iroh", feature = "tcp", feature = "usb"))]
        _ => {
            #[cfg(any(feature = "iroh", feature = "tcp"))]
            menu_items.push(connect);
            menu_items.push(disconnect); // Can disconnect from local also
        }
    }

    let model_string = format!(
        "{}: {}",
        hardware_view.get_hardware_connection().name(),
        hardware_view.hw_model().unwrap_or(""),
    );
    Item::with_menu(
        button(text(model_string))
            .on_press(Message::MenuBarButtonClicked) // Needed for highlighting
            .style(menu_bar_button),
        Menu::new(menu_items).width(215.0),
    )
}
