//! Collection mutations retain actual completions and never overwrite export siblings.
use super::*;
use crate::image_preview::ImageInputStamp;
use std::{collections::VecDeque, os::fd::AsRawFd};

type ExportResult = Vec<(usize, ImageInputStamp, Result<PathBuf, String>)>;
struct ExportInput {
    index: usize,
    name: std::ffi::OsString,
    source: FileSource,
    snapshot: crate::image_preview::DisplayImageSnapshotRequest,
    stamp: ImageInputStamp,
}
pub(super) struct Export {
    receiver: mpsc::Receiver<Result<ExportResult, JobError>>,
}
impl Collection {
    pub(super) fn writes_pending(&self) -> bool {
        self.export.is_some() || self.ratings.busy()
    }
    pub(super) fn export_ui(&mut self, ui: &mut Ui, active: Option<&mut ImagePreviewState>) {
        if !self.export_open {
            return;
        }
        let mut start = false;
        egui::Window::new("Export all edits")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.label("Export edited photos as PNG copies into one folder.");
                ui.label(
                    "Existing files are kept. Each successful export clears its Unsaved badge.",
                );
                ui.label("Output folder:");
                ui.add(egui::TextEdit::singleline(&mut self.export_directory).desired_width(420.0));
                ui.horizontal(|ui| {
                    start = ui.button("Export PNG copies").clicked();
                    if ui.button("Cancel").clicked() {
                        self.export_open = false;
                    }
                });
            });
        if start {
            let mut inputs = VecDeque::new();
            for (index, photo) in self.photos.iter().enumerate() {
                let state = if index == self.index {
                    active.as_deref()
                } else {
                    photo.session.as_deref()
                };
                if let Some(state) = state.filter(|state| state.has_collection_edits()) {
                    let Some(source) = state.file_source() else {
                        continue;
                    };
                    let mut name = photo.path.file_stem().unwrap_or_default().to_os_string();
                    name.push("_edit.png");
                    inputs.push_back(ExportInput {
                        index,
                        name,
                        source,
                        snapshot: state.display_image_snapshot_request(),
                        stamp: state.input_stamp(),
                    });
                }
            }
            let directory = PathBuf::from(&self.export_directory);
            let (_, receiver) =
                BackgroundJobs::global().submit(Duration::MAX, move || export(inputs, directory));
            self.export = Some(Export { receiver });
            self.export_open = false;
            self.status = None;
        }
    }
    pub(super) fn poll_export(
        &mut self,
        ctx: &Context,
        mut active: Option<&mut ImagePreviewState>,
    ) {
        let Some(export) = &self.export else {
            return;
        };
        let results = match export.receiver.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(40));
                return;
            }
            result => result
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string())),
        };
        self.export = None;
        match results {
            Err(error) => self.status = Some(format!("Export failed: {error}")),
            Ok(results) => {
                let mut errors = Vec::new();
                for (index, stamp, result) in results {
                    match result {
                        Ok(_) => {
                            let state = if index == self.index {
                                active.as_deref_mut()
                            } else {
                                self.photos[index].session.as_deref_mut()
                            };
                            if let Some(state) = state {
                                state.mark_collection_exported(stamp);
                            }
                        }
                        Err(error) => errors.push(format!(
                            "{}: {error}",
                            super::super::super::display_path(&self.photos[index].path)
                        )),
                    }
                }
                self.status =
                    (!errors.is_empty()).then(|| format!("Export failed:\n{}", errors.join("\n")));
            }
        }
    }
}

fn export(mut inputs: VecDeque<ExportInput>, directory: PathBuf) -> ExportResult {
    let pinned = little_clip_core::kernel_interface::create_content_directory(&directory)
        .map_err(|e| e.to_string());
    let mut results = Vec::new();
    while let Some(input) = inputs.pop_front() {
        let result = (|| {
            let directory = pinned.as_ref().map_err(Clone::clone)?;
            let parent = std::fs::read_link(format!("/proc/self/fd/{}", directory.as_raw_fd()))
                .map_err(|e| e.to_string())?;
            let destination = parent.join(input.name);
            let bytes = input.snapshot.compose_png().map_err(|e| e.to_string())?;
            input.source.write_in(&destination, &bytes, directory)?;
            Ok(destination)
        })();
        results.push((input.index, input.stamp, result));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_reports_each_result_keeps_collisions_and_preserves_originals() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("exports");
        std::fs::create_dir(&output).unwrap();
        std::fs::write(output.join("a_edit.png"), b"existing").unwrap();
        let mut inputs = VecDeque::new();
        for (index, name) in ["a", "b"].into_iter().enumerate() {
            let path = dir.path().join(format!("{name}.png"));
            image::RgbaImage::from_pixel(4, 4, image::Rgba([40, 80, 120, 255]))
                .save(&path)
                .unwrap();
            let mut state = ImagePreviewState::from_path(path.clone(), &AppConfig::default())
                .unwrap()
                .with_file_source(FileSource::capture(&path).unwrap());
            std::sync::Arc::make_mut(&mut state.current_rgba).put_pixel(
                0,
                0,
                image::Rgba([255, 0, 0, 255]),
            );
            state.mark_pixels_changed();
            inputs.push_back(ExportInput {
                index,
                name: format!("{name}_edit.png").into(),
                source: state.file_source().unwrap(),
                snapshot: state.display_image_snapshot_request(),
                stamp: state.input_stamp(),
            });
        }
        let results = export(inputs, output.clone());
        assert!(results[0].2.is_err());
        assert!(results[1].2.is_ok());
        assert_eq!(
            std::fs::read(output.join("a_edit.png")).unwrap(),
            b"existing"
        );
        let saved = image::open(output.join("b_edit.png")).unwrap().to_rgba8();
        assert_eq!(saved.get_pixel(0, 0).0, [255, 0, 0, 255]);
        let original = image::open(dir.path().join("b.png")).unwrap().to_rgba8();
        assert_eq!(original.get_pixel(0, 0).0, [40, 80, 120, 255]);
    }
}
