//! Empty private allocation remains automatically owned until its journal commit.
use super::*;

pub(super) struct Empty {
    pub scratch: tempfile::TempDir,
    pub stage: Root,
    pub pin: File,
    pub file: StorageIdentity,
}
impl Empty {
    // The capture constructor retains parent until this allocation is kept or dropped.
    pub fn new(parent: &File, id: &str) -> Result<Self> {
        let scratch = tempfile::Builder::new()
            .prefix(&format!(".capture-{id}-"))
            .tempdir_in(anchor(parent))?;
        let stage = Root::open(scratch.path())?;
        let pin = stage.directory(Path::new(""), false)?;
        let file = little_clip_core::file_source::create_file_in(Path::new(FILE), &[], &pin)?;
        File::open(anchor(&pin))?.sync_all()?;
        File::open(anchor(parent))?.sync_all()?;
        Ok(Self {
            scratch,
            stage,
            pin,
            file: StorageIdentity::pinned(&file)?,
        })
    }
}
