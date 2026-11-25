use super::{Loaded, Viewers};
use crate::file_manager::search::SourceGuard;
use little_clip_ops::diff::resolve_diff_target_from_path;
use std::{collections::HashMap, path::PathBuf};

impl Viewers {
    pub(in crate::file_manager) fn open_comparison(
        &mut self,
        paths: [PathBuf; 2],
        guards: HashMap<PathBuf, SourceGuard>,
    ) {
        if self.active() {
            return;
        }
        self.start_loading(paths[0].clone(), false, move |cancelled| {
            let read = |path: &PathBuf| {
                SourceGuard::read_checked(guards.get(path), cancelled, || {
                    resolve_diff_target_from_path(path).ok_or_else(|| {
                        format!(
                            "Cannot compare '{}'. Expected a readable image or text file.",
                            path.display()
                        )
                    })
                })
            };
            let base = read(&paths[0])?;
            let revised = read(&paths[1])?;
            if base.is_image() != revised.is_image() {
                return Err("Select two image files or two text files to compare.".into());
            }
            Ok((Loaded::Comparison([base, revised]), None))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ViewerSession, tests::finish_loading};
    use super::*;

    #[test]
    fn compare_opens_svg_and_svgz_in_the_shared_vector_viewer() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("base.svgz");
        let revised = root.path().join("revised.svg");
        std::fs::write(
            &base,
            include_bytes!("../../../../little-clip-core/tests/fixtures/panels.svgz"),
        )
        .unwrap();
        std::fs::write(
            &revised,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="32"><rect x=".125" width="32" height="32"/></svg>"#,
        )
        .unwrap();
        let mut viewers = Viewers::default();
        viewers.open_comparison([base, revised], HashMap::new());
        finish_loading(&mut viewers, &egui::Context::default());
        let Some(ViewerSession::Comparison(state)) = &viewers.session else {
            panic!("expected compare viewer, got {:?}", viewers.error);
        };
        assert!(state.is_svg());
        assert!(viewers.request_close());
        assert!(!viewers.active());
    }
}
