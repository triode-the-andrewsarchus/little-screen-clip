use super::{JOBS, Line, line_from_cue};
use crate::file_source::{FileSource, ReadCheck};
use little_clip_core::{
    jobs::task::Task,
    media_backend,
    media_backend::playback::{MAX_SUBTITLE_BYTES, SUBTITLE_READ_TIMEOUT, SubtitleCue},
    process::CommandOutput,
};
use std::{
    fs::File,
    os::unix::fs::FileExt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

pub(super) struct Loading {
    pub task: Task<Result<Arc<[Line]>, String>>,
    pub progress: Receiver<Arc<[Line]>>,
}

impl Loading {
    pub fn start(source: FileSource, check: ReadCheck, track: u32) -> Self {
        let (sender, progress) = mpsc::sync_channel(1);
        let timeout = SUBTITLE_READ_TIMEOUT + Duration::from_secs(6);
        let task = Task::start(&JOBS, timeout, move |cancel| {
            read_transcript(&source, &check, track, cancel, sender)
        });
        Self { task, progress }
    }
}

fn read_transcript(
    source: &FileSource,
    check: &ReadCheck,
    track: u32,
    cancel: &AtomicBool,
    sender: SyncSender<Arc<[Line]>>,
) -> Result<Arc<[Line]>, String> {
    let (input, _) = check.open(source.path(), Some(source), cancel)?;
    let scratch = tempfile::tempfile().map_err(|error| error.to_string())?;
    let request = media_backend::subtitle_track_request(track)?
        .arg_owned_content_file(input.try_clone().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let (request, descriptor) = request
        .inherit_handle(&scratch)
        .map_err(|error| error.to_string())?;
    let mut progress = Progress {
        file: scratch,
        offset: 0,
        lines: Vec::new(),
        next_poll: Instant::now(),
    };
    let output = request
        .arg(format!("/proc/self/fd/{descriptor}"))
        .timeout(SUBTITLE_READ_TIMEOUT + Duration::from_secs(5))
        .run_cancellable_with_progress(cancel, || {
            if progress.poll()
                && source.check_file(&input).is_ok()
                && check.validate(cancel).is_ok()
            {
                let _ = sender.try_send(progress.lines.clone().into());
            }
        })
        .map_err(|error| error.to_string())?;
    source.check_file(&input)?;
    check.validate(cancel)?;
    finished_transcript(output, cancel)
}

fn finished_transcript(output: CommandOutput, cancel: &AtomicBool) -> Result<Arc<[Line]>, String> {
    if output.stdout_truncated {
        return Err("The subtitle track exceeds the search limit.".into());
    }
    if !output.status.success() {
        return Err(format!(
            "Cannot read subtitle track. {}",
            output.stderr_lossy().trim()
        ));
    }
    let cues: Vec<SubtitleCue> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Cannot read subtitle track. {error}"))?;
    Ok(cues
        .into_iter()
        .take_while(|_| !cancel.load(Ordering::Relaxed))
        .filter_map(line_from_cue)
        .collect())
}

struct Progress {
    file: File,
    offset: u64,
    lines: Vec<Line>,
    next_poll: Instant,
}

impl Progress {
    fn poll(&mut self) -> bool {
        let now = Instant::now();
        if now < self.next_poll {
            return false;
        }
        self.next_poll = now + Duration::from_millis(300);
        let Ok(metadata) = self.file.metadata() else {
            return false;
        };
        if metadata.len() <= self.offset || metadata.len() > MAX_SUBTITLE_BYTES as u64 {
            return false;
        }
        let mut bytes = vec![0; (metadata.len() - self.offset) as usize];
        if self.file.read_exact_at(&mut bytes, self.offset).is_err() {
            return false;
        }
        let Some(end) = bytes.iter().rposition(|&byte| byte == b'\n') else {
            return false;
        };
        self.offset += (end + 1) as u64;
        let previous = self.lines.len();
        self.lines.extend(
            bytes[..end]
                .split(|&byte| byte == b'\n')
                .filter_map(|record| serde_json::from_slice(record).ok())
                .filter_map(line_from_cue),
        );
        self.lines.len() > previous
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn partial_transcript_keeps_an_incomplete_record_until_it_is_finished() {
        let mut writer = tempfile::tempfile().unwrap();
        let mut progress = Progress {
            file: writer.try_clone().unwrap(),
            offset: 0,
            lines: Vec::new(),
            next_poll: Instant::now(),
        };
        write!(
            writer,
            "{{\"start_ms\":2000,\"text\":\"First line\",\"markup\":false}}\n{{\"start_ms\":3500,"
        )
        .unwrap();
        assert!(progress.poll());
        assert_eq!(progress.lines.len(), 1);
        writeln!(writer, "\"text\":\"Next line\",\"markup\":false}}").unwrap();
        progress.next_poll = Instant::now();
        assert!(progress.poll());
        assert_eq!(progress.lines.len(), 2);
        assert_eq!(progress.lines[1].start_ms, 3500);
    }
}
