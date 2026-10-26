//! Hardware layer for SteelSeries Apex keyboards on Linux.
//!
//! - [`device::Keyboard`] opens the keyboard over hidraw and sends commands.
//! - [`protocol`] holds the pure packet encoders.
//! - [`keys`] maps key names to LED codes and physical positions.
//! - [`oled`] renders text and images into 128x40 OLED frames.

pub mod color;
pub mod device;
pub mod hidraw;
pub mod keys;
pub mod model;
pub mod oled;
pub mod protocol;

pub use color::Rgb;
pub use device::{DeviceInfo, Error, Keyboard};
pub use keys::{FormFactor, KeyCode};
pub use oled::Frame;
