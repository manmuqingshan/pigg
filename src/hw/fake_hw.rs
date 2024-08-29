use rand::Rng;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
/// Fake Implementation of GPIO for hosts that don't have GPIO (Linux, macOS, Windows)
use std::{io, thread};

use crate::hw::{BCMPinNumber, PinFunction, PinLevel};
use crate::hw_definition::description::{
    HardwareDetails, PinDescription, PinDescriptionSet, PinNumberingScheme,
};

use super::Hardware;
use super::HardwareDescription;
use crate::hw::pin_descriptions::*;
use crate::hw_definition::config::LevelChange;

/// FakeHW Pins - mimicking Model the 40 pin GPIO
//noinspection DuplicatedCode
const FAKE_PIN_DESCRIPTIONS: [PinDescription; 40] = [
    PIN_1, PIN_2, PIN_3, PIN_4, PIN_5, PIN_6, PIN_7, PIN_8, PIN_9, PIN_10, PIN_11, PIN_12, PIN_13,
    PIN_14, PIN_15, PIN_16, PIN_17, PIN_18, PIN_19, PIN_20, PIN_21, PIN_22, PIN_23, PIN_24, PIN_25,
    PIN_26, PIN_27, PIN_28, PIN_29, PIN_30, PIN_31, PIN_32, PIN_33, PIN_34, PIN_35, PIN_36, PIN_37,
    PIN_38, PIN_39, PIN_40,
];

pub struct HW;

pub fn get() -> impl Hardware {
    HW {}
}

impl Hardware for HW {
    fn description(&self) -> io::Result<HardwareDescription> {
        Ok(HardwareDescription {
            details: HardwareDetails {
                hardware: "NotAPi".to_string(),
                revision: "Unknown".to_string(),
                serial: "Unknown".to_string(),
                model: "Fake Hardware".to_string(),
            },
            pins: PinDescriptionSet {
                pin_numbering: PinNumberingScheme::Rows,
                pins: FAKE_PIN_DESCRIPTIONS.to_vec(),
            },
        })
    }

    fn apply_pin_config<C>(
        &mut self,
        bcm_pin_number: BCMPinNumber,
        pin_function: &PinFunction,
        mut callback: C,
    ) -> io::Result<()>
    where
        C: FnMut(BCMPinNumber, LevelChange) + Send + Sync + Clone + 'static,
    {
        if let PinFunction::Input(_) = pin_function {
            thread::spawn(move || {
                let mut rng = rand::thread_rng();
                loop {
                    let level: bool = rng.gen();
                    #[allow(clippy::unwrap_used)]
                    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
                    callback(bcm_pin_number, LevelChange::new(level, now));
                    thread::sleep(Duration::from_millis(666));
                }
            });
        }
        Ok(())
    }

    /// Read the input level of an input using the bcm pin number
    fn get_input_level(&self, _bcm_pin_number: BCMPinNumber) -> io::Result<PinLevel> {
        Ok(true)
    }

    /// Set the level of a Hardware Output using the bcm pin number
    fn set_output_level(
        &mut self,
        _bcm_pin_number: BCMPinNumber,
        _level: PinLevel,
    ) -> io::Result<()> {
        Ok(())
    }
}
