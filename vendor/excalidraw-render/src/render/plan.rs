//! The shape every format plan's elements share. Each backend lowers
//! the resolved scene into its own element list in its own units, but what an
//! element carries besides its kind is the same everywhere: the paint-order
//! contract of spec section 11.4 that every fallback and every ordinary element
//! preserves — frame clipping, transform, opacity and sanitized safe link —
//! plus the element's unrounded size. [`PlannedElement`] is that header over a
//! per-format `Kind`, so what differs per backend is exactly what the kind
//! enums spell.

/// One element of a format plan: what every planned element carries whatever
/// its kind, in the backend's units, and the backend's own `kind`.
///
/// `Transform` is the backend's placement (`SvgTransform` in SVG units,
/// `PngTransform` in device pixels, `ResolvedTransform` for PDF, which
/// projects at write time) and `Clip` its frame clip (`ResolvedClip`, or the
/// PNG plan's device-space projection). `width` and `height` are the element's
/// unrounded size: the link hit rectangle, the flip axis of an image and the
/// bounds of a fallback box. `link` is the sanitized safe link; a backend that
/// carries no links (PNG) still plans it and never writes it.
#[derive(Debug)]
pub(super) struct PlannedElement<Transform, Clip, Kind> {
    pub(super) frame_clips: Vec<Clip>,
    pub(super) transform: Transform,
    pub(super) opacity: f64,
    pub(super) link: Option<String>,
    pub(super) width: f64,
    pub(super) height: f64,
    pub(super) kind: Kind,
}
