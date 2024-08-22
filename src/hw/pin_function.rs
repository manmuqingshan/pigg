use crate::hw_definition::pin_function::PinFunction;
use std::fmt;
use std::fmt::{Display, Formatter};

impl Display for PinFunction {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        // Remove anything after the first opening bracket of debug representation
        let full = format!("{:?}", self);
        write!(f, "{}", full.split_once('(').unwrap_or((&full, "")).0)
    }
}

#[cfg(test)]
mod test {
    use crate::hw_definition::config::InputPull::{PullDown, PullUp};
    use crate::hw_definition::pin_function::PinFunction;

    #[test]
    fn display_pin_function() {
        let functions = vec![
            PinFunction::Ground,
            PinFunction::None,
            PinFunction::Power3V3,
            PinFunction::Output(None),
            PinFunction::Output(Some(true)),
            PinFunction::Output(Some(false)),
            PinFunction::Input(None),
            PinFunction::Input(Some(PullUp)),
            PinFunction::Input(Some(PullDown)),
            PinFunction::I2C_EEPROM_ID_SC,
            PinFunction::I2C_EEPROM_ID_SD,
            PinFunction::Power3V3,
            PinFunction::Power5V,
        ];

        for function in functions {
            println!("{}", function);
        }
    }
}
