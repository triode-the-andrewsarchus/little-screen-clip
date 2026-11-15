//! Transparent click regions reference the same guarded path pins as shortcuts.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "StoredRegion")]
pub(in crate::file_manager) struct Region {
    pub id: Uuid,
    pub label: String,
    pub points: Vec<[f64; 2]>,
    pub action: Option<RegionAction>,
    pub enabled: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::file_manager) enum RegionAction {
    Open {
        pin: Uuid,
    },
    Recipe {
        name: String,
        prompt: Option<String>,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRegion {
    id: Uuid,
    label: String,
    points: Vec<[f64; 2]>,
    enabled: bool,
    #[serde(default)]
    target: Option<Uuid>,
    #[serde(default)]
    action: Option<RegionAction>,
}
impl TryFrom<StoredRegion> for Region {
    type Error = String;
    fn try_from(stored: StoredRegion) -> Result<Self, String> {
        if stored.target.is_some() && stored.action.is_some() {
            return Err("Ambiguous Home region action".into());
        }
        Ok(Self {
            id: stored.id,
            label: stored.label,
            points: stored.points,
            enabled: stored.enabled,
            action: stored
                .action
                .or_else(|| stored.target.map(|pin| RegionAction::Open { pin })),
        })
    }
}
impl Region {
    pub fn pin(&self) -> Option<Uuid> {
        match self.action {
            Some(RegionAction::Open { pin }) => Some(pin),
            _ => None,
        }
    }
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
            && self.action.as_ref().is_none_or(|action| match action {
                RegionAction::Open { .. } => true,
                RegionAction::Recipe { name, prompt } => {
                    !name.trim().is_empty()
                        && name.len() <= 256
                        && prompt.as_ref().is_none_or(|value| value.len() <= 4096)
                }
            })
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
                || region.pin().is_some_and(|id| !pins.contains(&id))
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
    pub leave_notice: bool,
}
pub(in crate::file_manager) struct Draft {
    pub region: Region,
    pub original: Option<Region>,
    pub target: Option<super::Pin>,
    pub path: String,
    pub folder: bool,
    pub retrace: bool,
    pub recipe: bool,
    pub recipe_name: String,
    pub prompt: String,
}
impl Draft {
    pub fn from_region(region: Region, pins: &[super::Pin], original: Option<Region>) -> Self {
        let target = region
            .pin()
            .and_then(|id| pins.iter().find(|pin| pin.id == id))
            .cloned();
        let (recipe, recipe_name, prompt) = match &region.action {
            Some(RegionAction::Recipe { name, prompt }) => {
                (true, name.clone(), prompt.clone().unwrap_or_default())
            }
            _ => (false, String::new(), String::new()),
        };
        Self {
            path: target
                .as_ref()
                .map_or_else(String::new, |pin| pin.path.to_string_lossy().into_owned()),
            folder: target.as_ref().is_none_or(|pin| pin.folder),
            region,
            original,
            target,
            retrace: false,
            recipe,
            recipe_name,
            prompt,
        }
    }
    pub fn valid(&self) -> bool {
        self.region.valid()
            && !self.retrace
            && if self.recipe {
                !self.recipe_name.trim().is_empty()
                    && self.recipe_name.len() <= 256
                    && self.prompt.len() <= 4096
            } else {
                self.target
                    .as_ref()
                    .is_some_and(|pin| pin.path.is_absolute())
            }
    }
    pub fn choose(&mut self, pin: super::Pin) {
        self.recipe = false;
        self.path = pin.path.to_string_lossy().into_owned();
        self.folder = pin.folder;
        self.target = Some(pin);
    }
}
impl Interaction {
    pub fn retarget(
        &mut self,
        pins: &[super::Pin],
        replacements: &std::collections::BTreeMap<Uuid, Uuid>,
    ) {
        let Some(draft) = &mut self.draft else { return };
        for region in std::iter::once(&mut draft.region).chain(draft.original.iter_mut()) {
            if let Some(RegionAction::Open { pin }) = &mut region.action
                && let Some(replacement) = replacements.get(pin)
            {
                *pin = *replacement;
            }
        }
        if let Some(old) = &draft.target {
            let id = replacements.get(&old.id).copied().unwrap_or(old.id);
            if let Some(pin) = pins.iter().find(|pin| pin.id == id) {
                draft.path = pin.path.to_string_lossy().into_owned();
                draft.folder = pin.folder;
                draft.target = Some(pin.clone());
            }
        }
    }
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
            self.leave_notice = false;
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
            self.leave_notice = false;
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
    fn declared_reference_merge_updates_draft_without_changing_its_chosen_action_mode() {
        let old = super::super::Pin::new("/old".into(), true);
        let next = super::super::Pin::new("/new".into(), true);
        let region = Region {
            id: Uuid::new_v4(),
            label: "Notes".into(),
            enabled: true,
            points: vec![[0., 0.], [100., 0.], [0., 100.]],
            action: Some(RegionAction::Open { pin: old.id }),
        };
        let mut interaction = Interaction {
            draft: Some(Draft::from_region(
                region.clone(),
                std::slice::from_ref(&old),
                Some(region),
            )),
            ..Default::default()
        };
        interaction.draft.as_mut().unwrap().recipe = true;
        interaction.retarget(std::slice::from_ref(&next), &[(old.id, next.id)].into());
        let draft = interaction.draft.unwrap();
        assert!(draft.recipe);
        assert_eq!(draft.path, "/new");
        assert_eq!(draft.target.unwrap().id, next.id);
        assert_eq!(draft.original.unwrap().pin(), Some(next.id));
        assert_eq!(draft.region.pin(), Some(next.id));
    }
    #[test]
    fn legacy_file_actions_migrate_without_changing_geometry_and_ambiguous_actions_fail() {
        let id = Uuid::new_v4();
        let pin = Uuid::new_v4();
        let legacy = serde_json::json!({"id":id,"label":"Notes","points":[[0,0],[100,0],[0,100]],"target":pin,"enabled":true});
        let region: Region = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(region.pin(), Some(pin));
        assert!(region.valid());
        let saved = serde_json::to_value(&region).unwrap();
        assert!(saved.get("target").is_none());
        assert_eq!(
            region.points,
            serde_json::from_value::<Vec<[f64; 2]>>(legacy["points"].clone()).unwrap()
        );
        let mut ambiguous = legacy;
        ambiguous["action"] = serde_json::json!({"kind":"recipe","name":"Example","prompt":null});
        assert!(serde_json::from_value::<Region>(ambiguous).is_err());
    }
    #[test]
    fn recipe_actions_round_trip_and_do_not_require_a_file_pin() {
        let region = Region {
            id: Uuid::new_v4(),
            label: "Tidy the clipboard".into(),
            points: vec![[0., 0.], [100., 0.], [0., 100.]],
            action: Some(RegionAction::Recipe {
                name: "Clean and Slugify".into(),
                prompt: Some("example".into()),
            }),
            enabled: true,
        };
        let restored: Region =
            serde_json::from_slice(&serde_json::to_vec(&region).unwrap()).unwrap();
        assert_eq!(restored, region);
        assert!(region.pin().is_none());
        validate(&[region], &Default::default()).unwrap();
    }
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
            action: None,
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
            action: None,
            enabled: true,
        };
        assert!(!region.valid());
        region.points[1] = [50., 30.];
        assert!(region.valid());
        region.points[1][0] = f64::NAN;
        assert!(!region.valid());
    }
}
