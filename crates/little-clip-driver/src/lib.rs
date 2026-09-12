//! USB device driver for Trofeo Vision 9.16 secondary display hardware.
//!
//! Handles device discovery, custom Winbond handshake protocols, frame chunking,
//! and bulk USB streaming to the physical 1920x462 screen.

pub mod device;

pub use device::{DriverError, TrofeoDriver, DeviceInfo, PID_TROFEO, VID_WINBOND};
