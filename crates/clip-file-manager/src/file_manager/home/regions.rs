//! Transparent click regions reference the same guarded path pins as shortcuts.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::file_manager) struct Region {
    pub id: Uuid,
    pub label: String,
    pub points: Vec<[f64; 2]>,
    pub target: Option<Uuid>,
    pub enabled: bool,
}
impl Region {
    pub fn valid(&self) -> bool {
        !self.id.is_nil()
            && !self.label.trim().is_empty()
            && self.label.len() <= 256
            && (3..=512).contains(&self.points.len())
            && self
                .points
                .iter()
                .flatten()
                .all(|v| v.is_finite() && v.abs() <= 1e8)
            && self.area() > 0.01
    }
    fn area(&self) -> f64 {
        // Absolute even-odd lobe area is allowed for self-crossing freehand
        // paths. Reject a collapsed straight line without forcing convexity.
        let Some(origin) = self.points.first() else {
            return 0.0;
        };
        self.points
            .windows(2)
            .map(|p| {
                ((p[0][0] - origin[0]) * (p[1][1] - origin[1])
                    - (p[1][0] - origin[0]) * (p[0][1] - origin[1]))
                    .abs()
                    * 0.5
            })
            .sum()
    }
    pub fn contains(&self, point: [f64; 2]) -> bool {
        // Even-odd ray casting preserves concavities and freehand crossings;
        // the bounding rectangle must never become the actual click target.
        let mut inside = false;
        for (a, b) in self
            .points
            .iter()
            .zip(self.points.iter().cycle().skip(1))
            .take(self.points.len())
        {
            if (a[1] > point[1]) != (b[1] > point[1]) {
                let crossing = a[0] + (point[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
                if point[0] < crossing {
                    inside = !inside;
                }
            }
        }
        inside
    }
}
pub(super) fn validate(
    regions: &[Region],
    pins: &std::collections::BTreeSet<Uuid>,
) -> Result<(), String> {
    let mut ids = std::collections::BTreeSet::new();
    if regions.len() > 128
        || regions.iter().any(|region| {
            !region.valid()
                || !ids.insert(region.id)
                || region.target.is_some_and(|id| !pins.contains(&id))
        })
    {
        return Err("Invalid or oversized Home click regions".into());
    }
    Ok(())
}

/// Session-only drawing/picker state; only accepted regions enter Board history.
#[derive(Default)]
pub(in crate::file_manager) struct Interaction {
    pub open: bool,
    pub draft: Option<Draft>,
    pub stroke: Vec<[f64; 2]>,
    pub chooser: Vec<Uuid>,
    pub tracing: bool,
    pub adding: bool,
    pub error: Option<String>,
}
pub(in crate::file_manager) struct Draft {
    pub region: Region,
    pub target: Option<super::Pin>,
    pub path: String,
    pub folder: bool,
    pub retrace: bool,
}
impl Draft {
    pub fn choose(&mut self, pin: super::Pin) {
        self.path = pin.path.to_string_lossy().into_owned();
        self.folder = pin.folder;
        self.target = Some(pin);
    }
}
impl Interaction {
    pub fn drafting(&self) -> bool {
        self.draft.is_some() || self.tracing || self.adding
    }
    pub fn saved(&mut self, id: Uuid) {
        if self
            .draft
            .as_ref()
            .is_some_and(|draft| draft.region.id == id)
        {
            self.draft = None;
        }
    }
    pub fn cancel(&mut self) -> bool {
        if self.tracing || self.adding || self.draft.is_some() || !self.chooser.is_empty() {
            self.tracing = false;
            self.adding = false;
            self.stroke.clear();
            self.draft = None;
            self.chooser.clear();
            self.error = None;
            true
        } else {
            std::mem::take(&mut self.open)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concave_region_does_not_activate_the_empty_corner_of_its_bounds() {
        let region = Region {
            id: Uuid::new_v4(),
            label: "Notes".into(),
            points: vec![
                [0., 0.],
                [100., 0.],
                [100., 30.],
                [30., 30.],
                [30., 100.],
                [0., 100.],
            ],
            target: None,
            enabled: true,
        };
        assert!(region.valid());
        assert!(region.contains([10., 80.]));
        assert!(region.contains([80., 10.]));
        assert!(!region.contains([80., 80.]));
        assert!(!region.contains([-1., 20.]));
    }
    #[test]
    fn invalid_geometry_and_collapsed_paths_are_rejected() {
        let mut region = Region {
            id: Uuid::new_v4(),
            label: "Test".into(),
            points: vec![[0., 0.], [50., 0.], [100., 0.]],
            target: None,
            enabled: true,
        };
        assert!(!region.valid());
        region.points[1] = [50., 30.];
        assert!(region.valid());
        region.points[1][0] = f64::NAN;
        assert!(!region.valid());
    }
}
