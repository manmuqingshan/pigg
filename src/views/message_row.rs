use crate::views::dialog_styles::{NO_BORDER, NO_SHADOW};
use crate::views::hardware_styles::TOOLTIP_STYLE;
use crate::Message;
use iced::widget::tooltip::Position;
use iced::widget::{button, Button, Text, Tooltip};
use iced::{Background, Color, Element, Length, Task};
use iced_futures::Subscription;
use std::time::Duration;

/// There are three types of messages we can display in the message text in the status bar.
///
/// They are (in order of priority - highest to lowest):
/// * Error -  will remain until clicked
/// * Warning - will remain until clicked
/// * Info - will disappear after a short time
///
/// Messages of higher priority are shown before those of lower priority.
/// Clicking a message removes it and shows next message.
#[derive(Debug, Clone, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u8)]
pub enum MessageMessage {
    Error(String, String) = 2,
    Warning(String) = 1,
    Info(String) = 0,
}

/// [MessageRow] reacts to these message types
#[derive(Debug, Clone)]
pub enum MessageRowMessage {
    ShowStatusMessage(MessageMessage),
    ClearStatusMessage,
}

#[derive(Default)]
pub struct MessageQueue {
    queue: Vec<MessageMessage>,
    current_message: Option<MessageMessage>,
}

impl MessageQueue {
    /// Add a new [MessageMessage] to be displayed
    /// If none is being displayed currently, set it as the one that will be displayed by view().
    /// If a message is currently being displayed, add this one to the queue.
    pub fn add_message(&mut self, message: MessageMessage) {
        match self.current_message {
            None => self.current_message = Some(message),
            Some(_) => {
                self.queue.push(message);
                self.queue.sort();
            }
        }
    }

    /// Clear the current message being displayed.
    /// If there is another message in the queue then it sets that as the new message to be shown
    /// If there is no other message queues to be shown, then set to None and no message is shown
    pub fn clear_message(&mut self) {
        if self.queue.is_empty() {
            self.current_message = None;
        } else {
            self.current_message = self.queue.pop();
        }
    }

    /// Are there any [MessageMessage]  of type Info in the queue waiting to be displayed?
    pub fn showing_info_message(&self) -> bool {
        matches!(self.current_message, Some(MessageMessage::Info(_)))
    }
}

pub struct MessageRow {
    message_queue: MessageQueue,
}

impl MessageRow {
    /// Create a new [MessageRow]
    pub fn new() -> Self {
        MessageRow {
            message_queue: MessageQueue::default(),
        }
    }

    pub fn add_message(&mut self, msg: MessageMessage) {
        self.message_queue.add_message(msg);
    }

    /// Update the state and do actions depending on the [MessageRowMessage] sent
    pub fn update(&mut self, message: MessageRowMessage) -> Task<Message> {
        match message {
            MessageRowMessage::ShowStatusMessage(msg) => self.add_message(msg),
            MessageRowMessage::ClearStatusMessage => self.message_queue.clear_message(),
        }

        Task::none()
    }

    /// Create the view that represents a status row at the bottom of the screen
    pub fn view(&self) -> Element<MessageRowMessage> {
        let (text_color, message_text, details) = match &self.message_queue.current_message {
            None => (Color::TRANSPARENT, "".to_string(), ""),
            Some(msg) => match msg {
                MessageMessage::Error(text, details) => {
                    (Color::from_rgb8(255, 0, 0), text.into(), details as &str)
                }
                MessageMessage::Warning(text) => (
                    Color::new(1.0, 0.647, 0.0, 1.0),
                    text.into(),
                    "No additional details",
                ),
                MessageMessage::Info(text) => (Color::WHITE, text.into(), "No additional details"),
            },
        };

        let button_style = button::Style {
            background: Some(Background::Color(Color::TRANSPARENT)),
            text_color,
            border: NO_BORDER,
            shadow: NO_SHADOW,
        };

        let button = Button::new(Text::new(message_text))
            .on_press(MessageRowMessage::ClearStatusMessage)
            .style(move |_theme, _status| button_style)
            .clip(true)
            .height(Length::Shrink)
            .width(Length::Shrink);

        Tooltip::new(button, details, Position::Top)
            .gap(4.0)
            .style(|_| TOOLTIP_STYLE)
            .into()
    }

    pub fn subscription(&self) -> Subscription<MessageRowMessage> {
        if self.message_queue.showing_info_message() {
            iced::time::every(Duration::from_secs(3)).map(|_| MessageRowMessage::ClearStatusMessage)
        } else {
            Subscription::none()
        }
    }
}

#[cfg(test)]
mod test {
    use crate::views::message_row::MessageMessage::{Error, Info, Warning};
    use crate::views::message_row::MessageQueue;

    #[test]
    fn errors_first() {
        let mut queue: MessageQueue = Default::default();

        queue.add_message(Info("shown".into()));
        assert!(queue.showing_info_message());
        assert_eq!(queue.current_message, Some(Info("shown".into())));

        // Add three more messages that should be queued up
        queue.add_message(Info("last".into()));
        queue.add_message(Error("first".into(), "Details".into()));
        queue.add_message(Warning("middle".into()));
        assert_eq!(queue.queue.len(), 3);

        // clear the current message, it should be replaced by highest priority message in the queue
        queue.clear_message();
        assert_eq!(
            queue.current_message,
            Some(Error("first".into(), "Details".into()))
        );
        assert_eq!(queue.queue.len(), 2);

        queue.clear_message();
        assert_eq!(queue.current_message, Some(Warning("middle".into())));
        assert_eq!(queue.queue.len(), 1);

        queue.clear_message();
        assert_eq!(queue.current_message, Some(Info("last".into())));
        assert_eq!(queue.queue.len(), 0);
    }
}
