use super::signal_plot::Signal;
use crate::file_source::{FileSource, ReadCheck};
use little_clip_core::engineering::can::{
    self, Filter, Filtered, Frame, Index, Summary, dbc, intervals, series,
};
use std::{
    io::{BufWriter, Read, Write},
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

    pub fn database(
        &self,
        path: &Path,
        cancel: &AtomicBool,
    ) -> Result<(PathBuf, dbc::Database), String> {
        let path = self.directory().join(path);
        let (file, revision) = self.check.open(&path, None, cancel)?;
        let mut bytes = Vec::new();
        (&file)
            .take(dbc::MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        let database = dbc::Database::parse(&bytes, cancel)?;
        revision.check_file(&file)?;
        revision.check()?;
        self.check.validate(cancel)?;
        Ok((path, database))
    }

    pub fn signal_series(
        &self,
        index: &Index,
        database: &dbc::Database,
        target: &Signal,
        time_ns: Range<i128>,
        cancel: &AtomicBool,
    ) -> Result<series::Series, String> {
        let message = database
            .message(target.id)
            .ok_or("The DBC message is unavailable")?;
        self.read(cancel, |capture| {
            capture.signal_series(
                index,
                series::Query {
                    time_ns,
                    channel: &target.channel,
                    message,
                    field: target.field,
                },
                cancel,
            )
        })
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

    pub fn intervals(
        &self,
        index: &Index,
        stream: &intervals::Stream,
        time_ns: Range<i128>,
        cancel: &AtomicBool,
    ) -> Result<intervals::Report, String> {
        self.read(cancel, |capture| {
            capture.intervals(index, stream, time_ns, cancel)
        })
    }

    pub fn directory(&self) -> &Path {
        self.path.parent().unwrap_or_else(|| Path::new("."))
    }

    pub fn nearby_database(&self, cancel: &AtomicBool) -> Option<PathBuf> {
        let name = Path::new(self.path.file_name()?);
        ["dbc", "DBC"]
            .map(|extension| name.with_extension(extension))
            .into_iter()
            .find(|path| {
                self.check
                    .open(&self.directory().join(path), None, cancel)
                    .is_ok()
            })
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

#[cfg(test)]
mod tests {
    use super::{Capture, Signal};
    use std::{fs, path::Path, sync::atomic::AtomicBool};

    #[test]
    fn nearby_dbc_fields_keep_independent_units_times_and_source_frames() {
        let directory = tempfile::tempdir().unwrap();
        let asc = directory.path().join("Power supply.asc");
        let dbc = directory.path().join("Power supply.dbc");
        fs::write(
            &asc,
            concat!(
                "date Mon Nov 2 03:22:02 2026\n",
                "base hex timestamps absolute\n",
                "0.100 1 123 Tx d 2 0A 00\n",
                "0.180 1 124 Rx d 2 14 00\n",
                "0.400 1 123 Tx d 2 30 00\n",
                "0.680 1 124 Rx d 2 18 00\n",
            ),
        )
        .unwrap();
        fs::write(
            &dbc,
            concat!(
                "VERSION \"\"\nNS_ :\nBS_:\nBU_: Node\n",
                "BO_ 291 Command: 2 Node\n",
                " SG_ TargetVoltage : 0|16@1+ (0.5,0) [0|100] \"V\" Node\n",
                "BO_ 292 Response: 2 Node\n",
                " SG_ OutputCurrent : 0|16@1+ (0.25,0) [0|200] \"A\" Node\n",
            ),
        )
        .unwrap();

        let cancel = AtomicBool::new(false);
        let (capture, _) = Capture::open(&asc, &cancel).unwrap();
        let nearby = capture.nearby_database(&cancel).unwrap();
        assert_eq!(nearby, Path::new("Power supply.dbc"));
        let (loaded_from, database) = capture.database(&nearby, &cancel).unwrap();
        assert_eq!(loaded_from, dbc);
        let index = capture.index(&cancel).unwrap();

        let span = 100_000_000..680_000_001;
        for (first_row, unit, expected) in [
            (0, "V", [(0, 100_000_000, 5.0), (2, 400_000_000, 24.0)]),
            (1, "A", [(1, 180_000_000, 5.0), (3, 680_000_000, 6.0)]),
        ] {
            let frames = capture
                .rows(&index, first_row..first_row + 1, &cancel)
                .unwrap();
            let frame = &frames[0];
            let id = frame.id.unwrap();
            let target = Signal {
                channel: frame.channel.clone(),
                id,
                field: 0,
            };
            assert_eq!(database.message(id).unwrap().signals[0].unit, unit);

            let series = capture
                .signal_series(&index, &database, &target, span.clone(), &cancel)
                .unwrap();
            assert_eq!(series.points.len(), expected.len());
            assert!(!series.truncated);
            assert!(!series.reduced);
            for (point, (row, time, value)) in series.points.iter().zip(expected) {
                assert_eq!(
                    (point.row, point.time_ns, point.decoded.unwrap().value),
                    (row, time, value)
                );
                let frames = capture
                    .rows(&index, point.row..point.row + 1, &cancel)
                    .unwrap();
                assert_eq!((frames[0].id, frames[0].timestamp_ns), (Some(id), time));
            }
        }
    }
}
