use std::collections::HashMap;
use std::fs;
use std::io;
use std::time::Duration;

/// Implementation of GPIO for raspberry pi - uses rrpal
use rppal::gpio::{Gpio, InputPin, Level, OutputPin, Trigger};

use crate::hw_definition::pin_function::PinFunction;
use crate::hw_definition::{BCMPinNumber, PinLevel};

use crate::hw::pin_descriptions::*;

use super::Hardware;
use crate::hw_definition::config::{InputPull, LevelChange};
use crate::hw_definition::description::{
    HardwareDescription, HardwareDetails, PinDescription, PinDescriptionSet, PinNumberingScheme,
};

/// Model the 40 pin GPIO connections - including Ground, 3.3V and 5V outputs
/// For now, we will use the same descriptions for all hardware
//noinspection DuplicatedCode
const GPIO_PIN_DESCRIPTIONS: [PinDescription; 40] = [
    PIN_1, PIN_2, PIN_3, PIN_4, PIN_5, PIN_6, PIN_7, PIN_8, PIN_9, PIN_10, PIN_11, PIN_12, PIN_13,
    PIN_14, PIN_15, PIN_16, PIN_17, PIN_18, PIN_19, PIN_20, PIN_21, PIN_22, PIN_23, PIN_24, PIN_25,
    PIN_26, PIN_27, PIN_28, PIN_29, PIN_30, PIN_31, PIN_32, PIN_33, PIN_34, PIN_35, PIN_36, PIN_37,
    PIN_38, PIN_39, PIN_40,
];

enum Pin {
    Input(InputPin),
    Output(OutputPin),
}

struct HW {
    configured_pins: HashMap<BCMPinNumber, Pin>,
}

/// This method is used to get a "handle" onto the Hardware implementation
pub fn get() -> impl Hardware {
    HW {
        configured_pins: Default::default(),
    }
}

impl HW {
    fn get_details() -> io::Result<HardwareDetails> {
        let mut details = HardwareDetails {
            hardware: "Unknown".to_string(),
            revision: "Unknown".to_string(),
            serial: "Unknown".to_string(),
            model: "Unknown".to_string(),
        };

        for line in fs::read_to_string("/proc/cpuinfo")?.lines() {
            match line
                .split_once(':')
                .map(|(key, value)| (key.trim(), value.trim()))
            {
                Some(("Hardware", hw)) => details.hardware = hw.to_string(),
                Some(("Revision", revision)) => details.revision = revision.to_string(),
                Some(("Serial", serial)) => details.serial = serial.to_string(),
                Some(("Model", model)) => details.model = model.to_string(),
                _ => {}
            }
        }

        Ok(details)
    }

    /// Get the time since boot as a [Duration] that should be synced with timestamp of
    /// `rppal` generated events
    fn get_time_since_boot() -> Duration {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) };
        Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
    }
}

/// Implement the [Hardware] trait for Pi hardware.
// -> Result<(), Box<dyn Error>>
impl Hardware for HW {
    /// Find the Pi hardware description
    fn description(&self) -> io::Result<HardwareDescription> {
        Ok(HardwareDescription {
            details: Self::get_details()?,
            pins: PinDescriptionSet {
                pin_numbering: PinNumberingScheme::Rows,
                pins: GPIO_PIN_DESCRIPTIONS.to_vec(),
            },
        })
    }

    /// Apply the requested config to one pin, using bcm_pin_number
    fn apply_pin_config<C>(
        &mut self,
        bcm_pin_number: BCMPinNumber,
        pin_function: &PinFunction,
        mut callback: C,
    ) -> io::Result<()>
    where
        C: FnMut(BCMPinNumber, LevelChange) + Send + Sync + Clone + 'static,
    {
        // If it was already configured, remove it
        self.configured_pins.remove(&bcm_pin_number);

        match pin_function {
            PinFunction::None => {}

            PinFunction::Input(pull) => {
                let pin = Gpio::new()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
                    .get(bcm_pin_number)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

                let mut input = match pull {
                    None | Some(InputPull::None) => pin.into_input(),
                    Some(InputPull::PullUp) => pin.into_input_pullup(),
                    Some(InputPull::PullDown) => pin.into_input_pulldown(),
                };

                // Send current input level back via callback
                let timestamp = Self::get_time_since_boot();
                callback(
                    bcm_pin_number,
                    LevelChange::new(input.read() == Level::High, timestamp),
                );

                input
                    .set_async_interrupt(
                        Trigger::Both,
                        Some(Duration::from_millis(1)),
                        move |event| {
                            callback(
                                bcm_pin_number,
                                LevelChange::new(
                                    event.trigger == Trigger::RisingEdge,
                                    event.timestamp,
                                ),
                            );
                        },
                    )
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                self.configured_pins
                    .insert(bcm_pin_number, Pin::Input(input));
            }

            PinFunction::Output(value) => {
                let pin = Gpio::new()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
                    .get(bcm_pin_number)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                let output_pin = match value {
                    Some(true) => pin.into_output_high(),
                    Some(false) => pin.into_output_low(),
                    None => pin.into_output(),
                };
                self.configured_pins
                    .insert(bcm_pin_number, Pin::Output(output_pin));
            }
        }

        Ok(())
    }

    /// Read the input level of an input using the bcm pin number
    fn get_input_level(&self, bcm_pin_number: BCMPinNumber) -> io::Result<bool> {
        match self.configured_pins.get(&bcm_pin_number) {
            Some(Pin::Input(input_pin)) => Ok(input_pin.read() == Level::High),
            _ => Err(io::Error::new(
                io::ErrorKind::Other,
                "Could not find a configured input pin",
            )),
        }
    }

    /// Write the output level of an output using the bcm pin number
    fn set_output_level(
        &mut self,
        bcm_pin_number: BCMPinNumber,
        level: PinLevel,
    ) -> io::Result<()> {
        match self.configured_pins.get_mut(&bcm_pin_number) {
            Some(Pin::Output(output_pin)) => match level {
                true => output_pin.write(Level::High),
                false => output_pin.write(Level::Low),
            },
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Could not find a configured output pin",
                ))
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::hw::Hardware;

    #[test]
    fn get_hardware() {
        let hw = super::get();
        let description = hw
            .description()
            .expect("Could not read Hardware description");
        let pins = description.pins.pins();
        assert_eq!(pins.len(), 40);
        assert_eq!(pins[0].name, "3V3")
    }
}
