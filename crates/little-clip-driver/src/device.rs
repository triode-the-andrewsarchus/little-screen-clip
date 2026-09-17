//! Trofeo Vision USB hardware interface and protocol communication.

use rusb::{DeviceHandle, GlobalContext};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info};

/// Winbond Vendor ID (0x0416).
pub const VID_WINBOND: u16 = 0x0416;
/// Trofeo Vision Product ID (0x5408).
pub const PID_TROFEO: u16 = 0x5408;

/// USB OUT bulk endpoint for sending handshake and frame data.
pub const EP_OUT: u8 = 0x09;
/// USB IN bulk endpoint for receiving handshake responses and ACKs.
pub const EP_IN: u8 = 0x81;
/// Default USB interface index.
pub const USB_INTERFACE: u8 = 0;

/// Fixed chunk packet size in bytes.
pub const CHUNK_SIZE: usize = 512;
/// Size of chunk header preceding image bytes.
pub const CHUNK_HEADER_SIZE: usize = 16;
/// Usable image payload capacity per chunk packet.
pub const CHUNK_DATA_SIZE: usize = 496;
/// Bulk burst write transfer size.
pub const USB_BURST_WRITE_SIZE: usize = 4096;
/// Maximum allowed single frame JPEG payload size in bytes.
pub const MAX_JPEG_PAYLOAD_BYTES: usize = 450_000;

/// Trofeo Vision protocol requires chunk count to be padded to multiples of 4 (2048-byte burst alignment).
pub const CHUNK_BURST_ALIGNMENT: usize = 4;
/// Fixed size of the USB ACK response packet in bytes.
pub const USB_ACK_SIZE: usize = 512;

fn frame_chunk_layout(total_size: usize) -> (usize, usize) {
    if total_size == 0 {
        return (1, 0);
    }

    let num_chunks = total_size.div_ceil(CHUNK_DATA_SIZE);
    let last_chunk_data = total_size - ((num_chunks - 1) * CHUNK_DATA_SIZE);
    (num_chunks, last_chunk_data)
}

/// USB timeout for handshake requests.
pub const TIMEOUT_HANDSHAKE: Duration = Duration::from_millis(1500);
/// USB timeout for bulk frame writes.
pub const TIMEOUT_WRITE: Duration = Duration::from_millis(5000);
/// USB timeout for bulk ACK responses.
pub const TIMEOUT_READ: Duration = Duration::from_millis(1500);

/// Errors encountered when communicating with Trofeo Vision USB hardware.
#[derive(Debug, Error)]
pub enum DriverError {
    /// Trofeo Vision display device was not found on the USB bus.
    #[error("Device not found (0416:5408)")]
    DeviceNotFound,
    /// Underlying USB transport error from `rusb`.
    #[error("USB error: {0}")]
    Usb(#[from] rusb::Error),
    /// Device failed initial handshake or returned invalid response signature.
    #[error("Handshake failed: {0}")]
    HandshakeFailed(String),
    /// Rendered frame payload exceeded maximum allowable size limit.
    #[error("Payload too large: {size} bytes (limit {MAX_JPEG_PAYLOAD_BYTES} bytes)")]
    PayloadTooLarge { size: usize },
    /// Bulk write transfer or ACK read failed.
    #[error("Frame write failed: {0}")]
    WriteFailed(String),
}

/// Hardware identification and handshake parameters reported by the display.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// USB Vendor ID.
    pub vid: u16,
    /// USB Product ID.
    pub pid: u16,
    /// Hardware product model code returned by handshake.
    pub pm: u8,
    /// Hardware sub-model code returned by handshake.
    pub sub: u8,
    /// Chunk protocol command index negotiated during handshake.
    pub chunk_cmd: u8,
    /// Display resolution width in pixels.
    pub width: u32,
    /// Display resolution height in pixels.
    pub height: u32,
}

/// Active USB driver session connected to a Trofeo Vision secondary display.
pub struct TrofeoDriver {
    handle: DeviceHandle<GlobalContext>,
    /// Device parameters negotiated during handshake.
    pub info: DeviceInfo,
    serial: String,
}

impl TrofeoDriver {
    /// Discovers and opens the Trofeo Vision 9.16 device.
    pub fn open() -> Result<Self, DriverError> {
        info!(
            "Searching for Trofeo Vision USB device ({:04x}:{:04x})...",
            VID_WINBOND, PID_TROFEO
        );

        let handle = rusb::open_device_with_vid_pid(VID_WINBOND, PID_TROFEO)
            .ok_or(DriverError::DeviceNotFound)?;

        // Detach kernel driver if active
        if handle.kernel_driver_active(USB_INTERFACE).unwrap_or(false) {
            debug!("Detaching kernel driver on interface {}", USB_INTERFACE);
            let _ = handle.detach_kernel_driver(USB_INTERFACE);
        }

        // Set configuration 1 and claim interface 0
        debug!("Setting active configuration...");
        let _ = handle.set_active_configuration(1);

        debug!("Claiming interface {}...", USB_INTERFACE);
        handle.claim_interface(USB_INTERFACE)?;

        // Read serial number now so we can detect stale handles later.
        let serial = handle
            .read_serial_number_string_ascii(&handle.device().device_descriptor()?)
            .unwrap_or_default();

        let mut driver = Self {
            handle,
            info: DeviceInfo {
                vid: VID_WINBOND,
                pid: PID_TROFEO,
                pm: 0,
                sub: 0,
                chunk_cmd: 1,
                width: 1920,
                height: 462,
            },
            serial,
        };

        driver.do_handshake()?;
        Ok(driver)
    }

    /// Performs the 2048-byte query -> 512-byte reply handshake.
    pub fn do_handshake(&mut self) -> Result<(), DriverError> {
        debug!("Performing handshake...");

        let mut query = vec![0u8; 2048];
        query[0] = 0x02;
        query[1] = 0xFF;
        query[8] = 0x01;

        // Write handshake packet to EP_OUT
        let written = self.handle.write_bulk(EP_OUT, &query, TIMEOUT_HANDSHAKE)?;
        debug!("Wrote {} handshake bytes", written);

        // Read 512-byte response from EP_IN
        let mut response = vec![0u8; 512];
        let read = self
            .handle
            .read_bulk(EP_IN, &mut response, TIMEOUT_HANDSHAKE)?;
        debug!("Read {} handshake response bytes", read);

        if read < 37
            || response[0] != 3
            || response[1] != 0xFF
            || (response[8] != 1 && response[8] != 2)
        {
            return Err(DriverError::HandshakeFailed(format!(
                "Validation mismatch (read={}, [0]={:#x}, [1]={:#x}, [8]={:#x})",
                read, response[0], response[1], response[8]
            )));
        }

        let chunk_cmd = response[8];
        let (pm, sub) = if chunk_cmd == 2 {
            let pm = 49 + response[20];
            let sub = if response.len() > 22 { response[22] } else { 0 };
            (pm, sub)
        } else {
            let raw_pm = if response[20] <= 3 { 1 } else { response[20] };
            let pm = 64 + raw_pm;
            let raw_sub = if response.len() > 22 { response[22] } else { 0 };
            (pm, raw_sub + 1)
        };

        info!(
            "Trofeo handshake successful: variant={} (cmd={}), PM={}, SUB={}",
            if chunk_cmd == 2 { "LY1" } else { "LY" },
            chunk_cmd,
            pm,
            sub
        );

        self.info.pm = pm;
        self.info.sub = sub;
        self.info.chunk_cmd = chunk_cmd;
        Ok(())
    }

    /// Check that the underlying USB handle still points to the same
    /// live device by querying its descriptor and matching the serial number.
    /// Returns an error immediately if the device has reset and re-enumerated
    /// (stale fd case) or if it has fully disconnected.
    pub fn check_alive(&self) -> Result<(), DriverError> {
        let timeout = Duration::from_millis(500);
        let desc = self.handle.device().device_descriptor()?;
        let current_serial = self
            .handle
            .read_serial_number_string_ascii(&desc)
            .unwrap_or_default();
        if !self.serial.is_empty() && !current_serial.is_empty() && current_serial != self.serial {
            return Err(DriverError::Usb(rusb::Error::NoDevice));
        }
        // Also do a harmless control read (get device descriptor) to probe the bus.
        let mut buf = [0u8; 18];
        self.handle
            .read_control(
                rusb::request_type(
                    rusb::Direction::In,
                    rusb::RequestType::Standard,
                    rusb::Recipient::Device,
                ),
                0x06,   // GET_DESCRIPTOR
                0x0100, // Device descriptor
                0,
                &mut buf,
                timeout,
            )
            .map_err(DriverError::Usb)?;
        Ok(())
    }

    /// Prepares chunked USB frame from JPEG bytes and streams it to the display.
    pub fn send_frame(&mut self, jpeg_bytes: &[u8]) -> Result<(), DriverError> {
        let total_size = jpeg_bytes.len();
        if total_size > MAX_JPEG_PAYLOAD_BYTES {
            return Err(DriverError::PayloadTooLarge { size: total_size });
        }

        let (num_chunks, last_chunk_data) = frame_chunk_layout(total_size);

        // Pad chunks to multiple of CHUNK_BURST_ALIGNMENT (4)
        let remainder = num_chunks % CHUNK_BURST_ALIGNMENT;
        let padded_chunks = if remainder != 0 {
            num_chunks + (CHUNK_BURST_ALIGNMENT - remainder)
        } else {
            num_chunks
        };

        let mut buffer = vec![0u8; padded_chunks * CHUNK_SIZE];

        for i in 0..num_chunks {
            let offset = i * CHUNK_SIZE;
            let is_last = i == num_chunks - 1;
            let data_len = if is_last {
                last_chunk_data
            } else {
                CHUNK_DATA_SIZE
            };

            // 16-byte chunk header
            buffer[offset] = 0x01;
            buffer[offset + 1] = 0xFF;
            buffer[offset + 2..offset + 6].copy_from_slice(&(total_size as u32).to_le_bytes());
            buffer[offset + 6..offset + 8].copy_from_slice(&(data_len as u16).to_le_bytes());
            buffer[offset + 8] = self.info.chunk_cmd; // chunk_cmd from handshake!
            buffer[offset + 9..offset + 11].copy_from_slice(&(num_chunks as u16).to_le_bytes());
            buffer[offset + 11..offset + 13].copy_from_slice(&(i as u16).to_le_bytes());

            let src_offset = i * CHUNK_DATA_SIZE;
            buffer[offset + CHUNK_HEADER_SIZE..offset + CHUNK_HEADER_SIZE + data_len]
                .copy_from_slice(&jpeg_bytes[src_offset..src_offset + data_len]);
        }

        // Send in bursts of 4096 bytes
        let total_bytes = buffer.len();
        let mut pos = 0;
        while pos < total_bytes {
            let remaining = total_bytes - pos;
            let write_size = if remaining >= USB_BURST_WRITE_SIZE {
                USB_BURST_WRITE_SIZE
            } else {
                remaining.min(2048)
            };

            let written =
                self.handle
                    .write_bulk(EP_OUT, &buffer[pos..pos + write_size], TIMEOUT_WRITE)?;
            if written != write_size {
                return Err(DriverError::WriteFailed(format!(
                    "short USB bulk write: expected {write_size} bytes, wrote {written}"
                )));
            }
            pos += write_size;
        }

        // Read ACK packet and propagate errors — a failure here means
        // the device is gone/stale and the caller should trigger a reconnect.
        let mut ack = vec![0u8; USB_ACK_SIZE];
        self.handle
            .read_bulk(EP_IN, &mut ack, TIMEOUT_READ)
            .map_err(|e| DriverError::WriteFailed(format!("ACK read failed: {e}")))?;

        debug!(
            "Frame sent successfully ({} bytes, {} chunks)",
            total_size, num_chunks
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_chunk_layout_avoids_empty_final_chunk_for_exact_multiples() {
        assert_eq!(frame_chunk_layout(0), (1, 0));
        assert_eq!(frame_chunk_layout(1), (1, 1));
        assert_eq!(frame_chunk_layout(CHUNK_DATA_SIZE), (1, CHUNK_DATA_SIZE));
        assert_eq!(frame_chunk_layout(CHUNK_DATA_SIZE + 1), (2, 1));
    }
}

impl Drop for TrofeoDriver {
    fn drop(&mut self) {
        let _ = self.handle.release_interface(USB_INTERFACE);
    }
}
