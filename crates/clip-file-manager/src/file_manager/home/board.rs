//! Home-owned presentation; all file/project content remains outside this model.
use super::Pin;
use little_clip_core::diagram::drawing::{Bounds, Scene};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

const EMPTY_DRAWING: &str = r##"{"type":"excalidraw","version":2,"elements":[],"appState":{"viewBackgroundColor":"#ffffff"},"files":{}}"##;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::file_manager) struct Board {
    pub revision: u64,
    pub title: String,
    pub drawing: String,
    pub cards: Vec<Card>,
    pub bindings: Vec<Binding>,
    #[serde(default)]
    pub view: Option<clip_studios::drawing_canvas::ViewState>,
}
impl Default for Board {
    fn default() -> Self {
        Self {
            revision: 0,
            title: "Home".into(),
            drawing: EMPTY_DRAWING.into(),
            cards: Vec::new(),
            bindings: Vec::new(),
            view: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::file_manager) struct Card {
    pub id: Uuid,
    /// Stores x/y/width/height in the drawing's world space.
    pub rect: [f64; 4],
    pub content: Content,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::file_manager) enum Content {
    Shortcut { pin: Uuid, label: String },
    Section { title: String, color: [u8; 3] },
    Projects { title: String },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::file_manager) struct Binding {
    pub element: String,
    pub pin: Uuid,
}
impl Card {
    pub fn bounds(&self) -> Bounds {
        Bounds {
            x: self.rect[0],
            y: self.rect[1],
            width: self.rect[2],
            height: self.rect[3],
            angle: 0.0,
        }
    }
}
impl Board {
    pub fn is_empty(&self) -> bool {
        self.cards.is_empty() && self.drawing == EMPTY_DRAWING
    }
    pub fn add_shortcut(&mut self, pin: Uuid) {
        if self
            .cards
            .iter()
            .any(|card| matches!(card.content, Content::Shortcut { pin: id, .. } if id == pin))
        {
            return;
        }
        let ordinal = self.cards.len();
        self.cards.push(Card {
            id: Uuid::new_v4(),
            rect: [
                80.0 + (ordinal % 3) as f64 * 300.0,
                150.0 + (ordinal / 3) as f64 * 150.0,
                260.0,
                110.0,
            ],
            content: Content::Shortcut {
                pin,
                label: String::new(),
            },
        });
    }
    pub(super) fn retain_pin_refs(
        &mut self,
        valid: &BTreeSet<Uuid>,
        replacements: &std::collections::BTreeMap<Uuid, Uuid>,
    ) {
        for card in &mut self.cards {
            if let Content::Shortcut { pin, .. } = &mut card.content
                && let Some(replacement) = replacements.get(pin)
            {
                *pin = *replacement;
            }
        }
        for binding in &mut self.bindings {
            if let Some(replacement) = replacements.get(&binding.pin) {
                binding.pin = *replacement;
            }
        }
        self.cards.retain(
            |card| !matches!(card.content,Content::Shortcut {pin,..} if !valid.contains(&pin)),
        );
        self.bindings.retain(|binding| valid.contains(&binding.pin));
    }
    pub fn remove_pin(&mut self, pin: Uuid) {
        self.cards
            .retain(|card| !matches!(card.content, Content::Shortcut { pin: id, .. } if id == pin));
        self.bindings.retain(|binding| binding.pin != pin);
    }
    pub fn retain_artwork_bindings(&mut self) -> Result<(), String> {
        let scene = Scene::parse(&self.drawing).map_err(|error| error.to_string())?;
        let elements: BTreeSet<_> = scene.items.iter().map(|item| item.id.as_str()).collect();
        self.bindings
            .retain(|binding| elements.contains(binding.element.as_str()));
        Ok(())
    }
    pub fn validate(&self, pins: &[Pin]) -> Result<(), String> {
        if self.title.trim().is_empty()
            || self.title.len() > 256
            || self.drawing.len() > 384 * 1024
            || self.cards.len() > 1024
            || self.bindings.len() > 2048
            || self.revision == u64::MAX
            || self.view.is_some_and(|view| !view.valid())
        {
            return Err("Home board exceeds its title, artwork or card budget".into());
        }
        Scene::parse(&self.drawing).map_err(|error| format!("Home drawing: {error}"))?;
        let pin_ids: BTreeSet<_> = pins.iter().map(|pin| pin.id).collect();
        let mut card_ids = BTreeSet::new();
        for card in &self.cards {
            if card.id.is_nil() || !card_ids.insert(card.id) {
                return Err("Invalid or duplicate Home card identity".into());
            }
            card.validate(&pin_ids)?;
        }
        self.validate_bindings(&pin_ids)
    }

    fn validate_bindings(&self, pin_ids: &BTreeSet<Uuid>) -> Result<(), String> {
        let mut elements = BTreeSet::new();
        for binding in &self.bindings {
            if !pin_ids.contains(&binding.pin)
                || binding.element.is_empty()
                || binding.element.len() > 256
                || !elements.insert(&binding.element)
            {
                return Err("Invalid or duplicate Home artwork shortcut".into());
            }
        }
        Ok(())
    }
}

impl Card {
    fn validate(&self, pin_ids: &BTreeSet<Uuid>) -> Result<(), String> {
        if !self.bounds().valid()
            || !(40.0..=8000.0).contains(&self.rect[2])
            || !(30.0..=8000.0).contains(&self.rect[3])
        {
            return Err("Invalid Home card geometry".into());
        }
        match &self.content {
            Content::Shortcut { pin, label } if !pin_ids.contains(pin) || label.len() > 256 => {
                Err("Invalid Home shortcut reference".into())
            }
            Content::Section { title, .. } | Content::Projects { title }
                if title.trim().is_empty() || title.len() > 256 =>
            {
                Err("Invalid Home section title".into())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pin_removal_retires_only_its_presentation_and_shape_shortcuts() {
        let pins = [
            Pin::new("/one".into(), true),
            Pin::new("/two".into(), false),
        ];
        let mut board = Board::default();
        for pin in &pins {
            board.add_shortcut(pin.id);
        }
        board.add_shortcut(pins[0].id);
        assert_eq!(board.cards.len(), 2);
        board.bindings.push(Binding {
            element: "sketch".into(),
            pin: pins[0].id,
        });
        board.cards.push(Card {
            id: Uuid::new_v4(),
            rect: [0.0, 0.0, 900.0, 600.0],
            content: Content::Section {
                title: "Work".into(),
                color: [80, 120, 160],
            },
        });
        board.remove_pin(pins[0].id);
        board.validate(&pins[1..]).unwrap();
        assert_eq!(board.cards.len(), 2);
        assert!(board.bindings.is_empty());
    }
    #[test]
    fn invalid_geometry_payloads_and_orphaned_targets_cannot_enter_durable_state() {
        let pin = Pin::new("/target".into(), false);
        let mut board = Board::default();
        board.add_shortcut(pin.id);
        assert!(board.validate(&[]).is_err());
        board.cards[0].rect[0] = f64::NAN;
        assert!(board.validate(std::slice::from_ref(&pin)).is_err());
        board.cards.clear();
        board.drawing = "broken drawing".into();
        assert!(board.validate(&[]).is_err());
    }
    #[test]
    fn deleting_artwork_clears_its_binding_without_touching_the_pin() {
        let pin = Pin::new("/target".into(), false);
        let mut board = Board::default();
        board.bindings.push(Binding {
            element: "removed".into(),
            pin: pin.id,
        });
        board.retain_artwork_bindings().unwrap();
        assert!(board.bindings.is_empty());
        board.validate(&[pin]).unwrap();
    }
}
