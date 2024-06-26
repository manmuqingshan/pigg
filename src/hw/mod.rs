use std::fmt;
use std::fmt::{Display, Formatter};
use std::io;

use crate::hw::config::HardwareConfig;
use chrono::{DateTime, Utc};
use pin_description::PinDescriptionSet;
use serde::{Deserialize, Serialize};

use crate::hw::pin_function::PinFunction;

pub mod config;
#[cfg(feature = "fake_hw")]
mod fake_hw;
/// There are two implementations of [`Hardware`] trait:
/// * fake_hw - used on host (macOS, Linux, etc.) to show and develop GUI without real HW
/// * pi_hw - Raspberry Pi using "rppal" crate: Should support most Pi hardware from Model B
#[cfg(feature = "pi_hw")]
mod pi_hw;
pub(crate) mod pin_description;
mod pin_descriptions;
pub(crate) mod pin_function;

/// [BCMPinNumber] is used to refer to a GPIO pin by the Broadcom Chip Number
pub type BCMPinNumber = u8;

/// [BoardPinNumber] is used to refer to a GPIO pin by the numbering of the GPIO header on the Pi
pub type BoardPinNumber = u8;
/// [PinLevel] describes whether a Pin's logical level is High(true) or Low(false)
pub type PinLevel = bool;

/// Get the implementation we will use to access the underlying hardware via the [Hardware] trait
#[cfg(feature = "pi_hw")]
pub fn get() -> impl Hardware {
    pi_hw::get()
}
#[cfg(feature = "fake_hw")]
pub fn get() -> impl Hardware {
    fake_hw::get()
}

/// [HardwareDetails] captures a number of specific details about the Hardware we are connected to
#[derive(Clone, Debug)]
pub struct HardwareDetails {
    pub hardware: String,
    pub revision: String,
    pub serial: String,
    /// A Human friendly Hardware Model description
    pub model: String,
}

impl Display for HardwareDetails {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "Hardware: {}", self.hardware)?;
        writeln!(f, "Revision: {}", self.revision)?;
        writeln!(f, "Serial: {}", self.serial)?;
        write!(f, "Model: {}", self.model)
    }
}

/// [HardwareDescription] contains details about the board we are running on and the GPIO pins
#[derive(Clone, Debug)]
pub struct HardwareDescription {
    pub details: HardwareDetails,
    pub pins: PinDescriptionSet,
}

/// LevelChange describes the change in level of an input or Output
/// - `new_level` : [PinLevel]
/// - `timestamp` : [DateTime<Utc>]
#[derive(Clone, Debug)]
pub struct LevelChange {
    #[allow(dead_code)] // For piglet - TODO remove when used
    pub new_level: PinLevel,
    #[allow(dead_code)] // For piglet - TODO remove when used
    pub timestamp: DateTime<Utc>,
}

impl LevelChange {
    /// Create a new LevelChange event with the timestamp for now
    #[allow(dead_code)] // for piglet
    pub fn new(new_level: PinLevel) -> Self {
        Self {
            new_level,
            timestamp: Utc::now(),
        }
    }
}

/// An input can be configured to have an optional pull-up or pull-down
#[derive(Debug, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub enum InputPull {
    PullUp,
    PullDown,
    None,
}

impl Display for InputPull {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            InputPull::PullUp => write!(f, "Pull Up"),
            InputPull::PullDown => write!(f, "Pull Down"),
            InputPull::None => write!(f, "None"),
        }
    }
}

/// [`Hardware`] is a trait to be implemented depending on the hardware we are running on, to
/// interact with any possible GPIO hardware on the device to set config and get state
pub trait Hardware {
    /// Return a [HardwareDescription] struct describing the hardware that we are connected to:
    /// * [HardwareDescription] such as revision etc.
    /// * [PinDescriptionSet] describing all the pins
    fn description(&self) -> io::Result<HardwareDescription>;

    /// This takes the GPIOConfig struct and configures all the pins in it
    fn apply_config<C>(&mut self, config: &HardwareConfig, callback: C) -> io::Result<()>
    where
        C: FnMut(BCMPinNumber, PinLevel) + Send + Sync + Clone + 'static,
    {
        // Config only has pins that are configured
        for (bcm_pin_number, pin_function) in &config.pins {
            let mut callback_clone = callback.clone();
            let callback_wrapper = move |pin_number, level| {
                callback_clone(pin_number, level);
            };
            self.apply_pin_config(*bcm_pin_number, pin_function, callback_wrapper)?;
        }

        Ok(())
    }

    /// Apply a new config to one specific pin
    fn apply_pin_config<C>(
        &mut self,
        bcm_pin_number: BCMPinNumber,
        pin_function: &PinFunction,
        callback: C,
    ) -> io::Result<()>
    where
        C: FnMut(BCMPinNumber, PinLevel) + Send + Sync + 'static;

    /// Read the input level of an input using its [BCMPinNumber]
    #[allow(dead_code)] // for piglet
    fn get_input_level(&self, bcm_pin_number: BCMPinNumber) -> io::Result<PinLevel>;

    /// Write the output level of an output using its [BCMPinNumber]
    #[allow(dead_code)] // for piglet
    fn set_output_level(
        &mut self,
        bcm_pin_number: BCMPinNumber,
        level_change: LevelChange,
    ) -> io::Result<()>;
}

#[cfg(test)]
mod test {
    use crate::hw;
    use crate::hw::Hardware;

    #[test]
    fn hw_can_be_got() {
        let hw = hw::get();
        assert!(hw.description().is_ok());
        println!("{:?}", hw.description().unwrap());
    }

    #[test]
    fn forty_board_pins() {
        let hw = hw::get();
        let pin_set = hw.description().unwrap().pins;
        assert_eq!(pin_set.pins().len(), 40);
    }

    #[test]
    #[cfg(feature = "gui")]
    fn bcm_pins_sort_in_order() {
        // 0-27, not counting the gpio0 and gpio1 pins with no options
        let hw = hw::get();
        let pin_set = hw.description().unwrap().pins;
        let sorted_bcm_pins = pin_set.bcm_pins_sorted();
        assert_eq!(pin_set.bcm_pins_sorted().len(), 26);
        let mut previous = 1; // we start at GPIO2
        for pin in sorted_bcm_pins {
            assert_eq!(pin.bcm_pin_number.unwrap(), previous + 1);
            previous = pin.bcm_pin_number.unwrap();
        }
    }
}
