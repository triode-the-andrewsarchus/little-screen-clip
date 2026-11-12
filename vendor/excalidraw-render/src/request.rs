use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use serde::Serialize;

use crate::color::ColorLowering;
use crate::{DiagnosticDetails, ErrorCode, ErrorDescriptor, ResourceLimits};

/// Indicates that a render request could not be validated.
///
/// [`Error::source`] is always `None`: validation has no underlying cause
/// beyond the stable code and message.
#[derive(Clone, Debug)]
pub struct RequestError {
    code: ErrorCode,
    message: Box<str>,
    /// Always [`DiagnosticDetails::RequestValue`].
    details: Box<DiagnosticDetails>,
}

impl RequestError {
    pub(crate) fn new(code: ErrorCode, message: impl Into<Box<str>>, field: &'static str) -> Self {
        Self::with_value(code, message, field, None, None)
    }

    fn for_option(
        code: ErrorCode,
        message: impl Into<Box<str>>,
        field: &'static str,
        option: &'static str,
    ) -> Self {
        Self::with_value(code, message, field, None, Some(option))
    }

    fn for_asset(
        code: ErrorCode,
        message: impl Into<Box<str>>,
        asset_id: String,
        option: Option<&'static str>,
    ) -> Self {
        Self::with_value(code, message, "supplemental_assets", Some(asset_id), option)
    }

    fn with_value(
        code: ErrorCode,
        message: impl Into<Box<str>>,
        field: &'static str,
        asset_id: Option<String>,
        option: Option<&'static str>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            details: Box::new(DiagnosticDetails::RequestValue {
                field,
                asset_id,
                option,
            }),
        }
    }

    /// The rejected field, supplemental asset ID and format option.
    const fn request_value(&self) -> (&'static str, Option<&String>, Option<&'static str>) {
        match &*self.details {
            DiagnosticDetails::RequestValue {
                field,
                asset_id,
                option,
            } => (field, asset_id.as_ref(), *option),
            _ => unreachable!(),
        }
    }

    /// Returns the stable machine-readable request error code.
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// Returns the rejected canonical request field.
    pub const fn field(&self) -> &str {
        self.request_value().0
    }

    /// Returns the applicable supplemental asset ID.
    pub fn asset_id(&self) -> Option<&str> {
        self.request_value().1.map(String::as_str)
    }

    /// Returns the applicable format-option name.
    pub const fn option(&self) -> Option<&str> {
        self.request_value().2
    }

    /// Returns typed details of this failure: always
    /// [`DiagnosticDetails::RequestValue`], naming the rejected field and,
    /// where they apply, the supplemental asset and format option.
    pub fn details(&self) -> Option<&DiagnosticDetails> {
        Some(&self.details)
    }

    /// Returns this failure's stable serializable descriptor.
    pub fn descriptor(&self) -> ErrorDescriptor {
        ErrorDescriptor::request(self.code, self.to_string(), (*self.details).clone())
    }
}

impl fmt::Display for RequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for RequestError {}

/// The single artifact format produced by a render request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// A semantic SVG document. Every build supports it.
    Svg,
    /// A canvas-equivalent PNG image. Needs the default `png` feature.
    Png,
    /// A one-page vector-first PDF document. Needs the default `pdf` feature.
    Pdf,
}

impl OutputFormat {
    /// Returns the canonical media type for this format.
    pub const fn media_type(self) -> &'static str {
        match self {
            Self::Svg => "image/svg+xml",
            Self::Png => "image/png",
            Self::Pdf => "application/pdf",
        }
    }

    /// Returns the stable serialized spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Svg => "svg",
            Self::Png => "png",
            Self::Pdf => "pdf",
        }
    }

    /// The Cargo feature that compiles this format in, when it is not part of
    /// every build, and whether this build enables it.
    const fn feature(self) -> Option<(&'static str, bool)> {
        match self {
            Self::Svg => None,
            Self::Png => Some(("png", cfg!(feature = "png"))),
            Self::Pdf => Some(("pdf", cfg!(feature = "pdf"))),
        }
    }

    /// Rejects a format whose Cargo feature this build leaves out, before any
    /// other request validation: no option of the request could be honored.
    fn require_enabled(self) -> Result<(), RequestError> {
        match self.feature() {
            Some((feature, false)) => Err(RequestError::new(
                ErrorCode::REQUEST_FORMAT_UNAVAILABLE,
                format!(
                    "{} output is not enabled in this build; enable the `{feature}` feature \
                     of excalidraw-render",
                    self.as_str().to_ascii_uppercase()
                ),
                "format",
            )),
            Some((_, true)) | None => Ok(()),
        }
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Output sizing selected for one render.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Sizing {
    /// Multiplies logical output dimensions by the supplied scale.
    Scale(f64),
    /// Downscales PNG and PDF output so its larger side is at most the
    /// supplied number of logical pixels; never enlarges.
    ///
    /// PNG output then measures at most this many pixels on its larger side.
    /// A PDF page maps one logical pixel to 0.75 points, so its larger side
    /// is at most three quarters of this many points. SVG output ignores it
    /// and keeps its logical size; use [`Sizing::Scale`] to resize SVG.
    MaxDimension(f64),
}

/// The reference export size: `Scale(1.0)`.
impl Default for Sizing {
    fn default() -> Self {
        Self::Scale(1.0)
    }
}

/// Color mode used to interpret scene colors.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum ColorMode {
    /// Reference light-mode colors.
    #[default]
    Light,
    /// Reference dark-mode colors.
    Dark,
}

/// Whole-scene frame rendering controls.
///
/// The default renders frames the way the reference export does: frames are
/// enabled, their contents are clipped, and their names and outlines are
/// drawn. Start from [`FrameOptions::default`] and turn individual controls
/// off with the `with_*` setters:
///
/// ```
/// use excalidraw_render::FrameOptions;
///
/// let options = FrameOptions::default().with_show_name(false).with_outline(false);
/// assert!(options.enabled() && options.clip());
/// assert!(!options.show_name() && !options.outline());
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameOptions {
    enabled: bool,
    clip: bool,
    show_name: bool,
    outline: bool,
}

impl FrameOptions {
    /// Selects whether frames are rendered at all (default `true`).
    #[must_use]
    pub const fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Selects whether frame contents are clipped to the frame (default `true`).
    #[must_use]
    pub const fn with_clip(mut self, clip: bool) -> Self {
        self.clip = clip;
        self
    }

    /// Selects whether frame names are rendered (default `true`).
    #[must_use]
    pub const fn with_show_name(mut self, show_name: bool) -> Self {
        self.show_name = show_name;
        self
    }

    /// Selects whether frame outlines are rendered (default `true`).
    #[must_use]
    pub const fn with_outline(mut self, outline: bool) -> Self {
        self.outline = outline;
        self
    }

    /// Returns whether frames are rendered.
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Returns whether frame contents are clipped.
    pub const fn clip(&self) -> bool {
        self.clip
    }

    /// Returns whether frame names are rendered.
    pub const fn show_name(&self) -> bool {
        self.show_name
    }

    /// Returns whether frame outlines are rendered.
    pub const fn outline(&self) -> bool {
        self.outline
    }
}

impl Default for FrameOptions {
    /// Every frame control on: rendered, clipped, named and outlined.
    fn default() -> Self {
        Self {
            enabled: true,
            clip: true,
            show_name: true,
            outline: true,
        }
    }
}

/// Optional artifact metadata controls.
///
/// The default embeds no optional metadata. Opt in to each component with the
/// `with_*` setters:
///
/// ```
/// use excalidraw_render::MetadataOptions;
///
/// let options = MetadataOptions::default()
///     .with_embedded_scene(true)
///     .with_provenance(true);
/// assert!(options.embedded_scene() && options.provenance());
/// assert!(!options.diagnostic_summary());
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MetadataOptions {
    embedded_scene: bool,
    provenance: bool,
    diagnostic_summary: bool,
}

impl MetadataOptions {
    /// Selects whether the canonicalized original scene document is embedded
    /// (default `false`).
    #[must_use]
    pub const fn with_embedded_scene(mut self, embedded_scene: bool) -> Self {
        self.embedded_scene = embedded_scene;
        self
    }

    /// Selects whether renderer provenance is embedded (default `false`).
    #[must_use]
    pub const fn with_provenance(mut self, provenance: bool) -> Self {
        self.provenance = provenance;
        self
    }

    /// Selects whether the pre-metadata diagnostic summary is embedded
    /// (default `false`).
    #[must_use]
    pub const fn with_diagnostic_summary(mut self, diagnostic_summary: bool) -> Self {
        self.diagnostic_summary = diagnostic_summary;
        self
    }

    /// Returns whether the canonicalized original scene document is embedded.
    pub const fn embedded_scene(&self) -> bool {
        self.embedded_scene
    }

    /// Returns whether renderer provenance is embedded.
    pub const fn provenance(&self) -> bool {
        self.provenance
    }

    /// Returns whether the pre-metadata diagnostic summary is embedded.
    pub const fn diagnostic_summary(&self) -> bool {
        self.diagnostic_summary
    }
}

/// Policy for PDF-only isolated raster fallback.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum PdfRasterFallback {
    /// Rasterize only the isolated closures allowed by the compatibility contract.
    #[default]
    AllowIsolated,
    /// Fail if an isolated raster fallback closure remains after preflight.
    Forbid,
}

/// PDF-specific request controls.
///
/// The default allows isolated raster fallback:
///
/// ```
/// use excalidraw_render::{PdfOptions, PdfRasterFallback};
///
/// let options = PdfOptions::default().with_raster_fallback(PdfRasterFallback::Forbid);
/// assert_eq!(options.raster_fallback(), PdfRasterFallback::Forbid);
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PdfOptions {
    raster_fallback: PdfRasterFallback,
}

impl PdfOptions {
    /// Selects the isolated raster-fallback policy (default
    /// [`PdfRasterFallback::AllowIsolated`]).
    #[must_use]
    pub const fn with_raster_fallback(mut self, raster_fallback: PdfRasterFallback) -> Self {
        self.raster_fallback = raster_fallback;
        self
    }

    /// Returns the isolated raster-fallback policy.
    pub const fn raster_fallback(&self) -> PdfRasterFallback {
        self.raster_fallback
    }
}

/// One owned caller-supplied asset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupplementalAsset {
    bytes: Vec<u8>,
    advisory_media_type: Option<String>,
}

impl SupplementalAsset {
    /// Returns the owned payload bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the caller's advisory media type, when supplied.
    pub fn advisory_media_type(&self) -> Option<&str> {
        self.advisory_media_type.as_deref()
    }
}

/// Owned caller-supplied assets keyed by scene file ID.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SupplementalAssets {
    entries: BTreeMap<String, SupplementalAsset>,
}

impl SupplementalAssets {
    /// Starts an empty supplemental-assets builder.
    pub fn builder() -> SupplementalAssetsBuilder {
        SupplementalAssetsBuilder::default()
    }

    /// Returns whether the collection contains no supplemental assets.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the number of supplemental scene file IDs.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns the supplemental asset for a scene file ID.
    pub fn get(&self, file_id: &str) -> Option<&SupplementalAsset> {
        self.entries.get(file_id)
    }

    /// Iterates assets in ascending scene file-ID UTF-8 byte order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &SupplementalAsset)> {
        self.entries
            .iter()
            .map(|(file_id, asset)| (file_id.as_str(), asset))
    }
}

/// Builder for an owned deterministic supplemental-asset mapping.
#[must_use]
#[derive(Clone, Debug, Default)]
pub struct SupplementalAssetsBuilder {
    entries: Vec<(String, SupplementalAsset)>,
}

impl SupplementalAssetsBuilder {
    /// Adds owned bytes without an advisory media type.
    pub fn asset(mut self, file_id: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        self.entries.push((
            file_id.into(),
            SupplementalAsset {
                bytes: bytes.into(),
                advisory_media_type: None,
            },
        ));
        self
    }

    /// Adds owned bytes with an advisory media type.
    pub fn asset_with_media_type(
        mut self,
        file_id: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
        media_type: impl Into<String>,
    ) -> Self {
        self.entries.push((
            file_id.into(),
            SupplementalAsset {
                bytes: bytes.into(),
                advisory_media_type: Some(media_type.into()),
            },
        ));
        self
    }

    /// Validates and constructs the deterministic asset mapping.
    ///
    /// # Errors
    ///
    /// Returns a [`RequestError`] naming the first offending asset when a
    /// scene file ID is empty (`request.empty-asset-id`) or added more than
    /// once (`request.duplicate-asset-id`), or when an advisory media type is
    /// malformed (`request.invalid-asset-media-type`).
    pub fn build(self) -> Result<SupplementalAssets, RequestError> {
        let mut entries = BTreeMap::new();
        for (file_id, asset) in self.entries {
            if file_id.is_empty() {
                return Err(RequestError::for_asset(
                    ErrorCode::REQUEST_EMPTY_ASSET_ID,
                    "supplemental asset ID must not be empty",
                    file_id,
                    None,
                ));
            }
            if entries.contains_key(&file_id) {
                return Err(RequestError::for_asset(
                    ErrorCode::REQUEST_DUPLICATE_ASSET_ID,
                    format!("supplemental asset ID {file_id:?} occurs more than once"),
                    file_id,
                    None,
                ));
            }
            if let Some(media_type) = asset
                .advisory_media_type
                .as_deref()
                .filter(|media_type| !is_valid_media_type(media_type))
            {
                return Err(RequestError::for_asset(
                    ErrorCode::REQUEST_INVALID_ASSET_MEDIA_TYPE,
                    format!("supplemental asset {file_id:?} media type {media_type:?} is invalid"),
                    file_id,
                    Some("media_type"),
                ));
            }
            entries.insert(file_id, asset);
        }
        Ok(SupplementalAssets { entries })
    }
}

/// A validated one-format render request.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderRequest {
    format: OutputFormat,
    include_background: bool,
    padding: f64,
    sizing: Sizing,
    color_mode: ColorMode,
    frame_options: FrameOptions,
    metadata_options: MetadataOptions,
    supplemental_assets: SupplementalAssets,
    resource_limits: ResourceLimits,
    pdf_options: Option<PdfOptions>,
}

impl RenderRequest {
    /// Starts a validated request builder for exactly one output format.
    pub fn builder(format: OutputFormat) -> RenderRequestBuilder {
        RenderRequestBuilder {
            request: Self {
                format,
                include_background: true,
                padding: 10.0,
                sizing: Sizing::default(),
                color_mode: ColorMode::Light,
                frame_options: FrameOptions::default(),
                metadata_options: MetadataOptions::default(),
                supplemental_assets: SupplementalAssets::default(),
                resource_limits: ResourceLimits::default(),
                pdf_options: (format == OutputFormat::Pdf).then(PdfOptions::default),
            },
            pdf_options_supplied: false,
        }
    }

    /// Returns the single selected output format.
    pub const fn format(&self) -> OutputFormat {
        self.format
    }

    /// Returns whether the scene background is included.
    pub const fn include_background(&self) -> bool {
        self.include_background
    }

    /// Returns the logical padding on each side of the scene.
    pub const fn padding(&self) -> f64 {
        self.padding
    }

    /// Returns the selected output sizing behavior.
    pub const fn sizing(&self) -> Sizing {
        self.sizing
    }

    /// Returns the selected color mode.
    pub const fn color_mode(&self) -> ColorMode {
        self.color_mode
    }

    /// How the dark-mode filter is lowered into painted colors. The
    /// reference's SVG carries `THEME_FILTER` on its root and filters the
    /// composited image, which no per-color lowering reproduces at antialiased
    /// pixels, so SVG carries the same root filter and lowers nothing. PNG
    /// lowers as the canvas floors and PDF as the official SVG rounds (spec
    /// section 8.1).
    pub(crate) fn color_lowering(&self) -> ColorLowering {
        match (self.format(), self.color_mode()) {
            (OutputFormat::Svg, _) | (_, ColorMode::Light) => ColorLowering::None,
            (OutputFormat::Png, ColorMode::Dark) => ColorLowering::Floor,
            (OutputFormat::Pdf, ColorMode::Dark) => ColorLowering::Round,
        }
    }

    /// Returns the frame controls.
    pub const fn frame_options(&self) -> &FrameOptions {
        &self.frame_options
    }

    /// Returns the optional metadata controls.
    pub const fn metadata_options(&self) -> &MetadataOptions {
        &self.metadata_options
    }

    /// Returns the owned supplemental assets.
    pub const fn supplemental_assets(&self) -> &SupplementalAssets {
        &self.supplemental_assets
    }

    /// Returns the semantic resource ceilings.
    pub const fn resource_limits(&self) -> &ResourceLimits {
        &self.resource_limits
    }

    /// Returns PDF-specific options for a PDF request.
    pub const fn pdf_options(&self) -> Option<&PdfOptions> {
        self.pdf_options.as_ref()
    }
}

/// Builder for a validated one-format render request.
#[must_use]
#[derive(Clone, Debug)]
pub struct RenderRequestBuilder {
    request: RenderRequest,
    pdf_options_supplied: bool,
}

/// A validated one-format frame render request.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameRenderRequest {
    inner: RenderRequest,
}

impl FrameRenderRequest {
    /// Starts a validated frame request builder for exactly one output format.
    pub fn builder(format: OutputFormat) -> FrameRenderRequestBuilder {
        FrameRenderRequestBuilder {
            inner: RenderRequest::builder(format).padding(0.0).frame_options(
                FrameOptions::default()
                    .with_show_name(false)
                    .with_outline(false),
            ),
        }
    }

    /// Returns the single selected output format.
    pub const fn format(&self) -> OutputFormat {
        self.inner.format()
    }

    /// Returns whether the scene background is included.
    pub const fn include_background(&self) -> bool {
        self.inner.include_background()
    }

    /// Returns the selected output sizing behavior.
    pub const fn sizing(&self) -> Sizing {
        self.inner.sizing()
    }

    /// Returns the selected color mode.
    pub const fn color_mode(&self) -> ColorMode {
        self.inner.color_mode()
    }

    /// Returns the optional metadata controls.
    pub const fn metadata_options(&self) -> &MetadataOptions {
        self.inner.metadata_options()
    }

    /// Returns the owned supplemental assets.
    pub const fn supplemental_assets(&self) -> &SupplementalAssets {
        self.inner.supplemental_assets()
    }

    /// Returns the semantic resource ceilings applied to this frame.
    pub const fn resource_limits(&self) -> &ResourceLimits {
        self.inner.resource_limits()
    }

    /// Returns PDF-specific options for a PDF request.
    pub const fn pdf_options(&self) -> Option<&PdfOptions> {
        self.inner.pdf_options()
    }

    pub(crate) const fn as_render_request(&self) -> &RenderRequest {
        &self.inner
    }
}

/// Builder for a validated one-format frame render request.
#[must_use]
#[derive(Clone, Debug)]
pub struct FrameRenderRequestBuilder {
    inner: RenderRequestBuilder,
}

impl FrameRenderRequestBuilder {
    /// Selects whether the scene background is included.
    pub fn include_background(mut self, include: bool) -> Self {
        self.inner = self.inner.include_background(include);
        self
    }

    /// Selects explicit scale or downscale-only maximum-dimension sizing.
    pub fn sizing(mut self, sizing: Sizing) -> Self {
        self.inner = self.inner.sizing(sizing);
        self
    }

    /// Selects light or dark color interpretation.
    pub fn color_mode(mut self, color_mode: ColorMode) -> Self {
        self.inner = self.inner.color_mode(color_mode);
        self
    }

    /// Replaces all optional artifact metadata controls.
    pub fn metadata_options(mut self, options: MetadataOptions) -> Self {
        self.inner = self.inner.metadata_options(options);
        self
    }

    /// Replaces all owned supplemental assets.
    pub fn supplemental_assets(mut self, assets: SupplementalAssets) -> Self {
        self.inner = self.inner.supplemental_assets(assets);
        self
    }

    /// Replaces the validated semantic resource ceilings.
    pub fn resource_limits(mut self, limits: ResourceLimits) -> Self {
        self.inner = self.inner.resource_limits(limits);
        self
    }

    /// Replaces PDF-specific controls.
    pub fn pdf_options(mut self, options: PdfOptions) -> Self {
        self.inner = self.inner.pdf_options(options);
        self
    }

    /// Validates and constructs the frame request.
    ///
    /// # Errors
    ///
    /// Returns a [`RequestError`] for the same disabled format, invalid
    /// sizing and format options as [`RenderRequestBuilder::build`].
    pub fn build(self) -> Result<FrameRenderRequest, RequestError> {
        self.inner.build().map(|inner| FrameRenderRequest { inner })
    }
}

impl RenderRequestBuilder {
    /// Selects whether the scene background is included.
    pub fn include_background(mut self, include: bool) -> Self {
        self.request.include_background = include;
        self
    }

    /// Selects logical padding on each side of the scene.
    pub fn padding(mut self, padding: f64) -> Self {
        self.request.padding = padding;
        self
    }

    /// Selects explicit scale or downscale-only maximum-dimension sizing.
    pub fn sizing(mut self, sizing: Sizing) -> Self {
        self.request.sizing = sizing;
        self
    }

    /// Selects light or dark color interpretation.
    pub fn color_mode(mut self, color_mode: ColorMode) -> Self {
        self.request.color_mode = color_mode;
        self
    }

    /// Replaces all frame controls.
    pub fn frame_options(mut self, options: FrameOptions) -> Self {
        self.request.frame_options = options;
        self
    }

    /// Replaces all optional artifact metadata controls.
    pub fn metadata_options(mut self, options: MetadataOptions) -> Self {
        self.request.metadata_options = options;
        self
    }

    /// Replaces all owned supplemental assets.
    pub fn supplemental_assets(mut self, assets: SupplementalAssets) -> Self {
        self.request.supplemental_assets = assets;
        self
    }

    /// Replaces the validated semantic resource ceilings.
    pub fn resource_limits(mut self, limits: ResourceLimits) -> Self {
        self.request.resource_limits = limits;
        self
    }

    /// Replaces PDF-specific controls.
    pub fn pdf_options(mut self, options: PdfOptions) -> Self {
        self.request.pdf_options = Some(options);
        self.pdf_options_supplied = true;
        self
    }

    /// Validates and constructs the request.
    ///
    /// # Errors
    ///
    /// Returns a [`RequestError`] when the output format's Cargo feature is
    /// not enabled in this build (`request.format-unavailable`; see the
    /// [crate documentation](crate#cargo-features)), the padding is negative
    /// or not finite (`request.invalid-padding`), the [`Sizing`] scale or
    /// maximum dimension is not finite and positive (`request.invalid-scale`,
    /// `request.invalid-max-dimension`), or [`PdfOptions`] were supplied for
    /// another output format (`request.incompatible-format-options`).
    pub fn build(self) -> Result<RenderRequest, RequestError> {
        self.request.format.require_enabled()?;

        if !self.request.padding.is_finite() || self.request.padding < 0.0 {
            return Err(RequestError::new(
                ErrorCode::REQUEST_INVALID_PADDING,
                format!(
                    "padding must be finite and nonnegative, got {}",
                    self.request.padding
                ),
                "padding",
            ));
        }

        match self.request.sizing {
            Sizing::Scale(scale) if !scale.is_finite() || scale <= 0.0 => {
                return Err(RequestError::new(
                    ErrorCode::REQUEST_INVALID_SCALE,
                    format!("scale must be finite and positive, got {scale}"),
                    "scale",
                ));
            }
            Sizing::MaxDimension(dimension) if !dimension.is_finite() || dimension <= 0.0 => {
                return Err(RequestError::new(
                    ErrorCode::REQUEST_INVALID_MAX_DIMENSION,
                    format!("maximum dimension must be finite and positive, got {dimension}"),
                    "max_dimension",
                ));
            }
            Sizing::Scale(_) | Sizing::MaxDimension(_) => {}
        }

        if self.pdf_options_supplied && self.request.format != OutputFormat::Pdf {
            return Err(RequestError::for_option(
                ErrorCode::REQUEST_INCOMPATIBLE_FORMAT_OPTIONS,
                format!(
                    "PDF options require PDF output, but the request format is {}",
                    self.request.format.as_str()
                ),
                "pdf_options",
                "raster_fallback",
            ));
        }

        Ok(self.request)
    }
}

fn is_valid_media_type(value: &str) -> bool {
    let mut parts = value.split(';');
    let Some(essence) = parts.next() else {
        return false;
    };
    let Some((type_name, subtype_name)) = essence.split_once('/') else {
        return false;
    };
    if subtype_name.contains('/') || !is_token(type_name) || !is_token(subtype_name) {
        return false;
    }

    parts.all(|parameter| {
        let parameter = parameter.trim_matches([' ', '\t']);
        let Some((name, value)) = parameter.split_once('=') else {
            return false;
        };
        is_token(name.trim_end_matches([' ', '\t']))
            && valid_parameter_value(value.trim_start_matches([' ', '\t']))
    })
}

fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn valid_parameter_value(value: &str) -> bool {
    if is_token(value) {
        return true;
    }
    let Some(quoted) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return false;
    };

    let mut escaped = false;
    for byte in quoted.bytes() {
        if escaped {
            if !(b'\t'..=b'~').contains(&byte) {
                return false;
            }
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' || !(byte == b'\t' || (b' '..=b'~').contains(&byte)) {
            return false;
        }
    }
    !escaped
}
