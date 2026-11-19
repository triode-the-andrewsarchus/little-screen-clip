//! Worker-prepared source-position excerpts using the shared native Markdown view.
use super::{MarkdownBlock, Previews};
use little_clip_render::markdown::{LocatedBlock, MarkdownDocBlock, parse_located_blocks};
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
    /// Prepares an excerpt on a worker, keeping complete native diagram fences and tables.
    ///
    /// # Errors
    ///
    /// Returns an error for cancellation, oversized input/excerpts, or an
    /// incomplete fence/equation. The host can retain its source-text excerpt.
    pub fn prepare(source: &str, line: usize, cancel: &AtomicBool) -> Result<Arc<Self>, String> {
        if source.len() > 2 * 1024 * 1024 || cancel.load(Ordering::Relaxed) {
            return Err("Markdown excerpt input exceeds the budget or was cancelled".into());
        }
        let mut located = parse_located_blocks(source);
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
            .or_else(|| nearest_block_index(&located, offset))
            .unwrap_or(0);
        let selected = selection(&located, index)?;
        let start =
            source.floor_char_boundary(located[selected.start].source.start.min(source.len()));
        let end = source.floor_char_boundary(
            located[selected.end - 1]
                .source
                .end
                .clamp(start, source.len()),
        );
        validate(&located[selected.clone()], end - start)?;
        let first = source[..start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1;
        let last = source[..end].lines().count().max(first);
        let blocks = located.drain(selected).map(|item| item.block).collect();
        Ok(Arc::new(Self {
            blocks,
            source_lines: first..=last,
        }))
    }
}

fn nearest_block_index(located: &[LocatedBlock], offset: usize) -> Option<usize> {
    if located.is_empty() {
        return None;
    }
    let next = located.iter().position(|item| item.source.start >= offset);
    let prev = located.iter().rposition(|item| item.source.end <= offset);
    match (prev, next) {
        (Some(p), Some(n)) => {
            let dist_prev = offset.saturating_sub(located[p].source.end);
            let dist_next = located[n].source.start.saturating_sub(offset);
            Some(if dist_next <= dist_prev { n } else { p })
        }
        (None, Some(n)) => Some(n),
        (Some(p), None) => Some(p),
        (None, None) => Some(0),
    }
}

// Keep a nearby heading, without putting a large preceding image, equation,
// code block, table or front-matter table ahead of the position the person selected.
fn context_start(blocks: &[LocatedBlock], index: usize) -> usize {
    if index > 0 && matches!(blocks[index - 1].block, MarkdownDocBlock::Heading { .. }) {
        index - 1
    } else {
        index
    }
}

fn selection(blocks: &[LocatedBlock], index: usize) -> Result<Range<usize>, String> {
    if blocks[index].incomplete_fence {
        return Err("Incomplete Markdown fence; showing source".into());
    }
    if matches!(blocks[index].block, MarkdownDocBlock::IncompleteMath { .. }) {
        return Err("Incomplete equation; showing source".into());
    }
    if matches!(
        &blocks[index].block,
        MarkdownDocBlock::Code { language, .. }
            if little_clip_core::diagram::Format::from_language(language).is_some()
    ) || matches!(&blocks[index].block, MarkdownDocBlock::Table(_))
    {
        return Ok(index..index + 1);
    }

    let start = context_start(blocks, index);
    let mut end = (index + 5).min(blocks.len());
    for (offset, item) in blocks[start..end].iter().enumerate() {
        if item.incomplete_fence || matches!(item.block, MarkdownDocBlock::IncompleteMath { .. }) {
            end = (start + offset).max(start + 1);
            break;
        }
    }
    Ok(start..end)
}

fn count_block_spans(block: &MarkdownDocBlock) -> Result<usize, String> {
    Ok(match block {
        MarkdownDocBlock::Heading { spans, .. } | MarkdownDocBlock::Paragraph { spans } => {
            spans.len()
        }
        MarkdownDocBlock::Details {
            summary, blocks, ..
        } => {
            let mut total = summary.len();
            for inner in blocks {
                total += count_block_spans(inner)?;
            }
            total
        }
        MarkdownDocBlock::StyledContainer { blocks, .. }
        | MarkdownDocBlock::Quote { blocks, .. }
        | MarkdownDocBlock::Footnote { blocks, .. } => {
            let mut total = 0usize;
            for inner in blocks {
                total += count_block_spans(inner)?;
            }
            total
        }
        MarkdownDocBlock::PullQuote { source, blocks, .. } => {
            let mut total = source.len();
            for inner in blocks {
                total += count_block_spans(inner)?;
            }
            total
        }
        MarkdownDocBlock::Tabs { tabs } => {
            let mut total = 0usize;
            for tab in tabs {
                for inner in &tab.blocks {
                    total += count_block_spans(inner)?;
                }
            }
            total
        }
        MarkdownDocBlock::Columns { columns, .. } => {
            let mut total = 0usize;
            for col in columns {
                for inner in col {
                    total += count_block_spans(inner)?;
                }
            }
            total
        }
        MarkdownDocBlock::List { items, .. } => {
            let mut total = 0usize;
            for item in items {
                for inner in &item.blocks {
                    total += count_block_spans(inner)?;
                }
            }
            total
        }
        MarkdownDocBlock::Table(table) => {
            let header_spans: usize = table.headers.iter().map(Vec::len).sum();
            let row_spans: usize = table
                .rows
                .iter()
                .flat_map(|row| row.iter())
                .map(Vec::len)
                .sum();
            header_spans + row_spans
        }
        MarkdownDocBlock::IncompleteMath { .. } => {
            return Err("Incomplete equation; showing source".into());
        }
        MarkdownDocBlock::FrontMatter(_)
        | MarkdownDocBlock::Code { .. }
        | MarkdownDocBlock::Rule => 0,
    })
}

fn validate(blocks: &[LocatedBlock], bytes: usize) -> Result<(), String> {
    let format = match &blocks[0].block {
        MarkdownDocBlock::Code { language, .. } => {
            little_clip_core::diagram::Format::from_language(language)
        }
        _ => None,
    };
    let metadata_bytes = blocks
        .iter()
        .filter_map(|item| {
            matches!(item.block, MarkdownDocBlock::FrontMatter(_))
                .then_some(item.source.len().min(64 * 1024))
        })
        .sum::<usize>();
    let budget = match format {
        Some(little_clip_core::diagram::Format::Excalidraw) => 2 * 1024 * 1024,
        Some(_) => 256 * 1024,
        None => 32 * 1024 + metadata_bytes,
    };
    let mut spans = 0usize;
    for item in blocks {
        if item.incomplete_fence {
            return Err("Incomplete Markdown fence; showing source".into());
        }
        spans += count_block_spans(&item.block)?;
    }
    if bytes > budget || spans > 256 {
        return Err("Rendered Markdown excerpt exceeds the preview budget".into());
    }
    Ok(())
}

/// Per-hover native rendering state; dropping it retires diagram/math/image jobs.
pub struct MarkdownExcerptView {
    data: Arc<MarkdownExcerptData>,
    previews: Previews,
    images: super::images::ImageCache,
    links: super::LinkCache,
    pending_navigation: Option<std::path::PathBuf>,
}

impl MarkdownExcerptView {
    /// Creates a view over worker-prepared immutable blocks.
    pub fn new(data: Arc<MarkdownExcerptData>) -> Self {
        Self {
            data,
            previews: Previews::new(),
            images: Default::default(),
            links: Default::default(),
            pending_navigation: None,
        }
    }

    /// Binds the Markdown file's path so local/Hugo images and internal links resolve.
    pub fn with_base_path(mut self, path: std::path::PathBuf) -> Self {
        self.images.set_base_path(Some(&path));
        self.links.set_base_path(Some(&path));
        self
    }

    /// Consumes a clicked internal document link target requested from this excerpt.
    pub fn take_pending_navigation(&mut self) -> Option<std::path::PathBuf> {
        self.pending_navigation.take()
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
                super::render_blocks_with_media(
                    ui,
                    &self.data.blocks,
                    palette,
                    super::DocMedia {
                        previews: Some(&mut self.previews),
                        images: Some(&mut self.images),
                        links: Some(&mut self.links),
                        navigate_to: Some(&mut self.pending_navigation),
                    },
                )
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_render::markdown::MarkdownSpan;
    static LIVE: AtomicBool = AtomicBool::new(false);

    #[test]
    fn source_position_selects_the_matching_section_instead_of_the_start() {
        let source = format!(
            "# Beginning\n\n{}# Destination\n\n**Rendered** words here.\n",
            "Filler\n\n".repeat(200)
        );
        let data = MarkdownExcerptData::prepare(&source, 403, &LIVE).unwrap();
        assert!(*data.source_lines.start() > 390);
        assert!(data.blocks.iter().any(
            |block| matches!(block, MarkdownBlock::Heading { text, .. } if text == "Destination")
        ));
        assert!(!data.blocks.iter().any(
            |block| matches!(block, MarkdownBlock::Heading { text, .. } if text == "Beginning")
        ));
    }

    #[test]
    fn hovering_inside_a_diagram_table_or_equation_keeps_its_complete_native_content() {
        let source = "# Before\n\n```mermaid\nflowchart LR\n A --> B\n B --> C\n```\n\n$$\nx^2 +\ny^2\n$$\n\n| Col A | Col B |\n| --- | --- |\n| one | two |\n";
        for line in [3, 4, 5, 6, 7] {
            let data = MarkdownExcerptData::prepare(source, line, &LIVE).unwrap();
            assert_eq!(data.source_lines, 3..=7);
            assert!(
                matches!(&data.blocks[0], MarkdownBlock::Code { language, text } if language == "mermaid" && text.contains("B --> C"))
            );
        }
        let data = MarkdownExcerptData::prepare(source, 10, &LIVE).unwrap();
        assert!(data.blocks.iter().any(|block| matches!(
            block,
            MarkdownBlock::Paragraph { spans }
                if spans.iter().any(|span| matches!(span, MarkdownSpan::Math { source, display: true, .. } if source.contains("y^2")))
        )));

        let table_data = MarkdownExcerptData::prepare(source, 15, &LIVE).unwrap();
        assert!(matches!(&table_data.blocks[0], MarkdownBlock::Table(_)));
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
        assert!(matches!(&data.blocks[0], MarkdownBlock::Heading { text, .. } if text == "Body"));
    }

    #[test]
    fn blank_lines_suppressed_html_and_unboxed_containers_hover_nearby_instead_of_jumping_to_start()
    {
        let fillers = "Filler paragraph line.\n\n".repeat(40);
        let style_lines = ".rule { color: red; }\n".repeat(25);
        let source = format!(
            "# Document Start\n\nIntro lines.\n\n{fillers}\
             <div class=\"models\">\n\n\
             | Col A | Col B |\n| --- | --- |\n| one | two |\n\n\
             </div>\n\n\
             <style>\n{style_lines}</style>\n\n\
             # Nearby Section\n\nTarget body text.\n"
        );
        let table_line = source
            .lines()
            .position(|l| l.contains("| one | two |"))
            .unwrap()
            + 1;
        let style_mid_line = source
            .lines()
            .enumerate()
            .filter(|(_, l)| l.contains(".rule {"))
            .nth(15)
            .unwrap()
            .0
            + 1;

        let table_hover = MarkdownExcerptData::prepare(&source, table_line, &LIVE).unwrap();
        assert!(
            *table_hover.source_lines.start() >= table_line.saturating_sub(3),
            "unboxed <div> table hover should stay at table line {table_line}, got {:?}",
            table_hover.source_lines
        );
        assert!(matches!(&table_hover.blocks[0], MarkdownBlock::Table(_)));

        let style_hover = MarkdownExcerptData::prepare(&source, style_mid_line, &LIVE).unwrap();
        assert!(
            *style_hover.source_lines.start() > 50,
            "hover inside suppressed <style> block at line {style_mid_line} must not jump back to line 1, got {:?}",
            style_hover.source_lines
        );
    }
}
