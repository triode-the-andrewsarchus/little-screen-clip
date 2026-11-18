//! Offline 3D-globe / 2D-map projection, Oilslick elevation shading, and basemap data loaders.

pub mod storage;
pub mod tiles;
use storage::AssetDirectory;

use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

// Compact overview grids must fit the bounded in-memory representation.
fn grid_len(width: u32, height: u32, channels: usize) -> Option<usize> {
    if width == 0 || height == 0 {
        return None;
    }
    let length = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(channels)?;
    (length <= 128 * 1024 * 1024).then_some(length)
}

fn tiff_limits(bytes: usize) -> tiff::decoder::Limits {
    let mut limits = tiff::decoder::Limits::default();
    limits.decoding_buffer_size = bytes;
    limits.intermediate_buffer_size = bytes;
    limits
}

// Shared byte accounting for decoded TIFF chunks. Large chunks are rejected by
// decoder limits before allocation; eviction accounts for retained capacities.
fn cache_chunk<K: Copy + Eq + std::hash::Hash, T>(
    cache: &mut HashMap<K, Arc<Vec<T>>>,
    order: &mut VecDeque<K>,
    key: K,
    values: Vec<T>,
    budget: usize,
) -> Option<Arc<Vec<T>>> {
    let bytes = values.capacity().checked_mul(std::mem::size_of::<T>())?;
    if bytes > budget {
        return None;
    }
    let mut used: usize = cache
        .values()
        .map(|v| v.capacity() * std::mem::size_of::<T>())
        .sum();
    while used + bytes > budget {
        let old = order.pop_front()?;
        if let Some(v) = cache.remove(&old) {
            used -= v.capacity() * std::mem::size_of::<T>();
        }
    }
    let values = Arc::new(values);
    order.push_back(key);
    cache.insert(key, values.clone());
    Some(values)
}

/// Projection mode for the geography viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ProjectionMode {
    /// Smoothly transition from 3D globe at low zoom to 2D map when zooming in.
    #[default]
    Auto,
    /// Force 3D orthographic globe projection.
    Globe3D,
    /// Force 2D equirectangular / local-aspect map projection.
    Map2D,
}

/// Terrain / basemap shading mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BasemapStyle {
    /// Natural Earth II / collapsed land-cover palette with flat ocean and DEM hillshade.
    #[default]
    Natural,
    /// Classic Natural Earth-style hypsometric elevation tints + DEM hillshade.
    Hypsometric,
    /// High-contrast 500 m cyclic lightness + rotating hue elevation bands (mrgris.com/projects/oilslick).
    Oilslick,
    /// Neutral monochrome shaded relief from DEM slopes.
    Hillshade,
}

/// Collapsed 5-class global land cover (from Copernicus/ESA 300 m CCI).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LandCoverClass {
    Ocean = 0,
    Forest = 1,
    GrassCrops = 2,
    Scrub = 3,
    BareGround = 4,
    SnowIce = 5,
}

impl LandCoverClass {
    /// Maps an ESA CCI 300 m land-cover class code to the collapsed 5-class palette.
    pub fn from_esa_cci(code: u8) -> Self {
        match code {
            0 | 210 => Self::Ocean,
            50..=100 | 160 | 170 => Self::Forest,
            10..=40 | 130 | 180 => Self::GrassCrops,
            110..=122 | 140..=153 => Self::Scrub,
            190..=202 => Self::BareGround,
            220 => Self::SnowIce,
            _ => Self::Scrub,
        }
    }

    /// Natural RGB color for each collapsed land-cover class.
    pub fn rgb(self) -> [u8; 3] {
        match self {
            Self::Ocean => [26, 62, 108],
            Self::Forest => [54, 98, 56],
            Self::GrassCrops => [124, 152, 86],
            Self::Scrub => [158, 144, 98],
            Self::BareGround => [186, 166, 134],
            Self::SnowIce => [236, 242, 248],
        }
    }
}

/// One major city from Natural Earth Populated Places (`POP_MAX >= 250,000`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CityPoint {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub pop_max: u64,
}

/// One Natural Earth Admin 0 (country) or Admin 1 (state/province) boundary polyline.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BorderLine {
    /// True when Natural Earth `FEATURECLA` marks the segment as disputed/indefinite/lease.
    pub disputed: bool,
    /// Polyline vertices `[lon, lat]` in degrees.
    pub points: Vec<[f32; 2]>,
}

/// One GSHHG coastline or lake outline polygon.
#[derive(Clone, Debug)]
pub struct WaterPolygon {
    /// GSHHG level: 1 = land/coastline, 2 = lake, 3 = island-in-lake, 4 = pond.
    pub level: u8,
    /// Bounding box `[min_lon, min_lat, max_lon, max_lat]` in degrees.
    pub bounds: [f32; 4],
    /// Closed ring vertices `[lon, lat]` in degrees.
    pub points: Vec<[f32; 2]>,
}

/// One Natural Earth 1:10m road polyline.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoadLine {
    /// True for major highways / motorways / beltways.
    pub major: bool,
    /// Bounding box `[min_lon, min_lat, max_lon, max_lat]` in degrees.
    pub bounds: [f32; 4],
    pub points: Vec<[f32; 2]>,
}

/// One Natural Earth 1:10m inland lake / reservoir outline.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LakePolygon {
    #[serde(default)]
    pub holes: Vec<Vec<[f32; 2]>>,
    /// Bounding box `[min_lon, min_lat, max_lon, max_lat]` in degrees.
    pub bounds: [f32; 4],
    pub points: Vec<[f32; 2]>,
}

/// Loaded Natural Earth II equirectangular overview texture.
#[derive(Clone)]
pub struct EquirectTexture {
    pub width: u32,
    pub height: u32,
    /// RGB24 pixels, row-major from north (+90°) to south (-90°), west (-180°) to east (+180°).
    pub rgb: Arc<Vec<u8>>,
}

impl EquirectTexture {
    /// Bilinearly samples `(lat, lon)` in degrees.
    pub fn sample_rgb(&self, lat: f64, lon: f64) -> [u8; 3] {
        if grid_len(self.width, self.height, 3) != Some(self.rgb.len())
            || !lat.is_finite()
            || !lon.is_finite()
        {
            return [26, 62, 108];
        }
        let u = ((lon + 180.0) / 360.0).rem_euclid(1.0) * (self.width.saturating_sub(1) as f64);
        let v = ((90.0 - lat).clamp(0.0, 180.0) / 180.0) * (self.height.saturating_sub(1) as f64);
        let x0 = (u.floor() as u32).min(self.width - 1);
        let y0 = (v.floor() as u32).min(self.height - 1);
        let x1 = (x0 + 1) % self.width;
        let y1 = (y0 + 1).min(self.height - 1);
        let fx = (u - x0 as f64) as f32;
        let fy = (v - y0 as f64) as f32;

        let p = |x: u32, y: u32| -> [f32; 3] {
            let i = ((y * self.width + x) as usize) * 3;
            [
                self.rgb[i] as f32,
                self.rgb[i + 1] as f32,
                self.rgb[i + 2] as f32,
            ]
        };
        let c00 = p(x0, y0);
        let c10 = p(x1, y0);
        let c01 = p(x0, y1);
        let c11 = p(x1, y1);
        let mut out = [0u8; 3];
        for ch in 0..3 {
            let top = c00[ch] * (1.0 - fx) + c10[ch] * fx;
            let bot = c01[ch] * (1.0 - fx) + c11[ch] * fx;
            out[ch] = (top * (1.0 - fy) + bot * fy).clamp(0.0, 255.0) as u8;
        }
        out
    }
}

/// One 1°×1° SRTM-style `.hgt` elevation tile (big-endian `i16` meters).
pub struct SrtmTile {
    pub lat_floor: i32,
    pub lon_floor: i32,
    pub samples: usize,
    pub heights: Arc<Vec<i16>>,
}

impl SrtmTile {
    /// Parses a standard SRTM1 (`3601×3601`) or SRTM3 (`1201×1201`) `.hgt` file.
    pub fn from_hgt_bytes(lat_floor: i32, lon_floor: i32, bytes: &[u8]) -> Option<Self> {
        if bytes.len() > 3601 * 3601 * 2
            || !(-90..90).contains(&lat_floor)
            || !(-180..180).contains(&lon_floor)
        {
            return None;
        }
        let count = bytes.len() / 2;
        let samples = (count as f64).sqrt().round() as usize;
        if samples < 2 || samples * samples * 2 != bytes.len() {
            return None;
        }
        let mut heights = Vec::with_capacity(count);
        for pair in bytes.chunks_exact(2) {
            heights.push(i16::from_be_bytes([pair[0], pair[1]]));
        }
        Some(Self {
            lat_floor,
            lon_floor,
            samples,
            heights: Arc::new(heights),
        })
    }

    /// Samples elevation in meters at `(lat, lon)` inside this 1°×1° tile.
    pub fn sample(&self, lat: f64, lon: f64) -> Option<f32> {
        if self.samples < 2
            || self.samples.checked_mul(self.samples) != Some(self.heights.len())
            || !lat.is_finite()
            || !lon.is_finite()
        {
            return None;
        }
        let fx = (lon - self.lon_floor as f64).clamp(0.0, 1.0) * (self.samples - 1) as f64;
        let fy = (1.0 - (lat - self.lat_floor as f64).clamp(0.0, 1.0)) * (self.samples - 1) as f64;
        let x0 = (fx.floor() as usize).min(self.samples - 1);
        let y0 = (fy.floor() as usize).min(self.samples - 1);
        let x1 = (x0 + 1).min(self.samples - 1);
        let y1 = (y0 + 1).min(self.samples - 1);
        let tx = (fx - x0 as f64) as f32;
        let ty = (fy - y0 as f64) as f32;

        let get = |x: usize, y: usize| -> Option<f32> {
            let v = self.heights[y * self.samples + x];
            (v != -32768).then_some(v as f32)
        };
        let v00 = get(x0, y0)?;
        let v10 = get(x1, y0).unwrap_or(v00);
        let v01 = get(x0, y1).unwrap_or(v00);
        let v11 = get(x1, y1).unwrap_or(v00);
        let top = v00 * (1.0 - tx) + v10 * tx;
        let bot = v01 * (1.0 - tx) + v11 * tx;
        Some(top * (1.0 - ty) + bot * ty)
    }
}

/// Lazy local HGT inventory with a byte-bounded decoded tile cache.
#[derive(Default)]
pub struct HgtStore {
    sources: HashMap<(i32, i32), (Arc<AssetDirectory>, std::ffi::OsString)>,
    cache: Mutex<(HashMap<(i32, i32), Arc<Vec<i16>>>, VecDeque<(i32, i32)>)>,
}
impl HgtStore {
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
    fn sample(&self, lat: f64, lon: f64) -> Option<f32> {
        let key = (lat.floor() as i32, lon.floor() as i32);
        let (directory, name) = self.sources.get(&key)?;
        let mut cache = self.cache.lock().ok()?;
        let heights = if let Some(hit) = cache.0.get(&key) {
            hit.clone()
        } else {
            let file = directory.open_file(name).ok()?;
            if file.metadata().ok()?.len() > 3601 * 3601 * 2 {
                return None;
            }
            let bytes = storage::read(format!(
                "/proc/self/fd/{}",
                std::os::fd::AsRawFd::as_raw_fd(&file)
            ))
            .ok()?;
            let parsed = SrtmTile::from_hgt_bytes(key.0, key.1, &bytes)?;
            let (values, order) = &mut *cache;
            cache_chunk(
                values,
                order,
                key,
                Arc::unwrap_or_clone(parsed.heights),
                128 * 1024 * 1024,
            )?
        };
        let samples = (heights.len() as f64).sqrt() as usize;
        SrtmTile {
            lat_floor: key.0,
            lon_floor: key.1,
            samples,
            heights,
        }
        .sample(lat, lon)
    }
}

/// Optional global/local elevation & land-cover store.
#[derive(Default)]
pub struct BasemapAssets {
    pub assets_dir: PathBuf,
    pub ne2: Option<EquirectTexture>,
    pub ne2_full: Option<Ne2FullSampler>,
    pub cities: Vec<CityPoint>,
    pub country_borders: Vec<BorderLine>,
    pub state_borders: Vec<BorderLine>,
    pub roads: Vec<RoadLine>,
    pub lakes: Vec<LakePolygon>,
    pub coastlines: Vec<WaterPolygon>,
    pub terrain_tiles: HgtStore,
    pub global_dem: Option<GlobalDemGrid>,
    pub cog_dem: Option<CogDemPyramid>,
    pub land_cover: Option<GlobalLandCoverGrid>,
    pub rivers: Option<HydroRiversIndex>,
    pub grwl_summary: Option<GrwlSummaryIndex>,
    pub grwl_masks: Option<GrwlMaskStore>,
    pub has_hydrorivers: bool,
}

/// One GRWL river centerline segment with Landsat-measured median wetted width (`width_med` in meters).
#[derive(Clone, Debug)]
pub struct GrwlSegment {
    pub width_med_m: f32,
    pub points: Vec<[f32; 2]>,
}

/// 2°×2° spatially bucketed GRWL summary centerlines (`GRWL_summaryStats_V01.02`) index.
pub struct GrwlSummaryIndex {
    segments: Vec<GrwlSegment>,
    cells: Vec<Vec<u32>>,
}

impl GrwlSummaryIndex {
    pub fn segments(&self) -> &[GrwlSegment] {
        &self.segments
    }

    pub fn load_or_build(dir: &Path) -> Option<Self> {
        let cache_path = dir.join("grwl_summary_v2.bin");
        if let Ok(bytes) = storage::read(&cache_path)
            && let Some(idx) = Self::decode_bin(&bytes)
        {
            return Some(idx);
        }
        let grwl_dir = dir.join("grwl");
        let shp_path = find_first_with_ext(&grwl_dir, &["shp"])?;
        let dbf_path = shp_path.with_extension("dbf");
        let built = Self::build_from_shp(&shp_path, &dbf_path)?;
        if let Ok(encoded) = built.encode_bin() {
            let _ = storage::write(&cache_path, encoded);
        }
        Some(built)
    }

    pub fn query_bbox(
        &self,
        min_lon: f64,
        min_lat: f64,
        max_lon: f64,
        max_lat: f64,
        min_width_m: f32,
        limit: usize,
    ) -> Vec<&GrwlSegment> {
        let gx0 = (((min_lon + 180.0) / 2.0).floor() as i32).clamp(0, 179) as usize;
        let gx1 = (((max_lon + 180.0) / 2.0).floor() as i32).clamp(0, 179) as usize;
        let gy0 = (((min_lat + 90.0) / 2.0).floor() as i32).clamp(0, 89) as usize;
        let gy1 = (((max_lat + 90.0) / 2.0).floor() as i32).clamp(0, 89) as usize;
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for gy in gy0..=gy1 {
            for gx in gx0..=gx1 {
                for &seg_idx in &self.cells[gy * 180 + gx] {
                    let seg = &self.segments[seg_idx as usize];
                    if seg.width_med_m >= min_width_m && seen.insert(seg_idx) {
                        out.push(seg);
                        if out.len() >= limit {
                            return out;
                        }
                    }
                }
            }
        }
        out
    }

    fn build_from_shp(shp_path: &Path, dbf_path: &Path) -> Option<Self> {
        let mut dbf_widths: Vec<f32> = Vec::new();
        if let Ok(dbf_file) = crate::kernel_interface::open_content(dbf_path, true) {
            let mut rdr = BufReader::new(dbf_file);
            let mut hdr = [0u8; 32];
            if rdr.read_exact(&mut hdr).is_ok() {
                let header_len = u16::from_le_bytes([hdr[8], hdr[9]]) as usize;
                let rec_len = u16::from_le_bytes([hdr[10], hdr[11]]) as usize;
                let n_recs = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
                if header_len < 33
                    || rec_len < 2
                    || n_recs > 1_000_000
                    || (n_recs as u64)
                        .checked_mul(rec_len as u64)?
                        .checked_add(header_len as u64)?
                        > rdr.get_ref().metadata().ok()?.len()
                {
                    return None;
                }
                let mut field_buf = vec![0u8; header_len.saturating_sub(32)];
                if rdr.read_exact(&mut field_buf).is_ok() && rec_len > 1 {
                    let mut offset = 1usize;
                    let mut w_span = None;
                    for chunk in field_buf.chunks_exact(32) {
                        if chunk[0] == 0x0D {
                            break;
                        }
                        let end = chunk[..11].iter().position(|&b| b == 0).unwrap_or(11);
                        let name = std::str::from_utf8(&chunk[..end])
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        let flen = chunk[16] as usize;
                        if offset.checked_add(flen)? > rec_len {
                            return None;
                        }
                        if name == "width_med" || (w_span.is_none() && name == "width_mean") {
                            w_span = Some((offset, flen));
                        }
                        offset += flen;
                    }
                    if let Some((off, flen)) = w_span {
                        dbf_widths.reserve(n_recs);
                        let mut rec = vec![0u8; rec_len];
                        for _ in 0..n_recs {
                            if rdr.read_exact(&mut rec).is_err() {
                                break;
                            }
                            let s = std::str::from_utf8(&rec[off..(off + flen).min(rec_len)])
                                .unwrap_or("")
                                .trim();
                            dbf_widths.push(s.parse::<f32>().unwrap_or(30.0).max(1.0));
                        }
                    }
                }
            }
        }

        let shp_file = crate::kernel_interface::open_content(shp_path, true).ok()?;
        let mut rdr = BufReader::new(shp_file);
        let mut hdr = [0u8; 100];
        rdr.read_exact(&mut hdr).ok()?;
        let mut segments = Vec::new();
        let mut cells = vec![Vec::new(); 180 * 90];
        let mut memberships = 0usize;
        let mut rec_hdr = [0u8; 8];
        let mut rec_idx = 0usize;
        while rdr.read_exact(&mut rec_hdr).is_ok() {
            let content_words =
                usize::try_from(i32::from_be_bytes(rec_hdr[4..8].try_into().ok()?)).ok()?;
            let content_bytes = content_words.checked_mul(2)?;
            if content_bytes > 32 * 1024 * 1024
                || rec_idx >= 1_000_000
                || content_bytes as u64 > rdr.get_ref().metadata().ok()?.len()
            {
                return None;
            }
            if content_bytes < 4 {
                break;
            }
            let mut buf = vec![0u8; content_bytes];
            if rdr.read_exact(&mut buf).is_err() {
                break;
            }
            let width_med_m = dbf_widths.get(rec_idx).copied().unwrap_or(40.0);
            rec_idx += 1;
            if buf.len() < 44 {
                continue;
            }
            let shape_type = i32::from_le_bytes(buf[0..4].try_into().ok()?);
            if shape_type != 3 && shape_type != 13 && shape_type != 23 {
                continue;
            }
            let parts = shp_polyline_parts(&buf, 32)?;
            for points in parts {
                memberships += index_points(
                    &mut cells,
                    segments.len() as u32,
                    &points,
                    (16 * 1024 * 1024) - memberships,
                )?;
                segments.push(GrwlSegment {
                    width_med_m,
                    points,
                });
            }
        }
        Some(Self { segments, cells })
    }

    fn encode_bin(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.segments.len() as u32).to_le_bytes());
        for seg in &self.segments {
            out.extend_from_slice(&seg.width_med_m.to_le_bytes());
            let n = seg.points.len().min(u16::MAX as usize) as u16;
            out.extend_from_slice(&n.to_le_bytes());
            for &[lon, lat] in seg.points.iter().take(n as usize) {
                out.extend_from_slice(&lon.to_le_bytes());
                out.extend_from_slice(&lat.to_le_bytes());
            }
        }
        for cell in &self.cells {
            out.extend_from_slice(&(cell.len() as u32).to_le_bytes());
            for &id in cell {
                out.extend_from_slice(&id.to_le_bytes());
            }
        }
        Ok(out)
    }

    fn decode_bin(bytes: &[u8]) -> Option<Self> {
        let mut pos = 0usize;
        let seg_count = u32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?) as usize;
        pos += 4;
        if seg_count > 1_000_000 || seg_count > bytes.len().saturating_sub(pos) / 6 {
            return None;
        }
        let mut segments = Vec::with_capacity(seg_count);
        for _ in 0..seg_count {
            let width_med_m = f32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?);
            pos += 4;
            let n = u16::from_le_bytes(bytes.get(pos..pos + 2)?.try_into().ok()?) as usize;
            pos += 2;
            if !(2..=256).contains(&n) || n > bytes.len().saturating_sub(pos) / 8 {
                return None;
            }
            let mut points = Vec::with_capacity(n);
            for _ in 0..n {
                let lon = f32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?);
                let lat = f32::from_le_bytes(bytes.get(pos + 4..pos + 8)?.try_into().ok()?);
                pos += 8;
                if !(-180.0..=180.0).contains(&lon) || !(-90.0..=90.0).contains(&lat) {
                    return None;
                }
                points.push([lon, lat]);
            }
            segments.push(GrwlSegment {
                width_med_m,
                points,
            });
        }
        let mut cells = Vec::with_capacity(180 * 90);
        let mut memberships = 0usize;
        for _ in 0..(180 * 90) {
            let len = u32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?) as usize;
            pos += 4;
            memberships = memberships.checked_add(len)?;
            if len > seg_count
                || len > bytes.len().saturating_sub(pos) / 4
                || memberships > 16 * 1024 * 1024
            {
                return None;
            }
            let mut cell = Vec::with_capacity(len);
            for _ in 0..len {
                let id = u32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?);
                if id as usize >= seg_count {
                    return None;
                }
                cell.push(id);
                pos += 4;
            }
            cells.push(cell);
        }
        if pos != bytes.len() {
            return None;
        }
        Some(Self { segments, cells })
    }
}

/// SHP PolyLine/PolyLineZ/PolyLineM share the same XY part table. Sampling
/// restarts inside each part and always retains its original final endpoint.
fn shp_polyline_parts(bytes: &[u8], limit: usize) -> Option<Vec<Vec<[f32; 2]>>> {
    let integer = |offset| {
        usize::try_from(i32::from_le_bytes(
            bytes.get(offset..offset + 4)?.try_into().ok()?,
        ))
        .ok()
    };
    let part_count = integer(36)?;
    let point_count = integer(40)?;
    let points_offset = 44usize.checked_add(part_count.checked_mul(4)?)?;
    bytes.get(..points_offset.checked_add(point_count.checked_mul(16)?)?)?;
    if part_count == 0 || point_count < 2 {
        return None;
    }
    let mut starts: Vec<_> = (0..part_count)
        .map(|i| integer(44 + i * 4))
        .collect::<Option<_>>()?;
    if starts.first().copied() != Some(0)
        || starts.windows(2).any(|w| w[0] >= w[1])
        || *starts.last()? >= point_count
    {
        return None;
    }
    starts.push(point_count);
    let mut parts = Vec::new();
    for span in starts.windows(2) {
        if span[1] - span[0] < 2 {
            continue;
        }
        let length = span[1] - span[0];
        let count = length.min(limit.max(2));
        let indices = (0..count)
            .map(|i| span[0] + ((i as u64 * (length - 1) as u64) / (count - 1) as u64) as usize);
        let mut points = Vec::new();
        for i in indices {
            let p = points_offset + i * 16;
            let lon = f64::from_le_bytes(bytes[p..p + 8].try_into().ok()?) as f32;
            let lat = f64::from_le_bytes(bytes[p + 8..p + 16].try_into().ok()?) as f32;
            if !lon.is_finite() || !lat.is_finite() {
                return None;
            }
            points.push([lon, lat]);
        }
        parts.push(points);
    }
    Some(parts)
}

fn index_points(
    cells: &mut [Vec<u32>],
    id: u32,
    points: &[[f32; 2]],
    remaining: usize,
) -> Option<usize> {
    let bounds = points
        .iter()
        .fold([180.0_f32, 90.0, -180.0, -90.0], |b, p| {
            [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ]
        });
    let x = |lon: f32| (((lon + 180.0) / 2.0).floor() as i32).clamp(0, 179) as usize;
    let y = |lat: f32| (((lat + 90.0) / 2.0).floor() as i32).clamp(0, 89) as usize;
    let count = (y(bounds[3]) - y(bounds[1]) + 1) * (x(bounds[2]) - x(bounds[0]) + 1);
    if count > remaining {
        return None;
    }
    for gy in y(bounds[1])..=y(bounds[3]) {
        for gx in x(bounds[0])..=x(bounds[2]) {
            cells[gy * 180 + gx].push(id);
        }
    }
    Some(count)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct GrwlMaskEntry {
    file: String,
    width: u32,
    height: u32,
    min_lon: f64,
    max_lon: f64,
    min_lat: f64,
    max_lat: f64,
}

/// On-demand 30 m GRWL GeoTIFF water-mask store (`GRWL_mask_V01.01`).
pub struct GrwlMaskStore {
    mask_dir: AssetDirectory,
    entries: Vec<GrwlMaskEntry>,
    cells: Vec<Vec<u32>>,
    inner: Mutex<MaskInner>,
}

struct MaskInner {
    decoders: HashMap<usize, tiff::decoder::Decoder<BufReader<fs::File>>>,
    cache: HashMap<(usize, u32), Arc<Vec<u8>>>,
    order: VecDeque<(usize, u32)>,
}

impl GrwlMaskStore {
    pub fn load_or_index(dir: &Path) -> Option<Self> {
        let mask_dir = AssetDirectory::open(&dir.join("grwl_masks")).ok()?;
        let idx_path = dir.join("grwl_masks_idx.json");
        if let Ok(bytes) = storage::read(&idx_path)
            && let Ok(entries) = serde_json::from_slice::<Vec<GrwlMaskEntry>>(&bytes)
            && !entries.is_empty()
            && entries.len() <= 65_536
            && entries.iter().all(|e| {
                e.width > 0
                    && e.height > 0
                    && e.min_lon.is_finite()
                    && e.max_lon.is_finite()
                    && e.min_lat.is_finite()
                    && e.max_lat.is_finite()
                    && e.min_lon < e.max_lon
                    && e.min_lat < e.max_lat
                    && Path::new(&e.file).file_name() == Some(std::ffi::OsStr::new(&e.file))
            })
        {
            return Self::from_entries(mask_dir, entries);
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(mask_dir.path()).ok()?.flatten() {
            let p = entry.path();
            if !p
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("tif") || s.eq_ignore_ascii_case("tiff"))
            {
                continue;
            }
            let Ok(file) = crate::kernel_interface::open_content(&p, true) else {
                continue;
            };
            let Ok(mut dec) = tiff::decoder::Decoder::new(BufReader::new(file)) else {
                continue;
            };
            let Ok((w, h)) = dec.dimensions() else {
                continue;
            };
            let Ok(scale_val) = dec.get_tag_f64_vec(tiff::tags::Tag::Unknown(33550)) else {
                continue;
            };
            let Ok(tie_val) = dec.get_tag_f64_vec(tiff::tags::Tag::Unknown(33922)) else {
                continue;
            };
            if scale_val.len() < 2 || tie_val.len() < 6 {
                continue;
            }
            let sx = scale_val[0];
            let sy = scale_val[1];
            let min_lon = tie_val[3] - tie_val[0] * sx;
            let max_lat = tie_val[4] + tie_val[1] * sy;
            let max_lon = min_lon + (w as f64) * sx;
            let min_lat = max_lat - (h as f64) * sy;
            if (-180.0..=180.0).contains(&min_lon) && (-90.0..=90.0).contains(&min_lat) {
                entries.push(GrwlMaskEntry {
                    file: entry.file_name().to_string_lossy().into_owned(),
                    width: w,
                    height: h,
                    min_lon,
                    max_lon,
                    min_lat,
                    max_lat,
                });
            }
        }
        if entries.is_empty() {
            return None;
        }
        if let Ok(encoded) = serde_json::to_vec(&entries) {
            let _ = storage::write(&idx_path, encoded);
        }
        Self::from_entries(mask_dir, entries)
    }

    fn from_entries(mask_dir: AssetDirectory, entries: Vec<GrwlMaskEntry>) -> Option<Self> {
        let mut cells = vec![Vec::new(); 180 * 90];
        let mut memberships = 0;
        for (i, e) in entries.iter().enumerate() {
            memberships += index_points(
                &mut cells,
                i as u32,
                &[
                    [e.min_lon as f32, e.min_lat as f32],
                    [e.max_lon as f32, e.max_lat as f32],
                ],
                1_000_000 - memberships,
            )?;
        }
        Some(Self {
            mask_dir,
            entries,
            cells,
            inner: Mutex::new(MaskInner {
                decoders: HashMap::new(),
                cache: HashMap::new(),
                order: VecDeque::new(),
            }),
        })
    }

    /// Returns `Some(true)` for water, `Some(false)` for land/island inside a 30 m mask tile,
    /// or `None` if no mask tile covers `(lat, lon)`.
    pub fn sample_water(&self, lat: f64, lon: f64) -> Option<bool> {
        if !lat.is_finite() || !lon.is_finite() {
            return None;
        }
        let cell = (((lat + 90.0) / 2.0).floor() as i32).clamp(0, 89) as usize * 180
            + (((lon + 180.0) / 2.0).floor() as i32).clamp(0, 179) as usize;
        let idx = *self.cells[cell].iter().find(|&&i| {
            let e = &self.entries[i as usize];
            lon >= e.min_lon && lon < e.max_lon && lat >= e.min_lat && lat < e.max_lat
        })? as usize;
        let entry = &self.entries[idx];
        let px = (((lon - entry.min_lon) / (entry.max_lon - entry.min_lon)) * entry.width as f64)
            .floor() as u32;
        let py = (((entry.max_lat - lat) / (entry.max_lat - entry.min_lat)) * entry.height as f64)
            .floor() as u32;
        if px >= entry.width || py >= entry.height {
            return None;
        }
        let mut inner = self.inner.lock().ok()?;
        if !inner.decoders.contains_key(&idx) {
            let file = self
                .mask_dir
                .open_file(std::ffi::OsStr::new(&entry.file))
                .ok()?;
            let mut decoder = tiff::decoder::Decoder::new(BufReader::new(file))
                .ok()?
                .with_limits(tiff_limits(16 * 1024 * 1024));
            if decoder.dimensions().ok()? != (entry.width, entry.height)
                || decoder.colortype().ok()? != tiff::ColorType::Gray(8)
            {
                return None;
            }
            if inner.decoders.len() >= 8 {
                inner.decoders.clear();
            }
            inner.decoders.insert(idx, decoder);
        }
        let (cw, ch) = inner.decoders.get(&idx)?.chunk_dimensions();
        if cw == 0 || ch == 0 || (cw as u64) * (ch as u64) > 16 * 1024 * 1024 {
            return None;
        }
        let chunk_id = (py / ch)
            .checked_mul(entry.width.div_ceil(cw))?
            .checked_add(px / cw)?;
        let key = (idx, chunk_id);
        let data = if let Some(hit) = inner.cache.get(&key) {
            hit.clone()
        } else {
            let tiff::decoder::DecodingResult::U8(bytes) =
                inner.decoders.get_mut(&idx)?.read_chunk(chunk_id).ok()?
            else {
                return None;
            };
            let MaskInner { cache, order, .. } = &mut *inner;
            cache_chunk(cache, order, key, bytes, 32 * 1024 * 1024)?
        };
        let actual_w = (entry.width - (px / cw) * cw).min(cw);
        data.get(((py % ch) as usize) * (actual_w as usize) + (px % cw) as usize)
            .map(|&v| v > 0)
    }
}

/// One HydroRIVERS polyline segment with its Strahler stream order (`ORD_STRA`) and discharge (`DIS_AV_CMS`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RiverSegment {
    pub ord_stra: u8,
    pub dis_cms: f32,
    pub points: Vec<[f32; 2]>,
}

/// 2°×2° spatially bucketed HydroRIVERS index (`ORD_STRA >= 3`) for sub-millisecond viewport queries.
pub struct HydroRiversIndex {
    segments: Vec<RiverSegment>,
    /// 180 × 90 grid (`cell = ((lat+90)/2).clamp(0,89) * 180 + ((lon+180)/2).clamp(0,179)`).
    cells: Vec<Vec<u32>>,
}

impl HydroRiversIndex {
    pub fn segments(&self) -> &[RiverSegment] {
        &self.segments
    }

    pub fn load_or_build(dir: &Path) -> Option<Self> {
        let cache_path = dir.join("rivers_stra3_v3.bin");
        if let Ok(bytes) = storage::read(&cache_path)
            && let Some(idx) = Self::decode_bin(&bytes)
        {
            return Some(idx);
        }
        let rivers_dir = dir.join("hydrorivers");
        let shp_path = find_first_with_ext(&rivers_dir, &["shp"])?;
        let dbf_path = shp_path.with_extension("dbf");
        let built = Self::build_from_shp(&shp_path, &dbf_path)?;
        if let Ok(encoded) = built.encode_bin() {
            let _ = storage::write(&cache_path, encoded);
        }
        Some(built)
    }

    pub fn query_bbox(
        &self,
        min_lon: f64,
        min_lat: f64,
        max_lon: f64,
        max_lat: f64,
        min_ord: u8,
        limit: usize,
    ) -> Vec<&RiverSegment> {
        let gx0 = (((min_lon + 180.0) / 2.0).floor() as i32).clamp(0, 179) as usize;
        let gx1 = (((max_lon + 180.0) / 2.0).floor() as i32).clamp(0, 179) as usize;
        let gy0 = (((min_lat + 90.0) / 2.0).floor() as i32).clamp(0, 89) as usize;
        let gy1 = (((max_lat + 90.0) / 2.0).floor() as i32).clamp(0, 89) as usize;
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for gy in gy0..=gy1 {
            for gx in gx0..=gx1 {
                for &seg_idx in &self.cells[gy * 180 + gx] {
                    let seg = &self.segments[seg_idx as usize];
                    if seg.ord_stra >= min_ord && seen.insert(seg_idx) {
                        out.push(seg);
                        if out.len() >= limit {
                            return out;
                        }
                    }
                }
            }
        }
        out
    }

    fn build_from_shp(shp_path: &Path, dbf_path: &Path) -> Option<Self> {
        let mut dbf_ord: Vec<(u8, f32)> = Vec::new();
        if let Ok(dbf_file) = crate::kernel_interface::open_content(dbf_path, true) {
            let mut rdr = BufReader::new(dbf_file);
            let mut hdr = [0u8; 32];
            if rdr.read_exact(&mut hdr).is_ok() {
                let header_len = u16::from_le_bytes([hdr[8], hdr[9]]) as usize;
                let rec_len = u16::from_le_bytes([hdr[10], hdr[11]]) as usize;
                let n_recs = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
                if header_len < 33
                    || rec_len < 2
                    || n_recs > 1_000_000
                    || (n_recs as u64)
                        .checked_mul(rec_len as u64)?
                        .checked_add(header_len as u64)?
                        > rdr.get_ref().metadata().ok()?.len()
                {
                    return None;
                }
                let mut field_buf = vec![0u8; header_len.saturating_sub(32)];
                if rdr.read_exact(&mut field_buf).is_ok() && rec_len > 1 {
                    let mut offset = 1usize;
                    let mut ord_span = None;
                    let mut dis_span = None;
                    for chunk in field_buf.chunks_exact(32) {
                        if chunk[0] == 0x0D {
                            break;
                        }
                        let end = chunk[..11].iter().position(|&b| b == 0).unwrap_or(11);
                        let name = std::str::from_utf8(&chunk[..end]).unwrap_or("");
                        let flen = chunk[16] as usize;
                        if offset.checked_add(flen)? > rec_len {
                            return None;
                        }
                        if name == "ORD_STRA" {
                            ord_span = Some((offset, flen));
                        } else if name == "DIS_AV_CMS" {
                            dis_span = Some((offset, flen));
                        }
                        offset += flen;
                    }
                    if let Some((off, flen)) = ord_span {
                        dbf_ord.reserve(n_recs);
                        let mut rec = vec![0u8; rec_len];
                        for _ in 0..n_recs {
                            if rdr.read_exact(&mut rec).is_err() {
                                break;
                            }
                            let s = std::str::from_utf8(&rec[off..(off + flen).min(rec_len)])
                                .unwrap_or("")
                                .trim();
                            let ord = s.parse::<u8>().unwrap_or(1);
                            let dis = dis_span
                                .and_then(|(doff, dlen)| {
                                    std::str::from_utf8(&rec[doff..(doff + dlen).min(rec_len)])
                                        .ok()
                                        .and_then(|ds| ds.trim().parse::<f32>().ok())
                                })
                                .unwrap_or(10.0);
                            dbf_ord.push((ord, dis));
                        }
                    }
                }
            }
        }

        let shp_file = crate::kernel_interface::open_content(shp_path, true).ok()?;
        let mut rdr = BufReader::new(shp_file);
        let mut hdr = [0u8; 100];
        rdr.read_exact(&mut hdr).ok()?;
        let mut segments = Vec::new();
        let mut cells = vec![Vec::new(); 180 * 90];
        let mut memberships = 0usize;
        let mut rec_hdr = [0u8; 8];
        let mut rec_idx = 0usize;
        while rdr.read_exact(&mut rec_hdr).is_ok() {
            let content_words =
                usize::try_from(i32::from_be_bytes(rec_hdr[4..8].try_into().ok()?)).ok()?;
            let content_bytes = content_words.checked_mul(2)?;
            if content_bytes > 32 * 1024 * 1024
                || rec_idx >= 1_000_000
                || content_bytes as u64 > rdr.get_ref().metadata().ok()?.len()
            {
                return None;
            }
            if content_bytes < 4 {
                break;
            }
            let mut buf = vec![0u8; content_bytes];
            if rdr.read_exact(&mut buf).is_err() {
                break;
            }
            let (ord, dis) = dbf_ord.get(rec_idx).copied().unwrap_or((3, 15.0));
            rec_idx += 1;
            if ord < 3 || buf.len() < 44 {
                continue;
            }
            let shape_type = i32::from_le_bytes(buf[0..4].try_into().ok()?);
            if shape_type != 3 && shape_type != 13 && shape_type != 23 {
                continue;
            }
            let parts = shp_polyline_parts(&buf, 24)?;
            for points in parts {
                memberships += index_points(
                    &mut cells,
                    segments.len() as u32,
                    &points,
                    (16 * 1024 * 1024) - memberships,
                )?;
                segments.push(RiverSegment {
                    ord_stra: ord,
                    dis_cms: dis,
                    points,
                });
            }
        }
        Some(Self { segments, cells })
    }

    fn encode_bin(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.segments.len() as u32).to_le_bytes());
        for seg in &self.segments {
            out.push(seg.ord_stra);
            out.extend_from_slice(&seg.dis_cms.to_le_bytes());
            let n = seg.points.len().min(u16::MAX as usize) as u16;
            out.extend_from_slice(&n.to_le_bytes());
            for &[lon, lat] in seg.points.iter().take(n as usize) {
                out.extend_from_slice(&lon.to_le_bytes());
                out.extend_from_slice(&lat.to_le_bytes());
            }
        }
        for cell in &self.cells {
            out.extend_from_slice(&(cell.len() as u32).to_le_bytes());
            for &id in cell {
                out.extend_from_slice(&id.to_le_bytes());
            }
        }
        Ok(out)
    }

    fn decode_bin(bytes: &[u8]) -> Option<Self> {
        let mut pos = 0usize;
        let seg_count = u32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?) as usize;
        pos += 4;
        if seg_count > 1_000_000 || seg_count > bytes.len().saturating_sub(pos) / 6 {
            return None;
        }
        let mut segments = Vec::with_capacity(seg_count);
        for _ in 0..seg_count {
            let ord_stra = *bytes.get(pos)?;
            pos += 1;
            let dis_cms = f32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?);
            pos += 4;
            let n = u16::from_le_bytes(bytes.get(pos..pos + 2)?.try_into().ok()?) as usize;
            pos += 2;
            if !(2..=256).contains(&n) || n > bytes.len().saturating_sub(pos) / 8 {
                return None;
            }
            let mut points = Vec::with_capacity(n);
            for _ in 0..n {
                let lon = f32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?);
                let lat = f32::from_le_bytes(bytes.get(pos + 4..pos + 8)?.try_into().ok()?);
                pos += 8;
                if !(-180.0..=180.0).contains(&lon) || !(-90.0..=90.0).contains(&lat) {
                    return None;
                }
                points.push([lon, lat]);
            }
            segments.push(RiverSegment {
                ord_stra,
                dis_cms,
                points,
            });
        }
        let mut cells = Vec::with_capacity(180 * 90);
        let mut memberships = 0usize;
        for _ in 0..(180 * 90) {
            let len = u32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?) as usize;
            pos += 4;
            memberships = memberships.checked_add(len)?;
            if len > seg_count
                || len > bytes.len().saturating_sub(pos) / 4
                || memberships > 16 * 1024 * 1024
            {
                return None;
            }
            let mut cell = Vec::with_capacity(len);
            for _ in 0..len {
                let id = u32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?);
                if id as usize >= seg_count {
                    return None;
                }
                cell.push(id);
                pos += 4;
            }
            cells.push(cell);
        }
        if pos != bytes.len() {
            return None;
        }
        Some(Self { segments, cells })
    }
}

#[derive(Clone, Debug)]
struct CogLevel {
    ifd_index: usize,
    width: u32,
    height: u32,
    chunk_w: u32,
    chunk_h: u32,
    tiles_across: u32,
}

struct CogDemInner {
    decoder: tiff::decoder::Decoder<BufReader<fs::File>>,
    current_ifd: usize,
    cache: HashMap<(usize, u32), Arc<Vec<i16>>>,
    order: VecDeque<(usize, u32)>,
}

/// Multi-resolution Cloud-Optimized GeoTIFF DEM pyramid (e.g. `GEBCO_2025.tif` up to 86400×43200).
pub struct CogDemPyramid {
    levels: Vec<CogLevel>,
    inner: Mutex<CogDemInner>,
}

impl CogDemPyramid {
    pub fn open(tif_path: &Path) -> Option<Self> {
        let file = crate::kernel_interface::open_content(tif_path, true).ok()?;
        let mut decoder = tiff::decoder::Decoder::new(BufReader::new(file))
            .ok()?
            .with_limits(tiff_limits(16 * 1024 * 1024));
        let mut levels = Vec::new();
        for idx in 0..16usize {
            if idx > 0 && decoder.seek_to_image(idx).is_err() {
                break;
            }
            if let Ok((width, height)) = decoder.dimensions() {
                let (chunk_w, chunk_h) = decoder.chunk_dimensions();
                if width > 0
                    && width <= 1_000_000
                    && height > 0
                    && height <= 1_000_000
                    && chunk_w > 0
                    && chunk_h > 0
                    && (chunk_w as u64) * (chunk_h as u64) <= 4 * 1024 * 1024
                    && (width.div_ceil(chunk_w) as u64) * (height.div_ceil(chunk_h) as u64)
                        <= u32::MAX as u64
                    && matches!(decoder.colortype().ok(), Some(tiff::ColorType::Gray(_)))
                {
                    levels.push(CogLevel {
                        ifd_index: idx,
                        width,
                        height,
                        chunk_w,
                        chunk_h,
                        tiles_across: width.div_ceil(chunk_w),
                    });
                }
            }
        }
        if levels.is_empty() {
            return None;
        }
        // Sort coarsest-to-finest by width so picking `width >= target_width` is trivial.
        levels.sort_by_key(|l| l.width);
        let _ = decoder.seek_to_image(0);
        Some(Self {
            levels,
            inner: Mutex::new(CogDemInner {
                decoder,
                current_ifd: 0,
                cache: HashMap::new(),
                order: VecDeque::new(),
            }),
        })
    }

    pub fn sample_at(&self, lat: f64, lon: f64, target_global_width: f64) -> Option<f32> {
        if !lat.is_finite() || !lon.is_finite() || !target_global_width.is_finite() {
            return None;
        }
        let level = self
            .levels
            .iter()
            .find(|l| l.width as f64 >= target_global_width)
            .or_else(|| self.levels.last())?;
        let u = ((lon + 180.0) / 360.0).rem_euclid(1.0) * (level.width.saturating_sub(1) as f64);
        let v = ((90.0 - lat).clamp(0.0, 180.0) / 180.0) * (level.height.saturating_sub(1) as f64);
        let x0 = (u.floor() as u32).min(level.width - 1);
        let y0 = (v.floor() as u32).min(level.height - 1);
        let x1 = (x0 + 1) % level.width;
        let y1 = (y0 + 1).min(level.height - 1);
        let fx = (u - x0 as f64) as f32;
        let fy = (v - y0 as f64) as f32;

        let mut guard = self.inner.lock().ok()?;
        let get = |guard: &mut CogDemInner, x: u32, y: u32| -> Option<f32> {
            let tx = x / level.chunk_w;
            let ty = y / level.chunk_h;
            let chunk_idx = ty * level.tiles_across + tx;
            let key = (level.ifd_index, chunk_idx);
            let chunk = if let Some(hit) = guard.cache.get(&key) {
                hit.clone()
            } else {
                if guard.current_ifd != level.ifd_index {
                    guard.decoder.seek_to_image(level.ifd_index).ok()?;
                    guard.current_ifd = level.ifd_index;
                }
                let decoded = guard.decoder.read_chunk(chunk_idx).ok()?;
                let vals: Vec<i16> = match decoded {
                    tiff::decoder::DecodingResult::I16(v) => v,
                    tiff::decoder::DecodingResult::U16(v) => {
                        v.into_iter().map(|p| p as i16).collect()
                    }
                    tiff::decoder::DecodingResult::I32(v) => v
                        .into_iter()
                        .map(|p| p.clamp(-12000, 10000) as i16)
                        .collect(),
                    tiff::decoder::DecodingResult::F32(v) => v
                        .into_iter()
                        .map(|p| p.round().clamp(-12000.0, 10000.0) as i16)
                        .collect(),
                    _ => return None,
                };
                let CogDemInner { cache, order, .. } = &mut *guard;
                cache_chunk(cache, order, key, vals, 64 * 1024 * 1024)?
            };
            let lx = x % level.chunk_w;
            let ly = y % level.chunk_h;
            let actual_w = (level.width - tx * level.chunk_w).min(level.chunk_w);
            let idx = (ly * actual_w + lx) as usize;
            chunk.get(idx).map(|&h| h as f32)
        };

        let c00 = get(&mut guard, x0, y0)?;
        let c10 = get(&mut guard, x1, y0).unwrap_or(c00);
        let c01 = get(&mut guard, x0, y1).unwrap_or(c00);
        let c11 = get(&mut guard, x1, y1).unwrap_or(c00);
        let top = c00 * (1.0 - fx) + c10 * fx;
        let bot = c01 * (1.0 - fx) + c11 * fx;
        Some(top * (1.0 - fy) + bot * fy)
    }
}

struct Ne2FullInner {
    decoder: tiff::decoder::Decoder<BufReader<fs::File>>,
    cache: HashMap<u32, Arc<Vec<u8>>>,
    order: VecDeque<u32>,
}

/// On-demand full-resolution (`21600×10800`) Natural Earth II GeoTIFF strip sampler.
pub struct Ne2FullSampler {
    width: u32,
    height: u32,
    chunk_h: u32,
    inner: Mutex<Ne2FullInner>,
}

impl Ne2FullSampler {
    pub fn open(tif_path: &Path) -> Option<Self> {
        let file = crate::kernel_interface::open_content(tif_path, true).ok()?;
        let mut decoder = tiff::decoder::Decoder::new(BufReader::new(file))
            .ok()?
            .with_limits(tiff_limits(16 * 1024 * 1024));
        let (width, height) = decoder.dimensions().ok()?;
        if decoder.colortype().ok()? != tiff::ColorType::RGB(8) {
            return None;
        }
        let (chunk_w, chunk_h) = decoder.chunk_dimensions();
        if width < 8192
            || width > 1_000_000
            || height == 0
            || height > 1_000_000
            || chunk_w != width
            || chunk_h == 0
            || (width as u64) * (chunk_h as u64) * 3 > 16 * 1024 * 1024
        {
            return None;
        }
        Some(Self {
            width,
            height,
            chunk_h,
            inner: Mutex::new(Ne2FullInner {
                decoder,
                cache: HashMap::new(),
                order: VecDeque::new(),
            }),
        })
    }

    pub fn sample_rgb(&self, lat: f64, lon: f64) -> Option<[u8; 3]> {
        let u = ((lon + 180.0) / 360.0).rem_euclid(1.0) * (self.width.saturating_sub(1) as f64);
        let v = ((90.0 - lat).clamp(0.0, 180.0) / 180.0) * (self.height.saturating_sub(1) as f64);
        let x0 = (u.floor() as u32).min(self.width - 1);
        let y0 = (v.floor() as u32).min(self.height - 1);
        let x1 = (x0 + 1) % self.width;
        let y1 = (y0 + 1).min(self.height - 1);
        let fx = (u - x0 as f64) as f32;
        let fy = (v - y0 as f64) as f32;

        let mut guard = self.inner.lock().ok()?;
        let get = |guard: &mut Ne2FullInner, x: u32, y: u32| -> Option<[f32; 3]> {
            let chunk_idx = y / self.chunk_h;
            let chunk = if let Some(hit) = guard.cache.get(&chunk_idx) {
                hit.clone()
            } else {
                let decoded = guard.decoder.read_chunk(chunk_idx).ok()?;
                let tiff::decoder::DecodingResult::U8(bytes) = decoded else {
                    return None;
                };
                let Ne2FullInner { cache, order, .. } = &mut *guard;
                cache_chunk(cache, order, chunk_idx, bytes, 64 * 1024 * 1024)?
            };
            let ly = y % self.chunk_h;
            let idx = ((ly * self.width + x) as usize) * 3;
            if idx + 2 >= chunk.len() {
                return None;
            }
            Some([
                chunk[idx] as f32,
                chunk[idx + 1] as f32,
                chunk[idx + 2] as f32,
            ])
        };

        let c00 = get(&mut guard, x0, y0)?;
        let c10 = get(&mut guard, x1, y0).unwrap_or(c00);
        let c01 = get(&mut guard, x0, y1).unwrap_or(c00);
        let c11 = get(&mut guard, x1, y1).unwrap_or(c00);
        let mut out = [0u8; 3];
        for ch in 0..3 {
            let top = c00[ch] * (1.0 - fx) + c10[ch] * fx;
            let bot = c01[ch] * (1.0 - fx) + c11[ch] * fx;
            out[ch] = (top * (1.0 - fy) + bot * fy).clamp(0.0, 255.0) as u8;
        }
        Some(out)
    }
}

/// Compact global equirectangular `i16` elevation grid (e.g. downsampled GEBCO 2026 overview).
pub struct GlobalDemGrid {
    pub width: u32,
    pub height: u32,
    pub heights: Vec<i16>,
}

impl GlobalDemGrid {
    /// Samples elevation in meters at `(lat, lon)`.
    pub fn sample(&self, lat: f64, lon: f64) -> Option<f32> {
        if grid_len(self.width, self.height, 1) != Some(self.heights.len())
            || !lat.is_finite()
            || !lon.is_finite()
        {
            return None;
        }
        let u = ((lon + 180.0) / 360.0).rem_euclid(1.0) * (self.width.saturating_sub(1) as f64);
        let v = ((90.0 - lat).clamp(0.0, 180.0) / 180.0) * (self.height.saturating_sub(1) as f64);
        let x0 = (u.floor() as u32).min(self.width - 1);
        let y0 = (v.floor() as u32).min(self.height - 1);
        let x1 = (x0 + 1) % self.width;
        let y1 = (y0 + 1).min(self.height - 1);
        let fx = (u - x0 as f64) as f32;
        let fy = (v - y0 as f64) as f32;
        let get = |x: u32, y: u32| self.heights[(y * self.width + x) as usize] as f32;
        let top = get(x0, y0) * (1.0 - fx) + get(x1, y0) * fx;
        let bot = get(x0, y1) * (1.0 - fx) + get(x1, y1) * fx;
        Some(top * (1.0 - fy) + bot * fy)
    }
}

/// Compact global 1-byte-per-cell land-cover grid (`LandCoverClass` values).
pub struct GlobalLandCoverGrid {
    pub width: u32,
    pub height: u32,
    pub classes: Vec<u8>,
}

impl GlobalLandCoverGrid {
    pub fn sample(&self, lat: f64, lon: f64) -> LandCoverClass {
        if grid_len(self.width, self.height, 1) != Some(self.classes.len())
            || !lat.is_finite()
            || !lon.is_finite()
        {
            return LandCoverClass::Ocean;
        }
        let u = ((lon + 180.0) / 360.0).rem_euclid(1.0) * (self.width.saturating_sub(1) as f64);
        let v = ((90.0 - lat).clamp(0.0, 180.0) / 180.0) * (self.height.saturating_sub(1) as f64);
        let x = (u.round() as u32).min(self.width - 1);
        let y = (v.round() as u32).min(self.height - 1);
        match self.classes[(y * self.width + x) as usize] {
            0 => LandCoverClass::Ocean,
            1 => LandCoverClass::Forest,
            2 => LandCoverClass::GrassCrops,
            3 => LandCoverClass::Scrub,
            4 => LandCoverClass::BareGround,
            5 => LandCoverClass::SnowIce,
            _ => LandCoverClass::Scrub,
        }
    }
}

/// Status of offline basemap assets on disk.
#[derive(Clone, Debug, Default)]
pub struct AssetStatus {
    pub has_ne2: bool,
    pub has_cities: bool,
    pub has_borders: bool,
    pub has_roads_lakes: bool,
    pub has_grwl: bool,
    pub has_grwl_masks: bool,
    pub has_gshhg: bool,
    pub has_gebco: bool,
    pub has_land_cover: bool,
    pub has_hydrorivers: bool,
    pub local_hgt_count: usize,
}

impl AssetStatus {
    pub fn has_quick_start(&self) -> bool {
        self.has_ne2 && self.has_cities
    }
}

impl BasemapAssets {
    /// Reports layers that the loaders accepted and can actually render.
    pub fn status(&self) -> AssetStatus {
        AssetStatus {
            has_ne2: self.ne2.is_some() || self.ne2_full.is_some(),
            has_cities: !self.cities.is_empty(),
            has_borders: !self.country_borders.is_empty() && !self.state_borders.is_empty(),
            has_roads_lakes: !self.roads.is_empty() && !self.lakes.is_empty(),
            has_grwl: self
                .grwl_summary
                .as_ref()
                .is_some_and(|r| !r.segments.is_empty()),
            has_grwl_masks: self.grwl_masks.is_some(),
            has_gshhg: !self.coastlines.is_empty(),
            has_gebco: self.global_dem.is_some() || self.cog_dem.is_some(),
            has_land_cover: self.land_cover.is_some(),
            has_hydrorivers: self.rivers.as_ref().is_some_and(|r| !r.segments.is_empty()),
            local_hgt_count: self.terrain_tiles.sources.len(),
        }
    }

    /// Loads available offline basemap assets from `assets_dir` and optional `terrain_override_dir`.
    pub fn load(assets_dir: &Path, terrain_override_dir: Option<&Path>) -> Self {
        Self::load_with_cancel(
            assets_dir,
            terrain_override_dir,
            &std::sync::atomic::AtomicBool::new(false),
        )
    }
    pub fn load_with_cancel(
        assets_dir: &Path,
        terrain_override_dir: Option<&Path>,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Self {
        let stopped = || cancelled.load(std::sync::atomic::Ordering::Relaxed);
        if stopped() {
            return Self::default();
        }
        let root = AssetDirectory::open(assets_dir).ok();
        let anchor = root.as_ref().map(AssetDirectory::path);
        let assets_dir = anchor.as_deref().unwrap_or(assets_dir);
        let ne2 = load_ne2_overview(assets_dir, cancelled);
        if stopped() {
            return Self::default();
        }
        let ne2_full = find_first_with_ext(assets_dir, &["tif", "tiff"])
            .and_then(|p| Ne2FullSampler::open(&p));
        if stopped() {
            return Self::default();
        }
        let cities = load_cities_from_dir(assets_dir);
        let country_borders = load_borders_from_dir(
            assets_dir,
            "borders_admin0.json",
            "ne_10m_admin_0_boundary_lines_land.geojson",
        );
        let state_borders = load_borders_from_dir(
            assets_dir,
            "borders_admin1.json",
            "ne_10m_admin_1_states_provinces_lines.geojson",
        );
        if stopped() {
            return Self::default();
        }
        let roads = load_roads_from_dir(assets_dir);
        let lakes = load_lakes_from_dir(assets_dir);
        let coastlines = find_gshhg_file(assets_dir)
            .and_then(|p| storage::read(p).ok())
            .map(|bytes| parse_gshhg_b(&bytes, 12_000))
            .unwrap_or_default();
        if stopped() {
            return Self::default();
        }
        let mut terrain_tiles = HgtStore::default();
        if let Some(dir) = terrain_override_dir {
            load_hgt_dir(dir, &mut terrain_tiles);
        }
        load_hgt_dir(&assets_dir.join("terrain"), &mut terrain_tiles);

        if stopped() {
            return Self::default();
        }
        let global_dem = load_global_dem(assets_dir);
        let cog_dem = find_first_with_ext(&assets_dir.join("gebco"), &["tif", "tiff"])
            .and_then(|p| CogDemPyramid::open(&p));
        if stopped() {
            return Self::default();
        }
        let land_cover = load_land_cover(assets_dir);
        let rivers = HydroRiversIndex::load_or_build(assets_dir);
        if stopped() {
            return Self::default();
        }
        let grwl_summary = GrwlSummaryIndex::load_or_build(assets_dir);
        if stopped() {
            return Self::default();
        }
        let grwl_masks = GrwlMaskStore::load_or_index(assets_dir);
        let has_hydrorivers = rivers.as_ref().is_some_and(|r| !r.segments.is_empty());

        Self {
            assets_dir: root
                .as_ref()
                .map(|root| root.display_path().to_path_buf())
                .unwrap_or_else(|| assets_dir.to_path_buf()),
            ne2,
            ne2_full,
            cities,
            country_borders,
            state_borders,
            roads,
            lakes,
            coastlines,
            terrain_tiles,
            global_dem,
            cog_dem,
            land_cover,
            rivers,
            grwl_summary,
            grwl_masks,
            has_hydrorivers,
        }
    }

    /// Samples terrain elevation in meters at `(lat, lon)` from local overrides, COG pyramid, or
    /// overview DEM, returning `None` when no DEM is loaded.
    pub fn sample_elevation_at(&self, lat: f64, lon: f64, target_global_width: f64) -> Option<f32> {
        if let Some(h) = self.terrain_tiles.sample(lat, lon) {
            return Some(h);
        }
        if (target_global_width > 5400.0 || self.global_dem.is_none())
            && let Some(cog) = &self.cog_dem
            && let Some(h) = cog.sample_at(lat, lon, target_global_width)
        {
            return Some(h);
        }
        self.global_dem.as_ref().and_then(|g| g.sample(lat, lon))
    }

    /// Samples terrain elevation in meters at `(lat, lon)`.
    pub fn sample_elevation_m(&self, lat: f64, lon: f64) -> Option<f32> {
        self.sample_elevation_at(lat, lon, 5400.0)
    }

    /// Samples Natural Earth II RGB using the full 21600×10800 GeoTIFF when zoomed past the 4096×2048 overview.
    pub fn sample_ne2_at(&self, lat: f64, lon: f64, target_global_width: f64) -> Option<[u8; 3]> {
        if target_global_width > 4096.0
            && let Some(full) = &self.ne2_full
            && let Some(rgb) = full.sample_rgb(lat, lon)
        {
            return Some(rgb);
        }
        self.ne2.as_ref().map(|n| n.sample_rgb(lat, lon))
    }
}

/// Classic Natural Earth-style hypsometric elevation tint ramp modulated by DEM hillshade.
pub fn hypsometric_color(elevation_m: f32, hillshade: f32) -> [u8; 3] {
    if elevation_m <= 0.0 {
        return [26, 62, 108];
    }
    const STOPS: &[(f32, [f32; 3])] = &[
        (0.0, [96.0, 146.0, 88.0]),
        (250.0, [148.0, 174.0, 108.0]),
        (700.0, [204.0, 194.0, 132.0]),
        (1400.0, [194.0, 146.0, 96.0]),
        (2400.0, [168.0, 114.0, 88.0]),
        (3800.0, [196.0, 184.0, 178.0]),
        (5200.0, [240.0, 244.0, 248.0]),
    ];
    let mut base = STOPS[STOPS.len() - 1].1;
    for pair in STOPS.windows(2) {
        if elevation_m <= pair[1].0 {
            let t = ((elevation_m - pair[0].0) / (pair[1].0 - pair[0].0)).clamp(0.0, 1.0);
            base = [
                pair[0].1[0] + (pair[1].1[0] - pair[0].1[0]) * t,
                pair[0].1[1] + (pair[1].1[1] - pair[0].1[1]) * t,
                pair[0].1[2] + (pair[1].1[2] - pair[0].1[2]) * t,
            ];
            break;
        }
    }
    let shade = 0.64 + 0.36 * hillshade.clamp(0.0, 1.0);
    [
        (base[0] * shade).clamp(0.0, 255.0) as u8,
        (base[1] * shade).clamp(0.0, 255.0) as u8,
        (base[2] * shade).clamp(0.0, 255.0) as u8,
    ]
}

/// Maps elevation in meters and DEM hillshade (`0..=1`) to the `mrgris.com/projects/oilslick`
/// high-contrast cyclic color palette:
/// - Oceans (`<= 0.0 m`) are rendered flat slate-blue for a sharp sea-level discontinuity.
/// - Land cycles lightness `black -> white -> black` every `500 m` while rotating hue continuously.
pub fn oilslick_color(elevation_m: f32, hillshade: f32) -> [u8; 3] {
    if elevation_m <= 0.0 {
        return [24, 58, 102];
    }
    // 500 m cyclic lightness zig-zag
    let phase = (elevation_m / 500.0).rem_euclid(1.0);
    let zigzag = 1.0 - (2.0 * phase - 1.0).abs();
    let lightness = 0.14 + 0.76 * zigzag;

    // Continuous hue rotation starting at warm orange (~24°) right above sea level
    // for strong orange/blue coastline discontinuity.
    let hue = (24.0 + (elevation_m / 1850.0) * 360.0).rem_euclid(360.0);
    let sat = 0.86_f32;
    let [r, g, b] = hsl_to_rgb(hue, sat, lightness);
    let shade = 0.66 + 0.34 * hillshade.clamp(0.0, 1.0);
    [
        (r * shade * 255.0).clamp(0.0, 255.0) as u8,
        (g * shade * 255.0).clamp(0.0, 255.0) as u8,
        (b * shade * 255.0).clamp(0.0, 255.0) as u8,
    ]
}

fn hsl_to_rgb(h_deg: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h = h_deg / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r1, g1, b1) = if h < 1.0 {
        (c, x, 0.0)
    } else if h < 2.0 {
        (x, c, 0.0)
    } else if h < 3.0 {
        (0.0, c, x)
    } else if h < 4.0 {
        (0.0, x, c)
    } else if h < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let m = l - c * 0.5;
    [r1 + m, g1 + m, b1 + m]
}

/// Interactive camera supporting smooth 3D-globe to 2D-map transition.
#[derive(Clone, Copy, Debug)]
pub struct GlobeCamera {
    pub center_lat: f64,
    pub center_lon: f64,
    /// `1.0` = full globe; `>= 4.5` = 2D map; up to `512.0`.
    pub zoom: f64,
    pub mode: ProjectionMode,
}

impl Default for GlobeCamera {
    fn default() -> Self {
        Self {
            center_lat: 20.0,
            center_lon: 0.0,
            zoom: 1.0,
            mode: ProjectionMode::Auto,
        }
    }
}

impl GlobeCamera {
    /// Fits the document's compact, unwrapped geographic bounds, retaining the
    /// chosen projection mode. Missing positions restore the default globe.
    pub fn fit_bounds(&mut self, bounds: Option<[f64; 4]>) {
        let Some([west, south, east, north]) = bounds else {
            *self = Self::default();
            return;
        };
        self.center_lat = ((south + north) * 0.5).clamp(-85.0, 85.0);
        self.center_lon = wrap_lon((west + east) * 0.5);
        let lat_span = (north - self.center_lat)
            .abs()
            .max((south - self.center_lat).abs())
            * 2.0;
        let span = lat_span.max(east - west).max(0.05);
        self.zoom = (90.0 / span).clamp(1.15, 64.0);
    }

    /// Returns `1.0` for full 3D globe, `0.0` for flat 2D map, and smoothstepped values between.
    pub fn globe_blend(&self) -> f64 {
        match self.mode {
            ProjectionMode::Globe3D => 1.0,
            ProjectionMode::Map2D => 0.0,
            ProjectionMode::Auto => {
                if self.zoom <= 1.8 {
                    1.0
                } else if self.zoom >= 4.5 {
                    0.0
                } else {
                    let t = ((self.zoom - 1.8) / (4.5 - 1.8)).clamp(0.0, 1.0);
                    1.0 - t * t * (3.0 - 2.0 * t)
                }
            }
        }
    }

    /// Radius of the globe / scale of the map in pixels for a given viewport `(w, h)`.
    pub fn scale_px(&self, width: f32, height: f32) -> f64 {
        let base = (width.min(height) as f64) * 0.43;
        base * self.zoom
    }

    /// Projects `(lat, lon)` in degrees into viewport pixel coordinates `[x, y]`.
    /// Returns `None` if behind the visible hemisphere of the 3D globe.
    pub fn project(&self, lat: f64, lon: f64, width: f32, height: f32) -> Option<[f32; 2]> {
        let blend = self.globe_blend();
        let scale = self.scale_px(width, height);
        let cx = width as f64 * 0.5;
        let cy = height as f64 * 0.5;

        // 2D local equirectangular/cosine-corrected tangent map centered at (center_lat, center_lon)
        let d_lon = wrap_lon_delta(lon - self.center_lon);
        let d_lat = lat - self.center_lat;
        let cos_c = self.center_lat.to_radians().cos().abs().max(0.25);
        let rad_per_deg = std::f64::consts::PI / 180.0;
        let x_2d = d_lon * rad_per_deg * cos_c * scale;
        let y_2d = -d_lat * rad_per_deg * scale;

        if blend <= 0.0 {
            return Some([(cx + x_2d) as f32, (cy + y_2d) as f32]);
        }

        // 3D orthographic sphere rotated to (center_lat, center_lon)
        let phi = lat.to_radians();
        let phi0 = self.center_lat.to_radians();
        let dl = d_lon.to_radians();
        let cos_phi = phi.cos();
        let sin_phi = phi.sin();
        let cos_phi0 = phi0.cos();
        let sin_phi0 = phi0.sin();

        let z_3d = sin_phi0 * sin_phi + cos_phi0 * cos_phi * dl.cos();
        if blend > 0.2 && z_3d < -0.02 * (1.0 - blend) {
            return None;
        }
        let x_3d = cos_phi * dl.sin() * scale;
        let y_3d = -(cos_phi0 * sin_phi - sin_phi0 * cos_phi * dl.cos()) * scale;

        let px = cx + x_3d * blend + x_2d * (1.0 - blend);
        let py = cy + y_3d * blend + y_2d * (1.0 - blend);
        Some([px as f32, py as f32])
    }

    /// Inverse-projects the same blended surface used by the GPU and `project`.
    /// Pixels outside the visible geographic surface return `None`.
    pub fn unproject(&self, px: f32, py: f32, width: f32, height: f32) -> Option<(f64, f64)> {
        let scale = self.scale_px(width, height);
        if !scale.is_finite() || scale <= 0.0 || !px.is_finite() || !py.is_finite() {
            return None;
        }
        let x = (px as f64 - width as f64 * 0.5) / scale;
        let y = -(py as f64 - height as f64 * 0.5) / scale;
        let blend = self.globe_blend();
        let phi0 = self.center_lat.to_radians();
        let aspect = phi0.cos().abs().max(0.25);
        if blend <= 0.0 {
            let phi = phi0 + y;
            let lambda = x / aspect;
            return valid_inverse(phi, lambda).then(|| {
                (
                    phi.to_degrees(),
                    wrap_lon(self.center_lon + lambda.to_degrees()),
                )
            });
        }
        let globe = orthographic_inverse(x, y, phi0);
        if blend >= 1.0 - 1e-12 {
            return globe.map(|(phi, lambda)| {
                (
                    phi.to_degrees(),
                    wrap_lon(self.center_lon + lambda.to_degrees()),
                )
            });
        }
        // The inverse of a blend is not a blend of its inverses. Solve the two
        // forward equations with their analytic Jacobian, in radians, with a
        // bounded damped Newton iteration. Pure globe/map modes remain analytic.
        let guess = if blend > 0.5 { globe } else { None };
        let (mut phi, mut lambda) = guess.unwrap_or((
            (phi0 + y).clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2),
            (x / aspect).clamp(-std::f64::consts::PI, std::f64::consts::PI),
        ));
        let (s0, c0) = phi0.sin_cos();
        for _ in 0..32 {
            let (sp, cp) = phi.sin_cos();
            let (sl, cl) = lambda.sin_cos();
            let fx = blend * cp * sl + (1.0 - blend) * aspect * lambda - x;
            let fy = blend * (c0 * sp - s0 * cp * cl) + (1.0 - blend) * (phi - phi0) - y;
            let front = s0 * sp + c0 * cp * cl;
            if fx.hypot(fy) * scale < 0.002 {
                return (valid_inverse(phi, lambda)
                    && (blend <= 0.2 || front >= -0.02 * (1.0 - blend)))
                    .then(|| {
                        (
                            phi.to_degrees(),
                            wrap_lon(self.center_lon + lambda.to_degrees()),
                        )
                    });
            }
            let a = -blend * sp * sl;
            let b = blend * cp * cl + (1.0 - blend) * aspect;
            let c = blend * (c0 * cp + s0 * sp * cl) + (1.0 - blend);
            let d = blend * s0 * cp * sl;
            let determinant = a * d - b * c;
            if determinant.abs() < 1e-14 {
                return None;
            }
            let dp = (d * fx - b * fy) / determinant;
            let dl = (-c * fx + a * fy) / determinant;
            let damping = (0.35 / dp.abs().max(dl.abs()).max(0.35)).min(1.0);
            phi = (phi - dp * damping)
                .clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
            lambda = (lambda - dl * damping).clamp(-std::f64::consts::PI, std::f64::consts::PI);
        }
        None
    }

    /// Pans the camera by screen pixel delta `(dx, dy)`.
    pub fn pan_by(&mut self, dx: f32, dy: f32, width: f32, height: f32) {
        let scale = self.scale_px(width, height).max(1.0);
        let deg_per_px = (180.0 / std::f64::consts::PI) / scale;
        let cos_c = self.center_lat.to_radians().cos().abs().max(0.25);
        self.center_lon = wrap_lon(self.center_lon - (dx as f64 * deg_per_px) / cos_c);
        self.center_lat = (self.center_lat + dy as f64 * deg_per_px).clamp(-85.0, 85.0);
    }
}

fn valid_inverse(phi: f64, lambda: f64) -> bool {
    phi.is_finite()
        && lambda.is_finite()
        && phi.abs() <= std::f64::consts::FRAC_PI_2 + 1e-10
        && lambda.abs() <= std::f64::consts::PI + 1e-10
}
fn orthographic_inverse(x: f64, y: f64, phi0: f64) -> Option<(f64, f64)> {
    let r2 = x * x + y * y;
    if r2 > 1.0 {
        return None;
    }
    if r2 < 1e-16 {
        return Some((phi0, 0.0));
    }
    let z = (1.0 - r2).sqrt();
    let (s0, c0) = phi0.sin_cos();
    Some((
        (z * s0 + y * c0).clamp(-1.0, 1.0).asin(),
        x.atan2(c0 * z - y * s0),
    ))
}

pub fn choose_contour_interval_m(span_m: f32, zoom: f64) -> f32 {
    let target = (span_m.max(20.0) / 14.0) / (zoom as f32 / 6.0).clamp(0.6, 2.5);
    for &step in &[10.0_f32, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0] {
        if target <= step {
            return step;
        }
    }
    1000.0
}

/// Wraps longitude into the canonical half-open `[-180, 180)` interval.
pub fn wrap_lon(lon: f64) -> f64 {
    ((lon + 180.0).rem_euclid(360.0)) - 180.0
}

pub fn wrap_lon_delta(dlon: f64) -> f64 {
    ((dlon + 180.0).rem_euclid(360.0)) - 180.0
}

/// Parses Natural Earth Populated Places GeoJSON, sorted descending by `POP_MAX`
/// so largest visible cities are labeled first at every zoom level.
pub fn parse_cities_geojson(bytes: &[u8]) -> Result<Vec<CityPoint>, String> {
    let root: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| format!("Invalid cities GeoJSON: {e}"))?;
    let features = root
        .get("features")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Cities GeoJSON missing features array".to_string())?;
    let mut cities = Vec::new();
    for feat in features {
        let props = feat.get("properties").and_then(|v| v.as_object());
        let coords = feat
            .get("geometry")
            .and_then(|g| g.get("coordinates"))
            .and_then(|c| c.as_array());
        let (Some(props), Some(coords)) = (props, coords) else {
            continue;
        };
        if coords.len() < 2 {
            continue;
        }
        let pop_max = props
            .get("POP_MAX")
            .or_else(|| props.get("pop_max"))
            .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
            .unwrap_or(0);
        let name = props
            .get("NAME")
            .or_else(|| props.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let (Some(lon), Some(lat)) = (coords[0].as_f64(), coords[1].as_f64()) else {
            continue;
        };
        cities.push(CityPoint {
            name,
            lat,
            lon,
            pop_max,
        });
    }
    cities.sort_by(|a, b| b.pop_max.cmp(&a.pop_max));
    Ok(cities)
}

/// Parses a GMT GSHHG binary `.b` shoreline/lake file (44-byte big-endian header per polygon).
pub fn parse_gshhg_b(bytes: &[u8], max_polygons: usize) -> Vec<WaterPolygon> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 44 <= bytes.len() && out.len() < max_polygons {
        let _id = i32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap());
        let n = i32::from_be_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let flag = i32::from_be_bytes(bytes[pos + 8..pos + 12].try_into().unwrap()) as u32;
        let west = i32::from_be_bytes(bytes[pos + 12..pos + 16].try_into().unwrap()) as f32 * 1e-6;
        let east = i32::from_be_bytes(bytes[pos + 16..pos + 20].try_into().unwrap()) as f32 * 1e-6;
        let south = i32::from_be_bytes(bytes[pos + 20..pos + 24].try_into().unwrap()) as f32 * 1e-6;
        let north = i32::from_be_bytes(bytes[pos + 24..pos + 28].try_into().unwrap()) as f32 * 1e-6;
        pos += 44;
        let needed = n.saturating_mul(8);
        if n == 0 || pos + needed > bytes.len() {
            break;
        }
        let level = (flag & 0xff) as u8;
        let step = (n / 512).max(1);
        let mut points = Vec::with_capacity((n / step).min(512) + 1);
        for i in (0..n).step_by(step) {
            let p = pos + i * 8;
            let raw_lon = i32::from_be_bytes(bytes[p..p + 4].try_into().unwrap()) as f32 * 1e-6;
            let lon = if raw_lon > 180.0 {
                raw_lon - 360.0
            } else {
                raw_lon
            };
            let lat = i32::from_be_bytes(bytes[p + 4..p + 8].try_into().unwrap()) as f32 * 1e-6;
            points.push([lon, lat]);
        }
        pos += needed;
        if points.len() >= 3 {
            out.push(WaterPolygon {
                level,
                bounds: [west, south, east, north],
                points,
            });
        }
    }
    out
}

fn find_first_with_ext(dir: &Path, exts: &[&str]) -> Option<PathBuf> {
    let directory = AssetDirectory::open(dir).ok()?;
    let entries = fs::read_dir(directory.path()).ok()?;
    for entry in entries.flatten() {
        let p = dir.join(entry.file_name());
        if p.is_file()
            && let Some(ext) = p.extension().and_then(|e| e.to_str())
            && exts.iter().any(|want| ext.eq_ignore_ascii_case(want))
        {
            return Some(p);
        }
    }
    None
}

fn find_gshhg_file(dir: &Path) -> Option<PathBuf> {
    for name in [
        "gshhs_i.b",
        "gshhs_l.b",
        "gshhs_c.b",
        "gshhg/gshhs_i.b",
        "gshhg/gshhs_l.b",
        "gshhg/gshhs_c.b",
    ] {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn load_cities_from_dir(dir: &Path) -> Vec<CityPoint> {
    let compact = dir.join("cities_all.json");
    if let Ok(bytes) = storage::read(&compact)
        && let Ok(cities) = serde_json::from_slice::<Vec<CityPoint>>(&bytes)
    {
        return cities;
    }
    let raw = dir.join("ne_10m_populated_places_simple.geojson");
    if let Ok(bytes) = storage::read(&raw)
        && let Ok(cities) = parse_cities_geojson(&bytes)
    {
        if let Ok(encoded) = serde_json::to_vec(&cities) {
            let _ = storage::write(&compact, encoded);
        }
        return cities;
    }
    let legacy = dir.join("cities_250k.json");
    if let Ok(bytes) = storage::read(&legacy)
        && let Ok(cities) = serde_json::from_slice::<Vec<CityPoint>>(&bytes)
    {
        return cities;
    }
    Vec::new()
}

/// Parses Natural Earth 1:10m Admin 0 / Admin 1 boundary lines GeoJSON into compact polylines,
/// flagging disputed/indefinite/lease boundaries for distinct styling.
pub fn parse_borders_geojson(bytes: &[u8]) -> Result<Vec<BorderLine>, String> {
    let root: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| format!("Invalid borders GeoJSON: {e}"))?;
    let features = root
        .get("features")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Borders GeoJSON missing features array".to_string())?;
    let mut lines = Vec::new();
    for feat in features {
        let props = feat.get("properties").and_then(|v| v.as_object());
        let featurecla = props
            .and_then(|p| p.get("FEATURECLA").or_else(|| p.get("featurecla")))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let disputed = featurecla.contains("disput")
            || featurecla.contains("indefin")
            || featurecla.contains("claim")
            || featurecla.contains("lease");
        let Some(geom) = feat.get("geometry") else {
            continue;
        };
        let gtype = geom.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let Some(coords) = geom.get("coordinates").and_then(|v| v.as_array()) else {
            continue;
        };
        let push_ring = |ring: &[serde_json::Value], out: &mut Vec<BorderLine>| {
            let step = (ring.len() / 256).max(1);
            let mut pts = Vec::with_capacity((ring.len() / step).min(256) + 1);
            for (idx, pt) in ring.iter().enumerate() {
                if (idx % step != 0) && idx + 1 != ring.len() {
                    continue;
                }
                if let Some(arr) = pt.as_array()
                    && arr.len() >= 2
                    && let (Some(lon), Some(lat)) = (arr[0].as_f64(), arr[1].as_f64())
                {
                    pts.push([lon as f32, lat as f32]);
                }
            }
            if pts.len() >= 2 {
                out.push(BorderLine {
                    disputed,
                    points: pts,
                });
            }
        };
        if gtype == "LineString" {
            push_ring(coords, &mut lines);
        } else if gtype == "MultiLineString" {
            for part in coords {
                if let Some(ring) = part.as_array() {
                    push_ring(ring, &mut lines);
                }
            }
        }
    }
    Ok(lines)
}

fn load_borders_from_dir(dir: &Path, compact_name: &str, raw_name: &str) -> Vec<BorderLine> {
    let compact = dir.join(compact_name);
    if let Ok(bytes) = storage::read(&compact)
        && let Ok(lines) = serde_json::from_slice::<Vec<BorderLine>>(&bytes)
    {
        return lines;
    }
    let raw = dir.join(raw_name);
    if let Ok(bytes) = storage::read(&raw)
        && let Ok(lines) = parse_borders_geojson(&bytes)
    {
        if let Ok(encoded) = serde_json::to_vec(&lines) {
            let _ = storage::write(&compact, encoded);
        }
        return lines;
    }
    Vec::new()
}

/// Parses Natural Earth 1:10m roads GeoJSON into compact polylines with bounding boxes.
pub fn parse_roads_geojson(bytes: &[u8]) -> Result<Vec<RoadLine>, String> {
    let root: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| format!("Invalid roads GeoJSON: {e}"))?;
    let features = root
        .get("features")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Roads GeoJSON missing features array".to_string())?;
    let mut roads = Vec::new();
    for feat in features {
        let props = feat.get("properties").and_then(|v| v.as_object());
        let kind = props
            .and_then(|p| p.get("type").or_else(|| p.get("TYPE")))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let major = kind.contains("major") || kind.contains("beltway") || kind.contains("motorway");
        let Some(geom) = feat.get("geometry") else {
            continue;
        };
        let gtype = geom.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let Some(coords) = geom.get("coordinates").and_then(|v| v.as_array()) else {
            continue;
        };
        let push_ring = |ring: &[serde_json::Value], out: &mut Vec<RoadLine>| {
            let step = (ring.len() / 64).max(1);
            let mut pts = Vec::with_capacity((ring.len() / step).min(64) + 1);
            let mut b = [180.0_f32, 90.0_f32, -180.0_f32, -90.0_f32];
            for (idx, pt) in ring.iter().enumerate() {
                if (idx % step != 0) && idx + 1 != ring.len() {
                    continue;
                }
                if let Some(arr) = pt.as_array()
                    && arr.len() >= 2
                    && let (Some(lon), Some(lat)) = (arr[0].as_f64(), arr[1].as_f64())
                {
                    let (lon, lat) = (lon as f32, lat as f32);
                    b[0] = b[0].min(lon);
                    b[1] = b[1].min(lat);
                    b[2] = b[2].max(lon);
                    b[3] = b[3].max(lat);
                    pts.push([lon, lat]);
                }
            }
            if pts.len() >= 2 {
                out.push(RoadLine {
                    major,
                    bounds: b,
                    points: pts,
                });
            }
        };
        if gtype == "LineString" {
            push_ring(coords, &mut roads);
        } else if gtype == "MultiLineString" {
            for part in coords {
                if let Some(ring) = part.as_array() {
                    push_ring(ring, &mut roads);
                }
            }
        }
    }
    Ok(roads)
}

fn load_roads_from_dir(dir: &Path) -> Vec<RoadLine> {
    let compact = dir.join("roads_10m.json");
    if let Ok(bytes) = storage::read(&compact)
        && let Ok(lines) = serde_json::from_slice::<Vec<RoadLine>>(&bytes)
    {
        return lines;
    }
    let raw = dir.join("ne_10m_roads.geojson");
    if let Ok(bytes) = storage::read(&raw)
        && let Ok(lines) = parse_roads_geojson(&bytes)
    {
        if let Ok(encoded) = serde_json::to_vec(&lines) {
            let _ = storage::write(&compact, encoded);
        }
        return lines;
    }
    Vec::new()
}

/// Parses Natural Earth 1:10m lakes GeoJSON into compact polygons with bounding boxes.
pub fn parse_lakes_geojson(bytes: &[u8]) -> Result<Vec<LakePolygon>, String> {
    let root: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| format!("Invalid lakes GeoJSON: {e}"))?;
    let features = root
        .get("features")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Lakes GeoJSON missing features array".to_string())?;
    let mut lakes = Vec::new();
    for feat in features {
        let Some(geom) = feat.get("geometry") else {
            continue;
        };
        let gtype = geom.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let Some(coords) = geom.get("coordinates").and_then(|v| v.as_array()) else {
            continue;
        };
        let push_poly = |poly: &[serde_json::Value], out: &mut Vec<LakePolygon>| {
            let Some(outer) = poly.first().and_then(|v| v.as_array()) else {
                return;
            };
            let step = (outer.len() / 128).max(1);
            let mut pts = Vec::with_capacity((outer.len() / step).min(128) + 1);
            let mut b = [180.0_f32, 90.0_f32, -180.0_f32, -90.0_f32];
            for (idx, pt) in outer.iter().enumerate() {
                if (idx % step != 0) && idx + 1 != outer.len() {
                    continue;
                }
                if let Some(arr) = pt.as_array()
                    && arr.len() >= 2
                    && let (Some(lon), Some(lat)) = (arr[0].as_f64(), arr[1].as_f64())
                {
                    let (lon, lat) = (lon as f32, lat as f32);
                    b[0] = b[0].min(lon);
                    b[1] = b[1].min(lat);
                    b[2] = b[2].max(lon);
                    b[3] = b[3].max(lat);
                    pts.push([lon, lat]);
                }
            }
            if pts.len() >= 3 {
                let holes = poly
                    .iter()
                    .skip(1)
                    .filter_map(|ring| ring.as_array())
                    .map(|ring| {
                        ring.iter()
                            .filter_map(|v| {
                                let a = v.as_array()?;
                                Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32])
                            })
                            .collect::<Vec<_>>()
                    })
                    .filter(|ring| ring.len() >= 3)
                    .collect();
                out.push(LakePolygon {
                    bounds: b,
                    points: pts,
                    holes,
                });
            }
        };
        if gtype == "Polygon" {
            push_poly(coords, &mut lakes);
        } else if gtype == "MultiPolygon" {
            for poly in coords {
                if let Some(rings) = poly.as_array() {
                    push_poly(rings, &mut lakes);
                }
            }
        }
    }
    Ok(lakes)
}

fn load_lakes_from_dir(dir: &Path) -> Vec<LakePolygon> {
    let compact = dir.join("lakes_10m_v2.json");
    if let Ok(bytes) = storage::read(&compact)
        && let Ok(polys) = serde_json::from_slice::<Vec<LakePolygon>>(&bytes)
    {
        return polys;
    }
    let raw = dir.join("ne_10m_lakes.geojson");
    if let Ok(bytes) = storage::read(&raw)
        && let Ok(polys) = parse_lakes_geojson(&bytes)
    {
        if let Ok(encoded) = serde_json::to_vec(&polys) {
            let _ = storage::write(&compact, encoded);
        }
        return polys;
    }
    Vec::new()
}

fn load_ne2_overview(
    dir: &Path,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Option<EquirectTexture> {
    let cached = dir.join("ne2_overview.rgb");
    if let Ok(bytes) = storage::read(&cached)
        && bytes.len() >= 8
    {
        let w = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
        let h = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
        if grid_len(w, h, 3).and_then(|n| n.checked_add(8)) == Some(bytes.len()) {
            return Some(EquirectTexture {
                width: w,
                height: h,
                rgb: Arc::new(bytes[8..].to_vec()),
            });
        }
    }
    let img_path = find_first_with_ext(dir, &["png", "jpg", "jpeg", "tif", "tiff"])?;
    let file = crate::kernel_interface::open_content(&img_path, true).ok()?;
    let rgb = if let Some(sampler) = Ne2FullSampler::open(&img_path) {
        let mut rgb = Vec::with_capacity(4096 * 2048 * 3);
        for y in 0..2048 {
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                return None;
            }
            for x in 0..4096 {
                rgb.extend_from_slice(&sampler.sample_rgb(
                    90.0 - (y as f64 + 0.5) * 180.0 / 2048.0,
                    (x as f64 + 0.5) * 360.0 / 4096.0 - 180.0,
                )?);
            }
        }
        rgb
    } else {
        let mut reader = image::ImageReader::new(BufReader::new(file))
            .with_guessed_format()
            .ok()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(32768);
        limits.max_image_height = Some(16384);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        let decoded = reader.decode().ok()?;
        decoded
            .resize_exact(4096, 2048, image::imageops::FilterType::Triangle)
            .to_rgb8()
            .into_raw()
    };
    let mut blob = Vec::with_capacity(8 + rgb.len());
    blob.extend_from_slice(&4096u32.to_le_bytes());
    blob.extend_from_slice(&2048u32.to_le_bytes());
    blob.extend_from_slice(&rgb);
    let _ = storage::write(&cached, &blob);
    Some(EquirectTexture {
        width: 4096,
        height: 2048,
        rgb: Arc::new(rgb),
    })
}

fn load_hgt_dir(dir: &Path, out: &mut HgtStore) {
    let Ok(directory) = AssetDirectory::open(dir).map(Arc::new) else {
        return;
    };
    let Ok(entries) = fs::read_dir(directory.path()) else {
        return;
    };
    for entry in entries.take(65_536).flatten() {
        let path = entry.path();
        if !path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("hgt"))
        {
            continue;
        }
        let Some(key) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(parse_hgt_name)
        else {
            continue;
        };
        let Ok(file) = directory.open_file(&entry.file_name()) else {
            continue;
        };
        let Ok(metadata) = file.metadata() else {
            continue;
        };
        if metadata.len() <= 3601 * 3601 * 2 {
            out.sources
                .entry(key)
                .or_insert_with(|| (directory.clone(), entry.file_name()));
        }
    }
}

fn parse_hgt_name(stem: &str) -> Option<(i32, i32)> {
    let b = stem.as_bytes();
    if b.len() != 7 || !b.is_ascii() || !b[1..3].iter().chain(&b[4..7]).all(u8::is_ascii_digit) {
        return None;
    }
    let ns = b[0] as char;
    let ew = b[3] as char;
    let lat: i32 = stem[1..3].parse().ok()?;
    let lon: i32 = stem[4..7].parse().ok()?;
    let lat = match ns.to_ascii_uppercase() {
        'N' => lat,
        'S' => -lat,
        _ => return None,
    };
    let lon = match ew.to_ascii_uppercase() {
        'E' => lon,
        'W' => -lon,
        _ => return None,
    };
    ((-90..90).contains(&lat) && (-180..180).contains(&lon)).then_some((lat, lon))
}

fn load_global_dem(dir: &Path) -> Option<GlobalDemGrid> {
    let path = dir.join("gebco_overview.i16");
    if let Ok(bytes) = storage::read(&path)
        && bytes.len() >= 8
    {
        let width = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
        let height = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
        let expected = grid_len(width, height, 2)?.checked_add(8)?;
        if bytes.len() == expected {
            let heights = bytes[8..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| i16::from_le_bytes(*p))
                .collect();
            return Some(GlobalDemGrid {
                width,
                height,
                heights,
            });
        }
    }

    // Extract an overview IFD (around 2400..=6000 px wide) from downloaded GEBCO GeoTIFF.
    let tif_path = find_first_with_ext(&dir.join("gebco"), &["tif", "tiff"])?;
    let file = crate::kernel_interface::open_content(&tif_path, true).ok()?;
    let mut decoder = tiff::decoder::Decoder::new(file)
        .ok()?
        .with_limits(tiff_limits(128 * 1024 * 1024));
    let mut best_ifd = None;
    let mut best_diff = u32::MAX;
    for idx in 0..16usize {
        if idx > 0 && decoder.seek_to_image(idx).is_err() {
            break;
        }
        if let Ok((w, h)) = decoder.dimensions() {
            let diff = w.abs_diff(5400);
            if w <= 6000 && grid_len(w, h, 4).is_some() && diff < best_diff {
                best_diff = diff;
                best_ifd = Some(idx);
            }
        }
    }
    // A plain global TIFF without an overview is sampled by chunks on demand.
    // Never fall back to allocating its full-resolution image.
    decoder.seek_to_image(best_ifd?).ok()?;
    let (width, height) = decoder.dimensions().ok()?;
    let decoded = decoder.read_image().ok()?;
    let heights: Vec<i16> = match decoded {
        tiff::decoder::DecodingResult::I16(v) => v,
        tiff::decoder::DecodingResult::U16(v) => v.into_iter().map(|x| x as i16).collect(),
        tiff::decoder::DecodingResult::I32(v) => v
            .into_iter()
            .map(|x| x.clamp(-12000, 10000) as i16)
            .collect(),
        tiff::decoder::DecodingResult::F32(v) => v
            .into_iter()
            .map(|x| x.round().clamp(-12000.0, 10000.0) as i16)
            .collect(),
        _ => return None,
    };
    if heights.len() != (width as usize) * (height as usize) {
        return None;
    }
    let mut blob = Vec::with_capacity(8 + heights.len() * 2);
    blob.extend_from_slice(&width.to_le_bytes());
    blob.extend_from_slice(&height.to_le_bytes());
    for h in &heights {
        blob.extend_from_slice(&h.to_le_bytes());
    }
    let _ = storage::write(&path, &blob);
    Some(GlobalDemGrid {
        width,
        height,
        heights,
    })
}

fn load_land_cover(dir: &Path) -> Option<GlobalLandCoverGrid> {
    let path = dir.join("land_cover.u8");
    let bytes = storage::read(path).ok()?;
    if bytes.len() < 8 {
        return None;
    }
    let width = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    if grid_len(width, height, 1).and_then(|n| n.checked_add(8)) != Some(bytes.len()) {
        return None;
    }
    Some(GlobalLandCoverGrid {
        width,
        height,
        classes: bytes[8..].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_status_requires_usable_data_and_both_quick_start_layers() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("hydrorivers")).unwrap();
        fs::create_dir(root.path().join("gebco")).unwrap();
        for name in [
            "land_cover_lccs_class_2020_v2_1_1.nc",
            "land_cover.u8",
            "gebco/broken.tif",
            "ne2_overview.rgb",
        ] {
            fs::write(root.path().join(name), b"broken").unwrap();
        }
        let status = BasemapAssets::load(root.path(), None).status();
        assert!(
            !status.has_hydrorivers
                && !status.has_gebco
                && !status.has_land_cover
                && !status.has_ne2
        );
        let mut status = AssetStatus {
            has_ne2: true,
            ..AssetStatus::default()
        };
        assert!(!status.has_quick_start());
        status.has_cities = true;
        assert!(status.has_quick_start());
    }

    #[test]
    fn plain_dem_without_an_overview_is_sampled_by_chunks() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("gebco")).unwrap();
        let path = root.path().join("gebco/plain.tif");
        let file = fs::File::create(&path).unwrap();
        let mut encoder = tiff::encoder::TiffEncoder::new(file).unwrap();
        let mut image = encoder
            .new_image::<tiff::encoder::colortype::GrayI16>(6001, 4)
            .unwrap();
        image.rows_per_strip(1).unwrap();
        image.write_data(&vec![123i16; 6001 * 4]).unwrap();
        drop(encoder);
        assert!(load_global_dem(root.path()).is_none());
        let pyramid = CogDemPyramid::open(&path).unwrap();
        assert_eq!(pyramid.sample_at(0.0, 0.0, 5400.0), Some(123.0));
        assert!(
            pyramid
                .inner
                .lock()
                .unwrap()
                .cache
                .values()
                .map(|v| v.capacity() * 2)
                .sum::<usize>()
                <= 64 * 1024 * 1024
        );
    }

    #[test]
    fn masks_decode_only_requested_strips_and_index_coverage() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("mask.tif");
        let file = fs::File::create(&path).unwrap();
        let mut encoder = tiff::encoder::TiffEncoder::new(file).unwrap();
        let mut image = encoder
            .new_image::<tiff::encoder::colortype::Gray8>(8, 8)
            .unwrap();
        image.rows_per_strip(2).unwrap();
        let pixels: Vec<_> = (0..64).map(|i| if i % 8 < 4 { 255 } else { 0 }).collect();
        image.write_data(&pixels).unwrap();
        drop(encoder);
        let store = GrwlMaskStore::from_entries(
            AssetDirectory::open(root.path()).unwrap(),
            vec![GrwlMaskEntry {
                file: "mask.tif".into(),
                width: 8,
                height: 8,
                min_lon: 0.0,
                max_lon: 8.0,
                min_lat: 0.0,
                max_lat: 8.0,
            }],
        )
        .unwrap();
        assert_eq!(store.sample_water(7.5, 0.5), Some(true));
        assert_eq!(store.sample_water(7.5, 5.5), Some(false));
        assert_eq!(store.sample_water(0.5, 0.5), Some(true));
        assert_eq!(store.sample_water(30.0, 30.0), None);
        assert_eq!(
            store
                .inner
                .lock()
                .unwrap()
                .cache
                .values()
                .map(|v| v.len())
                .sum::<usize>(),
            32
        );
    }

    #[test]
    fn local_hgt_inventory_is_lazy_and_cache_eviction_accounts_for_bytes() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("N00E000.hgt"),
            [0, 10, 0, 10, 0, 10, 0, 10],
        )
        .unwrap();
        let mut store = HgtStore::default();
        load_hgt_dir(root.path(), &mut store);
        assert_eq!(store.sources.len(), 1);
        assert!(store.cache.lock().unwrap().0.is_empty());
        assert_eq!(store.sample(0.5, 0.5), Some(10.0));
        let mut cache = HashMap::new();
        let mut order = VecDeque::new();
        for key in 0..10 {
            cache_chunk(&mut cache, &mut order, key, vec![0u8; 6], 10).unwrap();
        }
        assert_eq!(cache.len(), 1);
        assert_eq!(order.len(), 1);
        assert!(cache_chunk(&mut cache, &mut order, 10, vec![0u8; 11], 10).is_none());
    }

    #[test]
    fn malformed_grids_and_unicode_tile_names_are_rejected() {
        for name in [
            "é12E123",
            "Né2E123",
            "N90E180",
            "N12E999",
            "N12E123extra",
            "N1xE123",
        ] {
            assert_eq!(parse_hgt_name(name), None, "{name}");
        }
        assert_eq!(parse_hgt_name("s12w123"), Some((-12, -123)));
        let root = tempfile::tempdir().unwrap();
        for (w, h) in [(0u32, 0u32), (u32::MAX, u32::MAX), (1, 0), (100, 100)] {
            let mut bytes = w.to_le_bytes().to_vec();
            bytes.extend(h.to_le_bytes());
            for name in ["land_cover.u8", "gebco_overview.i16", "ne2_overview.rgb"] {
                fs::write(root.path().join(name), &bytes).unwrap();
            }
            assert!(load_land_cover(root.path()).is_none());
            assert!(load_global_dem(root.path()).is_none());
            assert!(
                load_ne2_overview(root.path(), &std::sync::atomic::AtomicBool::new(false))
                    .is_none()
            );
        }
        assert!(
            GlobalDemGrid {
                width: 0,
                height: 0,
                heights: vec![]
            }
            .sample(0.0, 0.0)
            .is_none()
        );
        assert!(
            SrtmTile {
                lat_floor: 0,
                lon_floor: 0,
                samples: 0,
                heights: Arc::new(vec![])
            }
            .sample(0.0, 0.0)
            .is_none()
        );
    }

    #[test]
    fn river_caches_reject_unbounded_counts_truncation_and_invalid_references() {
        let grwl = GrwlSummaryIndex {
            segments: vec![GrwlSegment {
                width_med_m: 10.0,
                points: vec![[0.0, 0.0], [1.0, 1.0]],
            }],
            cells: vec![vec![]; 180 * 90],
        };
        let hydro = HydroRiversIndex {
            segments: vec![RiverSegment {
                ord_stra: 3,
                dis_cms: 10.0,
                points: vec![[0.0, 0.0], [1.0, 1.0]],
            }],
            cells: vec![vec![]; 180 * 90],
        };
        let g = grwl.encode_bin().unwrap();
        let h = hydro.encode_bin().unwrap();
        for bytes in [&g, &h] {
            for end in [0, 1, 4, 20, bytes.len() - 1] {
                assert!(GrwlSummaryIndex::decode_bin(&bytes[..end]).is_none());
                assert!(HydroRiversIndex::decode_bin(&bytes[..end]).is_none());
            }
        }
        for count in [u32::MAX, 1_000_001] {
            assert!(GrwlSummaryIndex::decode_bin(&count.to_le_bytes()).is_none());
            assert!(HydroRiversIndex::decode_bin(&count.to_le_bytes()).is_none());
        }
        let mut invalid = grwl;
        invalid.cells[0].push(1);
        assert!(GrwlSummaryIndex::decode_bin(&invalid.encode_bin().unwrap()).is_none());
        let mut invalid = hydro;
        invalid.cells[0].push(1);
        assert!(HydroRiversIndex::decode_bin(&invalid.encode_bin().unwrap()).is_none());
    }

    #[test]
    fn oilslick_has_sharp_sea_level_contrast_and_500m_cycle() {
        let ocean = oilslick_color(0.0, 0.8);
        let coast = oilslick_color(1.0, 0.8);
        assert_eq!(ocean, [24, 58, 102]);
        assert_ne!(ocean, coast);
        // 500 m cyclic palette repeats hue/lightness structure while shifting hue
        let low = oilslick_color(250.0, 1.0);
        let high = oilslick_color(500.0, 1.0);
        assert_ne!(low, high);
    }

    #[test]
    fn globe_camera_transitions_smoothly_between_3d_and_2d() {
        let mut cam = GlobeCamera {
            center_lat: 35.0,
            center_lon: -105.0,
            zoom: 1.0,
            mode: ProjectionMode::Auto,
        };
        assert_eq!(cam.globe_blend(), 1.0);
        // Point on the opposite side of the Earth is culled on the 3D globe.
        assert!(cam.project(-35.0, 75.0, 800.0, 600.0).is_none());
        // Center point projects to center of viewport.
        let center = cam.project(35.0, -105.0, 800.0, 600.0).unwrap();
        assert!((center[0] - 400.0).abs() < 1.0);
        assert!((center[1] - 300.0).abs() < 1.0);

        cam.zoom = 3.15;
        let mid = cam.globe_blend();
        assert!(mid > 0.0 && mid < 1.0);

        cam.zoom = 5.0;
        assert_eq!(cam.globe_blend(), 0.0);
    }

    #[test]
    fn blended_projection_roundtrips_visible_points_across_the_transition() {
        for center_lat in [-70.0, 0.0, 35.0, 70.0] {
            for center_lon in [-179.0, -105.0, 179.0] {
                for zoom in [1.0, 1.8, 2.0, 2.5, 3.15, 3.8, 4.49, 5.0, 512.0] {
                    let camera = GlobeCamera {
                        center_lat,
                        center_lon,
                        zoom,
                        mode: ProjectionMode::Auto,
                    };
                    for dy in [-40.0, -15.0, 0.0, 15.0, 40.0] {
                        for dx in [-80.0, -20.0, 0.0, 20.0, 80.0] {
                            let lat = (center_lat + dy).clamp(-89.5, 89.5);
                            let lon = wrap_lon(center_lon + dx);
                            let Some(p) = camera.project(lat, lon, 960.0, 720.0) else {
                                continue;
                            };
                            let result = camera.unproject(p[0], p[1], 960.0, 720.0);
                            let (actual_lat, actual_lon) = result.unwrap_or_else(|| {
                                panic!("inverse missing for {camera:?}, point {lat},{lon} at {p:?}")
                            });
                            let q = camera
                                .project(actual_lat, actual_lon, 960.0, 720.0)
                                .unwrap();
                            assert!(
                                (p[0] - q[0]).hypot(p[1] - q[1]) < 0.02,
                                "{camera:?}: {p:?} -> {q:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    fn multipart_shp() -> Vec<u8> {
        let mut record = Vec::new();
        record.extend_from_slice(&3i32.to_le_bytes());
        for value in [0.0_f64, 0.0, 10.1, 10.0] {
            record.extend_from_slice(&value.to_le_bytes());
        }
        record.extend_from_slice(&2i32.to_le_bytes());
        record.extend_from_slice(&4i32.to_le_bytes());
        record.extend_from_slice(&0i32.to_le_bytes());
        record.extend_from_slice(&2i32.to_le_bytes());
        for value in [0.0_f64, 0.0, 0.1, 0.0, 10.0, 10.0, 10.1, 10.0] {
            record.extend_from_slice(&value.to_le_bytes());
        }
        let mut bytes = vec![0u8; 100];
        bytes[..4].copy_from_slice(&9994i32.to_be_bytes());
        bytes[24..28].copy_from_slice(&(((108 + record.len()) / 2) as i32).to_be_bytes());
        bytes[28..32].copy_from_slice(&1000i32.to_le_bytes());
        bytes[32..36].copy_from_slice(&3i32.to_le_bytes());
        bytes[36..68].copy_from_slice(&record[4..36]);
        bytes.extend_from_slice(&1i32.to_be_bytes());
        bytes.extend_from_slice(&((record.len() / 2) as i32).to_be_bytes());
        bytes.extend(record);
        bytes
    }
    fn test_dbf(fields: &[(&str, &str)]) -> Vec<u8> {
        let header = 33 + fields.len() * 32;
        let mut bytes = vec![0u8; 32];
        bytes[0] = 3;
        bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
        bytes[8..10].copy_from_slice(&(header as u16).to_le_bytes());
        bytes[10..12].copy_from_slice(&((1 + fields.len() * 12) as u16).to_le_bytes());
        for (name, _) in fields {
            let mut field = [0u8; 32];
            field[..name.len()].copy_from_slice(name.as_bytes());
            field[11] = b'N';
            field[16] = 12;
            bytes.extend_from_slice(&field);
        }
        bytes.push(0x0d);
        bytes.push(b' ');
        for (_, value) in fields {
            bytes.extend(std::iter::repeat_n(b' ', 12 - value.len()));
            bytes.extend_from_slice(value.as_bytes());
        }
        bytes
    }
    #[test]
    fn both_river_loaders_preserve_multipart_boundaries_and_cache_them() {
        let root = tempfile::tempdir().unwrap();
        for name in ["grwl", "hydrorivers"] {
            fs::create_dir(root.path().join(name)).unwrap();
            fs::write(root.path().join(name).join("test.shp"), multipart_shp()).unwrap();
        }
        fs::write(
            root.path().join("grwl/test.dbf"),
            test_dbf(&[("width_med", "120.0")]),
        )
        .unwrap();
        fs::write(
            root.path().join("hydrorivers/test.dbf"),
            test_dbf(&[("ORD_STRA", "6"), ("DIS_AV_CMS", "100.0")]),
        )
        .unwrap();
        for round in 0..2 {
            let grwl = GrwlSummaryIndex::load_or_build(root.path()).unwrap();
            let hydro = HydroRiversIndex::load_or_build(root.path()).unwrap();
            assert_eq!(grwl.segments().len(), 2);
            assert_eq!(hydro.segments().len(), 2);
            assert_eq!(grwl.segments()[0].points, vec![[0.0, 0.0], [0.1, 0.0]]);
            assert_eq!(grwl.segments()[1].points, vec![[10.0, 10.0], [10.1, 10.0]]);
            assert_eq!(grwl.segments()[1].width_med_m, 120.0);
            assert_eq!(hydro.segments()[1].ord_stra, 6);
            assert_eq!(hydro.segments()[1].dis_cms, 100.0);
            assert_eq!(grwl.query_bbox(-0.5, -0.5, 0.5, 0.5, 0.0, 10).len(), 1);
            assert_eq!(hydro.query_bbox(9.5, 9.5, 10.5, 10.5, 3, 10).len(), 1);
            if round == 0 {
                for name in ["grwl", "hydrorivers"] {
                    fs::remove_file(root.path().join(name).join("test.shp")).unwrap();
                }
            }
        }
    }
    #[test]
    fn multipart_offsets_are_checked_before_sampling() {
        let mut file = multipart_shp();
        file[108 + 48..108 + 52].copy_from_slice(&0i32.to_le_bytes());
        assert!(shp_polyline_parts(&file[108..], 32).is_none());
    }

    #[test]
    fn fitting_dateline_documents_keeps_the_track_visible() {
        let document = crate::geography::Document::geojson(
            br#"{"type":"LineString","coordinates":[[179,0],[-179,0]]}"#,
        )
        .unwrap();
        let mut camera = GlobeCamera::default();
        camera.fit_bounds(document.geographic_bounds);
        assert!(wrap_lon_delta(camera.center_lon - 180.0).abs() < 1e-8);
        assert!(camera.zoom > 10.0);
        for point in document.features.iter().flat_map(|f| &f.paths).flatten() {
            let p = camera
                .project(point.point.lat, point.point.lon, 960.0, 720.0)
                .unwrap();
            assert!((0.0..960.0).contains(&p[0]) && (0.0..720.0).contains(&p[1]));
        }
        camera.mode = ProjectionMode::Globe3D;
        camera.fit_bounds(document.geographic_bounds);
        assert_eq!(camera.mode, ProjectionMode::Globe3D);
        camera.fit_bounds(None);
        assert_eq!(camera.zoom, 1.0);
    }

    #[test]
    fn inverse_rejects_background_pixels_and_invalid_viewports() {
        let mut camera = GlobeCamera::default();
        assert!(camera.unproject(0.0, 0.0, 960.0, 720.0).is_none());
        camera.mode = ProjectionMode::Map2D;
        assert!(camera.unproject(480.0, -10000.0, 960.0, 720.0).is_none());
        assert!(camera.unproject(f32::NAN, 0.0, 960.0, 720.0).is_none());
        assert!(camera.unproject(0.0, 0.0, 0.0, 0.0).is_none());
    }

    #[test]
    fn lake_parser_preserves_island_rings_for_gpu_tessellation() {
        let json=br#"{"features":[{"geometry":{"type":"Polygon","coordinates":[[[0,0],[4,0],[4,4],[0,4],[0,0]],[[1,1],[2,1],[2,2],[1,2],[1,1]]]}}]}"#;
        let lakes = parse_lakes_geojson(json).unwrap();
        assert_eq!(lakes.len(), 1);
        assert_eq!(lakes[0].holes.len(), 1);
        assert_eq!(lakes[0].holes[0][0], [1.0, 1.0]);
    }

    #[test]
    fn parses_all_cities_and_sorts_largest_first() {
        let json = br#"{
          "type": "FeatureCollection",
          "features": [
            {"type":"Feature","properties":{"NAME":"SmallTown","POP_MAX":120000},"geometry":{"type":"Point","coordinates":[0.0,0.0]}},
            {"type":"Feature","properties":{"NAME":"MidCity","POP_MAX":300000},"geometry":{"type":"Point","coordinates":[1.0,2.0]}},
            {"type":"Feature","properties":{"NAME":"MegaCity","POP_MAX":9500000},"geometry":{"type":"Point","coordinates":[139.7,35.6]}}
          ]
        }"#;
        let cities = parse_cities_geojson(json).unwrap();
        assert_eq!(cities.len(), 3);
        assert_eq!(cities[0].name, "MegaCity");
        assert_eq!(cities[1].name, "MidCity");
        assert_eq!(cities[2].name, "SmallTown");
    }
}
