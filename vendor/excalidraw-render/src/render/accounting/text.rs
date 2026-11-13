//! Text accounting over the semantic graph: the scalar-and-run preflight of a
//! normalized text element before layout, the layout-and-glyph-geometry charge
//! of a resolved text element and of a generated embeddable label, and the
//! label-text preflight of compatibility fallbacks. Each charge's event name
//! and order is part of the canonical trace (ADR-0002).

use crate::diagnostic_registry::{LocalLimit, LocalRecovery};
use crate::normalization::{NormalizedElement, NormalizedElementKind};
use crate::resource_accounting::{
    ResourceContext, ResourceDelta, ResourcePhase, ResourceRecordKind,
};
use crate::scene_surface::SurfaceFallback;
use crate::{RenderError, RenderRequest, ResourceValue};

use super::super::diagnostics::{
    fallback_font_diagnostic, known_element_fallback, official_missing_glyph_diagnostic,
    FontFallback,
};
use super::super::resolve::{
    fallback_fits_local_limits, logical_aligned_bytes, ElementLocalCrossing, ResolvedOutline,
    ResolvedTextElement, SemanticWorkingRecord,
};
use super::super::{text, try_owned, DiagnosticOwnership, RenderSession};

impl RenderSession {
    pub(crate) fn preflight_official_text(
        &mut self,
        source: &NormalizedElement,
        relationships: &crate::relationships::ResolvedRelationshipGraph,
        request: &RenderRequest,
        semantic_working: &mut Vec<SemanticWorkingRecord>,
        fallbacks: &mut Vec<SurfaceFallback>,
    ) -> Result<bool, RenderError> {
        let NormalizedElementKind::Text(text) = &source.kind else {
            return Err(self.internal_failure(
                request.format(),
                "official text preflight received a non-text element",
                "/elements",
            ));
        };
        let text_scalars = u64::try_from(text.text.chars().count()).unwrap_or(u64::MAX);
        let source_bytes = u64::try_from(text.text.len()).unwrap_or(u64::MAX);
        let prospective_glyphs = text::prospective_glyph_count(&text.text);
        let stored_line_steps = text::line_count(&text.text);
        let prepaid_work = text_scalars
            .saturating_mul(32)
            .saturating_add(prospective_glyphs.saturating_mul(32))
            .saturating_add(stored_line_steps.saturating_mul(32));
        let working_bytes = 256_u64.saturating_add(logical_aligned_bytes(source_bytes));
        let delta = ResourceDelta {
            text_scalars,
            working_bytes,
            total_work_units: prepaid_work,
            ..ResourceDelta::default()
        };
        let context = ResourceContext::new(
            ResourcePhase::SharedSemanticResolution,
            "text_scalar_and_run_preflight",
            source.source.pointer(),
        )
        .with_element_id(Some(source.source.id()))
        .with_local_owner(Some(source.source.id()));
        if self.meter.would_cross_fatal(delta) {
            self.transact(request, context, delta)?;
            return Err(self.internal_failure(
                request.format(),
                "fatal text scalar preflight committed unexpectedly",
                "/elements",
            ));
        }
        let crossing = if text_scalars
            > u64::from(request.resource_limits().max_text_scalars_per_element())
        {
            Some(ElementLocalCrossing {
                limit: LocalLimit::TextScalarsPerElement,
                configured: ResourceValue::Integer(u64::from(
                    request.resource_limits().max_text_scalars_per_element(),
                )),
                attempted: crate::Measurement::Attempted {
                    value: ResourceValue::Integer(text_scalars),
                },
                pointer_field: None,
                message: "text was omitted because it exceeds its scalar limit",
            })
        } else if prepaid_work > request.resource_limits().max_element_work_units() {
            Some(ElementLocalCrossing {
                limit: LocalLimit::ElementWorkUnits,
                configured: ResourceValue::Integer(
                    request.resource_limits().max_element_work_units(),
                ),
                attempted: crate::Measurement::Attempted {
                    value: ResourceValue::Integer(prepaid_work),
                },
                pointer_field: None,
                message: "text was omitted before shaping because scalar work exceeds its limit",
            })
        } else {
            None
        };
        if let Some(crossing) = crossing {
            self.commit_local_rejection_observation(
                request,
                context,
                ResourceDelta {
                    text_scalars,
                    ..ResourceDelta::default()
                },
            )?;
            let replacement = if source.width > 0.0 && source.height > 0.0 {
                let fallback = known_element_fallback(source).map_err(|_| {
                    self.memory_failure(request, "semantic_geometry", "known_element_fallback")
                })?;
                fallback_fits_local_limits(&fallback, request.resource_limits()).then_some(fallback)
            } else {
                None
            };
            let can_replace = replacement.is_some();
            let diagnostic = crossing
                .diagnostic_with_recovery(
                    request.format(),
                    Some(source.source.id()),
                    source.source.pointer(),
                    LocalRecovery::placeholder_if(can_replace),
                    super::super::resolve::live_text_occurrence(
                        text,
                        source.source.source_index(),
                        relationships,
                    )
                    .is_some(),
                )
                .map_err(|_| {
                    self.memory_failure(request, "compatibility_report", "text_preflight_recovery")
                })?;
            self.retain_diagnostic_with_owner(
                request,
                ResourcePhase::SharedSemanticResolution,
                "text_preflight_local_recovery",
                diagnostic,
                DiagnosticOwnership {
                    local_owner: Some(source.source.id()),
                    source_order: Some(source.source_order),
                    source_position: Some(0),
                },
            )?;
            if let Some(fallback) = replacement {
                fallbacks.try_reserve(1).map_err(|_| {
                    self.memory_failure(request, "semantic_geometry", "fallback_list")
                })?;
                fallbacks.push(fallback);
            } else {
                self.charge_local_omission(
                    request,
                    Some(source.source.id()),
                    source.source.pointer(),
                    "text_preflight_local_omission",
                )?;
            }
            return Ok(false);
        }
        self.transact(request, context, delta)?;
        semantic_working.try_reserve(1).map_err(|_| {
            self.memory_failure(request, "semantic_geometry", "text_run_working_record")
        })?;
        semantic_working.push(SemanticWorkingRecord {
            source_order: source.source_order,
            event: "resolved_text_run",
            element_id: Some(try_owned(source.source.id()).map_err(|_| {
                self.memory_failure(request, "semantic_geometry", "text_run_identity")
            })?),
            local_owner: Some(try_owned(source.source.id()).map_err(|_| {
                self.memory_failure(request, "semantic_geometry", "text_run_owner")
            })?),
            json_pointer: try_owned(source.source.pointer()).map_err(|_| {
                self.memory_failure(request, "semantic_geometry", "text_run_pointer")
            })?,
            bytes: working_bytes,
        });
        if !self.excalifont_selected {
            self.transact(
                request,
                ResourceContext::new(
                    ResourcePhase::SharedSemanticResolution,
                    "excalifont_builtin_selection",
                    source.source.pointer(),
                )
                .with_element_id(Some(source.source.id()))
                .with_local_owner(Some(source.source.id()))
                .with_record(ResourceRecordKind::BuiltInSelectionState),
                ResourceDelta {
                    working_bytes: 256,
                    ..ResourceDelta::default()
                },
            )?;
            self.excalifont_selected = true;
        }
        if !self.excalifont_range_selected {
            self.transact(
                request,
                ResourceContext::new(
                    ResourcePhase::SharedSemanticResolution,
                    "excalifont_basic_latin_range_selection",
                    source.source.pointer(),
                )
                .with_element_id(Some(source.source.id()))
                .with_local_owner(Some(source.source.id()))
                .with_record(ResourceRecordKind::BuiltInGlyphOrRangeState),
                ResourceDelta {
                    working_bytes: 64,
                    ..ResourceDelta::default()
                },
            )?;
            self.excalifont_range_selected = true;
        }
        Ok(true)
    }

    pub(crate) fn account_embeddable_label(
        &mut self,
        outline: &mut ResolvedOutline,
        request: &RenderRequest,
    ) -> Result<(), RenderError> {
        let Some(label) = outline.generated_label.as_ref() else {
            return Ok(());
        };
        let geometry_segments = label.outline_segments();
        let delta = super::super::resolve::admission::generated_label_delta(label);
        let total_work_units = delta.total_work_units;
        let context = ResourceContext::new(
            ResourcePhase::SharedSemanticResolution,
            "embeddable_label_layout_and_glyph_geometry",
            outline.source.pointer(),
        )
        .with_element_id(Some(outline.source.id()))
        .with_local_owner(Some(outline.source.id()));
        if self.meter.would_cross_fatal(delta) {
            self.transact(request, context, delta)?;
            return Err(self.internal_failure(
                request.format(),
                "fatal embeddable label resource preflight committed unexpectedly",
                "/elements",
            ));
        }
        let local_crossing = if geometry_segments
            > u64::from(
                request
                    .resource_limits()
                    .max_geometry_segments_per_element(),
            ) {
            Some(ElementLocalCrossing {
                limit: LocalLimit::GeometrySegmentsPerElement,
                configured: ResourceValue::Integer(u64::from(
                    request
                        .resource_limits()
                        .max_geometry_segments_per_element(),
                )),
                attempted: crate::Measurement::Attempted { value: ResourceValue::Integer(geometry_segments) },
                pointer_field: None,
                message: "embeddable label was omitted because its glyph outlines exceed the geometry limit",
            })
        } else if outline
            .base_element_work_units
            .saturating_add(total_work_units)
            > request.resource_limits().max_element_work_units()
        {
            Some(ElementLocalCrossing {
                limit: LocalLimit::ElementWorkUnits,
                configured: ResourceValue::Integer(
                    request.resource_limits().max_element_work_units(),
                ),
                attempted: crate::Measurement::Attempted {
                    value: ResourceValue::Integer(
                        outline
                            .base_element_work_units
                            .saturating_add(total_work_units),
                    ),
                },
                pointer_field: None,
                message: "embeddable label was omitted because shaping exceeds its work limit",
            })
        } else {
            None
        };
        if let Some(crossing) = local_crossing {
            self.commit_local_rejection_observation(
                request,
                context,
                ResourceDelta {
                    geometry_segments,
                    ..ResourceDelta::default()
                },
            )?;
            let diagnostic = crossing
                .diagnostic(
                    request.format(),
                    Some(outline.source.id()),
                    outline.source.pointer(),
                )
                .map_err(|_| {
                    self.memory_failure(
                        request,
                        "compatibility_report",
                        "embeddable_label_local_recovery",
                    )
                })?;
            self.retain_diagnostic_with_owner(
                request,
                ResourcePhase::SharedSemanticResolution,
                "embeddable_label_local_resource_recovery",
                diagnostic,
                DiagnosticOwnership {
                    local_owner: Some(outline.source.id()),
                    source_order: Some(outline.source_order),
                    source_position: Some(0),
                },
            )?;
            self.charge_local_omission(
                request,
                Some(outline.source.id()),
                outline.source.pointer(),
                "embeddable_label_local_omission",
            )?;
            outline.generated_label = None;
            outline.generated_label_color = None;
            return Ok(());
        }
        if label.noto_emoji_substituted {
            let diagnostic = fallback_font_diagnostic(
                FontFallback::EmbeddableNotoEmoji,
                request.format(),
                Some(outline.source.id()),
                outline.source.pointer(),
            )
            .map_err(|_| {
                self.memory_failure(
                    request,
                    "compatibility_report",
                    "embeddable_label_emoji_font_substitution",
                )
            })?;
            self.retain_diagnostic_with_owner(
                request,
                ResourcePhase::SharedSemanticResolution,
                "embeddable_label_emoji_font_substitution",
                diagnostic,
                DiagnosticOwnership {
                    local_owner: Some(outline.source.id()),
                    source_order: Some(outline.source_order),
                    source_position: Some(0),
                },
            )?;
        }
        for codepoint in &label.missing_codepoints {
            let diagnostic = official_missing_glyph_diagnostic(
                request.format(),
                outline.source.id(),
                outline.source.pointer(),
                *codepoint,
            )
            .map_err(|_| {
                self.memory_failure(
                    request,
                    "compatibility_report",
                    "embeddable_label_missing_glyph",
                )
            })?;
            self.retain_diagnostic_with_owner(
                request,
                ResourcePhase::SharedSemanticResolution,
                "embeddable_label_missing_glyph",
                diagnostic,
                DiagnosticOwnership {
                    local_owner: Some(outline.source.id()),
                    source_order: Some(outline.source_order),
                    source_position: Some(u64::MAX),
                },
            )?;
        }
        self.transact(request, context, delta)?;
        outline.base_element_work_units = outline
            .base_element_work_units
            .saturating_add(total_work_units);
        Ok(())
    }

    /// Charges one resolved text element's layout and glyph geometry over the
    /// resolved graph, retains its font diagnostics, and records its working
    /// bytes; a local crossing omits the text instead.
    pub(crate) fn account_resolved_text(
        &mut self,
        text: &mut ResolvedTextElement,
        request: &RenderRequest,
        semantic_working: &mut Vec<SemanticWorkingRecord>,
    ) -> Result<(), RenderError> {
        let clip_segments = text.frame_clips.iter().fold(0_u64, |total, clip| {
            total.saturating_add(u64::try_from(clip.path.operations.len()).unwrap_or(u64::MAX))
        });
        let outline_segments = text.text.outline_segments();
        let geometry_segments = outline_segments.saturating_add(clip_segments);
        let glyphs = text.text.glyph_count();
        let text_scalars = text.text.scalar_count;
        let line_break_steps = text.text.line_break_steps;
        let remaining_work_units = line_break_steps
            .saturating_sub(text.text.prepaid_line_break_steps)
            .saturating_mul(32)
            .saturating_add(
                glyphs
                    .saturating_sub(text.text.prepaid_glyphs)
                    .saturating_mul(32),
            )
            .saturating_add(text.text.fallback_probes.saturating_mul(32))
            .saturating_add(geometry_segments.saturating_mul(32));
        let total_work_units = text_scalars
            .saturating_mul(32)
            .saturating_add(text.text.prepaid_glyphs.saturating_mul(32))
            .saturating_add(text.text.prepaid_line_break_steps.saturating_mul(32))
            .saturating_add(remaining_work_units);
        text.base_element_work_units = total_work_units;
        let path_headers = text
            .text
            .run_count()
            .saturating_add(u64::try_from(text.frame_clips.len()).unwrap_or(u64::MAX));
        let working_bytes = glyphs
            .saturating_mul(64)
            .saturating_add(line_break_steps.saturating_mul(32))
            .saturating_add(path_headers.saturating_mul(128))
            .saturating_add(text.text.fallback_resource_count().saturating_mul(256))
            .saturating_add(geometry_segments.saturating_mul(64));
        let delta = ResourceDelta {
            geometry_segments,
            working_bytes,
            total_work_units: remaining_work_units,
            ..ResourceDelta::default()
        };
        let context = ResourceContext::new(
            ResourcePhase::SharedSemanticResolution,
            "text_layout_and_glyph_geometry",
            text.source.pointer(),
        )
        .with_element_id(Some(text.source.id()))
        .with_local_owner(Some(text.source.id()));
        if self.meter.would_cross_fatal(delta) {
            self.transact(request, context, delta)?;
            return Err(self.internal_failure(
                request.format(),
                "fatal text resource preflight committed unexpectedly",
                "/elements",
            ));
        }
        let local_crossing = if let Some(value) = text.coordinate_failure {
            Some(super::super::coordinates::crossing(value, request))
        } else if geometry_segments
            > u64::from(
                request
                    .resource_limits()
                    .max_geometry_segments_per_element(),
            )
        {
            Some(ElementLocalCrossing {
                limit: LocalLimit::GeometrySegmentsPerElement,
                configured: ResourceValue::Integer(u64::from(
                    request
                        .resource_limits()
                        .max_geometry_segments_per_element(),
                )),
                attempted: crate::Measurement::Attempted {
                    value: ResourceValue::Integer(geometry_segments),
                },
                pointer_field: None,
                message: "text was omitted because its glyph outlines exceed the geometry limit",
            })
        } else if total_work_units > request.resource_limits().max_element_work_units() {
            Some(ElementLocalCrossing {
                limit: LocalLimit::ElementWorkUnits,
                configured: ResourceValue::Integer(
                    request.resource_limits().max_element_work_units(),
                ),
                attempted: crate::Measurement::Attempted {
                    value: ResourceValue::Integer(total_work_units),
                },
                pointer_field: None,
                message: "text was omitted because shaping exceeds its work limit",
            })
        } else {
            None
        };
        if let Some(crossing) = local_crossing {
            self.commit_local_rejection_observation(
                request,
                context,
                ResourceDelta {
                    geometry_segments,
                    ..ResourceDelta::default()
                },
            )?;
            let diagnostic = crossing
                .diagnostic(
                    request.format(),
                    Some(text.source.id()),
                    text.source.pointer(),
                )
                .map_err(|_| {
                    self.memory_failure(request, "compatibility_report", "text_local_recovery")
                })?;
            self.retain_diagnostic_with_owner(
                request,
                ResourcePhase::SharedSemanticResolution,
                "text_local_resource_recovery",
                diagnostic,
                DiagnosticOwnership {
                    local_owner: Some(text.source.id()),
                    source_order: Some(text.source_order),
                    source_position: Some(0),
                },
            )?;
            self.charge_local_omission(
                request,
                Some(text.source.id()),
                text.source.pointer(),
                "text_local_omission",
            )?;
            text.render = false;
            return Ok(());
        }
        if text.font_family == 2 {
            let diagnostic = fallback_font_diagnostic(
                FontFallback::HelveticaText,
                request.format(),
                Some(text.source.id()),
                text.source.pointer(),
            )
            .map_err(|_| {
                self.memory_failure(request, "compatibility_report", "text_font_substitution")
            })?;
            self.retain_diagnostic_with_owner(
                request,
                ResourcePhase::SharedSemanticResolution,
                "text_font_substitution",
                diagnostic,
                DiagnosticOwnership {
                    local_owner: Some(text.source.id()),
                    source_order: Some(text.source_order),
                    source_position: Some(0),
                },
            )?;
        }
        if text.text.noto_emoji_substituted {
            let diagnostic = fallback_font_diagnostic(
                FontFallback::NotoEmoji,
                request.format(),
                Some(text.source.id()),
                text.source.pointer(),
            )
            .map_err(|_| {
                self.memory_failure(
                    request,
                    "compatibility_report",
                    "text_emoji_font_substitution",
                )
            })?;
            self.retain_diagnostic_with_owner(
                request,
                ResourcePhase::SharedSemanticResolution,
                "text_emoji_font_substitution",
                diagnostic,
                DiagnosticOwnership {
                    local_owner: Some(text.source.id()),
                    source_order: Some(text.source_order),
                    source_position: Some(0),
                },
            )?;
        }
        for codepoint in &text.text.missing_codepoints {
            let diagnostic = official_missing_glyph_diagnostic(
                request.format(),
                text.source.id(),
                text.source.pointer(),
                *codepoint,
            )
            .map_err(|_| {
                self.memory_failure(request, "compatibility_report", "official_missing_glyph")
            })?;
            self.retain_diagnostic_with_owner(
                request,
                ResourcePhase::SharedSemanticResolution,
                "official_missing_glyph",
                diagnostic,
                DiagnosticOwnership {
                    local_owner: Some(text.source.id()),
                    source_order: Some(text.source_order),
                    source_position: Some(u64::MAX),
                },
            )?;
        }
        self.transact(request, context, delta)?;
        for glyph_id in text
            .text
            .lines
            .iter()
            .flat_map(|line| line.glyphs.iter().map(|glyph| glyph.id))
        {
            if self.excalifont_glyphs.contains(&glyph_id) {
                continue;
            }
            self.transact(
                request,
                ResourceContext::new(
                    ResourcePhase::SharedSemanticResolution,
                    "excalifont_glyph_selection",
                    text.source.pointer(),
                )
                .with_element_id(Some(text.source.id()))
                .with_local_owner(Some(text.source.id()))
                .with_record(ResourceRecordKind::BuiltInGlyphOrRangeState),
                ResourceDelta {
                    working_bytes: 64,
                    ..ResourceDelta::default()
                },
            )?;
            self.excalifont_glyphs.try_reserve(1).map_err(|_| {
                self.memory_failure(request, "built_in_selection", "excalifont_glyph")
            })?;
            self.excalifont_glyphs.push(glyph_id);
        }
        let derived_bytes = glyphs.saturating_mul(8).saturating_add(
            u64::try_from(text.text.missing_codepoints.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(4),
        );
        if derived_bytes != 0 {
            let previous = self.excalifont_request_derived_bytes;
            let next = previous.saturating_add(derived_bytes);
            let working_bytes = logical_aligned_bytes(next)
                .saturating_sub(logical_aligned_bytes(previous))
                .saturating_add(u64::from(previous == 0) * 256);
            self.transact(
                request,
                ResourceContext::new(
                    ResourcePhase::SharedSemanticResolution,
                    "excalifont_request_derived_data",
                    text.source.pointer(),
                )
                .with_element_id(Some(text.source.id()))
                .with_local_owner(Some(text.source.id()))
                .with_record(ResourceRecordKind::RequestDerivedBuiltInBlob),
                ResourceDelta {
                    working_bytes,
                    ..ResourceDelta::default()
                },
            )?;
            self.excalifont_request_derived_bytes = next;
        }
        semantic_working.try_reserve(1).map_err(|_| {
            self.memory_failure(request, "semantic_geometry", "text_working_record")
        })?;
        semantic_working.push(SemanticWorkingRecord {
            source_order: text.source_order,
            event: "resolved_text_layout_and_glyph_geometry",
            element_id: Some(
                try_owned(text.source.id()).map_err(|_| {
                    self.memory_failure(request, "semantic_geometry", "text_identity")
                })?,
            ),
            local_owner: Some(
                try_owned(text.source.id())
                    .map_err(|_| self.memory_failure(request, "semantic_geometry", "text_owner"))?,
            ),
            json_pointer: try_owned(text.source.pointer())
                .map_err(|_| self.memory_failure(request, "semantic_geometry", "text_pointer"))?,
            bytes: working_bytes,
        });
        Ok(())
    }

    pub(crate) fn preflight_fallback_text(
        &mut self,
        fallbacks: &mut [SurfaceFallback],
        request: &RenderRequest,
        semantic_working: &mut Vec<SemanticWorkingRecord>,
    ) -> Result<(), RenderError> {
        let mut index = 0_usize;
        while index < fallbacks.len() {
            let fallback = &fallbacks[index];
            let text_scalars = u64::try_from(fallback.label.chars().count()).unwrap_or(u64::MAX);
            let working_bytes = 256_u64.saturating_add(logical_aligned_bytes(
                u64::try_from(fallback.label.len()).unwrap_or(u64::MAX),
            ));
            let work_units = text_scalars.saturating_mul(32);
            let base_geometry_segments = if fallback.width >= 2.0 && fallback.height >= 2.0 {
                9
            } else {
                5
            };
            let geometry_preflight = ResourceDelta {
                geometry_segments: base_geometry_segments,
                ..ResourceDelta::default()
            };
            let geometry_context = ResourceContext::new(
                ResourcePhase::SharedSemanticResolution,
                "compatibility_fallback_geometry_preflight",
                &fallback.json_pointer,
            )
            .with_element_id(fallback.element_id.as_deref())
            .with_local_owner(fallback.element_id.as_deref());
            if self.meter.would_cross_fatal(geometry_preflight) {
                self.transact(request, geometry_context, geometry_preflight)?;
                return Err(self.internal_failure(
                    request.format(),
                    "fatal fallback geometry preflight committed unexpectedly",
                    "/elements",
                ));
            }
            let delta = ResourceDelta {
                text_scalars,
                working_bytes,
                total_work_units: work_units,
                ..ResourceDelta::default()
            };
            let context = ResourceContext::new(
                ResourcePhase::SharedSemanticResolution,
                "compatibility_fallback_text",
                &fallback.json_pointer,
            )
            .with_element_id(fallback.element_id.as_deref())
            .with_local_owner(fallback.element_id.as_deref());
            if self.meter.would_cross_fatal(delta) {
                self.transact(request, context, delta)?;
                return Err(self.internal_failure(
                    request.format(),
                    "fatal fallback text preflight committed unexpectedly",
                    "/elements",
                ));
            }
            if text_scalars > u64::from(request.resource_limits().max_text_scalars_per_element()) {
                self.commit_local_rejection_observation(
                    request,
                    context,
                    ResourceDelta {
                        text_scalars,
                        ..ResourceDelta::default()
                    },
                )?;
                let diagnostic = ElementLocalCrossing {
                    limit: LocalLimit::TextScalarsPerElement,
                    configured: ResourceValue::Integer(u64::from(
                        request.resource_limits().max_text_scalars_per_element(),
                    )),
                    attempted: crate::Measurement::Attempted {
                        value: ResourceValue::Integer(text_scalars),
                    },
                    pointer_field: None,
                    message:
                        "compatibility fallback label omitted because it exceeds the text limit",
                }
                .diagnostic(
                    request.format(),
                    fallback.element_id.as_deref(),
                    &fallback.json_pointer,
                )
                .map_err(|_| {
                    self.memory_failure(request, "compatibility_report", "fallback_text_recovery")
                })?;
                self.retain_diagnostic_with_owner(
                    request,
                    ResourcePhase::SharedSemanticResolution,
                    "fallback_text_local_resource_recovery",
                    diagnostic,
                    DiagnosticOwnership {
                        local_owner: fallback.element_id.as_deref(),
                        source_order: Some(fallback.source_order),
                        source_position: Some(0),
                    },
                )?;
                self.charge_local_omission(
                    request,
                    fallback.element_id.as_deref(),
                    &fallback.json_pointer,
                    "fallback_text_local_omission",
                )?;
                fallbacks[index].label.clear();
                index = index.saturating_add(1);
                continue;
            }
            self.transact(request, context, delta)?;
            semantic_working.try_reserve(1).map_err(|_| {
                self.memory_failure(request, "semantic_text", "fallback_text_working_record")
            })?;
            semantic_working.push(SemanticWorkingRecord {
                source_order: fallback.source_order,
                event: "resolved_compatibility_fallback_text",
                element_id: fallback
                    .element_id
                    .as_deref()
                    .map(try_owned)
                    .transpose()
                    .map_err(|_| {
                        self.memory_failure(request, "semantic_text", "fallback_text_identity")
                    })?,
                local_owner: fallback
                    .element_id
                    .as_deref()
                    .map(try_owned)
                    .transpose()
                    .map_err(|_| {
                        self.memory_failure(request, "semantic_text", "fallback_text_owner")
                    })?,
                json_pointer: try_owned(&fallback.json_pointer).map_err(|_| {
                    self.memory_failure(request, "semantic_text", "fallback_text_pointer")
                })?,
                bytes: working_bytes,
            });
            index = index.saturating_add(1);
        }
        Ok(())
    }
}
