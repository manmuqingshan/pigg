#[cfg(any(feature = "iroh", feature = "tcp"))]
use crate::views::connect_dialog_handler::ConnectDialogMessage;
use crate::views::hardware_view::{HardwareTarget, HardwareView};
use crate::views::info_row::{
    MENU_BAR_BUTTON_HOVER_STYLE, MENU_BAR_BUTTON_STYLE, MENU_BUTTON_STYLE, MENU_STYLE,
};
use crate::HardwareTarget::*;
use crate::{Message, ModalMessage};
use iced::widget::button::Status::Hovered;
use iced::widget::{Button, Text};
use iced::{Element, Length, Renderer, Theme};
use iced_aw::menu::{Item, Menu, MenuBar};

/// Create the view that represents the clickable button that shows what hardware is connected
pub fn view<'a>(
    hardware_view: &'a HardwareView,
    hardware_target: &HardwareTarget,
) -> Element<'a, Message, Theme, Renderer> {
    let model = match hardware_view.hw_model() {
        None => "hardware: none".to_string(),
        Some(model) => match hardware_target {
            NoHW => "hardware: none".to_string(),
            Local => format!("hardware: {}@Local", model),
            #[cfg(feature = "iroh")]
            Iroh(_, _) => format!("hardware: {}@Remote", model),
            #[cfg(feature = "tcp")]
            Tcp(_, _) => format!("hardware: {}@Remote", model),
        },
    };

    // Conditionally render menu items based on hardware features
    let mut menu_items: Vec<Item<'a, Message, _, _>> = vec![];

    let disconnect: Item<'a, Message, _, _> = Item::new(
        Button::new("Disconnect")
            .width(Length::Fill)
            .on_press(Message::ConnectRequest(NoHW))
            .style(|_, _| MENU_BUTTON_STYLE),
    );

    #[cfg(any(feature = "iroh", feature = "tcp"))]
    let connect: Item<'a, Message, _, _> = Item::new(
        Button::new("Connect to remote Pi ...")
            .width(Length::Fill)
            .on_press(Message::ConnectDialog(
                ConnectDialogMessage::ShowConnectDialog,
            ))
            .style(move |_, _| MENU_BUTTON_STYLE),
    );

    #[cfg(not(target_arch = "wasm32"))]
    let connect_local: Item<'a, Message, _, _> = Item::new(
        Button::new("Connect to local")
            .on_press(Message::ConnectRequest(Local))
            .style(move |_, _| MENU_BUTTON_STYLE)
            .width(Length::Fill),
    );

    let show_details = Item::new(
        Button::new(Text::new("Show details..."))
            .on_press(Message::ModalHandle(ModalMessage::HardwareDetailsModal))
            .width(Length::Fill)
            .style(move |_, _| MENU_BUTTON_STYLE),
    );

    match hardware_target {
        NoHW => {
            #[cfg(any(feature = "iroh", feature = "tcp"))]
            menu_items.push(connect);
            #[cfg(not(target_arch = "wasm32"))]
            menu_items.push(connect_local);
        }
        #[cfg(not(target_arch = "wasm32"))]
        Local => {
            menu_items.push(disconnect);
            menu_items.push(show_details);
        }
        #[cfg(feature = "iroh")]
        Iroh(_, _) => {
            menu_items.push(disconnect);
            menu_items.push(show_details);
        }
        #[cfg(feature = "tcp")]
        Tcp(_, _) => {
            menu_items.push(disconnect);
            menu_items.push(show_details);
        }
    }

    #[cfg(feature = "discovery")]
    menu_items.push(Item::new(
        Button::new("Search for Pi's on local network...")
            .width(Length::Fill)
            .style(move |theme, status| MENU_BUTTON_STYLE),
    ));

    let menu_root = Item::with_menu(
        Button::new(Text::new(model))
            .style(move |_theme, status| {
                if status == Hovered {
                    MENU_BAR_BUTTON_HOVER_STYLE
                } else {
                    MENU_BAR_BUTTON_STYLE
                }
            })
            .on_press(Message::MenuBarButtonClicked),
        Menu::new(menu_items).width(235.0).offset(10.0),
    );

    // TODO define Style as a const like MENU_BAR_BUTTON_STYLE
    // which is an unused import right now?
    MenuBar::new(vec![menu_root])
        .style(|_, _| MENU_STYLE)
        .into()
}
