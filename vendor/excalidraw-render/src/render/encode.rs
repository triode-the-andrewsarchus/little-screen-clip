//! Per-format encoding, split out of `RenderSession::encode`.
//!
//! `encode` owns the format-independent envelope: dimension validation, the
//! planning-phase working-byte reservations, and taking the staged output. Each
//! method here owns one format's plan construction and the semantic accounting
//! that plan implies. They stay `RenderSession` methods because charging the
//! resource meter is the session's job, not the plan's. The PNG and PDF
//! methods exist only in builds with that format's feature.

#[cfg(feature = "png")]
use crate::resource_accounting::ResourceDelta;
use crate::resource_accounting::{ResourceContext, ResourcePhase, ResourceRecordKind};
use crate::{ErrorCode, RenderError, RenderRequest, SceneDocument};

use super::font_demand::{
    build_svg_font_bundle, release_svg_font_encoding_records, svg_font_demand,
    svg_font_demand_estimates, svg_font_text_order,
};
#[cfg(feature = "pdf")]
use super::pdf::PdfPlan;
#[cfg(feature = "png")]
use super::png::PngPlan;
use super::resolve::{ResolvedScene, SemanticWorkingRecord};
use super::svg::SvgPlan;
use super::{svg_fonts, RenderResourceAccount, RenderSession};

impl RenderSession {
    /// Plans and stages one SVG artifact, including its font-subset demand.
    pub(super) fn encode_svg(
        &mut self,
        resolved: ResolvedScene,
        scene: &SceneDocument,
        request: &RenderRequest,
        semantic_working: &[SemanticWorkingRecord],
        svg_font_payload: svg_fonts::Payload,
    ) -> Result<(), RenderError> {
        let text_order = svg_font_text_order(&resolved)
            .map_err(|error| self.format_failure(request, ErrorCode::ENCODE_SVG_FAILED, error))?;
        let demand_estimates = svg_font_demand_estimates(&resolved, &text_order)
            .map_err(|error| self.format_failure(request, ErrorCode::ENCODE_SVG_FAILED, error))?;
        let selection_working = svg_fonts::account_demand(
            &mut RenderResourceAccount::new(self, request),
            demand_estimates.into_iter(),
        )?;
        let demand = match svg_font_demand(&resolved, &text_order) {
            Ok(demand) => demand,
            Err(error) => {
                selection_working.release(&mut RenderResourceAccount::new(self, request))?;
                return Err(self.format_failure(request, ErrorCode::ENCODE_SVG_FAILED, error));
            }
        };
        let selection_working = svg_fonts::account_selection(
            &mut RenderResourceAccount::new(self, request),
            &demand,
            selection_working,
        )?;
        let dependency_envelope = match svg_fonts::account_dependency_envelope(
            &mut RenderResourceAccount::new(self, request),
            &demand,
        ) {
            Ok(envelope) => envelope,
            Err(error) => {
                selection_working.release(&mut RenderResourceAccount::new(self, request))?;
                return Err(error);
            }
        };
        let font_bundle = {
            let mut subset_session = self
                .font_subset_session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            build_svg_font_bundle(demand, svg_font_payload, &mut subset_session)
        };
        let font_bundle = match font_bundle {
            Ok(bundle) => bundle,
            Err(error) => {
                release_svg_font_encoding_records(self, request, None, dependency_envelope)?;
                selection_working.release(&mut RenderResourceAccount::new(self, request))?;
                return Err(self.format_failure(request, ErrorCode::ENCODE_SVG_FAILED, error));
            }
        };
        match font_bundle
            .account_resource_transcript(&mut RenderResourceAccount::new(self, request))
        {
            Ok(()) => {}
            Err(error) => {
                release_svg_font_encoding_records(self, request, None, dependency_envelope)?;
                selection_working.release(&mut RenderResourceAccount::new(self, request))?;
                return Err(error);
            }
        }
        let mut plan = match SvgPlan::new(resolved, request, font_bundle) {
            Ok(plan) => plan,
            Err(error) => {
                release_svg_font_encoding_records(self, request, None, dependency_envelope)?;
                selection_working.release(&mut RenderResourceAccount::new(self, request))?;
                return Err(self.format_failure(request, ErrorCode::ENCODE_SVG_FAILED, error));
            }
        };
        if let Err(error) = self.release_semantic_working(request, semantic_working, false) {
            release_svg_font_encoding_records(self, request, None, dependency_envelope)?;
            selection_working.release(&mut RenderResourceAccount::new(self, request))?;
            return Err(error);
        }
        let workspace_bytes = 256 * 1_024;
        let workspace = ResourceContext::new(
            ResourcePhase::EncodingAndOutcomeAssembly,
            "svg_serializer_workspace",
            "",
        )
        .with_record(ResourceRecordKind::SvgSerializerWorkspace);
        if let Err(error) = self.acquire_working(request, workspace, workspace_bytes) {
            release_svg_font_encoding_records(self, request, None, dependency_envelope)?;
            selection_working.release(&mut RenderResourceAccount::new(self, request))?;
            return Err(error);
        }
        let mandatory_notices = plan.take_mandatory_notices();
        let (metadata, measured) = match self.select_artifact_metadata(
            scene,
            request,
            mandatory_notices.as_ref(),
            ErrorCode::ENCODE_SVG_FAILED,
            0,
            |metadata| {
                plan.set_metadata(metadata.try_clone()?);
                plan.measure()
            },
        ) {
            Ok(measured) => measured,
            Err(error) => {
                release_svg_font_encoding_records(
                    self,
                    request,
                    Some((workspace, workspace_bytes)),
                    dependency_envelope,
                )?;
                selection_working.release(&mut RenderResourceAccount::new(self, request))?;
                return Err(error);
            }
        };
        if let Err(error) =
            selection_working.release(&mut RenderResourceAccount::new(self, request))
        {
            release_svg_font_encoding_records(
                self,
                request,
                Some((workspace, workspace_bytes)),
                dependency_envelope,
            )?;
            return Err(error);
        }
        plan.set_metadata(metadata);
        let output_bytes = u64::try_from(measured).unwrap_or(u64::MAX);
        if let Err(error) = self.admit_output(request, output_bytes, 0) {
            release_svg_font_encoding_records(
                self,
                request,
                Some((workspace, workspace_bytes)),
                dependency_envelope,
            )?;
            return Err(error);
        }
        let emitted = match plan.encode_exact(measured) {
            Ok(emitted) => emitted,
            Err(error) => {
                release_svg_font_encoding_records(
                    self,
                    request,
                    Some((workspace, workspace_bytes)),
                    dependency_envelope,
                )?;
                return Err(self.format_failure(request, ErrorCode::ENCODE_SVG_FAILED, error));
            }
        };
        self.staged_output = Some(emitted);
        if let Err(error) = self.release_semantic_working(request, semantic_working, true) {
            release_svg_font_encoding_records(
                self,
                request,
                Some((workspace, workspace_bytes)),
                dependency_envelope,
            )?;
            return Err(error);
        }
        release_svg_font_encoding_records(
            self,
            request,
            Some((workspace, workspace_bytes)),
            dependency_envelope,
        )?;
        for index in 0..plan.source_count() {
            let (element_id, json_pointer) = plan.source(index).ok_or_else(|| {
                self.internal_failure(
                    request.format(),
                    "SVG source operation is missing",
                    "/elements",
                )
            })?;
            self.release_working(
                request,
                ResourceContext::new(
                    ResourcePhase::EncodingAndOutcomeAssembly,
                    "element_operation",
                    json_pointer,
                )
                .with_element_id(element_id)
                .with_local_owner(element_id)
                .with_record(ResourceRecordKind::FormatPlanOperation),
                128,
            )?;
        }
        Ok(())
    }

    /// Plans and stages one PNG artifact, including its raster surface budget.
    #[cfg(feature = "png")]
    pub(super) fn encode_png(
        &mut self,
        resolved: ResolvedScene,
        scene: &SceneDocument,
        request: &RenderRequest,
        semantic_working: &[SemanticWorkingRecord],
    ) -> Result<(), RenderError> {
        let plan = PngPlan::new(resolved, request, &self.svg_trees)
            .map_err(|error| self.format_failure(request, ErrorCode::ENCODE_PNG_FAILED, error))?;
        self.release_semantic_working(request, semantic_working, false)?;
        let surface_limit = request.resource_limits().max_raster_surface_pixels();
        let pixels = match plan.surface_pixels() {
            Some(pixels) if pixels <= surface_limit => pixels,
            attempted => {
                return Err(self.required_raster_surface_failure(request, surface_limit, attempted))
            }
        };
        let rgba_bytes = pixels.saturating_mul(4);
        self.transact(
            request,
            ResourceContext::new(
                ResourcePhase::ProjectionAndFallback,
                "main_raster_surface",
                "",
            ),
            ResourceDelta {
                rasterized_pixels: pixels,
                ..ResourceDelta::default()
            },
        )?;
        let surface_bytes = 256_u64.saturating_add(rgba_bytes);
        let surface = ResourceContext::new(
            ResourcePhase::ProjectionAndFallback,
            "main_raster_surface",
            "",
        )
        .with_record(ResourceRecordKind::RasterColorSurface);
        self.acquire_working(request, surface, surface_bytes)?;
        // Every element a frame clips paints through one alpha mask of that
        // frame, so the mask is generated and recorded once per frame and never
        // owned by any one child (#112).
        let frame_clip_masks = plan
            .frame_clip_masks()
            .map_err(|error| self.format_failure(request, ErrorCode::ENCODE_PNG_FAILED, error))?;
        for mask in &frame_clip_masks {
            let (frame_id, frame_pointer) = frame_clip_mask_source(&plan, mask);
            self.transact(
                request,
                ResourceContext::new(
                    ResourcePhase::ProjectionAndFallback,
                    "frame_clip_mask_samples",
                    frame_pointer,
                )
                .with_element_id(frame_id),
                ResourceDelta {
                    rasterized_pixels: mask.pixels,
                    total_work_units: mask.pixels.saturating_mul(8),
                    ..ResourceDelta::default()
                },
            )?;
            self.acquire_working(
                request,
                frame_clip_mask_surface(frame_id, frame_pointer),
                frame_clip_mask_surface_bytes(mask),
            )?;
        }
        let paint_mask_bytes = plan
            .maximum_paint_mask_surface_area()
            .saturating_add(u64::from(plan.has_paint_masks()) * 256);
        let paint_mask_surface = ResourceContext::new(
            ResourcePhase::ProjectionAndFallback,
            "paint_mask_surface",
            "",
        )
        .with_record(ResourceRecordKind::RasterMaskSurface);
        if paint_mask_bytes != 0 {
            self.acquire_working(request, paint_mask_surface, paint_mask_bytes)?;
        }

        let raster_work = if plan.includes_background() {
            pixels.saturating_mul(4)
        } else {
            pixels
        };
        let raster_event = if plan.includes_background() {
            "solid_background_samples"
        } else {
            "clear_samples"
        };
        self.charge_work(
            request,
            ResourceContext::new(ResourcePhase::ProjectionAndFallback, raster_event, ""),
            raster_work,
        )?;
        for index in 0..plan.source_count() {
            let (element_id, json_pointer) = plan.source(index).ok_or_else(|| {
                self.internal_failure(
                    request.format(),
                    "PNG source operation is missing",
                    "/elements",
                )
            })?;
            // An element's own mask is bounded by the element and clamped to the
            // canvas, so it never exceeds the admitted main raster surface.
            let paint_mask_pixels = plan.paint_mask_surface_area(index).ok_or_else(|| {
                self.internal_failure(
                    request.format(),
                    "PNG paint mask source is missing",
                    "/elements",
                )
            })?;
            if let Some(fallback_pixels) = plan.fallback_surface_area(index).ok_or_else(|| {
                self.internal_failure(
                    request.format(),
                    "PNG fallback surface source is missing",
                    "/elements",
                )
            })? {
                // The placeholder's surface is its element rectangle's
                // coverage clamped to the canvas, so it is at most the main
                // raster surface the `max_raster_surface_pixels` ceiling
                // admitted above; it has no local ceiling of its own to cross
                // and is charged against the aggregate alone (#159).
                let raster_delta = ResourceDelta {
                    rasterized_pixels: fallback_pixels,
                    ..ResourceDelta::default()
                };
                let raster_context = ResourceContext::new(
                    ResourcePhase::ProjectionAndFallback,
                    "fallback_raster_surface",
                    json_pointer,
                )
                .with_element_id(element_id)
                .with_local_owner(element_id);
                if self.meter.would_cross_fatal(raster_delta) {
                    self.transact(request, raster_context, raster_delta)?;
                    return Err(self.internal_failure(
                        request.format(),
                        "fatal fallback raster preflight committed unexpectedly",
                        "/elements",
                    ));
                }
                self.transact(request, raster_context, raster_delta)?;
            }
            let image_projection = plan.image_projection_area(index).ok_or_else(|| {
                self.internal_failure(
                    request.format(),
                    "PNG image projection source is missing",
                    "/elements",
                )
            })?;
            let (projection_work, composite_work, projection_event) =
                if let Some(area) = image_projection {
                    (
                        area.saturating_mul(12),
                        area.saturating_mul(4),
                        "image_bilinear_projection_samples",
                    )
                } else {
                    (
                        plan.vector_coverage_area(index)
                            .ok_or_else(|| {
                                self.internal_failure(
                                    request.format(),
                                    "PNG vector coverage source is missing",
                                    "/elements",
                                )
                            })?
                            .saturating_mul(16),
                        0,
                        "vector_coverage_samples",
                    )
                };
            let mask_work = paint_mask_pixels.unwrap_or(0).saturating_mul(8);
            let raster_work = projection_work
                .saturating_add(mask_work)
                .saturating_add(composite_work);
            let context = ResourceContext::new(
                ResourcePhase::ProjectionAndFallback,
                projection_event,
                json_pointer,
            )
            .with_element_id(element_id)
            .with_local_owner(element_id);
            let combined_delta = ResourceDelta {
                rasterized_pixels: paint_mask_pixels.unwrap_or(0),
                total_work_units: raster_work,
                ..ResourceDelta::default()
            };
            if self.meter.would_cross_fatal(combined_delta) {
                self.transact(request, context, combined_delta)?;
                return Err(self.internal_failure(
                    request.format(),
                    "fatal PNG element raster preflight committed unexpectedly",
                    "/elements",
                ));
            }
            // Raster samples are checked against the aggregate only: `P` is
            // clamped to the surface `max_raster_surface_pixels` admitted, so a
            // per-element ceiling would omit elements that canvas admits.
            if let Some(mask_pixels) = paint_mask_pixels {
                self.transact(
                    request,
                    ResourceContext::new(
                        ResourcePhase::ProjectionAndFallback,
                        "paint_mask_samples",
                        json_pointer,
                    )
                    .with_element_id(element_id)
                    .with_local_owner(element_id)
                    .with_record(ResourceRecordKind::RasterMaskSurface),
                    ResourceDelta {
                        rasterized_pixels: mask_pixels,
                        total_work_units: mask_work,
                        ..ResourceDelta::default()
                    },
                )?;
            }
            self.charge_work(request, context, projection_work)?;
            if composite_work != 0 {
                self.charge_work(
                    request,
                    ResourceContext::new(
                        ResourcePhase::ProjectionAndFallback,
                        "image_composite_samples",
                        json_pointer,
                    )
                    .with_element_id(element_id)
                    .with_local_owner(element_id),
                    composite_work,
                )?;
            }
            // A nested SVG's filter effects cost resvg their own surfaces and
            // per-primitive work at the layer's device size, which the
            // projection above does not price; measure and charge them before
            // anything is rasterized.
            let filter_work = plan
                .nested_svg_filter_work(index, request.resource_limits().max_total_work_units())
                .map_err(|error| {
                    self.format_failure(request, ErrorCode::ENCODE_PNG_FAILED, error)
                })?;
            if !filter_work.is_empty() {
                self.transact(
                    request,
                    ResourceContext::new(
                        ResourcePhase::ProjectionAndFallback,
                        "nested_svg_filter_samples",
                        json_pointer,
                    )
                    .with_element_id(element_id)
                    .with_local_owner(element_id),
                    ResourceDelta {
                        rasterized_pixels: filter_work.surface_pixels,
                        total_work_units: filter_work.work_units,
                        ..ResourceDelta::default()
                    },
                )?;
            }
        }

        self.flush_pending_png_animation_diagnostics(request)?;

        // The canvas samples a strongly minified image from a smaller cc
        // decode level, which the render builds once per image, source
        // rectangle and level and keeps until the raster is done (#198). Each
        // build pays its level's surface area, its canonical samples (up to
        // nine taps per pixel of Skia's transient mipmap chain, four per level
        // pixel), and the working bytes of every kept level plus the largest
        // transient chain, for the rasterization's lifetime.
        let image_level_builds = plan
            .image_level_builds()
            .map_err(|error| self.format_failure(request, ErrorCode::ENCODE_PNG_FAILED, error))?;
        let mut image_level_bytes = 0_u64;
        let mut image_level_chain_bytes = 0_u64;
        for build in &image_level_builds {
            let (element_id, json_pointer) = plan.source(build.index).ok_or_else(|| {
                self.internal_failure(
                    request.format(),
                    "PNG image level source is missing",
                    "/elements",
                )
            })?;
            self.transact(
                request,
                ResourceContext::new(
                    ResourcePhase::ProjectionAndFallback,
                    "image_decode_level_samples",
                    json_pointer,
                )
                .with_element_id(element_id)
                .with_local_owner(element_id),
                ResourceDelta {
                    rasterized_pixels: build.level_pixels,
                    total_work_units: build
                        .chain_pixels
                        .saturating_mul(9)
                        .saturating_add(build.level_pixels.saturating_mul(4)),
                    ..ResourceDelta::default()
                },
            )?;
            image_level_bytes = image_level_bytes
                .saturating_add(256)
                .saturating_add(build.level_pixels.saturating_mul(4));
            image_level_chain_bytes = image_level_chain_bytes
                .max(256_u64.saturating_add(build.chain_pixels.saturating_mul(4)));
        }
        let image_level_working = if image_level_builds.is_empty() {
            0
        } else {
            image_level_bytes.saturating_add(image_level_chain_bytes)
        };
        let image_level_surface = ResourceContext::new(
            ResourcePhase::ProjectionAndFallback,
            "image_decode_level_surface",
            "",
        )
        .with_record(ResourceRecordKind::RasterColorSurface);
        if image_level_working != 0 {
            self.acquire_working(request, image_level_surface, image_level_working)?;
        }

        let workspace_bytes = 2_u64
            .saturating_mul(1_048_576)
            .saturating_add(16_u64.saturating_mul(u64::from(plan.width())));
        let workspace = ResourceContext::new(
            ResourcePhase::EncodingAndOutcomeAssembly,
            "png_encoder_workspace",
            "",
        )
        .with_record(ResourceRecordKind::PngEncoderWorkspace);
        self.acquire_working(request, workspace, workspace_bytes)?;
        self.charge_work(
            request,
            ResourceContext::new(
                ResourcePhase::EncodingAndOutcomeAssembly,
                "codec_measurement_input",
                "",
            ),
            rgba_bytes,
        )?;
        let mandatory_notices = plan
            .mandatory_notices()
            .map_err(|error| self.format_failure(request, ErrorCode::ENCODE_PNG_FAILED, error))?;
        // The IDAT stream is a pure function of the raster, so the plan is
        // rasterized and deflated once here and every metadata candidate and
        // the emission pass assemble their chunks from the same stream (#117).
        // Measurement and emission stay two semantic passes (spec §13.2) and
        // the emitted bytes are still compared with the measured ones. The
        // stream itself is a physical transient the ledger does not price
        // separately; its lifetime is the one the §13.2 PNG encoder workspace
        // row names (ADR-0004): it lives until the accepted candidate is
        // emitted and is dropped before the asset records are released.
        let stream = plan
            .compress()
            .map_err(|error| self.format_failure(request, ErrorCode::ENCODE_PNG_FAILED, error))?;
        let (metadata, measured) = self.select_artifact_metadata(
            scene,
            request,
            mandatory_notices.as_ref(),
            ErrorCode::ENCODE_PNG_FAILED,
            rgba_bytes,
            |metadata| stream.assemble(Some(metadata)),
        )?;
        self.emit_premeasured_output(
            request,
            ErrorCode::ENCODE_PNG_FAILED,
            rgba_bytes,
            measured,
            || stream.assemble(Some(&metadata)),
        )?;
        drop(stream);
        self.release_semantic_working(request, semantic_working, true)?;
        if image_level_working != 0 {
            self.release_working(request, image_level_surface, image_level_working)?;
        }
        if paint_mask_bytes != 0 {
            self.release_working(request, paint_mask_surface, paint_mask_bytes)?;
        }
        for mask in frame_clip_masks.iter().rev() {
            let (frame_id, frame_pointer) = frame_clip_mask_source(&plan, mask);
            self.release_working(
                request,
                frame_clip_mask_surface(frame_id, frame_pointer),
                frame_clip_mask_surface_bytes(mask),
            )?;
        }
        self.release_working(request, workspace, workspace_bytes)?;
        self.release_working(request, surface, surface_bytes)?;
        for index in 0..plan.source_count() {
            let (element_id, json_pointer) = plan.source(index).ok_or_else(|| {
                self.internal_failure(
                    request.format(),
                    "PNG source operation is missing",
                    "/elements",
                )
            })?;
            self.release_working(
                request,
                ResourceContext::new(
                    ResourcePhase::EncodingAndOutcomeAssembly,
                    "element_operation",
                    json_pointer,
                )
                .with_element_id(element_id)
                .with_local_owner(element_id)
                .with_record(ResourceRecordKind::FormatPlanOperation),
                128,
            )?;
        }
        Ok(())
    }

    /// Plans and stages one PDF artifact, including any raster fallbacks.
    #[cfg(feature = "pdf")]
    pub(super) fn encode_pdf(
        &mut self,
        resolved: ResolvedScene,
        scene: &SceneDocument,
        request: &RenderRequest,
        semantic_working: &[SemanticWorkingRecord],
    ) -> Result<(), RenderError> {
        let workspace_bytes = 1_048_576;
        let mut plan = PdfPlan::new(resolved, request, &self.svg_trees)
            .map_err(|error| self.format_failure(request, ErrorCode::ENCODE_PDF_FAILED, error))?;
        let fallback_surface_working = self.preflight_pdf_raster_fallbacks(request, &plan)?;
        self.release_semantic_working(request, semantic_working, false)?;
        let image_resource_pixels = plan
            .image_resource_pixel_counts()
            .map_err(|error| self.format_failure(request, ErrorCode::ENCODE_PDF_FAILED, error))?;
        let mut image_resource_working = Vec::new();
        image_resource_working
            .try_reserve_exact(image_resource_pixels.len())
            .map_err(|_| self.memory_failure(request, "format_planning", "pdf_image_resources"))?;
        for pixels in image_resource_pixels {
            let rgb_bytes = 256_u64.saturating_add(pixels.saturating_mul(3));
            let alpha_bytes = 256_u64.saturating_add(pixels);
            self.acquire_working(
                request,
                ResourceContext::new(
                    ResourcePhase::FormatPlanning,
                    "pdf_image_rgb_format_resource",
                    "",
                )
                .with_record(ResourceRecordKind::PdfImageRgbFormatResource),
                rgb_bytes,
            )?;
            self.acquire_working(
                request,
                ResourceContext::new(
                    ResourcePhase::FormatPlanning,
                    "pdf_image_alpha_format_resource",
                    "",
                )
                .with_record(ResourceRecordKind::PdfImageAlphaFormatResource),
                alpha_bytes,
            )?;
            image_resource_working.push((rgb_bytes, alpha_bytes));
        }
        // Krilla reads and compares the whole cluster text once per glyph of a
        // cluster shaped to several glyphs; charge that before it runs.
        for index in 0..plan.source_count() {
            let repeated = plan.repeated_cluster_text(index);
            if repeated == 0 {
                continue;
            }
            let (element_id, json_pointer) = plan.source(index).ok_or_else(|| {
                self.internal_failure(
                    request.format(),
                    "PDF source operation is missing",
                    "/elements",
                )
            })?;
            self.charge_work(
                request,
                ResourceContext::new(
                    ResourcePhase::FormatPlanning,
                    "pdf_repeated_cluster_text",
                    json_pointer,
                )
                .with_element_id(element_id)
                .with_local_owner(element_id),
                repeated,
            )?;
        }
        let workspace = ResourceContext::new(
            ResourcePhase::EncodingAndOutcomeAssembly,
            "pdf_stream_workspace",
            "",
        )
        .with_record(ResourceRecordKind::PdfStreamWorkspace);
        self.acquire_working(request, workspace, workspace_bytes)?;
        let mandatory_notices = plan.take_mandatory_notices();
        let (metadata, measured) = self.select_artifact_metadata(
            scene,
            request,
            mandatory_notices.as_ref(),
            ErrorCode::ENCODE_PDF_FAILED,
            0,
            |metadata| {
                plan.set_metadata(metadata.try_clone()?);
                plan.encode()
            },
        )?;
        plan.set_metadata(metadata);
        self.emit_premeasured_output(request, ErrorCode::ENCODE_PDF_FAILED, 0, measured, || {
            plan.encode()
        })?;
        self.release_semantic_working(request, semantic_working, true)?;
        for (rgb_bytes, alpha_bytes) in image_resource_working {
            self.release_working(
                request,
                ResourceContext::new(
                    ResourcePhase::EncodingAndOutcomeAssembly,
                    "pdf_image_rgb_format_resource",
                    "",
                )
                .with_record(ResourceRecordKind::PdfImageRgbFormatResource),
                rgb_bytes,
            )?;
            self.release_working(
                request,
                ResourceContext::new(
                    ResourcePhase::EncodingAndOutcomeAssembly,
                    "pdf_image_alpha_format_resource",
                    "",
                )
                .with_record(ResourceRecordKind::PdfImageAlphaFormatResource),
                alpha_bytes,
            )?;
        }
        for (closure, bytes) in plan.raster_closures().iter().zip(fallback_surface_working) {
            self.release_working(
                request,
                ResourceContext::new(
                    ResourcePhase::EncodingAndOutcomeAssembly,
                    "pdf_raster_fallback_surface",
                    &closure.json_pointer,
                )
                .with_element_id(Some(&closure.element_id))
                .with_local_owner(Some(&closure.element_id))
                .with_record(ResourceRecordKind::RasterColorSurface),
                bytes,
            )?;
        }
        self.release_working(request, workspace, workspace_bytes)?;
        for index in 0..plan.source_count() {
            let (element_id, json_pointer) = plan.source(index).ok_or_else(|| {
                self.internal_failure(
                    request.format(),
                    "PDF source operation is missing",
                    "/elements",
                )
            })?;
            self.release_working(
                request,
                ResourceContext::new(
                    ResourcePhase::EncodingAndOutcomeAssembly,
                    "element_operation",
                    json_pointer,
                )
                .with_element_id(element_id)
                .with_local_owner(element_id)
                .with_record(ResourceRecordKind::FormatPlanOperation),
                128,
            )?;
        }
        Ok(())
    }
}

/// The clipping frame's identity for a shared frame clip mask's ledger events:
/// its element id and JSON pointer, or the scene itself when the frame emits no
/// element of its own.
#[cfg(feature = "png")]
fn frame_clip_mask_source<'a>(
    plan: &'a PngPlan,
    mask: &super::png::PngFrameClipMask,
) -> (Option<&'a str>, &'a str) {
    mask.frame_element_index
        .and_then(|index| plan.source(index))
        .unwrap_or((None, ""))
}

#[cfg(feature = "png")]
fn frame_clip_mask_surface<'a>(
    frame_id: Option<&'a str>,
    frame_pointer: &'a str,
) -> ResourceContext<'a> {
    ResourceContext::new(
        ResourcePhase::ProjectionAndFallback,
        "frame_clip_mask_surface",
        frame_pointer,
    )
    .with_element_id(frame_id)
    .with_record(ResourceRecordKind::RasterMaskSurface)
}

/// The canonical raster mask surface record: `256 + pixels`.
#[cfg(feature = "png")]
fn frame_clip_mask_surface_bytes(mask: &super::png::PngFrameClipMask) -> u64 {
    256_u64.saturating_add(mask.pixels)
}
