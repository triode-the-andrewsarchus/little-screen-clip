use std::ffi::c_int;

use log::{error, warn};
use symphonia_core::errors::{Error, Result};

fn error_code_to_str(code: c_int) -> &'static str {
    match code {
        opusic_sys::OPUS_BAD_ARG => "One or more invalid/out of range arguments.",
        opusic_sys::OPUS_BUFFER_TOO_SMALL => "The mode struct passed is invalid.",
        opusic_sys::OPUS_INTERNAL_ERROR => "An internal error was detected.",
        opusic_sys::OPUS_INVALID_PACKET => "The compressed data passed is corrupted.",
        opusic_sys::OPUS_UNIMPLEMENTED => "Invalid/unsupported request number.",
        opusic_sys::OPUS_INVALID_STATE => {
            "An encoder or decoder structure is invalid or already freed."
        }
        opusic_sys::OPUS_ALLOC_FAIL => "Memory allocation has failed. ",
        _ => "",
    }
}

#[derive(Debug)]
pub(crate) struct Decoder {
    ptr: *mut opusic_sys::OpusDecoder,
    channels: u32,
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: successful construction uniquely owns a non-null libopus
        // decoder. No native use can overlap Drop, which destroys it once.
        unsafe {
            opusic_sys::opus_decoder_destroy(self.ptr);
        }
    }
}

// SAFETY: libopus decoder state has no thread affinity; moving unique ownership
// does not race it, and every native operation requires &mut Decoder.
unsafe impl Send for Decoder {}
// SAFETY: shared references expose no native state reads or mutation. Decoding,
// control and destruction require exclusive access; the raw pointer stays private.
unsafe impl Sync for Decoder {}

impl Decoder {
    pub(crate) fn new(sample_rate: u32, channels: u32) -> Result<Self> {
        let mut error = 0;
        // SAFETY: error points to initialized writable storage. The scalar
        // arguments are validated by libopus; success returns a new owned decoder.
        let ptr = unsafe {
            opusic_sys::opus_decoder_create(sample_rate as i32, channels as c_int, &mut error)
        };
        if error != opusic_sys::OPUS_OK || ptr.is_null() {
            let error_str = error_code_to_str(error);
            error!("decoder failed to create with error code {error}: {error_str}");
            return Err(Error::DecodeError("opus: error creating decoder"));
        }
        Ok(Self { ptr, channels })
    }

    pub(crate) fn decode(&mut self, input: &[u8], output: &mut [f32]) -> Result<usize> {
        let ptr = match input.len() {
            0 => std::ptr::null(),
            _ => input.as_ptr(),
        };
        // SAFETY: self owns live decoder state and &mut self excludes other calls.
        // input is readable for its checked C-int length (null requests concealment).
        // output is writable for frame_size*channels floats, bounded by its slice;
        // the accepted decoder configuration has one or two nonzero channels.
        let len = unsafe {
            opusic_sys::opus_decode_float(
                self.ptr,
                ptr,
                len(input)?,
                output.as_mut_ptr(),
                len(output)? / self.channels as c_int,
                0 as c_int,
            )
        };
        if len < 0 {
            let error_str = error_code_to_str(len);
            warn!("decode failed with error code {len}: {error_str}");
            return Err(Error::DecodeError("opus: decode failed"));
        }
        Ok(len as usize)
    }

    pub(crate) fn reset(&mut self) {
        // SAFETY: reset is the no-extra-argument OPUS_RESET_STATE request on
        // uniquely borrowed live decoder state; libopus retains no Rust pointers.
        let result =
            unsafe { opusic_sys::opus_decoder_ctl(self.ptr, opusic_sys::OPUS_RESET_STATE) };

        if result != opusic_sys::OPUS_OK {
            let error_str = error_code_to_str(result);
            warn!("reset failed with error code {result}: {error_str}");
        }
    }
}

fn check_len(val: usize) -> Result<c_int> {
    c_int::try_from(val).map_err(|_| {
        error!("buffer length of out range: {val}");
        Error::DecodeError("buffer length of out range")
    })
}

#[inline]
fn len<T>(slice: &[T]) -> Result<c_int> {
    check_len(slice.len())
}
