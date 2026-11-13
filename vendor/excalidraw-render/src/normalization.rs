//! Compatibility normalization: the deterministic preprocessing that
//! reproduces the rendering-affecting behavior of the reference renderer's
//! restore path. This file owns the normalized types, the scene-level pass
//! (`normalize_scene_impl`) with its journal of preflight, commit and diagnostic
//! events, the file-record and frame-rendering normalizers, generated identities
//! and seeds, and the diagnostic constructors; `kinds` normalizes each element
//! kind's values, `planning` prices them for the ledger, and `fractional_index`
//! repairs element order.

mod fractional_index;
mod kinds;
mod planning;
#[cfg(test)]
mod tests;

use kinds::{element_fields, normalize_element_kind, normalized_roundness, ElementBox};
pub(crate) use planning::NORMALIZATION_WORK_UNITS;
use planning::{
    kind_working_bytes, normalized_working_bytes, owned_field_bytes, planned_kind_working_bytes,
    GENERATED_ID_BYTES,
};

use std::collections::{HashSet, TryReserveError};

use sha2::{Digest, Sha256};

use crate::diagnostic_registry::{DiagnosticKind, FallbackRecipe, ImageFailure, ImageRecovery};
use crate::outcome::PendingDiagnostic;
use crate::scene::{
    pointer_child, pointer_index, ElementKind, ElementSource, FrameRenderingSource, SourceField,
};
#[cfg(test)]
use crate::{CompatibilityDiagnostic, DiagnosticCode};
use crate::{
    CompatibilityRevision, DiagnosticEffect, OutputFormat, Recovery, SceneDocument,
    SupplementalAssets,
};

const MISSING_ID_DOMAIN: &[u8] = b"excalidraw-render:missing-element-id:v1\0";
const DUPLICATE_ID_DOMAIN: &[u8] = b"excalidraw-render:duplicate-element-id:v1\0";
const ID_COLLISION_DOMAIN: &[u8] = b"excalidraw-render:element-id-collision:v1\0";
const ROUGH_SEED_DOMAIN: &[u8] = b"excalidraw-render:rough-seed:v1\0";

#[derive(Clone, Debug)]
pub(crate) struct NormalizedElement {
    pub(crate) source: NormalizedElementSource,
    pub(crate) source_order: u64,
    pub(crate) index: String,
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
    pub(crate) angle: f64,
    pub(crate) stroke_color: String,
    pub(crate) background_color: String,
    pub(crate) fill_style: String,
    pub(crate) stroke_width: f64,
    pub(crate) stroke_style: String,
    pub(crate) roughness: f64,
    pub(crate) opacity: f64,
    pub(crate) link: Option<String>,
    pub(crate) frame_id: Option<String>,
    pub(crate) seed: i32,
    pub(crate) roundness: Option<NormalizedRoundness>,
    pub(crate) normalized_working_bytes: u64,
    pub(crate) normalization_work_units: u64,
    pub(crate) repair_work_units: u64,
    pub(crate) kind_name: &'static str,
    pub(crate) kind: NormalizedElementKind,
    pub(crate) direct_outline_compatible: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NormalizedElementKind {
    Rectangle,
    Diamond,
    Ellipse,
    Text(NormalizedText),
    Linear(NormalizedLinear),
    Freedraw(NormalizedFreedraw),
    Image(NormalizedImage),
    Frame(NormalizedFrame),
    MagicFrame(NormalizedFrame),
    Embeddable { iframe: bool, label: String },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NormalizedText {
    pub(crate) font_size: f64,
    pub(crate) font_family: i32,
    pub(crate) text: String,
    pub(crate) original_text: String,
    pub(crate) text_align: String,
    pub(crate) vertical_align: String,
    pub(crate) line_height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum NormalizedRoundness {
    Proportional,
    Adaptive { radius: f64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NormalizedLinearKind {
    Line,
    Arrow,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NormalizedLinear {
    pub(crate) kind: NormalizedLinearKind,
    pub(crate) points: Vec<[f64; 2]>,
    pub(crate) start_binding: Option<NormalizedBinding>,
    pub(crate) end_binding: Option<NormalizedBinding>,
    pub(crate) last_committed_point: Option<[f64; 2]>,
    pub(crate) start_arrowhead: Option<String>,
    pub(crate) end_arrowhead: Option<String>,
    pub(crate) elbowed: bool,
    pub(crate) geometry_available: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NormalizedBinding {
    pub(crate) element_id: String,
    pub(crate) focus: f64,
    pub(crate) gap: f64,
    pub(crate) fixed_point: Option<[f64; 2]>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NormalizedFreedraw {
    pub(crate) points: Vec<[f64; 2]>,
    pub(crate) pressures: Vec<f64>,
    pub(crate) simulate_pressure: bool,
    pub(crate) last_committed_point: Option<[f64; 2]>,
    pub(crate) geometry_available: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NormalizedImageStatus {
    Pending,
    Saved,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NormalizedImage {
    pub(crate) file_id: Option<String>,
    pub(crate) status: NormalizedImageStatus,
    pub(crate) scale: [i8; 2],
    pub(crate) crop: Option<NormalizedImageCrop>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NormalizedImageCrop {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
    pub(crate) natural_width: f64,
    pub(crate) natural_height: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NormalizedFrame {
    pub(crate) name: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NormalizedFrameRendering {
    pub(crate) enabled: bool,
    pub(crate) clip: bool,
    pub(crate) name: bool,
    pub(crate) outline: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NormalizedFileRecord {
    pub(crate) key: String,
    pub(crate) id: String,
    pub(crate) created: Option<u64>,
    pub(crate) source_pointer: String,
    pub(crate) source_order: u64,
    pub(crate) normalized_working_bytes: u64,
    normalization_work_units: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct NormalizedElementSource {
    source_index: usize,
    id: String,
    pointer: String,
    coordinate_positions: [usize; 4],
}

impl NormalizedElementSource {
    pub(crate) fn try_clone(&self) -> Result<Self, TryReserveError> {
        Ok(Self {
            source_index: self.source_index,
            id: try_owned(&self.id)?,
            pointer: try_owned(&self.pointer)?,
            coordinate_positions: self.coordinate_positions,
        })
    }
    pub(crate) const fn source_index(&self) -> usize {
        self.source_index
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn pointer(&self) -> &str {
        &self.pointer
    }

    /// The source position of one coordinate member in the total diagnostic
    /// order, packed the way every other element diagnostic packs its member
    /// position so a resource crossing on `width` sorts after a repair on `y`.
    pub(crate) fn coordinate_position(&self, field: &str) -> u64 {
        let position = match field {
            "x" => self.coordinate_positions[0],
            "y" => self.coordinate_positions[1],
            "width" => self.coordinate_positions[2],
            "height" => self.coordinate_positions[3],
            _ => usize::MAX,
        };
        SourceDiagnosticPosition::top(position).as_u64()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct NormalizationOutcome {
    pub(crate) elements: Vec<NormalizedElement>,
    pub(crate) fallbacks: Vec<crate::scene_surface::SurfaceFallback>,
    pub(crate) view_background_color: String,
    pub(crate) view_background_working_bytes: u64,
    pub(crate) frame_rendering: NormalizedFrameRendering,
    pub(crate) files: Vec<NormalizedFileRecord>,
}

pub(crate) enum NormalizationEvent<'a> {
    PreflightRecord {
        event: &'static str,
        json_pointer: &'a str,
        working_bytes: u64,
        work_units: u64,
    },
    CommitRecord {
        event: &'static str,
        element_id: Option<&'a str>,
        local_owner: Option<&'a str>,
        json_pointer: &'a str,
        working_bytes: u64,
        work_units: u64,
    },
    Diagnostic {
        diagnostic: PendingDiagnostic,
        local_owner: Option<&'a str>,
        source_order: u64,
        source_position: u64,
    },
}

struct PendingNormalizationEvent {
    source_order: u64,
    ordinal: u64,
    kind: PendingNormalizationEventKind,
}

enum PendingNormalizationEventKind {
    Record {
        event: &'static str,
        preflighted: bool,
        element_id: Option<String>,
        local_owner: Option<String>,
        json_pointer: String,
        working_bytes: u64,
        work_units: u64,
    },
    Diagnostic {
        diagnostic: PendingDiagnostic,
        local_owner: Option<String>,
        source_position: u64,
    },
}

#[derive(Debug)]
pub(crate) enum NormalizationError<E> {
    Observer(E),
    MemoryUnavailable { source_index: Option<usize> },
}

impl<E> From<TryReserveError> for NormalizationError<E> {
    fn from(_: TryReserveError) -> Self {
        Self::MemoryUnavailable { source_index: None }
    }
}

struct CommonFields<'a> {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    angle: f64,
    stroke_color: &'a str,
    background_color: &'a str,
    fill_style: &'a str,
    stroke_width: f64,
    stroke_style: &'a str,
    roughness: f64,
    opacity: f64,
    source_seed: i32,
}

enum IdentityPlan<'a> {
    Retained(&'a str),
    Duplicate(&'a str),
    Missing,
    Invalid,
}

struct PendingElement<'a> {
    source_index: usize,
    source: ElementSource<'a>,
    fields: CommonFields<'a>,
    direct_outline_compatible: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SourceDiagnosticPosition {
    member: usize,
    nested: usize,
}

impl SourceDiagnosticPosition {
    const fn top(member: usize) -> Self {
        Self { member, nested: 0 }
    }

    const fn nested(member: usize, nested: usize) -> Self {
        Self {
            member,
            nested: nested.saturating_add(1),
        }
    }

    /// Packs the member position into the high half and the nested ordinal
    /// into the low half, so a diagnostic sorts by the source member it
    /// belongs to before its position inside that member. A missing nested
    /// member is spelled with a saturated nested ordinal; clamping it to the
    /// low half keeps it last within its own member rather than after every
    /// later member of the element.
    fn as_u64(self) -> u64 {
        // A missing source member has no packed position. Widen its sentinel
        // explicitly: on wasm32, shifting usize::MAX would otherwise produce
        // 0xffffffff00000000 instead of the native saturated u64::MAX, putting
        // missing-timestamp repairs before render-phase missing-glyph reports.
        if self.member == usize::MAX {
            return u64::MAX;
        }
        let nested = u64::try_from(self.nested)
            .unwrap_or(u64::MAX)
            .min(u64::from(u32::MAX));
        u64::try_from(self.member)
            .unwrap_or(u64::MAX)
            .saturating_mul(u64::from(u32::MAX) + 1)
            .saturating_add(nested)
    }
}

fn nested_path_ordinal(first: usize, second: usize) -> usize {
    first
        .saturating_add(1)
        .saturating_mul(usize::from(u16::MAX) + 1)
        .saturating_add(second.saturating_add(1))
}

type ElementDiagnostic = (SourceDiagnosticPosition, PendingDiagnostic);

struct ElementDiagnosticContext<'a, 'scene> {
    diagnostics: &'a mut Vec<ElementDiagnostic>,
    source: &'a ElementSource<'scene>,
    element_pointer: &'a str,
    element_id: &'a str,
    format: OutputFormat,
    policy: &'static crate::scene_surface::SceneSurfacePolicy,
    diagnostic_limit: usize,
}

#[derive(Clone, Copy)]
struct NormalizationParticipation {
    authoritative_file: bool,
    frame_names: bool,
}

impl ElementDiagnosticContext<'_, '_> {
    fn can_retain(&self) -> bool {
        self.diagnostics.len() < self.diagnostic_limit
    }

    /// Diagnoses `field`'s unsupported value with the manifested field
    /// policy's code, effects, recovery and recipe when the sealed policy has
    /// one for the element kind, otherwise as an unsupported property value
    /// recovered as the caller's normalization rule says.
    fn unsupported(
        &mut self,
        field: &str,
        effects: &[DiagnosticEffect],
        recovery: Recovery,
        recipe: Option<FallbackRecipe>,
    ) -> Result<(), TryReserveError> {
        if !self.can_retain() {
            return Ok(());
        }
        let pointer = pointer_child(self.element_pointer, field)?;
        let semantic_path = pointer_child("", field)?;
        let manifested = crate::scene_surface::manifested_element_field(
            self.policy,
            self.source.kind.scene_surface_kind(),
            &semantic_path,
        );
        let manifested_effects = manifested
            .map(|field| manifested_effects(field.invalid.effects))
            .transpose()?;
        let recovery = manifested
            .and_then(|field| field.invalid.recovery)
            .and_then(Recovery::from_manifest)
            .unwrap_or(recovery);
        let manifested_recipe = manifested.and_then(|field| field.invalid.recipe);
        let kind = match manifested.and_then(|field| field.invalid.code) {
            Some(code) => {
                let Some(kind) = DiagnosticKind::manifested(code, recovery, manifested_recipe)
                else {
                    return Ok(());
                };
                kind
            }
            None => DiagnosticKind::UnsupportedPropertyValue {
                recovery,
                recipe: match manifested_recipe {
                    Some(recipe) => FallbackRecipe::from_id(recipe),
                    None => recipe,
                },
            },
        };
        let diagnostic = PendingDiagnostic::new(
            kind,
            self.format,
            manifested_effects.as_deref().unwrap_or(effects),
            Some(self.element_id),
            &pointer,
            "unsupported property value used its revision-1 recovery",
        )?;
        self.diagnostics.try_reserve(1)?;
        self.diagnostics.push((
            SourceDiagnosticPosition::top(self.source.member_position(field).unwrap_or(usize::MAX)),
            diagnostic,
        ));
        Ok(())
    }

    fn unsupported_nested(
        &mut self,
        field: &str,
        nested_field: &str,
        nested_position: Option<usize>,
        component: Option<usize>,
        effects: &[DiagnosticEffect],
        recovery: Recovery,
    ) -> Result<(), TryReserveError> {
        if !self.can_retain() {
            return Ok(());
        }
        let mut pointer =
            pointer_child(&pointer_child(self.element_pointer, field)?, nested_field)?;
        if let Some(component) = component {
            pointer = pointer_index(&pointer, component)?;
        }
        let diagnostic = PendingDiagnostic::new(
            DiagnosticKind::UnsupportedPropertyValue {
                recovery,
                recipe: None,
            },
            self.format,
            effects,
            Some(self.element_id),
            &pointer,
            "unsupported nested property value used its revision-1 recovery",
        )?;
        self.diagnostics.try_reserve(1)?;
        self.diagnostics.push((
            SourceDiagnosticPosition::nested(
                self.source.member_position(field).unwrap_or(usize::MAX),
                nested_position.unwrap_or(usize::MAX - 1),
            ),
            diagnostic,
        ));
        Ok(())
    }

    fn unsupported_property_nested(
        &mut self,
        field: &str,
        nested_field: &str,
        nested_position: usize,
    ) -> Result<(), TryReserveError> {
        if !self.can_retain() {
            return Ok(());
        }
        let pointer = pointer_child(&pointer_child(self.element_pointer, field)?, nested_field)?;
        let editor_only = matches!(field, "startBinding" | "endBinding")
            && crate::scene_surface::is_editor_only_property(
                crate::scene_surface::PropertyOwner::Binding,
                &pointer_child("", nested_field)?,
            );
        let diagnostic = crate::scene_surface::unrecognized_property_diagnostic(
            self.format,
            Some(self.element_id),
            &pointer,
            editor_only,
            "unrecognized nested property was ignored",
        )?;
        self.diagnostics.try_reserve(1)?;
        self.diagnostics.push((
            SourceDiagnosticPosition::nested(
                self.source.member_position(field).unwrap_or(usize::MAX),
                nested_position,
            ),
            diagnostic,
        ));
        Ok(())
    }

    fn unsupported_property_at(
        &mut self,
        field: &str,
        nested_position: usize,
        pointer: &str,
    ) -> Result<(), TryReserveError> {
        if !self.can_retain() {
            return Ok(());
        }
        let diagnostic = crate::scene_surface::unrecognized_property_diagnostic(
            self.format,
            Some(self.element_id),
            pointer,
            false,
            "unrecognized nested property was ignored",
        )?;
        self.diagnostics.try_reserve(1)?;
        self.diagnostics.push((
            SourceDiagnosticPosition::nested(
                self.source.member_position(field).unwrap_or(usize::MAX),
                nested_position,
            ),
            diagnostic,
        ));
        Ok(())
    }

    fn missing_image_file(
        &mut self,
        file_id: Option<&str>,
        status: NormalizedImageStatus,
    ) -> Result<(), TryReserveError> {
        if !self.can_retain() {
            return Ok(());
        }
        let pointer = pointer_child(self.element_pointer, "fileId")?;
        let diagnostic = PendingDiagnostic::new(
            DiagnosticKind::ImageUnavailable {
                failure: ImageFailure::MissingFile,
                asset_id: file_id.unwrap_or(""),
                recovery: ImageRecovery::for_format(
                    self.format,
                    status == NormalizedImageStatus::Error,
                ),
            },
            self.format,
            &[DiagnosticEffect::Visual],
            Some(self.element_id),
            &pointer,
            "image reference had no authoritative file payload",
        )?;
        self.diagnostics.try_reserve(1)?;
        self.diagnostics.push((
            SourceDiagnosticPosition::top(
                self.source.member_position("fileId").unwrap_or(usize::MAX),
            ),
            diagnostic,
        ));
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn normalize_scene<E>(
    scene: &SceneDocument,
    revision: CompatibilityRevision,
    format: OutputFormat,
    observe: impl FnMut(NormalizationEvent<'_>) -> Result<(), E>,
) -> Result<Option<NormalizationOutcome>, NormalizationError<E>> {
    let Some(policy) =
        crate::catalog::for_revision(revision).map(|catalog| catalog.scene_surface())
    else {
        return Ok(None);
    };
    normalize_scene_impl(
        scene,
        revision,
        policy,
        format,
        true,
        usize::MAX,
        |_| false,
        observe,
    )
}

/// The diagnostics revision-1 normalization emits for `scene`, in the order
/// the session's final report lists them, with the normalized outcome. The
/// session retains each normalization diagnostic under the source owner and
/// member position of the event that carried it (`DiagnosticOrder::retained`)
/// and sorts the report (`render::sort_diagnostics`); this does the same, so a
/// test at this seam asserts the order a render would report for diagnostics
/// normalization alone produces. `frame_names` is the request's frame-name
/// control as the session derives it; `supplemental_assets` decides which file
/// records have a supplemental payload.
#[cfg(test)]
pub(crate) fn reported_diagnostics(
    scene: &SceneDocument,
    format: OutputFormat,
    frame_names: bool,
    supplemental_assets: &SupplementalAssets,
) -> (Vec<CompatibilityDiagnostic>, NormalizationOutcome) {
    use crate::outcome::{DiagnosticAttribution, DiagnosticOrder};

    let policy = crate::catalog::for_revision(CompatibilityRevision::REVISION_1)
        .expect("revision 1 is cataloged")
        .scene_surface();
    let mut diagnostics: Vec<CompatibilityDiagnostic> = Vec::new();
    let normalized = normalize_scene_with_assets(
        scene,
        CompatibilityRevision::REVISION_1,
        policy,
        format,
        supplemental_assets,
        frame_names,
        usize::MAX,
        |event| {
            if let NormalizationEvent::Diagnostic {
                diagnostic,
                source_order,
                source_position,
                ..
            } = event
            {
                let order = DiagnosticOrder::retained(
                    Some(source_order),
                    Some(source_position),
                    u64::try_from(diagnostics.len()).unwrap_or(u64::MAX),
                    DiagnosticAttribution::SceneGlobal,
                );
                diagnostics.push(diagnostic.retained(order));
            }
            Ok::<_, ()>(())
        },
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");
    crate::render::sort_diagnostics(&mut diagnostics);
    (diagnostics, normalized)
}

/// `(code, pointer)` per diagnostic: what most assertions on a report compare.
#[cfg(test)]
pub(crate) fn codes_and_pointers(
    diagnostics: &[CompatibilityDiagnostic],
) -> Vec<(DiagnosticCode, &str)> {
    diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.code(), diagnostic.json_pointer()))
        .collect()
}

/// The pointer of each diagnostic, in report order.
#[cfg(test)]
pub(crate) fn pointers(diagnostics: &[CompatibilityDiagnostic]) -> Vec<&str> {
    diagnostics
        .iter()
        .map(CompatibilityDiagnostic::json_pointer)
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn normalize_scene_with_assets<E>(
    scene: &SceneDocument,
    revision: CompatibilityRevision,
    policy: &'static crate::scene_surface::SceneSurfacePolicy,
    format: OutputFormat,
    supplemental_assets: &SupplementalAssets,
    frame_names: bool,
    surface_diagnostic_limit: usize,
    observe: impl FnMut(NormalizationEvent<'_>) -> Result<(), E>,
) -> Result<Option<NormalizationOutcome>, NormalizationError<E>> {
    normalize_scene_impl(
        scene,
        revision,
        policy,
        format,
        frame_names,
        surface_diagnostic_limit,
        |file_id| supplemental_assets.get(file_id).is_some(),
        observe,
    )
}

#[allow(clippy::too_many_arguments)]
fn normalize_scene_impl<E>(
    scene: &SceneDocument,
    revision: CompatibilityRevision,
    policy: &'static crate::scene_surface::SceneSurfacePolicy,
    format: OutputFormat,
    requested_frame_names: bool,
    surface_diagnostic_limit: usize,
    has_supplemental_file: impl Fn(&str) -> bool,
    mut observe: impl FnMut(NormalizationEvent<'_>) -> Result<(), E>,
) -> Result<Option<NormalizationOutcome>, NormalizationError<E>> {
    // Name participation follows both request and persisted scene controls.
    // Invalid/missing booleans default to true; only explicit false disables.
    let frame_names = requested_frame_names
        && match scene.frame_rendering() {
            SourceField::Value(rendering) => {
                !matches!(rendering.boolean("enabled"), SourceField::Value(false))
                    && !matches!(rendering.boolean("name"), SourceField::Value(false))
            }
            SourceField::Missing | SourceField::Invalid => true,
        };
    let view_background_color = manifested_value(
        scene.view_background_color(),
        "#ffffff",
        crate::scene_surface::css_color_is_admitted,
    );
    let view_background_working_bytes = owned_field_bytes(view_background_color.len());
    let element_count = scene.element_occurrence_count();
    let mut pending_events = Vec::new();
    pending_events.try_reserve(element_count.saturating_add(2))?;
    let surface = crate::scene_surface::classify(
        scene,
        policy,
        format,
        frame_names,
        surface_diagnostic_limit,
    )?;
    let mut planned_diagnostic_count = surface.diagnostics.len();
    for surface_diagnostic in surface.diagnostics {
        push_pending_event(
            &mut pending_events,
            surface_diagnostic.source_order,
            PendingNormalizationEventKind::Diagnostic {
                diagnostic: surface_diagnostic.diagnostic,
                local_owner: surface_diagnostic.local_owner,
                source_position: surface_diagnostic.source_position,
            },
        )?;
    }
    let mut pending = Vec::new();
    pending.try_reserve_exact(element_count)?;

    for index in 0..element_count {
        if scene.element_is_silent(index) {
            continue;
        }
        let Some(source) = scene.element_at(index) else {
            continue;
        };
        let is_deleted = match source.is_deleted {
            SourceField::Value(value) => value,
            SourceField::Missing | SourceField::Invalid => false,
        };
        if is_deleted {
            continue;
        }
        let Some(fields) = element_fields(&source, policy) else {
            return Ok(None);
        };
        let direct_outline_compatible = matches!(
            source.kind,
            ElementKind::Rectangle
                | ElementKind::Diamond
                | ElementKind::Ellipse
                | ElementKind::Line
                | ElementKind::LegacyDraw
                | ElementKind::Freedraw
                | ElementKind::Arrow
                | ElementKind::Iframe
                | ElementKind::Embeddable
        );
        pending.push(PendingElement {
            source_index: index,
            source,
            fields,
            direct_outline_compatible,
        });
    }

    let mut referenced_file_ids = Vec::new();
    referenced_file_ids.try_reserve_exact(pending.len())?;
    for element in &pending {
        if matches!(element.source.kind, ElementKind::Image) {
            if let SourceField::Value(Some(file_id)) = element.source.file_id {
                referenced_file_ids.push(file_id);
            }
        }
    }
    referenced_file_ids.sort_unstable();
    referenced_file_ids.dedup();
    let file_surface_diagnostics = crate::scene_surface::classify_referenced_files(
        scene,
        policy,
        &referenced_file_ids,
        format,
        surface_diagnostic_limit.saturating_sub(planned_diagnostic_count),
    )?;
    planned_diagnostic_count =
        planned_diagnostic_count.saturating_add(file_surface_diagnostics.len());
    for surface_diagnostic in file_surface_diagnostics {
        push_pending_event(
            &mut pending_events,
            surface_diagnostic.source_order,
            PendingNormalizationEventKind::Diagnostic {
                diagnostic: surface_diagnostic.diagnostic,
                local_owner: surface_diagnostic.local_owner,
                source_position: surface_diagnostic.source_position,
            },
        )?;
    }

    let mut scene_file_ids = Vec::new();
    scene_file_ids.try_reserve_exact(scene.file_record_count())?;
    for index in 0..scene.file_record_count() {
        if let Some(file) = scene.file_record_at(index) {
            scene_file_ids.push(file.key());
        }
    }
    scene_file_ids.sort_unstable();
    for file_id in &referenced_file_ids {
        if planned_diagnostic_count >= surface_diagnostic_limit {
            break;
        }
        if scene_file_ids.binary_search(file_id).is_ok() && has_supplemental_file(file_id) {
            if let Some(surface_diagnostic) =
                crate::scene_surface::supplement_conflict(scene, file_id, format)?
            {
                push_pending_event(
                    &mut pending_events,
                    surface_diagnostic.source_order,
                    PendingNormalizationEventKind::Diagnostic {
                        diagnostic: surface_diagnostic.diagnostic,
                        local_owner: surface_diagnostic.local_owner,
                        source_position: surface_diagnostic.source_position,
                    },
                )?;
                planned_diagnostic_count = planned_diagnostic_count.saturating_add(1);
            }
        }
    }

    let mut source_indices = Vec::new();
    source_indices.try_reserve_exact(pending.len())?;
    for element in &pending {
        source_indices.push(match element.source.index {
            SourceField::Value(Some(index)) if !index.is_empty() => Some(index),
            SourceField::Missing | SourceField::Value(None | Some(_)) | SourceField::Invalid => {
                None
            }
        });
    }
    if !fractional_index::are_strictly_ascending(&source_indices) {
        for index in &mut source_indices {
            if index.is_some_and(|index| !fractional_index::is_valid_key(index)) {
                *index = None;
            }
        }
    }
    let normalized_indices = fractional_index::repair(&source_indices)?;

    let mut elements = Vec::new();
    elements.try_reserve_exact(pending.len())?;
    // The identities already committed, for duplicate detection and generated
    // collision avoidance. It is only queried for membership, never iterated,
    // so its order cannot reach the output; a set keeps a scene of many long
    // identities linear rather than comparing each against every earlier one.
    let mut used_ids = HashSet::<String>::new();

    for (pending, normalized_index) in pending.into_iter().zip(normalized_indices) {
        let index = pending.source_index;
        let source = pending.source;
        let fields = pending.fields;
        macro_rules! memory_at_source {
            ($result:expr) => {
                $result.map_err(|_| NormalizationError::MemoryUnavailable {
                    source_index: Some(index),
                })?
            };
        }
        let element_pointer = memory_at_source!(pointer_index("/elements", index));
        let participation = NormalizationParticipation {
            authoritative_file: matches!(
                source.file_id,
                SourceField::Value(Some(file_id))
                    if scene_file_ids.binary_search(&file_id).is_ok()
                        || has_supplemental_file(file_id)
            ),
            frame_names,
        };

        let identity_plan = match source.id {
            SourceField::Value("") | SourceField::Invalid => IdentityPlan::Invalid,
            SourceField::Value(id) if used_ids.contains(id) => IdentityPlan::Duplicate(id),
            SourceField::Value(id) => IdentityPlan::Retained(id),
            SourceField::Missing => IdentityPlan::Missing,
        };
        let normalized_id_bytes = match identity_plan {
            IdentityPlan::Retained(id) => id.len(),
            IdentityPlan::Duplicate(_) | IdentityPlan::Missing | IdentityPlan::Invalid => {
                GENERATED_ID_BYTES
            }
        };
        let normalization_work_units = NORMALIZATION_WORK_UNITS.saturating_add(8).saturating_add(
            source
                .kind
                .normalized_field_count(participation)
                .saturating_mul(8),
        );
        let planned_working_bytes = normalized_working_bytes(normalized_id_bytes, &fields)
            .saturating_add(owned_field_bytes(normalized_index.len()))
            .saturating_add(memory_at_source!(planned_kind_working_bytes(
                &source,
                participation
            )));
        let normalization_event = source.kind.normalization_event_name();
        observe(NormalizationEvent::PreflightRecord {
            event: normalization_event,
            json_pointer: &element_pointer,
            working_bytes: planned_working_bytes,
            work_units: normalization_work_units,
        })
        .map_err(NormalizationError::Observer)?;

        let mut id_diagnostic = None;
        let id = match identity_plan {
            IdentityPlan::Retained(id) => memory_at_source!(try_owned(id)),
            IdentityPlan::Duplicate(original_id) => {
                let canonical_element = memory_at_source!(source.canonical_element_bytes());
                let id = memory_at_source!(generated_duplicate_element_id(
                    revision,
                    u64::try_from(index).unwrap_or(u64::MAX),
                    original_id,
                    &canonical_element,
                ));
                let id = memory_at_source!(unique_generated_id(
                    id,
                    revision,
                    u64::try_from(index).unwrap_or(u64::MAX),
                    &used_ids,
                ));
                id_diagnostic = Some((
                    DiagnosticKind::DuplicateElementId,
                    "duplicate element identity was rekeyed deterministically",
                ));
                id
            }
            IdentityPlan::Missing => {
                let canonical_element = memory_at_source!(source.canonical_element_bytes());
                let id = memory_at_source!(generated_element_id(
                    revision,
                    u64::try_from(index).unwrap_or(u64::MAX),
                    &canonical_element,
                ));
                let id = memory_at_source!(unique_generated_id(
                    id,
                    revision,
                    u64::try_from(index).unwrap_or(u64::MAX),
                    &used_ids,
                ));
                id_diagnostic = Some((
                    DiagnosticKind::MissingElementId,
                    "missing element identity was repaired deterministically",
                ));
                id
            }
            IdentityPlan::Invalid => {
                let canonical_element = memory_at_source!(source.canonical_element_bytes());
                let id = memory_at_source!(generated_element_id(
                    revision,
                    u64::try_from(index).unwrap_or(u64::MAX),
                    &canonical_element,
                ));
                let id = memory_at_source!(unique_generated_id(
                    id,
                    revision,
                    u64::try_from(index).unwrap_or(u64::MAX),
                    &used_ids,
                ));
                id_diagnostic = Some((
                    DiagnosticKind::UnsupportedPropertyValue {
                        recovery: Recovery::Repaired,
                        recipe: None,
                    },
                    "invalid element identity was repaired deterministically",
                ));
                id
            }
        };
        let element_diagnostic_limit =
            surface_diagnostic_limit.saturating_sub(planned_diagnostic_count);
        let mut element_diagnostics = Vec::new();
        memory_at_source!(element_diagnostics.try_reserve_exact(16));
        if element_diagnostics.len() < element_diagnostic_limit {
            if let Some((kind, message)) = id_diagnostic {
                let pointer = memory_at_source!(pointer_child(&element_pointer, "id"));
                let diagnostic = memory_at_source!(PendingDiagnostic::new(
                    kind,
                    format,
                    &[DiagnosticEffect::InputRepair],
                    Some(&id),
                    &pointer,
                    message,
                ));
                element_diagnostics.push((
                    SourceDiagnosticPosition::top(source.member_position("id").unwrap_or(0)),
                    diagnostic,
                ));
            }
        }

        let seed = if fields.source_seed == 0 {
            let seed = generated_rough_seed(revision, &id);
            if element_diagnostics.len() < element_diagnostic_limit {
                let pointer = memory_at_source!(pointer_child(&element_pointer, "seed"));
                let diagnostic = memory_at_source!(PendingDiagnostic::new(
                    DiagnosticKind::ZeroRoughSeed,
                    format,
                    &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
                    Some(&id),
                    &pointer,
                    "zero Rough seed was repaired deterministically",
                ));
                element_diagnostics.push((
                    SourceDiagnosticPosition::top(
                        source.member_position("seed").unwrap_or(usize::MAX - 1),
                    ),
                    diagnostic,
                ));
            }
            seed
        } else {
            fields.source_seed
        };

        if source.updated_is_missing && element_diagnostics.len() < element_diagnostic_limit {
            let pointer = memory_at_source!(pointer_child(&element_pointer, "updated"));
            let diagnostic = memory_at_source!(PendingDiagnostic::new(
                DiagnosticKind::MissingTimestamp,
                format,
                &[DiagnosticEffect::InputRepair],
                Some(&id),
                &pointer,
                "missing required timestamp was repaired to canonical zero",
            ));
            element_diagnostics.push((SourceDiagnosticPosition::top(usize::MAX), diagnostic));
        }

        let (kind, element_box) = memory_at_source!(normalize_element_kind(
            &source,
            &fields,
            policy,
            &element_pointer,
            &id,
            format,
            participation,
            element_diagnostic_limit,
            &mut element_diagnostics,
        ));
        let ElementBox {
            x,
            y,
            width,
            height,
        } = element_box;
        planned_diagnostic_count =
            planned_diagnostic_count.saturating_add(element_diagnostics.len());

        let actual_working_bytes = normalized_working_bytes(id.len(), &fields)
            .saturating_add(owned_field_bytes(normalized_index.len()))
            .saturating_add(kind_working_bytes(&kind, participation));
        debug_assert_eq!(planned_working_bytes, actual_working_bytes);
        let source_order = scene.source_owner_order("elements", index);
        push_pending_event(
            &mut pending_events,
            source_order,
            PendingNormalizationEventKind::Record {
                event: normalization_event,
                preflighted: true,
                element_id: Some(memory_at_source!(try_owned(&id))),
                local_owner: Some(memory_at_source!(try_owned(&id))),
                json_pointer: memory_at_source!(try_owned(&element_pointer)),
                working_bytes: planned_working_bytes,
                work_units: normalization_work_units,
            },
        )?;
        memory_at_source!(used_ids.try_reserve(1));
        used_ids.insert(memory_at_source!(try_owned(&id)));

        if width == 0.0 || height == 0.0 {
            for (_, diagnostic) in &mut element_diagnostics {
                memory_at_source!(diagnostic.omit_unbounded_known_element_fallback());
            }
        }
        element_diagnostics.sort_by(|(left_position, left), (right_position, right)| {
            left_position
                .cmp(right_position)
                .then_with(|| left.json_pointer().cmp(right.json_pointer()))
                .then_with(|| left.code().as_str().cmp(right.code().as_str()))
        });
        let repair_work_units = u64::try_from(element_diagnostics.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(8);
        for (position, diagnostic) in element_diagnostics {
            push_pending_event(
                &mut pending_events,
                source_order,
                PendingNormalizationEventKind::Diagnostic {
                    diagnostic,
                    local_owner: Some(memory_at_source!(try_owned(&id))),
                    source_position: position.as_u64(),
                },
            )?;
        }

        elements.push(NormalizedElement {
            source: NormalizedElementSource {
                source_index: index,
                id,
                pointer: element_pointer,
                coordinate_positions: [
                    source.member_position("x").unwrap_or(usize::MAX),
                    source.member_position("y").unwrap_or(usize::MAX),
                    source.member_position("width").unwrap_or(usize::MAX),
                    source.member_position("height").unwrap_or(usize::MAX),
                ],
            },
            source_order,
            index: normalized_index,
            x,
            y,
            width,
            height,
            angle: fields.angle,
            stroke_color: memory_at_source!(try_owned(fields.stroke_color)),
            background_color: memory_at_source!(try_owned(fields.background_color)),
            fill_style: memory_at_source!(try_owned(fields.fill_style)),
            stroke_width: fields.stroke_width,
            stroke_style: memory_at_source!(try_owned(fields.stroke_style)),
            roughness: fields.roughness,
            opacity: fields.opacity,
            link: match source.link {
                SourceField::Value(Some(link)) => {
                    memory_at_source!(crate::scene_surface::sanitized_link(link))
                }
                SourceField::Missing | SourceField::Invalid | SourceField::Value(None) => None,
            },
            frame_id: match source.frame_id {
                SourceField::Value(Some(frame_id)) if !frame_id.is_empty() => {
                    Some(memory_at_source!(try_owned(frame_id)))
                }
                SourceField::Missing
                | SourceField::Invalid
                | SourceField::Value(None | Some(_)) => None,
            },
            seed,
            roundness: normalized_roundness(&source, policy),
            normalized_working_bytes: planned_working_bytes,
            normalization_work_units,
            repair_work_units,
            kind_name: source.kind.scene_surface_kind(),
            kind,
            direct_outline_compatible: pending.direct_outline_compatible,
        });
    }

    let (frame_rendering, frame_diagnostics) = normalize_frame_rendering(
        scene.frame_rendering(),
        scene.app_state_member_position("frameRendering"),
        policy,
        format,
        surface_diagnostic_limit.saturating_sub(planned_diagnostic_count),
    )?;
    planned_diagnostic_count = planned_diagnostic_count.saturating_add(frame_diagnostics.len());
    let frame_source_order = scene.source_owner_order("appState", 0);
    push_pending_event(
        &mut pending_events,
        frame_source_order,
        PendingNormalizationEventKind::Record {
            event: "normalized_frame_rendering",
            preflighted: false,
            element_id: None,
            local_owner: None,
            json_pointer: try_owned("/appState/frameRendering")?,
            working_bytes: 4 * 64 + view_background_working_bytes,
            work_units: 5 * 8,
        },
    )?;
    for (position, diagnostic) in frame_diagnostics {
        push_pending_event(
            &mut pending_events,
            frame_source_order,
            PendingNormalizationEventKind::Diagnostic {
                diagnostic,
                local_owner: None,
                source_position: position,
            },
        )?;
    }
    let (files, file_diagnostics) = normalize_referenced_file_records(
        scene,
        &referenced_file_ids,
        format,
        surface_diagnostic_limit.saturating_sub(planned_diagnostic_count),
    )?;
    for file in &files {
        push_pending_event(
            &mut pending_events,
            file.source_order,
            PendingNormalizationEventKind::Record {
                event: "normalized_file_record",
                preflighted: false,
                element_id: None,
                local_owner: Some(try_owned(&file.key)?),
                json_pointer: try_owned(&file.source_pointer)?,
                working_bytes: file.normalized_working_bytes,
                work_units: file.normalization_work_units,
            },
        )?;
    }
    for (local_owner, source_order, source_position, diagnostic) in file_diagnostics {
        push_pending_event(
            &mut pending_events,
            source_order,
            PendingNormalizationEventKind::Diagnostic {
                diagnostic,
                local_owner: Some(local_owner),
                source_position,
            },
        )?;
    }

    replay_normalization_events(&mut pending_events, &mut observe)?;

    Ok(Some(NormalizationOutcome {
        elements,
        fallbacks: surface.fallbacks,
        view_background_color: try_owned(view_background_color)?,
        view_background_working_bytes,
        frame_rendering,
        files,
    }))
}

fn push_pending_event(
    events: &mut Vec<PendingNormalizationEvent>,
    source_order: u64,
    kind: PendingNormalizationEventKind,
) -> Result<(), TryReserveError> {
    let ordinal = u64::try_from(events.len()).unwrap_or(u64::MAX);
    events.try_reserve(1)?;
    events.push(PendingNormalizationEvent {
        source_order,
        ordinal,
        kind,
    });
    Ok(())
}

fn replay_normalization_events<E>(
    events: &mut Vec<PendingNormalizationEvent>,
    observe: &mut impl FnMut(NormalizationEvent<'_>) -> Result<(), E>,
) -> Result<(), NormalizationError<E>> {
    events.sort_unstable_by(|left, right| {
        left.source_order
            .cmp(&right.source_order)
            .then_with(|| match (&left.kind, &right.kind) {
                (
                    PendingNormalizationEventKind::Record { .. },
                    PendingNormalizationEventKind::Diagnostic { .. },
                ) => std::cmp::Ordering::Less,
                (
                    PendingNormalizationEventKind::Diagnostic { .. },
                    PendingNormalizationEventKind::Record { .. },
                ) => std::cmp::Ordering::Greater,
                (
                    PendingNormalizationEventKind::Diagnostic {
                        diagnostic: left_diagnostic,
                        source_position: left_position,
                        ..
                    },
                    PendingNormalizationEventKind::Diagnostic {
                        diagnostic: right_diagnostic,
                        source_position: right_position,
                        ..
                    },
                ) => left_position
                    .cmp(right_position)
                    .then_with(|| {
                        left_diagnostic
                            .json_pointer()
                            .cmp(right_diagnostic.json_pointer())
                    })
                    .then_with(|| {
                        left_diagnostic
                            .code()
                            .as_str()
                            .cmp(right_diagnostic.code().as_str())
                    })
                    .then_with(|| left.ordinal.cmp(&right.ordinal)),
                (
                    PendingNormalizationEventKind::Record { .. },
                    PendingNormalizationEventKind::Record { .. },
                ) => left.ordinal.cmp(&right.ordinal),
            })
    });
    for event in events.drain(..) {
        match event.kind {
            PendingNormalizationEventKind::Record {
                event,
                preflighted,
                element_id,
                local_owner,
                json_pointer,
                working_bytes,
                work_units,
            } => {
                if !preflighted {
                    observe(NormalizationEvent::PreflightRecord {
                        event,
                        json_pointer: &json_pointer,
                        working_bytes,
                        work_units,
                    })
                    .map_err(NormalizationError::Observer)?;
                }
                observe(NormalizationEvent::CommitRecord {
                    event,
                    element_id: element_id.as_deref(),
                    local_owner: local_owner.as_deref(),
                    json_pointer: &json_pointer,
                    working_bytes,
                    work_units,
                })
                .map_err(NormalizationError::Observer)?;
            }
            PendingNormalizationEventKind::Diagnostic {
                diagnostic,
                local_owner,
                source_position,
            } => observe(NormalizationEvent::Diagnostic {
                diagnostic,
                local_owner: local_owner.as_deref(),
                source_order: event.source_order,
                source_position,
            })
            .map_err(NormalizationError::Observer)?,
        }
    }
    Ok(())
}

fn manifested_value<T: Copy>(
    source: SourceField<T>,
    default: T,
    accepts: impl FnOnce(T) -> bool,
) -> T {
    match source {
        SourceField::Value(value) if accepts(value) => value,
        SourceField::Missing | SourceField::Invalid | SourceField::Value(_) => default,
    }
}

fn source_accepts<T>(source: SourceField<T>, accepts: impl FnOnce(T) -> bool) -> bool {
    match source {
        SourceField::Missing => true,
        SourceField::Value(value) => accepts(value),
        SourceField::Invalid => false,
    }
}

type OwnedNormalizationDiagnostic = (String, u64, u64, PendingDiagnostic);
type NormalizedFileRecords = (Vec<NormalizedFileRecord>, Vec<OwnedNormalizationDiagnostic>);

fn normalize_referenced_file_records(
    scene: &SceneDocument,
    referenced_file_ids: &[&str],
    format: OutputFormat,
    diagnostic_limit: usize,
) -> Result<NormalizedFileRecords, TryReserveError> {
    let mut files = Vec::new();
    let mut diagnostics = Vec::new();
    files.try_reserve_exact(scene.file_record_count().min(referenced_file_ids.len()))?;
    for source_index in 0..scene.file_record_count() {
        let Some(source) = scene.file_record_at(source_index) else {
            continue;
        };
        if referenced_file_ids.binary_search(&source.key()).is_err()
            || !source.is_record()
            || source.key().is_empty()
        {
            continue;
        }
        let source_pointer = pointer_child("/files", source.key())?;
        let id = match source.id() {
            SourceField::Value(id) if !id.is_empty() => try_owned(id)?,
            SourceField::Missing | SourceField::Invalid | SourceField::Value(_) => {
                if diagnostics.len() < diagnostic_limit {
                    let pointer = pointer_child(&source_pointer, "id")?;
                    diagnostics.try_reserve(1)?;
                    diagnostics.push((
                        try_owned(source.key())?,
                        scene.source_owner_order("files", source_index),
                        u64::try_from(source.member_position("id").unwrap_or(0))
                            .unwrap_or(u64::MAX),
                        PendingDiagnostic::new(
                            DiagnosticKind::UnsupportedPropertyValue {
                                recovery: Recovery::Repaired,
                                recipe: None,
                            },
                            format,
                            &[DiagnosticEffect::InputRepair],
                            None,
                            &pointer,
                            "missing or invalid file identity was repaired to its map key",
                        )?,
                    ));
                }
                try_owned(source.key())?
            }
        };
        let created = match source.created() {
            SourceField::Value(created) => Some(created),
            SourceField::Invalid => None,
            SourceField::Missing => {
                if diagnostics.len() < diagnostic_limit {
                    let pointer = pointer_child(&source_pointer, "created")?;
                    diagnostics.try_reserve(1)?;
                    diagnostics.push((
                        try_owned(source.key())?,
                        scene.source_owner_order("files", source_index),
                        u64::try_from(source.member_position("created").unwrap_or(usize::MAX))
                            .unwrap_or(u64::MAX),
                        PendingDiagnostic::new(
                            DiagnosticKind::MissingTimestamp,
                            format,
                            &[DiagnosticEffect::InputRepair],
                            None,
                            &pointer,
                            "missing required file timestamp was repaired to canonical zero",
                        )?,
                    ));
                }
                Some(0)
            }
        };
        let normalized_working_bytes = 256_u64
            .saturating_add(owned_field_bytes(source.key().len()))
            .saturating_add(owned_field_bytes(id.len()))
            .saturating_add(u64::from(created.is_some()) * 64);
        let normalization_work_units = (1 + u64::from(created.is_some())) * 8;
        files.push(NormalizedFileRecord {
            key: try_owned(source.key())?,
            id,
            created,
            source_pointer,
            source_order: scene.source_owner_order("files", source_index),
            normalized_working_bytes,
            normalization_work_units,
        });
    }
    Ok((files, diagnostics))
}

fn normalize_frame_rendering(
    source: SourceField<FrameRenderingSource<'_>>,
    app_state_position: Option<usize>,
    policy: &crate::scene_surface::SceneSurfacePolicy,
    format: OutputFormat,
    diagnostic_limit: usize,
) -> Result<(NormalizedFrameRendering, Vec<(u64, PendingDiagnostic)>), TryReserveError> {
    let mut diagnostics = Vec::new();
    let SourceField::Value(source) = source else {
        return Ok((
            NormalizedFrameRendering {
                enabled: true,
                clip: true,
                name: true,
                outline: true,
            },
            diagnostics,
        ));
    };
    let mut value = |key: &str| -> Result<bool, TryReserveError> {
        match source.boolean(key) {
            SourceField::Missing => Ok(true),
            SourceField::Value(value) => Ok(value),
            SourceField::Invalid => {
                if diagnostics.len() >= diagnostic_limit {
                    return Ok(true);
                }
                let pointer = pointer_child("/appState/frameRendering", key)?;
                diagnostics.try_reserve(1)?;
                diagnostics.push((
                    SourceDiagnosticPosition::nested(
                        app_state_position.unwrap_or(usize::MAX),
                        source.member_position(key).unwrap_or(usize::MAX),
                    )
                    .as_u64(),
                    PendingDiagnostic::new(
                        DiagnosticKind::UnsupportedPropertyValue {
                            recovery: Recovery::Substituted,
                            recipe: None,
                        },
                        format,
                        &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
                        None,
                        &pointer,
                        "unsupported frame rendering control used the revision-1 default",
                    )?,
                ));
                Ok(true)
            }
        }
    };
    let normalized = NormalizedFrameRendering {
        enabled: value("enabled")?,
        clip: value("clip")?,
        name: value("name")?,
        outline: value("outline")?,
    };
    for (position, member) in source.members().iter().enumerate() {
        if diagnostics.len() >= diagnostic_limit {
            break;
        }
        let semantic_path = pointer_child("", &member.key)?;
        if crate::scene_surface::is_manifested_path(policy, "frame_rendering", &semantic_path) {
            continue;
        }
        diagnostics.try_reserve(1)?;
        diagnostics.push((
            SourceDiagnosticPosition::nested(app_state_position.unwrap_or(usize::MAX), position)
                .as_u64(),
            crate::scene_surface::unrecognized_property_diagnostic(
                format,
                None,
                &pointer_child("/appState/frameRendering", &member.key)?,
                false,
                "unrecognized frame-rendering property was ignored",
            )?,
        ));
    }
    diagnostics.sort_by(|(left_position, left), (right_position, right)| {
        left_position
            .cmp(right_position)
            .then_with(|| left.json_pointer().cmp(right.json_pointer()))
    });
    Ok((normalized, diagnostics))
}

fn generated_element_id(
    revision: CompatibilityRevision,
    source_index: u64,
    canonical_element: &[u8],
) -> Result<String, TryReserveError> {
    let mut hasher = Sha256::new();
    hasher.update(MISSING_ID_DOMAIN);
    hasher.update(revision.get().to_le_bytes());
    hasher.update(source_index.to_le_bytes());
    hasher.update(
        u64::try_from(canonical_element.len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    hasher.update(canonical_element);
    generated_id_from_digest(&hasher.finalize())
}

fn generated_rough_seed(revision: CompatibilityRevision, normalized_id: &str) -> i32 {
    let mut hasher = Sha256::new();
    hasher.update(ROUGH_SEED_DOMAIN);
    hasher.update(revision.get().to_le_bytes());
    hasher.update(
        u64::try_from(normalized_id.len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    hasher.update(normalized_id.as_bytes());
    let digest = hasher.finalize();
    let seed = i32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) & i32::MAX;
    if seed == 0 {
        1
    } else {
        seed
    }
}

fn generated_duplicate_element_id(
    revision: CompatibilityRevision,
    source_index: u64,
    original_id: &str,
    canonical_element: &[u8],
) -> Result<String, TryReserveError> {
    let mut hasher = Sha256::new();
    hasher.update(DUPLICATE_ID_DOMAIN);
    hasher.update(revision.get().to_le_bytes());
    hasher.update(source_index.to_le_bytes());
    hasher.update(
        u64::try_from(original_id.len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    hasher.update(original_id.as_bytes());
    hasher.update(
        u64::try_from(canonical_element.len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    hasher.update(canonical_element);
    generated_id_from_digest(&hasher.finalize())
}

fn generated_id_from_digest(digest: &[u8]) -> Result<String, TryReserveError> {
    let mut id = String::new();
    id.try_reserve_exact("generated-".len() + digest.len() * 2)?;
    id.push_str("generated-");
    push_lower_hex(&mut id, digest);
    Ok(id)
}

fn unique_generated_id(
    mut candidate: String,
    revision: CompatibilityRevision,
    source_index: u64,
    used_ids: &HashSet<String>,
) -> Result<String, TryReserveError> {
    let base = try_owned(&candidate)?;
    let mut collision_ordinal = 0_u64;
    while used_ids.contains(&candidate) {
        collision_ordinal = collision_ordinal.saturating_add(1);
        let mut hasher = Sha256::new();
        hasher.update(ID_COLLISION_DOMAIN);
        hasher.update(revision.get().to_le_bytes());
        hasher.update(source_index.to_le_bytes());
        hasher.update(collision_ordinal.to_le_bytes());
        hasher.update(u64::try_from(base.len()).unwrap_or(u64::MAX).to_le_bytes());
        hasher.update(base.as_bytes());
        candidate = generated_id_from_digest(&hasher.finalize())?;
    }
    Ok(candidate)
}

fn defaulted<T: Copy>(field: SourceField<T>, default: T) -> Option<T> {
    match field {
        SourceField::Missing => Some(default),
        SourceField::Value(value) => Some(value),
        SourceField::Invalid => None,
    }
}

fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 {
        0.0
    } else {
        value
    }
}

/// The effects a manifested field fallback declares, skipping any spelling
/// the registry does not know.
fn manifested_effects(effects: &[&str]) -> Result<Vec<DiagnosticEffect>, TryReserveError> {
    let mut manifested = Vec::new();
    manifested.try_reserve_exact(effects.len())?;
    for effect in effects {
        if let Some(effect) = DiagnosticEffect::from_manifest(effect) {
            manifested.push(effect);
        }
    }
    Ok(manifested)
}

fn try_owned(value: &str) -> Result<String, TryReserveError> {
    let mut owned = String::new();
    owned.try_reserve_exact(value.len())?;
    owned.push_str(value);
    Ok(owned)
}

fn push_lower_hex(output: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
}
