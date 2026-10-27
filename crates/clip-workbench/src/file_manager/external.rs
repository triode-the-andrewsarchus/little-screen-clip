//! Explicit external launches validate off-thread; launched applications own their lifetime.
use crate::file_source::FileSource;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

pub(super) struct Request {
    pub path: PathBuf,
    pub search: Option<super::search::SourceGuard>,
    pub saved: Option<FileSource>,
}
impl Request {
    fn validate(&self, cancel: &AtomicBool) -> Result<(), String> {
        if let Some(source) = &self.saved {
            if source.path() != self.path {
                return Err("External launch does not match the saved source".into());
            }
            source.check()?;
        } else if let Some(guard) = &self.search {
            guard.check(cancel)?;
        }
        little_clip_core::kernel_interface::require_content(&self.path)
            .map_err(|error| error.to_string())?;
        if cancel.load(Ordering::Relaxed) {
            return Err("External launch cancelled".into());
        }
        Ok(())
    }
    fn run(&self, program: &std::path::Path, cancel: &AtomicBool) -> Result<(), String> {
        self.validate(cancel)?;
        let status = std::process::Command::new(program)
            .arg(&self.path)
            .stdin(std::process::Stdio::null())
            .status()
            .map_err(|error| format!("Cannot launch external application: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("External application launcher failed: {status}"))
        }
    }
}

struct Job {
    cancel: Arc<AtomicBool>,
    result: mpsc::Receiver<Result<(), String>>,
}
impl Drop for Job {
    fn drop(&mut self) {
        // Cancel validation still in progress; never terminate an accepted external app.
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(super) struct Launcher {
    jobs: Vec<Job>,
}
impl Launcher {
    pub fn start(&mut self, request: Request) -> Result<(), String> {
        if self.jobs.len() >= 8 {
            return Err(
                "Eight external launches are still running; close one before opening another"
                    .into(),
            );
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = cancel.clone();
        let (send, result) = mpsc::channel();
        // xdg-open may wait for its application. Do not occupy the media executor.
        std::thread::Builder::new()
            .name("file-external-launch".into())
            .spawn(move || {
                let _ = send.send(request.run(std::path::Path::new("xdg-open"), &worker));
            })
            .map_err(|error| error.to_string())?;
        self.jobs.push(Job { cancel, result });
        Ok(())
    }
    pub fn poll(&mut self, ctx: &egui::Context) -> Option<String> {
        let mut failure = None;
        self.jobs.retain(|job| match job.result.try_recv() {
            Ok(result) => {
                if let Err(error) = result {
                    failure = Some(error);
                }
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                failure = Some("External launch worker disconnected".into());
                false
            }
        });
        if !self.jobs.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        failure
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_source_launch_accepts_owned_save_and_rejects_later_replacement() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("notes.txt");
        std::fs::write(&path, "before").unwrap();
        let source = FileSource::capture(&path).unwrap();
        let saved = source.write(&path, b"saved").unwrap();
        let request = Request {
            path: path.clone(),
            search: None,
            saved: Some(saved),
        };
        let cancel = AtomicBool::new(false);
        assert!(
            request
                .run(std::path::Path::new("/usr/bin/true"), &cancel)
                .is_ok()
        );
        std::fs::write(path, "outside edit").unwrap();
        assert!(
            request
                .run(std::path::Path::new("/usr/bin/true"), &cancel)
                .is_err()
        );
    }
    #[test]
    fn launch_limit_and_owner_drop_bound_pending_workers() {
        let mut launcher = Launcher::default();
        let mut senders = Vec::new();
        let mut tokens = Vec::new();
        for _ in 0..8 {
            let (sender, result) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            tokens.push(cancel.clone());
            senders.push(sender);
            launcher.jobs.push(Job { cancel, result });
        }
        let request = Request {
            path: "/unused".into(),
            search: None,
            saved: None,
        };
        assert!(launcher.start(request).unwrap_err().contains("Eight"));
        assert!(launcher.poll(&egui::Context::default()).is_none());
        assert_eq!(launcher.jobs.len(), 8);
        senders[0].send(Ok(())).unwrap();
        launcher.poll(&egui::Context::default());
        assert_eq!(launcher.jobs.len(), 7);
        drop(launcher);
        assert!(tokens.iter().all(|token| token.load(Ordering::Relaxed)));
    }

    #[test]
    fn launch_reports_missing_program_nonzero_status_and_cancellation() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("notes.txt");
        std::fs::write(&path, "text").unwrap();
        let request = Request {
            path,
            search: None,
            saved: None,
        };
        assert!(
            request
                .run(&folder.path().join("missing"), &AtomicBool::new(false))
                .is_err()
        );
        assert!(
            request
                .run(
                    std::path::Path::new("/usr/bin/false"),
                    &AtomicBool::new(false)
                )
                .is_err()
        );
        assert!(
            request
                .run(
                    std::path::Path::new("/usr/bin/true"),
                    &AtomicBool::new(true)
                )
                .is_err()
        );
    }
}
