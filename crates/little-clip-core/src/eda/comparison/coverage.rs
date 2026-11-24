//! Maximum-coverage pyramid for bounded circular queries over a composed mask.

use crate::eda::check_cancelled;
use std::sync::atomic::AtomicBool;
use tiny_skia::Pixmap;

pub(super) struct CoverageIndex {
    levels: Vec<Level>,
}

struct Level {
    size: [usize; 2],
    maxima: Vec<u8>,
}

impl CoverageIndex {
    pub(super) fn new(mask: &Pixmap, cancel: &AtomicBool) -> Result<Self, String> {
        let size = [mask.width() as usize, mask.height() as usize];
        let mut levels = vec![Level {
            size,
            maxima: mask.pixels().iter().map(|pixel| pixel.alpha()).collect(),
        }];

        while levels.last().unwrap().size != [1, 1] {
            check_cancelled(cancel, "Fabrication comparison cancelled")?;
            let previous = levels.last().unwrap();
            let size = previous.size.map(|side| side.div_ceil(2));
            let mut maxima = Vec::with_capacity(size[0] * size[1]);
            for y in 0..size[1] {
                for x in 0..size[0] {
                    let children = children(x, y);
                    let max = children
                        .into_iter()
                        .map(|[cx, cy]| previous.at(cx, cy))
                        .max()
                        .unwrap();
                    maxima.push(max);
                }
            }
            levels.push(Level { size, maxima });
        }
        Ok(Self { levels })
    }

    pub(super) fn nearby(&self, x: usize, y: usize, radius: f64, coverage: u8) -> bool {
        self.search(
            self.levels.len() - 1,
            [0, 0],
            [x, y],
            radius * radius,
            coverage,
        )
    }

    fn search(
        &self,
        level: usize,
        [x, y]: [usize; 2],
        [cx, cy]: [usize; 2],
        radius_squared: f64,
        coverage: u8,
    ) -> bool {
        if self.levels[level].at(x, y) < coverage {
            return false;
        }
        let side = 1 << level;
        let min = [x * side, y * side];
        let max = min.map(|edge| edge + side - 1);
        let dx = cx.abs_diff(cx.clamp(min[0], max[0]));
        let dy = cy.abs_diff(cy.clamp(min[1], max[1]));
        if (dx * dx + dy * dy) as f64 > radius_squared {
            return false;
        }
        let dx = cx.abs_diff(min[0]).max(cx.abs_diff(max[0]));
        let dy = cy.abs_diff(min[1]).max(cy.abs_diff(max[1]));
        if (dx * dx + dy * dy) as f64 <= radius_squared {
            return true;
        }
        children(x, y)
            .into_iter()
            .any(|p| self.search(level - 1, p, [cx, cy], radius_squared, coverage))
    }
}

impl Level {
    fn at(&self, x: usize, y: usize) -> u8 {
        if x < self.size[0] && y < self.size[1] {
            self.maxima[y * self.size[0] + x]
        } else {
            0
        }
    }
}

fn children(x: usize, y: usize) -> [[usize; 2]; 4] {
    let [x, y] = [x * 2, y * 2];
    [[x, y], [x + 1, y], [x, y + 1], [x + 1, y + 1]]
}
