use std::error::Error;
use std::fmt;

use crate::scene::{ElementKind, SourceField};
use crate::{
    CompatibilityDiagnostic, ErrorCode, ErrorDescriptor, FrameRenderRequest, RenderError,
    RenderOutcome, SceneDocument,
};

/// Kind of live frame occurrence discovered in a scene document.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum FrameKind {
    /// An ordinary Excalidraw frame.
    Frame,
    /// An Excalidraw magic frame.
    MagicFrame,
}

impl FrameKind {
    /// Returns the stable Excalidraw element-kind spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Frame => "frame",
            Self::MagicFrame => "magicframe",
        }
    }
}

impl fmt::Display for FrameKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Borrowed identity and display metadata for one live frame occurrence.
#[derive(Clone, Copy)]
pub struct FrameRef<'scene> {
    pub(crate) scene: &'scene SceneDocument,
    pub(crate) source_index: usize,
    ordinal: u32,
    kind: FrameKind,
    id: Option<&'scene str>,
    name: Option<&'scene str>,
}

impl FrameRef<'_> {
    /// Returns the one-based source-order position among live frame occurrences.
    pub const fn ordinal(self) -> u32 {
        self.ordinal
    }

    /// Returns whether this is an ordinary or magic frame.
    pub const fn kind(self) -> FrameKind {
        self.kind
    }

    pub(crate) fn belongs_to(self, scene: &SceneDocument) -> bool {
        self.scene.same_identity(scene)
    }
}

impl<'scene> FrameRef<'scene> {
    /// Returns the source element ID when it is a non-empty string.
    ///
    /// Excalidraw gives every element a unique ID, but a hand-edited or
    /// damaged scene can omit or repeat one; the renderer repairs such IDs
    /// when it renders, and the [ordinal](Self::ordinal) always identifies
    /// the occurrence.
    pub const fn id(self) -> Option<&'scene str> {
        self.id
    }

    /// Returns the source display name when it was a string.
    pub const fn name(self) -> Option<&'scene str> {
        self.name
    }
}

/// Two references are equal when they name the same frame occurrence of the
/// same admitted scene. A clone of a [`SceneDocument`] is the same scene; a
/// second parse of the same bytes is not.
impl PartialEq for FrameRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.belongs_to(other.scene) && self.source_index == other.source_index
    }
}

impl Eq for FrameRef<'_> {}

impl std::hash::Hash for FrameRef<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.scene.hash_identity(state);
        self.source_index.hash(state);
    }
}

impl fmt::Debug for FrameRef<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrameRef")
            .field("ordinal", &self.ordinal)
            .field("kind", &self.kind)
            .field("id", &self.id)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Iterator over live ordinary and magic frame occurrences.
#[derive(Clone, Debug)]
pub struct FrameRefs<'scene> {
    scene: &'scene SceneDocument,
    source_index: usize,
    ordinal: u32,
}

impl<'scene> FrameRefs<'scene> {
    pub(crate) const fn new(scene: &'scene SceneDocument) -> Self {
        Self {
            scene,
            source_index: 0,
            ordinal: 0,
        }
    }
}

impl<'scene> Iterator for FrameRefs<'scene> {
    type Item = FrameRef<'scene>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.source_index < self.scene.element_occurrence_count() {
            let source_index = self.source_index;
            self.source_index += 1;
            if self.scene.element_is_silent(source_index) {
                continue;
            }
            let Some(source) = self.scene.element_at(source_index) else {
                continue;
            };
            let kind = match source.kind {
                ElementKind::Frame => FrameKind::Frame,
                ElementKind::MagicFrame => FrameKind::MagicFrame,
                _ => continue,
            };
            self.ordinal = self.ordinal.saturating_add(1);
            let id = match source.id {
                SourceField::Value(id) if !id.is_empty() => Some(id),
                SourceField::Value(_) | SourceField::Missing | SourceField::Invalid => None,
            };
            let name = match source.name {
                SourceField::Value(name) => name,
                SourceField::Missing | SourceField::Invalid => None,
            };
            return Some(FrameRef {
                scene: self.scene,
                source_index,
                ordinal: self.ordinal,
                kind,
                id,
                name,
            });
        }
        None
    }
}

/// Stable category of a lazy frame-batch construction failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FrameBatchErrorKind {
    /// A frame reference belongs to another scene document.
    ForeignTarget,
    /// The same frame occurrence was requested more than once.
    DuplicateTarget,
    /// Shared target-neutral preparation could not be completed.
    SharedPreparationFailed,
}

/// Failure to construct a lazy multi-frame render operation.
///
/// [`Error::source`] is always `None`: a shared-preparation failure is
/// flattened into its kind and subject.
#[derive(Clone, Debug)]
pub struct FrameBatchError {
    kind: FrameBatchErrorKind,
    ordinal: Option<u32>,
    preparation_subject: Option<&'static str>,
}

impl FrameBatchError {
    pub(crate) const fn new(kind: FrameBatchErrorKind, ordinal: u32) -> Self {
        Self {
            kind,
            ordinal: Some(ordinal),
            preparation_subject: None,
        }
    }

    pub(crate) const fn preparation(error: &crate::render::FramePreparationError) -> Self {
        Self::from_preparation_subject(error.subject())
    }

    const fn from_preparation_subject(subject: &'static str) -> Self {
        Self {
            kind: FrameBatchErrorKind::SharedPreparationFailed,
            ordinal: None,
            preparation_subject: Some(subject),
        }
    }

    /// Returns the stable batch-construction failure category.
    pub const fn kind(&self) -> FrameBatchErrorKind {
        self.kind
    }

    /// Returns the implicated frame ordinal for a target-selection failure.
    pub const fn ordinal(&self) -> Option<u32> {
        self.ordinal
    }

    /// Returns the stable machine-readable error code.
    pub const fn code(&self) -> ErrorCode {
        match self.kind {
            FrameBatchErrorKind::ForeignTarget => ErrorCode::FRAME_BATCH_FOREIGN_TARGET,
            FrameBatchErrorKind::DuplicateTarget => ErrorCode::FRAME_BATCH_DUPLICATE_TARGET,
            FrameBatchErrorKind::SharedPreparationFailed => {
                ErrorCode::FRAME_BATCH_SHARED_PREPARATION_FAILED
            }
        }
    }

    /// Returns this failure's stable serializable descriptor.
    pub fn descriptor(&self) -> ErrorDescriptor {
        ErrorDescriptor::frame_batch(self.code(), self.to_string())
    }
}

impl fmt::Display for FrameBatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            FrameBatchErrorKind::ForeignTarget => {
                write!(
                    formatter,
                    "frame {} belongs to another scene",
                    self.ordinal.unwrap_or(0)
                )
            }
            FrameBatchErrorKind::DuplicateTarget => {
                write!(
                    formatter,
                    "frame {} was selected more than once",
                    self.ordinal.unwrap_or(0)
                )
            }
            FrameBatchErrorKind::SharedPreparationFailed => write!(
                formatter,
                "shared frame preparation failed for {}",
                self.preparation_subject.unwrap_or("unknown_subject")
            ),
        }
    }
}

impl Error for FrameBatchError {}

/// One target result yielded by a lazy multi-frame render operation.
#[derive(Debug)]
pub struct FrameRenderEntry<'scene> {
    frame: FrameRef<'scene>,
    result: Result<RenderOutcome, RenderError>,
    target_diagnostics: Vec<CompatibilityDiagnostic>,
}

impl<'scene> FrameRenderEntry<'scene> {
    /// Returns the target frame identity.
    pub const fn frame(&self) -> FrameRef<'scene> {
        self.frame
    }

    /// Returns the complete target render result.
    pub const fn result(&self) -> &Result<RenderOutcome, RenderError> {
        &self.result
    }

    /// Returns diagnostics attributed specifically to this target.
    pub fn target_diagnostics(&self) -> &[CompatibilityDiagnostic] {
        &self.target_diagnostics
    }

    /// Consumes the entry and returns its complete target result.
    ///
    /// # Errors
    ///
    /// Returns this frame's [`RenderError`] when it could not be rendered,
    /// as [`Renderer::render_frame`](crate::Renderer::render_frame) would.
    pub fn into_result(self) -> Result<RenderOutcome, RenderError> {
        self.result
    }
}

/// Lazy source-ordered rendering of selected frames from one scene document.
pub struct FrameRenderCursor<'scene, 'request> {
    catalog: &'static crate::catalog::RevisionCatalog,
    frames: Vec<FrameRef<'scene>>,
    request: &'request FrameRenderRequest,
    prepared: Option<crate::render::PreparedFrameRender>,
    next_index: usize,
    global_diagnostics: Vec<CompatibilityDiagnostic>,
    global_diagnostics_initialized: bool,
}

impl fmt::Debug for FrameRenderCursor<'_, '_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrameRenderCursor")
            .field("frame_count", &self.frames.len())
            .field("next_index", &self.next_index)
            .field("prepared", &self.prepared.is_some())
            .finish_non_exhaustive()
    }
}

impl<'scene, 'request> FrameRenderCursor<'scene, 'request> {
    pub(crate) fn new(
        catalog: &'static crate::catalog::RevisionCatalog,
        scene: &'scene SceneDocument,
        frames: impl IntoIterator<Item = FrameRef<'scene>>,
        request: &'request FrameRenderRequest,
    ) -> Result<Self, FrameBatchError> {
        let mut selected = Vec::new();
        for frame in frames {
            selected
                .try_reserve(1)
                .map_err(|_| FrameBatchError::from_preparation_subject("target_selection"))?;
            selected.push(frame);
        }
        let mut frames = selected;
        for frame in &frames {
            if !frame.belongs_to(scene) {
                return Err(FrameBatchError::new(
                    FrameBatchErrorKind::ForeignTarget,
                    frame.ordinal(),
                ));
            }
        }
        frames.sort_unstable_by_key(|frame| frame.ordinal());
        if let Some(duplicate) = frames
            .windows(2)
            .find(|frames| frames[0].source_index == frames[1].source_index)
        {
            return Err(FrameBatchError::new(
                FrameBatchErrorKind::DuplicateTarget,
                duplicate[0].ordinal(),
            ));
        }
        let prepared = if frames.is_empty() {
            None
        } else {
            Some(
                crate::render::PreparedFrameRender::new(
                    catalog.frame_profile(),
                    catalog.scene_surface(),
                    scene,
                    request.as_render_request(),
                )
                .map_err(|error| FrameBatchError::preparation(&error))?,
            )
        };
        Ok(Self {
            catalog,
            frames,
            request,
            prepared,
            next_index: 0,
            global_diagnostics: Vec::new(),
            global_diagnostics_initialized: false,
        })
    }

    /// Returns scene-global diagnostics discovered by targets rendered so far.
    pub fn global_diagnostics(&self) -> &[CompatibilityDiagnostic] {
        &self.global_diagnostics
    }
}

impl<'scene> Iterator for FrameRenderCursor<'scene, '_> {
    type Item = FrameRenderEntry<'scene>;

    fn next(&mut self) -> Option<Self::Item> {
        let frame = *self.frames.get(self.next_index)?;
        self.next_index += 1;
        let (result, global_diagnostics, target_diagnostics) = crate::render::render_frame_scoped(
            self.catalog,
            frame,
            self.request,
            self.prepared.as_ref(),
        );
        if !self.global_diagnostics_initialized {
            self.global_diagnostics = global_diagnostics;
            self.global_diagnostics_initialized = true;
        }
        Some(FrameRenderEntry {
            frame,
            result,
            target_diagnostics,
        })
    }
}
