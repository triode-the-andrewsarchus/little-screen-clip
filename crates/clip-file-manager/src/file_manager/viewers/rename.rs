//! One shared rename dialog for every ordinary file-backed editor.
use super::super::{edits, system_route};
use super::*;
use clip_ui::buttons::ButtonRole;

type RenameResult = Result<(PathBuf, FileSource), String>;
#[derive(Default)]
pub(super) struct Rename {
    pub open: bool,
    focus: bool,
    name: String,
    source: Option<(PathBuf, FileSource)>,
    requested: bool,
    admission: system_route::Admission,
    receiver: Option<mpsc::Receiver<Result<RenameResult, JobError>>>,
}
impl Rename {
    pub fn busy(&self) -> bool {
        self.requested
    }
    pub fn begin(&mut self, path: PathBuf, source: FileSource) {
        self.name = super::super::display_path(Path::new(path.file_name().unwrap_or_default()));
        self.source = Some((path, source));
        self.open = true;
        self.focus = true;
    }
    pub fn ui(&mut self, ui: &mut Ui, palette: &AppPalette) -> Option<(PathBuf, RenameResult)> {
        if let Some(error) = self.dialog(ui, palette) {
            return Some((self.source.as_ref()?.0.clone(), Err(error)));
        }
        if !self.requested {
            return None;
        }
        let (path, source) = self.source.as_ref()?.clone();
        let check = path.clone();
        match self.admission.poll(BackgroundJobs::global(), move || {
            system_route::tree(&check, &std::sync::atomic::AtomicBool::new(false))
        }) {
            Ok(false) => {
                ui.ctx().request_repaint_after(Duration::from_millis(40));
                return None;
            }
            Err(error) => {
                self.finish();
                return Some((path, Err(error)));
            }
            Ok(true) => {}
        }
        if self.receiver.is_none() {
            let name = self.name.clone();
            let old = path.clone();
            let (_, receiver) = BackgroundJobs::global().submit(Duration::MAX, move || {
                edits::rename_file(&old, &name, &source)
            });
            self.receiver = Some(receiver);
        }
        let result = match self.receiver.as_ref()?.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {
                ui.ctx().request_repaint_after(Duration::from_millis(40));
                return None;
            }
            Ok(Err(JobError::QueueFull)) => {
                self.receiver = None;
                ui.ctx().request_repaint();
                return None;
            }
            result => result
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()))
                .and_then(|r| r),
        };
        self.finish();
        Some((path, result))
    }
    fn dialog(&mut self, ui: &mut Ui, palette: &AppPalette) -> Option<String> {
        if !self.open {
            return None;
        }
        let mut submit = false;
        egui::Window::new("Rename file")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.label("Filename (including extension):");
                let field_id = egui::Id::new("files_editor_rename_name");
                ui.ctx().memory_mut(|memory| memory.request_focus(field_id));
                let mut output = egui::TextEdit::singleline(&mut self.name)
                    .id(field_id)
                    .desired_width(360.0)
                    .show(ui);
                if self.focus {
                    self.focus = false;
                    super::super::ui::focus_text(ui.ctx(), &mut output, self.name.chars().count());
                }
                submit = (output.response.has_focus() || output.response.lost_focus())
                    && ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                    });
                if !submit && !output.response.has_focus() {
                    output.response.request_focus();
                }
                ui.horizontal(|ui| {
                    submit |= ui
                        .add(ButtonRole::Primary.button("Rename", palette))
                        .clicked();
                    if ui
                        .add(ButtonRole::Secondary.button("Cancel", palette))
                        .clicked()
                    {
                        self.open = false;
                    }
                });
            });
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.open = false;
            return None;
        }
        if !submit {
            return None;
        }
        if let Err(error) = edits::parse_name(&self.name) {
            return Some(error);
        }
        self.open = false;
        self.requested = true;
        None
    }
    fn finish(&mut self) {
        self.requested = false;
        self.receiver = None;
        self.admission = Default::default();
    }
}
