//! Independent, cached chunks with header metadata, background packing and a height index.
pub(in crate::file_manager) mod hover;
mod jobs;
pub(super) mod metadata;
mod packing;
pub(in crate::file_manager) mod similarity;
#[cfg(test)]
mod tests;
pub(super) mod thumbnail;
mod thumbnail_cache;
use super::{filesystem::Directory, filesystem::Entry};
pub(in crate::file_manager) use hover::GalleryHover;
use metadata::Metadata;
use packing::{Layout, square_placeholder};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

pub(super) const CHUNK_SIZE: usize = 48;
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    path: PathBuf,
    revision: u64,
    photo: bool,
}
impl Key {
    fn from_entry(entry: &Entry) -> Self {
        Self {
            path: entry.path.clone(),
            revision: entry.revision,
            photo: entry.is_photo(),
        }
    }
}
struct Chunk {
    first: usize,
    keys: Arc<[Key]>,
    metadata: Option<Arc<[Metadata]>>,
    layout: Option<Arc<Layout>>,
    probe: Option<jobs::Pending<Vec<Metadata>>>,
    packing: Option<jobs::Pending<Layout>>,
    generation: u64,
    final_chunk: bool,
}
impl Chunk {
    fn new(first: usize, keys: Arc<[Key]>, generation: u64) -> Self {
        Self {
            first,
            keys,
            final_chunk: false,
            generation,
            metadata: None,
            layout: None,
            probe: None,
            packing: None,
        }
    }
    fn invalidate_layout(&mut self, generation: u64) {
        self.generation = generation;
        self.layout = None;
        self.packing = None;
    }
}
#[derive(Default)]
pub(super) struct Gallery {
    chunks: Vec<Chunk>,
    metadata_cache: BTreeMap<Key, Metadata>,
    offsets: Vec<f32>,
    width: usize,
    chunk_size: usize,
    generation: u64,
    entries_signature: Option<(PathBuf, u64, bool, usize)>,
    viewport: (f32, f32),
    direction: bool,
    scheduled_focus: Option<usize>,
    pending_anchor: Option<Anchor>,
}
#[derive(Clone, Copy)]
pub(super) struct Placement {
    pub index: usize,
    pub x: usize,
    pub y: f32,
    pub width: usize,
    pub height: usize,
    pub ready: bool,
    pub orientation: u8,
    pub aspect: f32,
}
impl Placement {
    pub fn thumbnail_edge(self, unit: f32, dpi: f32) -> u32 {
        let width = (self.width as f32 * unit - 2.0).max(1.0) * dpi;
        let height = (self.height as f32 * unit - 2.0).max(1.0) * dpi;
        // Cover cropping needs enough pixels along BOTH axes. Merely asking
        // for the tile's longest side can undersample a mismatched source ratio.
        width
            .max(height * self.aspect)
            .max(height.max(width / self.aspect))
            .ceil() as u32
    }
}
#[derive(Clone, Copy)]
struct Anchor {
    chunk: usize,
    item: Option<usize>,
    delta: f32,
}
impl Gallery {
    pub fn sync(
        &mut self,
        directory: &Directory,
        visible: &[usize],
        show_hidden: bool,
        width: usize,
    ) {
        let signature = (
            directory.path.clone(),
            directory.signature,
            show_hidden,
            visible.len(),
        );
        let columns = width.max(2) / 2;
        let chunk_size = ((CHUNK_SIZE + columns / 2) / columns).max(1) * columns;
        if self.entries_signature.as_ref() != Some(&signature) || self.chunk_size != chunk_size {
            if self
                .entries_signature
                .as_ref()
                .is_some_and(|previous| previous.0 == signature.0 && previous.3 == signature.3)
            {
                self.pending_anchor = Some(self.anchor(self.viewport.0));
            }
            self.chunk_size = chunk_size;
            self.sync_entries(directory, visible);
            self.entries_signature = Some(signature);
        }
        if self.width != width {
            if self.width > 0 && !self.chunks.is_empty() {
                self.pending_anchor = Some(self.anchor(self.viewport.0));
            }
            self.width = width;
            self.generation = self.generation.wrapping_add(1);
            for chunk in &mut self.chunks {
                chunk.invalidate_layout(self.generation);
            }
        }
        self.reindex();
    }
    fn sync_entries(&mut self, directory: &Directory, visible: &[usize]) {
        let mut previous = std::mem::take(&mut self.chunks).into_iter();
        for (index, group) in visible.chunks(self.chunk_size).enumerate() {
            let keys: Arc<[Key]> = group
                .iter()
                .map(|i| Key::from_entry(&directory.entries[*i]))
                .collect();
            let old = previous.next();
            let mut chunk = old
                .filter(|chunk| chunk.keys == keys)
                .unwrap_or_else(|| Chunk::new(index * self.chunk_size, keys, self.generation));
            let final_chunk = (index + 1) * self.chunk_size >= visible.len();
            if chunk.final_chunk != final_chunk {
                chunk.invalidate_layout(self.generation);
                chunk.final_chunk = final_chunk;
            }
            self.chunks.push(chunk);
        }
        if self.metadata_cache.len() > 20_000 {
            self.metadata_cache.clear();
        }
    }
    fn anchor(&self, offset: f32) -> Anchor {
        let chunk = self.chunk_at(offset);
        let base = self.offsets.get(chunk).copied().unwrap_or(0.0);
        let tile = self
            .chunks
            .get(chunk)
            .and_then(|c| c.layout.as_ref())
            .and_then(|layout| {
                layout
                    .tiles
                    .iter()
                    .find(|tile| tile.y as f32 + tile.height as f32 > offset - base)
            });
        let item = tile.map(|tile| tile.item).or_else(|| {
            let current = self.chunks.get(chunk)?;
            let row = ((offset - base).max(0.0) / 2.0) as usize;
            Some((row * (self.width.max(2) / 2)).min(current.keys.len().saturating_sub(1)))
        });
        let y = item
            .and_then(|item| {
                self.chunks
                    .get(chunk)
                    .map(|c| self.item_offset(c.first + item))
            })
            .unwrap_or(base);
        Anchor {
            chunk,
            item: item.and_then(|item| self.chunks.get(chunk).map(|c| c.first + item)),
            delta: offset - y,
        }
    }
    fn anchored_offset(&self, anchor: Anchor) -> f32 {
        let y = anchor
            .item
            .map(|item| self.item_offset(item))
            .unwrap_or_else(|| self.offsets.get(anchor.chunk).copied().unwrap_or(0.0));
        (y + anchor.delta).max(0.0)
    }
    pub fn prepare(&mut self, ctx: &egui::Context, offset: f32, extent: f32) -> Option<f32> {
        let anchor = self
            .pending_anchor
            .take()
            .unwrap_or_else(|| self.anchor(offset));
        self.poll(ctx);
        self.reindex();
        let corrected = self.anchored_offset(anchor);
        self.set_viewport(corrected, corrected + extent);
        self.schedule();
        if self
            .chunks
            .iter()
            .any(|chunk| chunk.metadata.is_none() || chunk.layout.is_none())
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        (corrected - offset).abs().gt(&0.01).then_some(corrected)
    }
    pub fn set_viewport(&mut self, min: f32, max: f32) {
        self.direction = min >= self.viewport.0;
        self.viewport = (min.max(0.0), max.max(min));
    }
    fn reindex(&mut self) {
        self.offsets.clear();
        self.offsets.push(0.0);
        for chunk in &self.chunks {
            let height = chunk.layout.as_ref().map_or_else(
                || (chunk.keys.len().div_ceil(self.width.max(2) / 2) * 2) as f32,
                |layout| layout.height as f32,
            );
            self.offsets
                .push(self.offsets.last().copied().unwrap_or(0.0) + height);
        }
    }
    pub fn retire(&mut self) {
        for chunk in &mut self.chunks {
            chunk.probe = None;
            chunk.packing = None;
        }
    }
    pub fn invalidate_order(&mut self) {
        self.retire();
        self.entries_signature = None;
        self.pending_anchor = None;
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn height(&self) -> f32 {
        self.offsets.last().copied().unwrap_or(0.0)
    }
    fn chunk_at(&self, offset: f32) -> usize {
        self.offsets
            .partition_point(|value| *value <= offset)
            .saturating_sub(1)
            .min(self.chunks.len().saturating_sub(1))
    }
    pub fn placements(&self, min: f32, max: f32) -> Vec<Placement> {
        if self.chunks.is_empty() {
            return Vec::new();
        }
        let mut placements = Vec::new();
        for chunk_index in self.chunk_at(min)..=self.chunk_at(max) {
            let chunk = &self.chunks[chunk_index];
            let placeholder;
            let layout = if let Some(layout) = &chunk.layout {
                layout.as_ref()
            } else {
                // Cheap, local square placeholders; no preceding-chunk solve,
                // metadata reads, decoding or resources are needed here.
                placeholder = square_placeholder(chunk.keys.len(), self.width);
                &placeholder
            };
            let base = self.offsets[chunk_index];
            for tile in &layout.tiles {
                let y = base + tile.y as f32;
                if y + tile.height as f32 >= min && y <= max {
                    placements.push(Placement {
                        index: chunk.first + tile.item,
                        x: tile.x,
                        y,
                        width: tile.width,
                        height: tile.height,
                        ready: chunk.layout.is_some(),
                        orientation: chunk
                            .metadata
                            .as_ref()
                            .and_then(|values| values.get(tile.item))
                            .map_or(1, |value| value.orientation),
                        aspect: chunk
                            .metadata
                            .as_ref()
                            .and_then(|values| values.get(tile.item))
                            .map_or(1.0, |value| value.aspect()),
                    });
                }
            }
        }
        placements
    }
    pub fn item_offset(&self, index: usize) -> f32 {
        let chunk_index = index / self.chunk_size.max(1);
        let item = index % self.chunk_size.max(1);
        self.offsets.get(chunk_index).copied().unwrap_or(0.0)
            + self
                .chunks
                .get(chunk_index)
                .and_then(|chunk| chunk.layout.as_ref())
                .and_then(|layout| layout.tiles.get(item))
                .map_or((item / (self.width.max(2) / 2)) as f32 * 2.0, |tile| {
                    tile.y as f32
                })
    }
}
