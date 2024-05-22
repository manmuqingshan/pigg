use std::io;

use crate::gpio::{GPIOConfig, GPIOState};

/// There are three implementations of [`Hardware`] trait:
/// * None - used on host (macos, Linux, etc) to show and develop GUI without real HW
/// * Pi - Raspberry Pi using "rppal" crate: Should support most Pi hardware from Model B
/// * Pico - Raspberry Pi Pico Microcontroller (To Be done)
#[cfg_attr(all(feature = "pico", not(feature = "pi")), path = "pico.rs")]
#[cfg_attr(all(feature = "pi", not(feature = "pico")), path = "pi.rs")]
#[cfg_attr(not(any(feature = "pico", feature = "pi")), path = "none.rs")]
mod implementation;

pub fn get() -> impl Hardware {
    implementation::get()
}

/// [`Hardware`] is a trait to be implemented depending on the hardware we are running on, to
/// interact with any possible GPIO hardware on the device to set config and get state
#[must_use]
pub trait Hardware {
    fn apply_config(&mut self, config: &GPIOConfig) -> io::Result<()>;
    #[allow(dead_code)] // TODO remove later when used
    fn get_state(&self) -> GPIOState;
}
