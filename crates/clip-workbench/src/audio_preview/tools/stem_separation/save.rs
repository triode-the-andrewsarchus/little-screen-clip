//! Accepted stem exports share the audio save completion and close guard.
use super::*;
use crate::file_source::{FileSource, ReadCheck};
use std::os::fd::AsRawFd;

struct StemSaveInput {
    path: PathBuf,
    check: ReadCheck,
    identity: Option<FileSource>,
    filename: String,
}

fn save_stems(inputs: Vec<StemSaveInput>, destination: &Path) -> Result<AudioSaveOutcome, String> {
    let cancel = AtomicBool::new(false);
    // Admit the complete batch before creating outputs. The handles and checks
    // retain owned stem files even if the viewer changes after acceptance.
    let admitted = inputs
        .into_iter()
        .map(|input| {
            let (file, observed) =
                input
                    .check
                    .open(&input.path, input.identity.as_ref(), &cancel)?;
            Ok((input, file, observed))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let directory = little_clip_core::kernel_interface::create_content_directory(destination)
        .map_err(|error| error.to_string())?;
    let anchor = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let mut outcome = AudioSaveOutcome {
        saved_count: 0,
        saved_paths: Vec::new(),
        failures: Vec::new(),
    };
    for (input, mut file, observed) in admitted {
        let result = (|| -> Result<(), String> {
            if Path::new(&input.filename).file_name() != Some(std::ffi::OsStr::new(&input.filename))
            {
                return Err("Enter one stem export filename".into());
            }
            input.check.validate(&cancel)?;
            observed.check()?;
            let mut staging =
                tempfile::NamedTempFile::new_in(&anchor).map_err(|e| e.to_string())?;
            std::io::copy(&mut file, staging.as_file_mut()).map_err(|e| e.to_string())?;
            staging.as_file().sync_all().map_err(|e| e.to_string())?;
            input.check.validate(&cancel)?;
            observed.check()?;
            little_clip_core::kernel_interface::pin_content_directory(&anchor)
                .map_err(|e| e.to_string())?;
            staging
                .persist_noclobber(anchor.join(&input.filename))
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                outcome.saved_count += 1;
                outcome.saved_paths.push(destination.join(&input.filename));
            }
            Err(error) => outcome
                .failures
                .push(format!("{}: {error}", input.filename)),
        }
    }
    // Keep completed outputs in the result, including partial success.
    Ok(outcome)
}

impl AudioPreviewState {
    pub(crate) fn save_all_stems(&mut self) {
        if self.save_job.is_some() {
            return;
        }
        let destination = PathBuf::from(self.stem_separation.save_destination_dir.trim());
        if destination.as_os_str().is_empty() || self.stem_separation.stems.is_empty() {
            self.stem_separation.error =
                Some("Choose stems and a destination folder first.".into());
            return;
        }
        let base = self
            .path
            .file_stem()
            .and_then(|v| v.to_str())
            .unwrap_or("audio");
        let inputs = self
            .stem_separation
            .stems
            .iter()
            .map(|stem| StemSaveInput {
                path: stem.preview.path.clone(),
                check: stem.preview.read_check.clone(),
                identity: stem.preview.file_source.clone(),
                filename: format!("{base}_{}.wav", stem.name),
            })
            .collect();
        let destination_name = destination.display().to_string();
        let (ticket, receiver) = BackgroundJobs::global()
            .submit(Duration::MAX, move || save_stems(inputs, &destination));
        self.save_job = Some(AudioSaveJob {
            ticket,
            receiver,
            destination_name,
            stem_export: true,
        });
        self.stem_separation.error = None;
        self.stem_separation.status = Some("Saving stems…".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(path: &Path, filename: &str) -> StemSaveInput {
        StemSaveInput {
            path: path.to_owned(),
            filename: filename.into(),
            check: Default::default(),
            identity: Some(FileSource::capture(path).unwrap()),
        }
    }

    #[test]
    fn stem_save_preserves_existing_outputs_and_reports_completed_items() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("stem.wav");
        std::fs::write(&source, b"owned stem bytes").unwrap();
        let destination = directory.path().join("outputs");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("vocals.wav"), b"keep existing").unwrap();
        let result = save_stems(
            vec![input(&source, "vocals.wav"), input(&source, "drums.wav")],
            &destination,
        )
        .unwrap();
        assert_eq!(result.saved_count, 1);
        assert_eq!(result.failures.len(), 1);
        assert_eq!(
            std::fs::read(destination.join("vocals.wav")).unwrap(),
            b"keep existing"
        );
        assert_eq!(
            std::fs::read(destination.join("drums.wav")).unwrap(),
            b"owned stem bytes"
        );
        assert_eq!(std::fs::read_dir(destination).unwrap().count(), 2);
    }

    #[test]
    fn stem_save_retains_destination_and_cleans_rejected_staging() {
        for expire in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let source = directory.path().join("stem.wav");
            std::fs::write(&source, b"original stem").unwrap();
            let destination = directory.path().join("outputs");
            let retained = directory.path().join("retained");
            let replacement = directory.path().join("replacement");
            std::fs::create_dir(&replacement).unwrap();
            let mut stem = input(&source, "vocals.wav");
            let observed_destination = destination.clone();
            let held = retained.clone();
            let other = replacement.clone();
            stem.check = ReadCheck::new(move |_| {
                let staged = std::fs::read_dir(&observed_destination)
                    .ok()
                    .is_some_and(|entries| {
                        entries
                            .filter_map(Result::ok)
                            .any(|entry| entry.metadata().is_ok_and(|meta| meta.len() > 0))
                    });
                if staged {
                    if expire {
                        return Err("source expired during copy".into());
                    }
                    std::fs::rename(&observed_destination, &held).unwrap();
                    std::os::unix::fs::symlink(&other, &observed_destination).unwrap();
                }
                Ok(())
            });
            let result = save_stems(vec![stem], &destination).unwrap();
            assert_eq!(result.saved_count, usize::from(!expire));
            assert_eq!(std::fs::read_dir(replacement).unwrap().count(), 0);
            if expire {
                assert_eq!(result.failures.len(), 1);
                assert_eq!(std::fs::read_dir(destination).unwrap().count(), 0);
            } else {
                assert_eq!(
                    std::fs::read(retained.join("vocals.wav")).unwrap(),
                    b"original stem"
                );
            }
        }
    }

    #[test]
    fn stem_save_rejects_expired_or_replaced_inputs_before_creating_outputs() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("stem.wav");
        std::fs::write(&source, b"original").unwrap();
        let destination = directory.path().join("outputs");
        let mut expired = input(&source, "vocals.wav");
        expired.check = ReadCheck::new(|_| Err("source expired".into()));
        assert!(save_stems(vec![expired], &destination).is_err());
        let replaced = input(&source, "vocals.wav");
        std::fs::write(&source, b"replacement").unwrap();
        assert!(save_stems(vec![replaced], &destination).is_err());
        assert!(!destination.exists());
    }
}
