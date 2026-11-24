//! Character-level CRDT reconstruction and default tablet text metrics.
use super::text::{InlineFormat, ParagraphStyle, RootText, TextContent};
use crate::{CrdtId, ParseError};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
};

#[derive(Debug, Clone)]
pub struct LayoutParagraph {
    pub style: ParagraphStyle,
    pub spans: Vec<TextSpan>,
    pub baseline: f32,
}

#[derive(Debug, Clone, Default)]
pub struct TextSpan {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Debug, Default)]
pub struct TextLayout {
    pub paragraphs: Vec<LayoutParagraph>,
    pub anchors: HashMap<CrdtId, f32>,
    pub bottom: f32,
}

struct Character {
    id: CrdtId,
    left: CrdtId,
    right: CrdtId,
    value: Option<TextContent>,
}

/// A whole deleted interval is one graph node. References inside it resolve to
/// that node, avoiding allocations proportional to an untrusted deleted length.
struct Tombstone {
    start: CrdtId,
    length: u64,
    index: usize,
}

fn characters(text: &RootText) -> Result<(Vec<Character>, Vec<Tombstone>), ParseError> {
    let mut chars = Vec::new();
    let mut tombstones = Vec::new();
    for item in &text.items {
        if item.deleted_length > 0 {
            tombstones.push(Tombstone {
                start: item.item_id,
                length: u64::from(item.deleted_length),
                index: chars.len(),
            });
            chars.push(Character {
                id: item.item_id,
                left: item.left_id,
                right: item.right_id,
                value: None,
            });
            continue;
        }
        let values = match &item.content {
            Some(TextContent::String(s)) if !s.is_empty() => s
                .chars()
                .map(|c| Some(TextContent::String(c.to_string())))
                .collect(),
            value => vec![value.clone()],
        };
        for (offset, value) in values.iter().enumerate() {
            let seq = item
                .item_id
                .seq
                .checked_add(u64::try_from(offset).map_err(|_| ParseError::VarUIntOverflow)?)
                .ok_or(ParseError::VarUIntOverflow)?;
            let id = CrdtId {
                author: item.item_id.author,
                seq,
            };
            let left = if offset == 0 {
                item.left_id
            } else {
                CrdtId { seq: seq - 1, ..id }
            };
            let right = if offset + 1 == values.len() {
                item.right_id
            } else {
                CrdtId {
                    seq: seq.checked_add(1).ok_or(ParseError::VarUIntOverflow)?,
                    ..id
                }
            };
            chars.push(Character {
                id,
                left,
                right,
                value: value.clone(),
            });
        }
    }
    Ok((chars, tombstones))
}

fn ordered_characters(text: &RootText) -> Result<Vec<Character>, ParseError> {
    let (chars, tombstones) = characters(text)?;
    let mut index = HashMap::new();
    for (i, c) in chars.iter().enumerate() {
        if index.insert(c.id, i).is_some() {
            return Err(ParseError::InvalidText(format!(
                "duplicate character ID {:?}",
                c.id
            )));
        }
    }
    let resolve = |id: &CrdtId| {
        index.get(id).copied().or_else(|| {
            tombstones.iter().find_map(|t| {
                (id.author == t.start.author
                    && id.seq >= t.start.seq
                    && id.seq - t.start.seq < t.length)
                    .then_some(t.index)
            })
        })
    };
    let mut edges = vec![Vec::new(); chars.len()];
    let mut degrees = vec![0; chars.len()];
    for (i, c) in chars.iter().enumerate() {
        for (from, to) in [(resolve(&c.left), Some(i)), (Some(i), resolve(&c.right))] {
            if let (Some(from), Some(to)) = (from, to)
                && from != to
                && !edges[from].contains(&to)
            {
                edges[from].push(to);
                degrees[to] += 1;
            }
        }
    }
    // Concurrent insertions sort by descending author, then ascending sequence.
    let mut ready = BinaryHeap::new();
    for (i, c) in chars.iter().enumerate() {
        if degrees[i] == 0 {
            ready.push((c.id.author, Reverse(c.id.seq), i));
        }
    }
    let mut order = Vec::new();
    while let Some((_, _, i)) = ready.pop() {
        order.push(i);
        for &next in &edges[i] {
            degrees[next] -= 1;
            if degrees[next] == 0 {
                ready.push((chars[next].id.author, Reverse(chars[next].id.seq), next));
            }
        }
    }
    if order.len() != chars.len() {
        return Err(ParseError::InvalidText(format!(
            "cyclic character references: resolved {} of {} items",
            order.len(),
            chars.len()
        )));
    }
    let mut chars: Vec<_> = chars.into_iter().map(Some).collect();
    Ok(order
        .into_iter()
        .map(|i| chars[i].take().unwrap())
        .collect())
}

fn line_height(style: ParagraphStyle) -> f32 {
    if style == ParagraphStyle::Heading {
        150.0
    } else {
        70.0
    }
}

impl RootText {
    /// Resolve edits before paragraph layout. Anchors refer to the beginning of
    /// a paragraph; text baselines are one line height below those anchors.
    /// Metrics are defaults for notebooks without external `.content` overrides.
    #[allow(clippy::cast_possible_truncation)]
    pub fn layout(&self) -> Result<TextLayout, ParseError> {
        let chars = ordered_characters(self)?;
        let style_for = |id| {
            self.paragraphs
                .iter()
                .filter(|p| p.char_id == id)
                .max_by_key(|p| p.timestamp)
                .map_or(ParagraphStyle::Plain, |p| p.style)
        };
        let mut result = TextLayout::default();
        let mut y = self.pos_y as f32 - 88.0;
        result.anchors.insert(
            CrdtId {
                author: 0,
                seq: 0xffff_ffff_fffe,
            },
            y,
        );
        let mut para = LayoutParagraph {
            style: style_for(CrdtId::SENTINEL),
            spans: Vec::new(),
            baseline: y,
        };
        let mut span = TextSpan::default();
        for c in chars {
            let Some(value) = c.value else {
                continue;
            };
            if matches!(&value,TextContent::String(s) if s=="\n") {
                let height = line_height(para.style);
                para.baseline = y + height;
                para.spans.push(span);
                result.paragraphs.push(para);
                y += height;
                para = LayoutParagraph {
                    style: style_for(c.id),
                    spans: Vec::new(),
                    baseline: y,
                };
                span = TextSpan::default();
            } else {
                match value {
                    TextContent::String(s) => span.text.push_str(&s),
                    TextContent::Format(format) => {
                        let mut next = TextSpan {
                            bold: span.bold,
                            italic: span.italic,
                            ..TextSpan::default()
                        };
                        match format {
                            InlineFormat::BoldStart => next.bold = true,
                            InlineFormat::BoldEnd => next.bold = false,
                            InlineFormat::ItalicStart => next.italic = true,
                            InlineFormat::ItalicEnd => next.italic = false,
                        }
                        para.spans.push(span);
                        span = next;
                    }
                }
            }
            result.anchors.insert(c.id, y);
        }
        para.baseline = y + line_height(para.style);
        result.bottom = para.baseline;
        para.spans.push(span);
        result.paragraphs.push(para);
        result.anchors.insert(
            CrdtId {
                author: 0,
                seq: 0xffff_ffff_ffff,
            },
            result.bottom,
        );
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::text::TextItem;

    fn text(items: Vec<TextItem>) -> RootText {
        RootText {
            items,
            paragraphs: Vec::new(),
            pos_x: -468.0,
            pos_y: 234.0,
            width: 936.0,
        }
    }

    fn item(seq: u64, left: CrdtId, right: CrdtId, value: &str) -> TextItem {
        TextItem {
            item_id: CrdtId { author: 1, seq },
            left_id: left,
            right_id: right,
            deleted_length: 0,
            content: Some(TextContent::String(value.into())),
        }
    }

    #[test]
    fn cyclic_text_is_rejected_instead_of_silently_disappearing() {
        let a = CrdtId { author: 1, seq: 1 };
        let b = CrdtId { author: 1, seq: 2 };
        let error = text(vec![
            item(1, b, CrdtId::SENTINEL, "A"),
            item(2, a, CrdtId::SENTINEL, "B"),
        ])
        .layout()
        .unwrap_err();
        assert!(error.to_string().contains("resolved 0 of 2"), "got {error}");
    }

    #[test]
    fn references_inside_large_tombstones_do_not_allocate_deleted_characters() {
        let deleted = TextItem {
            item_id: CrdtId { author: 1, seq: 10 },
            left_id: CrdtId::SENTINEL,
            right_id: CrdtId::SENTINEL,
            deleted_length: u32::MAX,
            content: None,
        };
        let mut survivor = item(
            5,
            CrdtId {
                author: 1,
                seq: 1000,
            },
            CrdtId::SENTINEL,
            "B",
        );
        survivor.item_id.author = 2;
        let text = text(vec![deleted, survivor]);
        let (chars, _) = characters(&text).unwrap();
        assert_eq!(chars.len(), 2);
        let layout = text.layout().unwrap();
        assert_eq!(layout.paragraphs[0].spans[0].text, "B");
    }

    #[test]
    fn character_id_overflow_is_reported() {
        let error = text(vec![item(
            u64::MAX,
            CrdtId::SENTINEL,
            CrdtId::SENTINEL,
            "AB",
        )])
        .layout()
        .unwrap_err();
        assert!(matches!(error, ParseError::VarUIntOverflow), "got {error}");
    }
}
