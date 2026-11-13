//! Worker-prepared source-position excerpts using the shared native Markdown view.
use super::{MarkdownBlock, Previews, prepare_blocks, render_blocks};
use little_clip_render::markdown::{LocatedLine, MarkdownLine, parse_located};
use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Immutable native blocks near one original source line.
pub struct MarkdownExcerptData {
    blocks: Vec<MarkdownBlock>,
    /// Inclusive one-based original line numbers covered by this excerpt.
    pub source_lines: std::ops::RangeInclusive<usize>,
}
impl MarkdownExcerptData {
    /// Prepares an excerpt on a worker, keeping complete native diagram fences.
    ///
    /// # Errors
    ///
    /// Returns an error for cancellation, oversized input/excerpts, or an
    /// incomplete fence/equation. The host can retain its source-text excerpt.
    pub fn prepare(source: &str, line: usize, cancel: &AtomicBool) -> Result<Arc<Self>, String> {
        if source.len() > 2 * 1024 * 1024 || cancel.load(Ordering::Relaxed) {
            return Err("Markdown excerpt input exceeds the budget or was cancelled".into());
        }
        let mut located = parse_located(source);
        if cancel.load(Ordering::Relaxed) {
            return Err("Markdown excerpt cancelled".into());
        }
        if located.is_empty() {
            return Ok(Arc::new(Self {
                blocks: Vec::new(),
                source_lines: 1..=1,
            }));
        }
        let offset = source
            .split_inclusive('\n')
            .take(line.saturating_sub(1))
            .map(str::len)
            .sum::<usize>()
            .min(source.len().saturating_sub(1));
        let index = located
            .iter()
            .rposition(|item| item.source.contains(&offset))
            .unwrap_or(0);
        let selected = selection(&located, index)?;
        let bytes = located[selected.start].source.start..located[selected.end - 1].source.end;
        validate(&located[selected.clone()], bytes.len())?;
        let first = source[..bytes.start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1;
        let last = source[..bytes.end].lines().count().max(first);
        let blocks = prepare_blocks(located.drain(selected).map(|item| item.line));
        Ok(Arc::new(Self {
            blocks,
            source_lines: first..=last,
        }))
    }
}

// Keep a nearby heading, without putting a large preceding image, equation,
// code block or front-matter table ahead of the position the person selected.
fn context_start(lines: &[LocatedLine], index: usize) -> usize {
    let mut start = index;
    while start > 0 && lines[start - 1].source == lines[index].source {
        start -= 1;
    }
    if start > 0 && matches!(lines[start - 1].line, MarkdownLine::Heading { .. }) {
        start - 1
    } else if start > 1
        && matches!(lines[start - 1].line, MarkdownLine::Empty)
        && matches!(lines[start - 2].line, MarkdownLine::Heading { .. })
    {
        start - 2
    } else {
        start
    }
}

fn selection(lines: &[LocatedLine], index: usize) -> Result<Range<usize>, String> {
    let mut selected = context_start(lines, index)..(index + 9).min(lines.len());
    let mut open = None;
    for (position, item) in lines.iter().enumerate() {
        if !matches!(item.line, MarkdownLine::CodeFence { .. }) {
            continue;
        }
        if let Some(start) = open.take() {
            let fence = start..position + 1;
            if fence.contains(&index) {
                return Ok(fence);
            }
            if fence.contains(&selected.start) {
                selected.start = fence.end;
            }
            if selected.end > start && selected.end < fence.end {
                selected.end = start;
            }
        } else {
            open = Some(position);
        }
    }
    if let Some(start) = open {
        if index >= start {
            return Err("Incomplete Markdown fence; showing source".into());
        }
        selected.end = selected.end.min(start);
    }
    Ok(selected)
}

fn validate(lines: &[LocatedLine], bytes: usize) -> Result<(), String> {
    let format = match &lines[0].line {
        MarkdownLine::CodeFence { language } => {
            little_clip_core::diagram::Format::from_language(language)
        }
        _ => None,
    };
    let metadata_bytes = lines
        .iter()
        .filter_map(|item| {
            matches!(item.line, MarkdownLine::FrontMatter(_))
                .then_some(item.source.len().min(64 * 1024))
        })
        .sum::<usize>();
    let budget = match format {
        Some(little_clip_core::diagram::Format::Excalidraw) => 2 * 1024 * 1024,
        Some(_) => 256 * 1024,
        None => 32 * 1024 + metadata_bytes,
    };
    let mut spans = 0;
    for item in lines {
        spans += match &item.line {
            MarkdownLine::Heading { spans, .. }
            | MarkdownLine::Bullet { spans, .. }
            | MarkdownLine::Numbered { spans, .. }
            | MarkdownLine::Quote { spans }
            | MarkdownLine::Paragraph { spans } => spans.len(),
            MarkdownLine::IncompleteMath { .. } => {
                return Err("Incomplete equation; showing source".into());
            }
            _ => 0,
        };
    }
    if bytes > budget || spans > 128 {
        return Err("Rendered Markdown excerpt exceeds the preview budget".into());
    }
    Ok(())
}

/// Per-hover native rendering state; dropping it retires diagram/math jobs.
pub struct MarkdownExcerptView {
    data: Arc<MarkdownExcerptData>,
    previews: Previews,
}
impl MarkdownExcerptView {
    /// Creates a view over worker-prepared immutable blocks.
    pub fn new(data: Arc<MarkdownExcerptData>) -> Self {
        Self {
            data,
            previews: Previews::new(),
        }
    }
    /// Renders inside a bounded hover area without source parsing or file I/O.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        height: f32,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        if self.data.blocks.is_empty() {
            ui.weak("Empty Markdown file");
            return;
        }
        egui::ScrollArea::vertical()
            .id_source((
                "markdown_source_excerpt",
                self.data.source_lines.start(),
                self.data.source_lines.end(),
            ))
            .min_scrolled_height(0.0)
            .max_height(height.clamp(1.0, 640.0))
            .show(ui, |ui| {
                render_blocks(ui, &self.data.blocks, palette, Some(&mut self.previews))
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static LIVE: AtomicBool = AtomicBool::new(false);

    #[test]
    fn source_position_selects_the_matching_section_instead_of_the_start() {
        let source = format!(
            "# Beginning\n\n{}# Destination\n\n**Rendered** words here.\n",
            "Filler\n\n".repeat(200)
        );
        let data = MarkdownExcerptData::prepare(&source, 403, &LIVE).unwrap();
        assert!(*data.source_lines.start() > 390);
        assert!(data.blocks.iter().any(|block| matches!(block, MarkdownBlock::Line(MarkdownLine::Heading {text,..}) if text == "Destination")));
        assert!(!data.blocks.iter().any(|block| matches!(block, MarkdownBlock::Line(MarkdownLine::Heading {text,..}) if text == "Beginning")));
    }

    #[test]
    fn hovering_inside_a_diagram_or_equation_keeps_its_complete_native_content() {
        let source =
            "# Before\n\n```mermaid\nflowchart LR\n A --> B\n B --> C\n```\n\n$$\nx^2 +\ny^2\n$$\n";
        for line in [3, 4, 5, 6, 7] {
            let data = MarkdownExcerptData::prepare(source, line, &LIVE).unwrap();
            assert_eq!(data.source_lines, 3..=7);
            assert!(
                matches!(&data.blocks[0], MarkdownBlock::Code {language,text} if language == "mermaid" && text.contains("B --> C"))
            );
        }
        let data = MarkdownExcerptData::prepare(source, 10, &LIVE).unwrap();
        assert!(data.blocks.iter().any(|block| matches!(block, MarkdownBlock::Line(MarkdownLine::Paragraph {spans}) if spans.iter().any(|span| matches!(span, little_clip_render::markdown::MarkdownSpan::Math {source, display:true,..} if source.contains("y^2"))))));
    }

    #[test]
    fn unfinished_and_oversized_fragments_fall_back_without_rendering_partial_diagrams() {
        for source in ["```mermaid\nflowchart LR\n A --> B", "$$\nx + y"] {
            assert!(MarkdownExcerptData::prepare(source, 2, &LIVE).is_err());
        }
        assert!(MarkdownExcerptData::prepare(&"x".repeat(32 * 1024 + 1), 1, &LIVE).is_err());
        assert!(MarkdownExcerptData::prepare("# Heading", 1, &AtomicBool::new(true)).is_err());
    }
    #[test]
    fn body_hover_does_not_put_a_frontmatter_table_ahead_of_the_selected_heading() {
        let source = format!(
            "---\n{}---\n# Body\n\nActual content.\n",
            (0..30)
                .map(|i| format!("field{i}: value\n"))
                .collect::<String>()
        );
        let data = MarkdownExcerptData::prepare(&source, 33, &LIVE).unwrap();
        assert_eq!(*data.source_lines.start(), 33);
        assert!(
            matches!(&data.blocks[0], MarkdownBlock::Line(MarkdownLine::Heading {text,..}) if text == "Body")
        );
    }
}
