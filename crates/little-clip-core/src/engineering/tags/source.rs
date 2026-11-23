use super::Error;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_CHECKPOINTS: usize = 32_768;

pub(super) fn cancelled(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(super) struct Checkpoints {
    offsets: Vec<u64>,
    stride: u64,
}

impl Checkpoints {
    pub fn new(stride: u64) -> Self {
        Self {
            offsets: Vec::new(),
            stride,
        }
    }

    pub fn record(&mut self, row: u64, offset: u64) {
        if !row.is_multiple_of(self.stride) {
            return;
        }
        if self.offsets.len() == MAX_CHECKPOINTS {
            let mut slot = 0;
            self.offsets.retain(|_| {
                slot += 1;
                slot % 2 == 1
            });
            self.stride *= 2;
        }
        if row.is_multiple_of(self.stride) {
            self.offsets.push(offset);
        }
    }

    pub fn before(&self, row: u64) -> Result<[u64; 2], Error> {
        let slot = row / self.stride;
        let offset = self
            .offsets
            .get(slot as usize)
            .copied()
            .ok_or(Error::Truncated)?;
        Ok([slot * self.stride, offset])
    }
}
