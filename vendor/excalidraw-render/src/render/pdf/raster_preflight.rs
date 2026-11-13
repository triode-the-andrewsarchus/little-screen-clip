//! The session's PDF raster-fallback preflight: admits every nested-SVG raster
//! closure the plan selected against the request's fallback policy and raster
//! ceilings, charges each closure's surface in canonical order, and retains the
//! rasterization diagnostics. Only the PDF backend rasterizes on fallback, so
//! the failure constructors it needs live here beside it.

use crate::resource_accounting::{
    ResourceContext, ResourceDelta, ResourcePhase, ResourceRecordKind,
};
use crate::{
    DiagnosticDetails, ErrorCode, Measurement, PdfRasterFallback, RenderError, RenderErrorCategory,
    RenderRequest, ResourceValue, ValueKind,
};

use super::super::diagnostics::pdf_raster_fallback_diagnostic;
use super::super::{DiagnosticOwnership, RenderSession};
use super::{PdfPlan, PdfRasterClosure};

impl RenderSession {
    pub(crate) fn preflight_pdf_raster_fallbacks(
        &mut self,
        request: &RenderRequest,
        plan: &PdfPlan,
    ) -> Result<Vec<u64>, RenderError> {
        for closure in plan.raster_closures() {
            if closure.unisolatable {
                return Err(self.pdf_raster_fallback_failure(
                    request,
                    closure,
                    ErrorCode::PDF_RASTER_FALLBACK_UNISOLATABLE,
                    "PDF raster fallback requires content outside an isolated nested-SVG closure",
                ));
            }
            if request
                .pdf_options()
                .is_some_and(|options| options.raster_fallback() == PdfRasterFallback::Forbid)
            {
                return Err(self.pdf_raster_fallback_failure(
                    request,
                    closure,
                    ErrorCode::PDF_RASTER_FALLBACK_FORBIDDEN,
                    "PDF raster fallback is forbidden by the render request",
                ));
            }
        }
        let mut surface_working = Vec::new();
        surface_working
            .try_reserve_exact(plan.raster_closures().len())
            .map_err(|_| {
                self.memory_failure(request, "format_planning", "pdf_raster_fallback_surfaces")
            })?;
        for closure in plan.raster_closures() {
            let mut closure_bytes = 0_u64;
            for (width, height) in closure.surface_dimensions() {
                let pixels = width.saturating_mul(height);
                let limits = request.resource_limits();
                let delta = ResourceDelta {
                    rasterized_pixels: pixels,
                    total_work_units: pixels,
                    ..ResourceDelta::default()
                };
                let context = ResourceContext::new(
                    ResourcePhase::ProjectionAndFallback,
                    "pdf_raster_fallback",
                    &closure.json_pointer,
                )
                .with_element_id(Some(&closure.element_id))
                .with_local_owner(Some(&closure.element_id));
                if self.meter.would_cross_fatal(delta) {
                    self.transact(request, context, delta)?;
                    return Err(self.internal_failure(
                        request.format(),
                        "fatal PDF fallback preflight committed unexpectedly",
                        "/elements",
                    ));
                }
                if width > u64::from(limits.max_raster_dimension())
                    || height > u64::from(limits.max_raster_dimension())
                {
                    self.preflight_local_rejection(request, context, delta)?;
                    return Err(self.pdf_raster_surface_failure(
                        request,
                        closure,
                        "max_raster_dimension",
                        u64::from(limits.max_raster_dimension()),
                        width.max(height),
                        ValueKind::Dimension,
                    ));
                }
                if pixels > limits.max_raster_surface_pixels() {
                    self.preflight_local_rejection(request, context, delta)?;
                    return Err(self.pdf_raster_surface_failure(
                        request,
                        closure,
                        "max_raster_surface_pixels",
                        limits.max_raster_surface_pixels(),
                        pixels,
                        ValueKind::Pixels,
                    ));
                }
                self.transact(request, context, delta)?;
                let bytes = 256_u64.saturating_add(pixels.saturating_mul(4));
                self.acquire_working(
                    request,
                    ResourceContext::new(
                        ResourcePhase::ProjectionAndFallback,
                        "pdf_raster_fallback_surface",
                        &closure.json_pointer,
                    )
                    .with_element_id(Some(&closure.element_id))
                    .with_local_owner(Some(&closure.element_id))
                    .with_record(ResourceRecordKind::RasterColorSurface),
                    bytes,
                )?;
                closure_bytes = closure_bytes.saturating_add(bytes);
            }
            // The closure's filter effects cost resvg their own surfaces and
            // per-primitive work beyond painting the input surface.
            let filter_work = plan
                .raster_closure_filter_work(
                    closure,
                    request.resource_limits().max_total_work_units(),
                )
                .map_err(|error| {
                    self.format_failure(request, ErrorCode::ENCODE_PDF_FAILED, error)
                })?;
            if !filter_work.is_empty() {
                self.transact(
                    request,
                    ResourceContext::new(
                        ResourcePhase::ProjectionAndFallback,
                        "pdf_raster_fallback_filter_samples",
                        &closure.json_pointer,
                    )
                    .with_element_id(Some(&closure.element_id))
                    .with_local_owner(Some(&closure.element_id)),
                    ResourceDelta {
                        rasterized_pixels: filter_work.surface_pixels,
                        total_work_units: filter_work.work_units,
                        ..ResourceDelta::default()
                    },
                )?;
            }
            surface_working.push(closure_bytes);
        }
        for closure in plan.raster_closures() {
            let diagnostic = pdf_raster_fallback_diagnostic(closure).map_err(|_| {
                self.memory_failure(request, "compatibility_report", "pdf_raster_fallback")
            })?;
            self.retain_diagnostic_with_owner(
                request,
                ResourcePhase::ProjectionAndFallback,
                "pdf_raster_fallback",
                diagnostic,
                DiagnosticOwnership {
                    local_owner: Some(&closure.element_id),
                    source_order: Some(closure.source_order),
                    source_position: Some(closure.source_position),
                },
            )?;
        }
        Ok(surface_working)
    }

    fn pdf_raster_surface_failure(
        &mut self,
        request: &RenderRequest,
        closure: &PdfRasterClosure,
        field: &'static str,
        configured: u64,
        attempted: u64,
        value_kind: ValueKind,
    ) -> RenderError {
        RenderError {
            category: RenderErrorCategory::ResourceLimit,
            code: ErrorCode::RESOURCE_RASTER_SURFACE_EXCEEDED,
            message: Box::from("PDF raster fallback surface exceeds its configured limit"),
            format: request.format(),
            element_id: Some(closure.element_id.clone()),
            json_pointer: closure.json_pointer.clone().into_boxed_str(),
            details: Some(Box::new(DiagnosticDetails::ResourceLimit {
                field,
                configured: ResourceValue::Integer(configured),
                value_kind,
                measurement: Measurement::Attempted {
                    value: ResourceValue::Integer(attempted),
                },
                recovery: None,
            })),
            report: self.take_report(),
        }
    }

    fn pdf_raster_fallback_failure(
        &mut self,
        request: &RenderRequest,
        closure: &PdfRasterClosure,
        code: ErrorCode,
        message: &'static str,
    ) -> RenderError {
        RenderError {
            category: RenderErrorCategory::NoMeaningfulArtifact,
            code,
            message: Box::from(message),
            format: request.format(),
            element_id: Some(closure.element_id.clone()),
            json_pointer: closure.json_pointer.clone().into_boxed_str(),
            details: Some(Box::new(DiagnosticDetails::PdfRasterFallback {
                recovery: None,
                logical_bounds: Some(closure.logical_bounds),
                pixel_width: Some(closure.pixel_width),
                pixel_height: Some(closure.pixel_height),
                causes: closure.causes.clone(),
            })),
            report: self.take_report(),
        }
    }
}
