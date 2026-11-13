//! Coalesced, worker-side regeneration of native connection geometry.
use super::*;
use clip_ui::vectors::svg::{Replacement, Scene as VectorScene};
use little_clip_core::diagram::drawing::MovedPath;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) type Lines = HashMap<String, little_clip_render::diagram::retained::LinearPreview>;

#[derive(Clone)]
struct Request {
    model: Arc<Scene>,
    paths: HashMap<String, MovedPath>,
}
impl PartialEq for Request {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.model, &other.model) && self.paths == other.paths
    }
}
struct Batch {
    request: Request,
    replacements: HashMap<String, Replacement>,
}
#[derive(Default)]
pub(super) struct Connections {
    pending: Option<Task<Result<Batch, String>>>,
    shown: Option<Batch>,
    submitted: Option<Request>,
    retry: Option<Instant>,
    pub(super) error: Option<String>,
}
impl Connections {
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn update(
        &mut self,
        ctx: &egui::Context,
        model: &Arc<Scene>,
        lines: &Arc<Lines>,
        paths: HashMap<String, MovedPath>,
    ) -> HashMap<String, Replacement> {
        if paths.is_empty() {
            self.clear();
            return HashMap::new();
        }
        let request = Request {
            model: model.clone(),
            paths,
        };
        if self
            .shown
            .as_ref()
            .is_some_and(|shown| !Arc::ptr_eq(&shown.request.model, &request.model))
        {
            self.shown = None;
        }
        if let Some(result) = self.pending.as_ref().and_then(Task::poll) {
            self.pending = None;
            match result {
                Ok(Ok(batch)) if Arc::ptr_eq(&batch.request.model, &request.model) => {
                    self.shown = Some(batch);
                    self.error = None;
                }
                Ok(Ok(_)) => {}
                Err(JobError::QueueFull) => {
                    self.submitted = None;
                    self.retry = Some(Instant::now() + Duration::from_millis(100));
                }
                Ok(Err(error)) => self.error = Some(error),
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        if self.pending.is_none()
            && self.submitted.as_ref() != Some(&request)
            && self.retry.is_none_or(|due| Instant::now() >= due)
        {
            let model = model.clone();
            let lines = lines.clone();
            self.submitted = Some(request.clone());
            self.pending = Some(Task::start(
                BackgroundJobs::global(),
                Duration::from_secs(3),
                move |cancel| regenerate(request, &model, &lines, cancel),
            ));
        }
        if self.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(8));
        }
        self.shown
            .as_ref()
            .map(|batch| batch.replacements.clone())
            .unwrap_or_default()
    }
}

fn regenerate(
    request: Request,
    model: &Scene,
    lines: &Lines,
    cancel: &AtomicBool,
) -> Result<Batch, String> {
    let mut replacements = HashMap::new();
    let mut bytes = 0;
    let mut points = 0;
    for (id, path) in &request.paths {
        points += path.points.len();
        if cancel.load(Ordering::Relaxed) || points > 131_072 {
            return Err("Live connection preparation cancelled or over budget".into());
        }
        let line = lines
            .get(id)
            .ok_or("Native connection geometry is unavailable")?;
        let original = model
            .items
            .iter()
            .find(|item| &item.id == id)
            .ok_or("Connection disappeared")?;
        let svg = line.svg(&path.points)?;
        bytes += svg.len();
        if bytes > 2 * 1024 * 1024 {
            return Err("Live connection geometry exceeds the preview budget".into());
        }
        let prepared = little_clip_render::diagram::retained::prepare(&svg)?;
        let scene = VectorScene::new(prepared.tree, prepared.images)?;
        replacements.insert(
            group_id("geometry", id),
            Replacement {
                scene,
                offset: [
                    (path.origin[0] - original.bounds.x) as f32,
                    (path.origin[1] - original.bounds.y) as f32,
                ],
            },
        );
    }
    Ok(Batch {
        request,
        replacements,
    })
}

pub(super) fn group_id(kind: &str, id: &str) -> String {
    let encoded: String = id
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("graph-0-drawing-{kind}-{encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_batch_is_local_bounded_and_does_not_mutate_source() {
        let source = include_str!("../../../../docs/examples/diagrams/drawing.excalidraw");
        let model = Arc::new(Scene::parse(source).unwrap());
        let lines = little_clip_render::diagram::retained::drawing_lines(source).unwrap();
        let paths = model
            .movement(&["capture".into()], [-75.0, 43.0])
            .unwrap()
            .paths;
        let request = Request {
            model: model.clone(),
            paths,
        };
        assert!(regenerate(request.clone(), &model, &lines, &AtomicBool::new(true)).is_err());
        let batch = regenerate(request, &model, &lines, &AtomicBool::new(false)).unwrap();
        assert_eq!(batch.replacements.len(), 1);
        assert_eq!(
            batch.replacements[&group_id("geometry", "connection")].offset,
            [-75.0, 43.0]
        );
        assert_eq!(model.source(), source);
    }
}
