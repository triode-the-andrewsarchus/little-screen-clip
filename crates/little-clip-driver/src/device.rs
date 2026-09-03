use rusb::{DeviceHandle, GlobalContext};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info};

pub const VID_WINBOND: u16 = 0x0416;
pub const PID_TROFEO: u16 = 0x5408;

pub const EP_OUT: u8 = 0x09;
pub const EP_IN: u8 = 0x81;
pub const USB_INTERFACE: u8 = 0;

pub const CHUNK_SIZE: usize = 512;
pub const CHUNK_HEADER_SIZE: usize = 16;
pub const CHUNK_DATA_SIZE: usize = 496;
pub const USB_BURST_WRITE_SIZE: usize = 4096;
pub const MAX_JPEG_PAYLOAD_BYTES: usize = 450_000;

pub const TIMEOUT_HANDSHAKE: Duration = Duration::from_millis(1500);
pub const TIMEOUT_WRITE: Duration = Duration::from_millis(5000);
pub const TIMEOUT_READ: Duration = Duration::from_millis(1500);

#[derive(Debug, Error)]
pub enum DriverError {
    #[error("Device not found (0416:5408)")]
    DeviceNotFound,
    #[error("USB error: {0}")]
    Usb(#[from] rusb::Error),
    #[error("Handshake failed: {0}")]
    HandshakeFailed(String),
    #[error("Payload too large: {size} bytes (limit {MAX_JPEG_PAYLOAD_BYTES} bytes)")]
    PayloadTooLarge { size: usize },
    #[error("Frame write failed: {0}")]
    WriteFailed(String),
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub vid: u16,
    pub pid: u16,
    pub pm: u8,
    pub sub: u8,
    pub chunk_cmd: u8,
    pub width: u32,
    pub height: u32,
}

pub struct TrofeoDriver {
    handle: DeviceHandle<GlobalContext>,
    pub info: DeviceInfo,
}

impl TrofeoDriver {
    /// Discovers and opens the Trofeo Vision 9.16 device.
    pub fn open() -> Result<Self, DriverError> {
        info!("Searching for Trofeo Vision USB device ({:04x}:{:04x})...", VID_WINBOND, PID_TROFEO);

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
        let read = self.handle.read_bulk(EP_IN, &mut response, TIMEOUT_HANDSHAKE)?;
        debug!("Read {} handshake response bytes", read);

        if read < 37 || response[0] != 3 || response[1] != 0xFF || (response[8] != 1 && response[8] != 2) {
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

    /// Prepares chunked USB frame from JPEG bytes and streams it to the display.
    pub fn send_frame(&mut self, jpeg_bytes: &[u8]) -> Result<(), DriverError> {
        let total_size = jpeg_bytes.len();
        if total_size > MAX_JPEG_PAYLOAD_BYTES {
            return Err(DriverError::PayloadTooLarge { size: total_size });
        }

        let num_chunks = (total_size / CHUNK_DATA_SIZE) + 1;
        let last_chunk_data = total_size % CHUNK_DATA_SIZE;

        // Pad chunks to multiple of 4
        let remainder = num_chunks % 4;
        let padded_chunks = if remainder != 0 {
            num_chunks + (4 - remainder)
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

            self.handle.write_bulk(
                EP_OUT,
                &buffer[pos..pos + write_size],
                TIMEOUT_WRITE,
            )?;
            pos += USB_BURST_WRITE_SIZE;
        }

        // Read 512-byte ACK
        let mut ack = vec![0u8; 512];
        let _ = self.handle.read_bulk(EP_IN, &mut ack, TIMEOUT_READ);

        debug!("Frame sent successfully ({} bytes, {} chunks)", total_size, num_chunks);
        Ok(())
    }
}

impl Drop for TrofeoDriver {
    fn drop(&mut self) {
        let _ = self.handle.release_interface(USB_INTERFACE);
    }
}
