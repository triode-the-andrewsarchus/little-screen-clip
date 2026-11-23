use crate::file_source::{FileSource, ReadCheck};
use little_clip_core::engineering::can::{self, Filter, Filtered, Frame, Index, Summary};
use std::{
    io::{BufWriter, Write},
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

const MAX_COPY: usize = 4096;

#[derive(Clone)]
pub(super) enum Selection {
    Rows(Range<u64>),
    Time(Range<i128>),
}

pub(super) struct Capture {
    path: PathBuf,
    revision: FileSource,
    pub check: ReadCheck,
}

impl Capture {
    pub fn open(path: &Path, cancel: &AtomicBool) -> Result<(Self, Summary), String> {
        let check = ReadCheck::default();
        let (file, revision) = check.open(path, None, cancel)?;
        let core = can::Capture::open(file)?;
        let summary = core.preview(cancel)?;
        revision.check()?;
        check.validate(cancel)?;
        Ok((
            Self {
                path: path.into(),
                revision,
                check,
            },
            summary,
        ))
    }

    fn read<T>(
        &self,
        cancel: &AtomicBool,
        read: impl FnOnce(can::Capture) -> Result<T, String>,
    ) -> Result<T, String> {
        let (file, _) = self.check.open(&self.path, Some(&self.revision), cancel)?;
        let result = read(can::Capture::open(file)?)?;
        self.revision.check()?;
        self.check.validate(cancel)?;
        Ok(result)
    }

    pub fn index(&self, cancel: &AtomicBool) -> Result<Index, String> {
        self.read(cancel, |capture| capture.index(cancel))
    }

    pub fn rows(
        &self,
        index: &Index,
        rows: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Vec<Frame>, String> {
        self.read(cancel, |capture| capture.rows(index, rows, cancel))
    }

    pub fn filter(
        &self,
        index: &Index,
        filter: &Filter,
        cancel: &AtomicBool,
    ) -> Result<Filtered, String> {
        self.read(cancel, |capture| capture.filter(index, filter, cancel))
    }

    pub fn copy(
        &self,
        index: &Index,
        selection: Selection,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        let frames = self.read(cancel, |capture| match selection {
            Selection::Rows(rows) if rows.end - rows.start <= MAX_COPY as u64 => {
                capture.rows(index, rows, cancel)
            }
            Selection::Time(time) => {
                let rows = capture.frames(index, time, MAX_COPY, cancel)?;
                if rows.len() == MAX_COPY {
                    return Err(format!(
                        "Select a shorter interval to copy fewer than {MAX_COPY} frames"
                    ));
                }
                Ok(rows)
            }
            Selection::Rows(_) => Err(format!("Select at most {MAX_COPY} frames to copy")),
        })?;
        if frames.is_empty() {
            return Err("No CAN frames are present in this interval".into());
        }
        super::super::preview_copy::copy_file(&self.path, "can-frames", "csv", |file| {
            let mut file = BufWriter::new(file);
            write_csv(&mut file, &frames).map_err(|error| error.to_string())
        })?;
        Ok(format!("Copied {} CAN frames as CSV", frames.len()))
    }
}

fn write_csv(output: &mut impl Write, frames: &[Frame]) -> std::io::Result<()> {
    writeln!(
        output,
        "frame,timestamp_ns,channel,id,extended,direction,kind,dlc,declared_bytes,captured_bytes,payload"
    )?;
    for frame in frames {
        let id = frame
            .id
            .map_or_else(String::new, |id| format!("{:X}", id.value));
        let extended = frame
            .id
            .map_or_else(String::new, |id| id.extended.to_string());
        let dlc = frame.dlc.map_or_else(String::new, |dlc| dlc.to_string());
        writeln!(
            output,
            "{},{},{},{},{},{},{},{},{},{},{}",
            frame.row + 1,
            frame.timestamp_ns,
            frame.channel,
            id,
            extended,
            super::direction(frame.direction),
            super::kind(frame.kind),
            dlc,
            frame.data_len,
            frame.data.len(),
            super::hex(&frame.data),
        )?;
    }
    output.flush()
}
