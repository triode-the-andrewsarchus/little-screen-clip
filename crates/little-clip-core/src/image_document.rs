//! Read-only source information for layered image documents.
/// One source layer or group, in display order.
#[derive(Clone, Debug)]
pub struct Layer {
    /// Original Unicode name.
    pub name: String,
    /// Top, left, bottom and right in original document coordinates, when known.
    pub bounds: Option<[i32; 4]>,
    /// Source offset when extent is unavailable.
    pub offset: Option<(i32, i32)>,
    /// Original node type, when supplied separately.
    pub kind: Option<String>,
    /// Layer opacity, from zero to 255.
    pub opacity: Option<u8>,
    /// Source visibility flag.
    pub visible: Option<bool>,
    /// Original blend-mode identifier.
    pub blend_mode: String,
    /// Nesting level within groups.
    pub depth: usize,
    /// Whether this entry begins a group.
    pub group: bool,
    /// Presence of a raster mask.
    pub mask: bool,
    /// Presence of a vector mask.
    pub vector_mask: bool,
}

impl Layer {
    /// Returns the standard blend-mode name, retaining unknown source keys.
    pub fn blend_name(&self) -> &str {
        [
            ("pass", "Pass through"),
            ("norm", "Normal"),
            ("normal", "Normal"),
            ("diss", "Dissolve"),
            ("dark", "Darken"),
            ("mul ", "Multiply"),
            ("multiply", "Multiply"),
            ("idiv", "Color burn"),
            ("lbrn", "Linear burn"),
            ("dkCl", "Darker color"),
            ("lite", "Lighten"),
            ("scrn", "Screen"),
            ("div ", "Color dodge"),
            ("lddg", "Linear dodge"),
            ("lgCl", "Lighter color"),
            ("over", "Overlay"),
            ("sLit", "Soft light"),
            ("hLit", "Hard light"),
            ("vLit", "Vivid light"),
            ("lLit", "Linear light"),
            ("pLit", "Pin light"),
            ("hMix", "Hard mix"),
            ("diff", "Difference"),
            ("smud", "Exclusion"),
            ("fsub", "Subtract"),
            ("fdiv", "Divide"),
            ("hue ", "Hue"),
            ("sat ", "Saturation"),
            ("colr", "Color"),
            ("lum ", "Luminosity"),
        ]
        .into_iter()
        .find_map(|(key, name)| (key == self.blend_mode).then_some(name))
        .unwrap_or(self.blend_mode.trim())
    }
}

/// Original document information; contains no layer pixel buffers.
#[derive(Clone, Debug)]
pub struct Summary {
    /// Width and height in pixels.
    pub dimensions: (u32, u32),
    /// Original bits per channel, when known.
    pub depth: Option<i32>,
    /// Original color profile name.
    pub profile: Option<String>,
    /// Original color mode.
    pub color_mode: String,
    /// Layers and groups, with group-end markers omitted.
    pub layers: Vec<Layer>,
    /// Number of saved paths.
    pub paths: usize,
}
