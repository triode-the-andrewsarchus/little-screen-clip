//! Physical-pixel windows into a uniformly displayed document page.

/// The full page's virtual raster size and a bounded output window within it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRegion {
    /// Virtual page width and height at the selected resolution.
    pub page: [u32; 2],
    /// Top-left pixel of the window, measured from the rendered page's top left.
    pub origin: [u32; 2],
    /// Output width and height in pixels.
    pub size: [u32; 2],
}

impl PageRegion {
    /// Largest supported dimension of the virtual page.
    pub const MAX_PAGE_SIDE: u32 = 131_072;
    /// Largest supported output dimension.
    pub const MAX_SIDE: u32 = 4096;
    /// Largest supported output area in pixels.
    pub const MAX_PIXELS: u32 = 8 * 1024 * 1024;

    pub(crate) fn validate(self) -> Result<(), String> {
        for axis in 0..2 {
            if self.page[axis] > Self::MAX_PAGE_SIDE
                || !(1..=Self::MAX_SIDE).contains(&self.size[axis])
                || self.origin[axis].saturating_add(self.size[axis]) > self.page[axis]
            {
                return Err("Document detail is outside the page or raster budget".into());
            }
        }
        if self.size[0] * self.size[1] > Self::MAX_PIXELS {
            return Err("Document detail exceeds the pixel budget".into());
        }
        Ok(())
    }
}
