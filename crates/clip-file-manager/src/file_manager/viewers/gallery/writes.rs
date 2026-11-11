//! Collection mutations retain actual completions and never overwrite export siblings.
use super::*;
use crate::image_preview::ImageInputStamp;
use clip_ui::buttons::ButtonRole;
use std::{
    collections::{HashMap, VecDeque},
    os::fd::AsRawFd,
};

type ExportResult = Vec<(usize, ImageInputStamp, Result<PathBuf, String>)>;
struct ExportInput {
    index: usize,
    stem: std::ffi::OsString,
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
    pub(super) fn export_ui(
        &mut self,
        ui: &mut Ui,
        active: Option<&mut ImagePreviewState>,
        palette: &AppPalette,
    ) {
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
                    start = ui
                        .add(ButtonRole::Primary.button("Export PNG copies", palette))
                        .clicked();
                    if ui
                        .add(ButtonRole::Secondary.button("Cancel", palette))
                        .clicked()
                    {
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
                    inputs.push_back(ExportInput {
                        index,
                        stem: photo.path.file_stem().unwrap_or_default().to_os_string(),
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
    let mut occurrences = HashMap::<std::ffi::OsString, usize>::new();
    while let Some(input) = inputs.pop_front() {
        let count = occurrences.entry(input.stem.clone()).or_default();
        *count += 1;
        let mut name = input.stem;
        if *count == 1 {
            name.push("_edit.png");
        } else {
            name.push(format!("_edit_{count}.png"));
        }
        let result = (|| {
            let directory = pinned.as_ref().map_err(Clone::clone)?;
            let parent = std::fs::read_link(format!("/proc/self/fd/{}", directory.as_raw_fd()))
                .map_err(|e| e.to_string())?;
            let destination = parent.join(name);
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
        for (index, name) in ["a.png", "b.png", "b.jpg"].into_iter().enumerate() {
            let path = dir.path().join(name);
            image::RgbImage::from_pixel(4, 4, image::Rgb([40, 80, 120]))
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
                stem: path.file_stem().unwrap().to_os_string(),
                source: state.file_source().unwrap(),
                snapshot: state.display_image_snapshot_request(),
                stamp: state.input_stamp(),
            });
        }
        let results = export(inputs, output.clone());
        assert!(results[0].2.is_err());
        assert_eq!(results[1].2.as_ref().unwrap(), &output.join("b_edit.png"));
        assert_eq!(results[2].2.as_ref().unwrap(), &output.join("b_edit_2.png"));
        assert_eq!(
            image::open(output.join("b_edit_2.png"))
                .unwrap()
                .to_rgba8()
                .get_pixel(0, 0)
                .0,
            [255, 0, 0, 255]
        );
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
