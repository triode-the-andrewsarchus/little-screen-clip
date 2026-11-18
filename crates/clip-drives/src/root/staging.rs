//! An unpublished output owns its stream, optional receipt and test interleaving.
use super::*;
use little_clip_core::file_source::{CopyStats, copy_checked};

pub(super) struct Temporary<'a> {
    pub file: tempfile::NamedTempFile,
    journal: Option<&'a mut dyn CopyStage>,
    #[cfg(test)]
    gate: Box<dyn FnMut(u64)>,
}
impl<'a> Temporary<'a> {
    pub fn new(
        _root: &Root,
        _target: &Path,
        parent: &File,
        mut journal: Option<&'a mut dyn CopyStage>,
    ) -> Result<Self> {
        let file = match journal.as_deref_mut() {
            Some(stage) => stage.create(parent)?,
            None => tempfile::NamedTempFile::new_in(anchor(parent))?,
        };
        #[cfg(test)]
        let gate = Box::new(copy_gate::progress(_root, _target, file.as_file())?);
        Ok(Self {
            file,
            journal,
            #[cfg(test)]
            gate,
        })
    }
    pub fn reflink(&self, delta: bool, old: Option<&File>) -> bool {
        delta
            && old.is_some_and(|old| {
                // SAFETY: FICLONE borrows two live regular-file descriptors.
                unsafe {
                    libc::ioctl(
                        self.file.as_file().as_raw_fd(),
                        0x40049409_u64 as libc::c_ulong,
                        old.as_raw_fd(),
                    ) == 0
                }
            })
    }
    pub fn stream(
        &mut self,
        input: &mut File,
        cancel: &AtomicBool,
        cloned: bool,
        source_hash: Option<&str>,
    ) -> Result<CopyStats> {
        let stats = copy_checked(
            input,
            self.file.as_file_mut(),
            cancel,
            cloned,
            source_hash,
            |bytes| {
                if let Some(stage) = self.journal.as_deref_mut() {
                    stage.progress(bytes).map_err(|error| error.to_string())?;
                }
                #[cfg(test)]
                (self.gate)(bytes);
                Ok(())
            },
        )?;
        Ok(stats)
    }
}
