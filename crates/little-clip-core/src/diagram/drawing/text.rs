//! Bounded text transactions over the preserving document record APIs.
use super::*;

#[derive(Clone, Debug)]
pub struct TextCreation {
    pub id: String,
    pub position: [f64; 2],
    pub content: TextContent,
    pub font: TextStyle,
    pub style: Style,
    pub container: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub family: i32,
    pub size: f64,
    pub line_height: f64,
}
impl Default for TextStyle {
    fn default() -> Self {
        Self {
            family: 5,
            size: 24.0,
            line_height: 1.25,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TextInfo {
    pub id: String,
    pub text: String,
    pub font: TextStyle,
}

impl Scene {
    /// Resolves a text element or the one reciprocal label of a selected box.
    pub fn text_info(&self, id: &str) -> Result<Option<TextInfo>, Error> {
        let item = self.editable_item(id)?;
        let values = elements(&self.document)?;
        let text = if item.kind == Kind::Text {
            Some(item)
        } else {
            let labels: Vec<_> = self
                .items
                .iter()
                .filter(|label| {
                    label.kind == Kind::Text
                        && values[label.index]
                            .get("containerId")
                            .and_then(Value::as_str)
                            == Some(id)
                })
                .collect();
            if labels.len() > 1 {
                return Err(invalid("Multiple bound labels need source editing"));
            }
            labels.first().copied()
        };
        let Some(text) = text else {
            return Ok(None);
        };
        self.text_container(text)?;
        let value = &values[text.index];
        let display = value
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("Text content is invalid"))?;
        let original = value
            .get("originalText")
            .and_then(Value::as_str)
            .unwrap_or(display);
        if display != original {
            return Err(invalid(
                "Wrapped text needs source editing until native reflow is available",
            ));
        }
        Ok(Some(TextInfo {
            id: text.id.clone(),
            text: original.into(),
            font: text_style(value)?,
        }))
    }

    pub(super) fn editable_item(&self, id: &str) -> Result<&Item, Error> {
        let item = self
            .items
            .iter()
            .find(|item| item.id == id)
            .ok_or_else(|| invalid("The element no longer exists"))?;
        if item.locked {
            return Err(invalid("The selected element is locked"));
        }
        Ok(item)
    }

    pub(super) fn text_container(&self, text: &Item) -> Result<Option<&Item>, Error> {
        if text.locked || text.bounds.angle != 0.0 {
            return Err(invalid("Locked or rotated text needs source editing"));
        }
        let values = elements(&self.document)?;
        let Some(id) = values[text.index]
            .get("containerId")
            .filter(|value| !value.is_null())
        else {
            return Ok(None);
        };
        let id = id
            .as_str()
            .ok_or_else(|| invalid("Invalid text container binding"))?;
        let container = self.editable_item(id)?;
        if container.kind != Kind::Rectangle || container.bounds.angle != 0.0 {
            return Err(invalid(
                "This text container needs source editing; native label edits support unrotated rectangles",
            ));
        }
        let reciprocal = values[container.index]
            .get("boundElements")
            .and_then(Value::as_array)
            .is_some_and(|entries| {
                entries
                    .iter()
                    .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("text"))
                    .map(|entry| entry.get("id").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    == [Some(text.id.as_str())]
            });
        if !reciprocal {
            return Err(invalid(
                "Text/container bindings must agree before graphical editing",
            ));
        }
        Ok(Some(container))
    }

    pub(super) fn replace_text(
        &self,
        document: &mut Document,
        id: &str,
        content: TextContent,
    ) -> Result<(), Error> {
        let item = self.editable_item(id)?;
        if item.kind != Kind::Text {
            return Err(invalid("Select a text element or its label"));
        }
        let info = self
            .text_info(id)?
            .ok_or_else(|| invalid("No editable text"))?;
        let [width, height] = validate_content(&content)?;
        if info.text == content.original {
            return Ok(());
        }
        let position = self.replacement_position(item, [width, height])?;
        edit(document, item.index, |shape| {
            shape.set_text_content(content)?;
            for (key, value) in [(element::X, position[0]), (element::Y, position[1])] {
                shape.set(key, Number::from_f64(value)?)?;
            }
            Ok(())
        })
    }

    fn replacement_position(&self, item: &Item, size: [f64; 2]) -> Result<[f64; 2], Error> {
        let value = &elements(&self.document)?[item.index];
        let position = aligned_position(item.bounds, size, value);
        if let Some(container) = self.text_container(item)? {
            fit_label(container.bounds, position, size)?;
        }
        Ok(position)
    }

    pub(super) fn create_text(
        &self,
        document: &mut Document,
        creation: TextCreation,
    ) -> Result<(), Error> {
        let container = self.validate_creation(&creation)?;
        let shape = creation.element()?;
        let mut next = elements(document)?.to_vec();
        next.push(Value::Object(shape.into_object()));
        document.set_root("elements", Value::Array(next))?;
        if let Some(container) = container {
            edit(document, container.index, |shape| {
                let mut bound = shape
                    .as_object()
                    .get("boundElements")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                bound.push(json!({"id":creation.id,"type":"text"}));
                shape.set_raw("boundElements", json!(bound));
                Ok(())
            })?;
        }
        Ok(())
    }

    fn validate_creation(&self, creation: &TextCreation) -> Result<Option<&Item>, Error> {
        let size = validate_content(&creation.content)?;
        validate_font(&creation.font)?;
        let values = elements(&self.document)?;
        if values.len() >= MAX_ELEMENTS
            || creation.id.is_empty()
            || creation.id.len() > 128
            || values
                .iter()
                .any(|value| value.get("id").and_then(Value::as_str) == Some(&creation.id))
        {
            return Err(invalid(
                "New text needs an unused identity and space in the scene",
            ));
        }
        let bounds = Bounds {
            x: creation.position[0],
            y: creation.position[1],
            width: size[0],
            height: size[1],
            angle: 0.0,
        };
        if !bounds.valid() {
            return Err(invalid("Invalid text position"));
        }
        let container = creation
            .container
            .as_deref()
            .map(|id| self.editable_item(id))
            .transpose()?;
        if let Some(container) = container {
            self.validate_new_label(container, creation.position, size)?;
        }
        Ok(container)
    }

    fn validate_new_label(
        &self,
        container: &Item,
        position: [f64; 2],
        size: [f64; 2],
    ) -> Result<(), Error> {
        let values = elements(&self.document)?;
        let value = &values[container.index];
        if value
            .get("boundElements")
            .is_some_and(|bound| !bound.is_null() && !bound.is_array())
        {
            return Err(invalid("Malformed container bindings need source editing"));
        }
        let reverse = value
            .get("boundElements")
            .and_then(Value::as_array)
            .is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.get("type").and_then(Value::as_str) == Some("text"))
            });
        let forward = self.items.iter().any(|text| {
            values[text.index]
                .get("containerId")
                .and_then(Value::as_str)
                == Some(&container.id)
        });
        if container.kind != Kind::Rectangle || container.bounds.angle != 0.0 || reverse || forward
        {
            return Err(invalid(
                "A new label requires an unrotated rectangle without another label",
            ));
        }
        fit_label(container.bounds, position, size)
    }
}

impl TextCreation {
    fn element(&self) -> Result<Element, Error> {
        let mut shape = Element::new(
            Kind::Text,
            Profile::V0_18_1,
            self.id.as_str().into(),
            timestamp().into(),
        )?;
        shape.set_text_content(self.content.clone())?;
        for (key, value) in [
            (element::X, self.position[0]),
            (element::Y, self.position[1]),
            (element::FONT_SIZE, self.font.size),
            (element::LINE_HEIGHT, self.font.line_height),
        ] {
            shape.set(key, Number::from_f64(value)?)?;
        }
        shape.set_raw("fontFamily", json!(self.font.family));
        shape.set_raw("textAlign", json!("left"));
        shape.set_raw("verticalAlign", json!("top"));
        shape.set_raw("autoResize", json!(true));
        if let Some(container) = &self.container {
            shape.set_raw("containerId", json!(container));
        }
        self.style.apply(&mut shape)?;
        Ok(shape)
    }
}

pub(super) fn text_style(value: &Value) -> Result<TextStyle, Error> {
    let font = TextStyle {
        family: value
            .get("fontFamily")
            .map_or(Some(1), Value::as_i64)
            .and_then(|family| i32::try_from(family).ok())
            .ok_or_else(|| invalid("Invalid text font family"))?,
        size: value
            .get("fontSize")
            .map_or(Some(20.0), Value::as_f64)
            .ok_or_else(|| invalid("Invalid font size"))?,
        line_height: value
            .get("lineHeight")
            .map_or(Some(1.25), Value::as_f64)
            .ok_or_else(|| invalid("Invalid line height"))?,
    };
    validate_font(&font)?;
    Ok(font)
}
fn validate_font(font: &TextStyle) -> Result<(), Error> {
    if ![1, 2, 3, 5, 6, 7, 8, 9].contains(&font.family)
        || !font.size.is_finite()
        || !(1.0..=512.0).contains(&font.size)
        || !font.line_height.is_finite()
        || !(0.5..=4.0).contains(&font.line_height)
    {
        return Err(invalid(
            "Text font or line height exceeds native editing limits",
        ));
    }
    Ok(())
}
fn validate_content(content: &TextContent) -> Result<[f64; 2], Error> {
    let width = content.width.as_f64().unwrap_or(f64::NAN);
    let height = content.height.as_f64().unwrap_or(f64::NAN);
    if content.original != content.display
        || content.original.chars().count() > 4096
        || content.original.contains('\0')
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || width > 16_384.0
        || height > 16_384.0
    {
        return Err(invalid(
            "Text needs bounded measured unwrapped content (4096 characters)",
        ));
    }
    Ok([width, height])
}
fn aligned_position(old: Bounds, size: [f64; 2], value: &Value) -> [f64; 2] {
    let factor = |key, center, end| match value.get(key).and_then(Value::as_str) {
        Some(value) if value == end => 1.0,
        Some(value) if value == center => 0.5,
        _ => 0.0,
    };
    [
        old.x + (old.width - size[0]) * factor("textAlign", "center", "right"),
        old.y + (old.height - size[1]) * factor("verticalAlign", "middle", "bottom"),
    ]
}
pub(super) fn fit_label(
    container: Bounds,
    position: [f64; 2],
    size: [f64; 2],
) -> Result<(), Error> {
    if position[0] < container.x
        || position[1] < container.y
        || position[0] + size[0] > container.x + container.width
        || position[1] + size[1] > container.y + container.height
    {
        return Err(invalid(
            "The label does not fit its container; enlarge the box or edit/reflow in source",
        ));
    }
    Ok(())
}
