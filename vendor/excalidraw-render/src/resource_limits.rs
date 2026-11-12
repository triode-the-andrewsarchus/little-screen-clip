use crate::{ErrorCode, RequestError};

/// The accepted domain of one limit kind, for its setter's documentation.
macro_rules! validation_domain {
    (integer) => {
        "at least `1`"
    };
    (finite) => {
        "finite, positive"
    };
}

macro_rules! define_resource_limits {
    ($(
        #[doc = $doc:literal]
        $field:ident: $type:ty = $default:literal, $maximum:literal, $validation:ident, $unit:literal;
    )+) => {
        /// Validated semantic resource ceilings for one render.
        ///
        /// Every limit is a deterministic, logical ceiling evaluated by the
        /// renderer's own accounting, never a measurement of the process: the
        /// same scene and request cross the same limit on every target.
        /// Equality is accepted. Start from [`ResourceLimits::default`] (or
        /// [`ResourceLimits::builder`]) and lower the limits that matter for
        /// your deployment; no limit can be raised above
        /// [`ResourceLimits::absolute_maxima`].
        ///
        /// `max_working_bytes` budgets the renderer's canonical logical working
        /// set, not allocator usage: actual peak memory is higher, notably for
        /// PNG, whose raster surfaces are bounded by `max_raster_dimension` and
        /// `max_raster_surface_pixels`. See the crate documentation's section
        /// on serving untrusted input.
        #[derive(Clone, Debug, PartialEq)]
        pub struct ResourceLimits {
            $($field: $type,)+
        }

        const DEFAULT_LIMITS: ResourceLimits = ResourceLimits {
            $($field: $default,)+
        };

        const ABSOLUTE_MAXIMA: ResourceLimits = ResourceLimits {
            $($field: $maximum,)+
        };

        impl ResourceLimits {
            /// Starts a builder from the revision-1 default profile.
            pub fn builder() -> ResourceLimitsBuilder {
                ResourceLimitsBuilder::default()
            }

            /// Returns the immutable absolute-maximum profile.
            pub const fn absolute_maxima() -> &'static Self {
                &ABSOLUTE_MAXIMA
            }

            $(
                #[doc = concat!(
                    "Returns the configured `", stringify!($field), "` ceiling: ", $doc,
                    " Measured in ", $unit, "."
                )]
                pub const fn $field(&self) -> $type {
                    self.$field
                }
            )+
        }

        impl Default for ResourceLimits {
            fn default() -> Self {
                DEFAULT_LIMITS.clone()
            }
        }

        /// Builder for validated semantic resource ceilings.
        #[must_use]
        #[derive(Clone, Debug, Default)]
        pub struct ResourceLimitsBuilder {
            limits: ResourceLimits,
        }

        impl ResourceLimitsBuilder {
            $(
                #[doc = concat!(
                    "Sets the `", stringify!($field), "` ceiling: ", $doc,
                    "\n\nMeasured in ", $unit, ". Default `", stringify!($default),
                    "`; accepted values are ",
                    validation_domain!($validation), " and at most `", stringify!($maximum),
                    "`, checked by [`build`](Self::build)."
                )]
                pub fn $field(mut self, value: $type) -> Self {
                    self.limits.$field = value;
                    self
                }
            )+

            fn validate_individual_limits(&self) -> Result<(), RequestError> {
                macro_rules! validate_limit {
                    (integer, $name:ident) => {
                        if self.limits.$name == 0
                            || self.limits.$name > ABSOLUTE_MAXIMA.$name
                        {
                            return Err(invalid_limit(
                                stringify!($name),
                                format!(
                                    "must be between 1 and {}, got {}",
                                    ABSOLUTE_MAXIMA.$name,
                                    self.limits.$name,
                                ),
                            ));
                        }
                    };
                    (finite, $name:ident) => {
                        if !self.limits.$name.is_finite()
                            || self.limits.$name <= 0.0
                            || self.limits.$name > ABSOLUTE_MAXIMA.$name
                        {
                            return Err(invalid_limit(
                                stringify!($name),
                                format!(
                                    "must be finite, positive and at most {}, got {}",
                                    ABSOLUTE_MAXIMA.$name,
                                    self.limits.$name,
                                ),
                            ));
                        }
                    };
                }

                $(validate_limit!($validation, $field);)+
                Ok(())
            }
        }
    };
}

define_resource_limits! {
    /// Bytes of the admitted scene document.
    max_scene_bytes: u64 = 33_554_432, 67_108_864, integer, "bytes";
    /// Source element occurrences: every member of the scene's `elements` array, live or deleted.
    max_elements: u32 = 16_384, 65_536, integer, "element occurrences";
    /// Asset entries: members of the scene's `files` map.
    max_asset_entries: u32 = 1_024, 4_096, integer, "asset entries";
    /// Bytes of any one asset payload.
    max_asset_bytes: u64 = 33_554_432, 67_108_864, integer, "bytes";
    /// Bytes of all asset payloads together; at least `max_asset_bytes`.
    max_total_asset_bytes: u64 = 134_217_728, 268_435_456, integer, "bytes";
    /// Depth of element relationship traversal, such as nested groups and frames.
    max_relationship_depth: u32 = 64, 128, integer, "levels";
    /// Depth of assets nested inside SVG image assets.
    max_nested_asset_depth: u32 = 4, 8, integer, "levels";
    /// Nodes in any one nested SVG image.
    max_nested_svg_nodes: u32 = 65_536, 262_144, integer, "nodes";
    /// Geometry segments generated for any one element.
    max_geometry_segments_per_element: u32 = 262_144, 1_048_576, integer, "segments";
    /// Geometry segments generated for the whole render; at least `max_geometry_segments_per_element`.
    max_total_geometry_segments: u64 = 4_194_304, 16_777_216, integer, "segments";
    /// Unicode scalars of text in any one element.
    max_text_scalars_per_element: u32 = 262_144, 1_048_576, integer, "Unicode scalar values";
    /// Unicode scalars of text in the whole render; at least `max_text_scalars_per_element`.
    max_total_text_scalars: u64 = 4_194_304, 16_777_216, integer, "Unicode scalar values";
    /// Width or height of any decoded image or required raster surface.
    max_raster_dimension: u32 = 16_384, 32_768, integer, "pixels per side";
    /// Decoded pixels of any one image asset.
    max_decoded_pixels_per_asset: u64 = 16_777_216, 67_108_864, integer, "pixels";
    /// Decoded pixels of all image assets together; at least `max_decoded_pixels_per_asset`.
    max_total_decoded_pixels: u64 = 33_554_432, 134_217_728, integer, "pixels";
    /// Area of any one required raster surface: the PNG canvas, or a PDF raster fallback.
    max_raster_surface_pixels: u64 = 33_554_432, 134_217_728, integer, "pixels";
    /// Area of all raster surfaces together, placeholders included; at least `max_raster_surface_pixels`.
    max_total_rasterized_pixels: u64 = 67_108_864, 268_435_456, integer, "pixels";
    /// Logical `viewBox` extent and final scaled width or height of an SVG.
    max_svg_dimension: f64 = 1_000_000.0, 10_000_000.0, finite, "SVG user units";
    /// PDF page width or height after padding and sizing.
    max_pdf_page_points: f64 = 14_400.0, 14_400.0, finite, "PDF points";
    /// Magnitude of any normalized coordinate, dimension or transform value.
    max_coordinate_magnitude: f64 = 1e9, 1e12, finite, "scene units";
    /// Diagnostics in one compatibility report.
    max_diagnostics: u32 = 65_536, 262_144, integer, "diagnostics";
    /// Peak canonical logical working set. A deterministic budget, not an allocator cap: actual memory use is higher.
    max_working_bytes: u64 = 268_435_456, 536_870_912, integer, "logical bytes";
    /// Bytes of the complete artifact.
    max_output_bytes: u64 = 134_217_728, 536_870_912, integer, "bytes";
    /// Deterministic work spent on any one element.
    max_element_work_units: u64 = 67_108_864, 268_435_456, integer, "work units";
    /// Deterministic work spent on any one asset.
    max_asset_work_units: u64 = 134_217_728, 536_870_912, integer, "work units";
    /// Deterministic work spent on the whole render; at least `max_element_work_units` and `max_asset_work_units`.
    max_total_work_units: u64 = 4_294_967_296, 17_179_869_184, integer, "work units";
}

impl ResourceLimitsBuilder {
    /// Validates and constructs the immutable resource-limit profile.
    ///
    /// # Errors
    ///
    /// Returns a [`RequestError`] with code
    /// `request.invalid-resource-limit`, naming the field, when a limit is
    /// zero, not finite, or above its [absolute
    /// maximum](ResourceLimits::absolute_maxima), or when a per-item limit
    /// exceeds the aggregate it contributes to (for example
    /// `max_asset_bytes` above `max_total_asset_bytes`).
    pub fn build(self) -> Result<ResourceLimits, RequestError> {
        self.validate_individual_limits()?;

        // Each per-item ceiling must fit inside the aggregate it contributes to.
        let limits = &self.limits;
        for (field, value, total_field, total) in [
            (
                "max_asset_bytes",
                limits.max_asset_bytes,
                "max_total_asset_bytes",
                limits.max_total_asset_bytes,
            ),
            (
                "max_geometry_segments_per_element",
                u64::from(limits.max_geometry_segments_per_element),
                "max_total_geometry_segments",
                limits.max_total_geometry_segments,
            ),
            (
                "max_text_scalars_per_element",
                u64::from(limits.max_text_scalars_per_element),
                "max_total_text_scalars",
                limits.max_total_text_scalars,
            ),
            (
                "max_decoded_pixels_per_asset",
                limits.max_decoded_pixels_per_asset,
                "max_total_decoded_pixels",
                limits.max_total_decoded_pixels,
            ),
            (
                "max_raster_surface_pixels",
                limits.max_raster_surface_pixels,
                "max_total_rasterized_pixels",
                limits.max_total_rasterized_pixels,
            ),
            (
                "max_element_work_units",
                limits.max_element_work_units,
                "max_total_work_units",
                limits.max_total_work_units,
            ),
            (
                "max_asset_work_units",
                limits.max_asset_work_units,
                "max_total_work_units",
                limits.max_total_work_units,
            ),
        ] {
            if value > total {
                return Err(invalid_limit(
                    field,
                    format!("({value}) must not exceed {total_field} ({total})"),
                ));
            }
        }

        Ok(self.limits)
    }
}

/// A `request.invalid-resource-limit` failure naming the rejected field and
/// why: `reason` follows the field name in the message.
fn invalid_limit(field: &'static str, reason: String) -> RequestError {
    RequestError::new(
        ErrorCode::REQUEST_INVALID_RESOURCE_LIMIT,
        format!("resource limit {field} {reason}"),
        field,
    )
}
