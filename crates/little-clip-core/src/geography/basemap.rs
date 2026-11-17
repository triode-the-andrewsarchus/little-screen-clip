//! Offline 3D-globe / 2D-map projection, Oilslick elevation shading, and basemap data loaders.

use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

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
    pub heights: Vec<i16>,
}

impl SrtmTile {
    /// Parses a standard SRTM1 (`3601×3601`) or SRTM3 (`1201×1201`) `.hgt` file.
    pub fn from_hgt_bytes(lat_floor: i32, lon_floor: i32, bytes: &[u8]) -> Option<Self> {
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
            heights,
        })
    }

    /// Samples elevation in meters at `(lat, lon)` inside this 1°×1° tile.
    pub fn sample(&self, lat: f64, lon: f64) -> Option<f32> {
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

/// Optional global/local elevation & land-cover store.
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
    pub terrain_tiles: HashMap<(i32, i32), SrtmTile>,
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
    pub fn load_or_build(dir: &Path) -> Option<Self> {
        let cache_path = dir.join("grwl_summary.bin");
        if let Ok(bytes) = fs::read(&cache_path)
            && let Some(idx) = Self::decode_bin(&bytes)
        {
            return Some(idx);
        }
        let grwl_dir = dir.join("grwl");
        let shp_path = find_first_with_ext(&grwl_dir, &["shp"])?;
        let dbf_path = shp_path.with_extension("dbf");
        let built = Self::build_from_shp(&shp_path, &dbf_path)?;
        if let Ok(encoded) = built.encode_bin() {
            let _ = fs::write(&cache_path, encoded);
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
        if let Ok(dbf_file) = fs::File::open(dbf_path) {
            let mut rdr = BufReader::new(dbf_file);
            let mut hdr = [0u8; 32];
            if rdr.read_exact(&mut hdr).is_ok() {
                let header_len = u16::from_le_bytes([hdr[8], hdr[9]]) as usize;
                let rec_len = u16::from_le_bytes([hdr[10], hdr[11]]) as usize;
                let n_recs = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
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

        let shp_file = fs::File::open(shp_path).ok()?;
        let mut rdr = BufReader::new(shp_file);
        let mut hdr = [0u8; 100];
        rdr.read_exact(&mut hdr).ok()?;
        let mut segments = Vec::new();
        let mut cells = vec![Vec::new(); 180 * 90];
        let mut rec_hdr = [0u8; 8];
        let mut rec_idx = 0usize;
        while rdr.read_exact(&mut rec_hdr).is_ok() {
            let content_words = i32::from_be_bytes(rec_hdr[4..8].try_into().ok()?) as usize;
            let content_bytes = content_words.saturating_mul(2);
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
            let num_parts = i32::from_le_bytes(buf[36..40].try_into().ok()?) as usize;
            let num_points = i32::from_le_bytes(buf[40..44].try_into().ok()?) as usize;
            let pts_off = 44 + num_parts.saturating_mul(4);
            if num_parts == 0 || num_points < 2 || pts_off + num_points * 16 > buf.len() {
                continue;
            }
            let step = (num_points / 32).max(1);
            let mut points = Vec::with_capacity((num_points / step).min(32) + 1);
            for i in (0..num_points).step_by(step) {
                let p = pts_off + i * 16;
                let lon = f64::from_le_bytes(buf[p..p + 8].try_into().ok()?) as f32;
                let lat = f64::from_le_bytes(buf[p + 8..p + 16].try_into().ok()?) as f32;
                points.push([lon, lat]);
            }
            if let Some(last) = num_points.checked_sub(1) {
                let p = pts_off + last * 16;
                let lon = f64::from_le_bytes(buf[p..p + 8].try_into().ok()?) as f32;
                let lat = f64::from_le_bytes(buf[p + 8..p + 16].try_into().ok()?) as f32;
                if points.last().copied() != Some([lon, lat]) {
                    points.push([lon, lat]);
                }
            }
            let seg_id = segments.len() as u32;
            let xmin = f64::from_le_bytes(buf[4..12].try_into().ok()?);
            let ymin = f64::from_le_bytes(buf[12..20].try_into().ok()?);
            let xmax = f64::from_le_bytes(buf[20..28].try_into().ok()?);
            let ymax = f64::from_le_bytes(buf[28..36].try_into().ok()?);
            let gx0 = (((xmin + 180.0) / 2.0).floor() as i32).clamp(0, 179) as usize;
            let gx1 = (((xmax + 180.0) / 2.0).floor() as i32).clamp(0, 179) as usize;
            let gy0 = (((ymin + 90.0) / 2.0).floor() as i32).clamp(0, 89) as usize;
            let gy1 = (((ymax + 90.0) / 2.0).floor() as i32).clamp(0, 89) as usize;
            for gy in gy0..=gy1 {
                for gx in gx0..=gx1 {
                    cells[gy * 180 + gx].push(seg_id);
                }
            }
            segments.push(GrwlSegment {
                width_med_m,
                points,
            });
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
        let mut segments = Vec::with_capacity(seg_count);
        for _ in 0..seg_count {
            let width_med_m = f32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?);
            pos += 4;
            let n = u16::from_le_bytes(bytes.get(pos..pos + 2)?.try_into().ok()?) as usize;
            pos += 2;
            let mut points = Vec::with_capacity(n);
            for _ in 0..n {
                let lon = f32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?);
                let lat = f32::from_le_bytes(bytes.get(pos + 4..pos + 8)?.try_into().ok()?);
                pos += 8;
                points.push([lon, lat]);
            }
            segments.push(GrwlSegment {
                width_med_m,
                points,
            });
        }
        let mut cells = Vec::with_capacity(180 * 90);
        for _ in 0..(180 * 90) {
            let len = u32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?) as usize;
            pos += 4;
            let mut cell = Vec::with_capacity(len);
            for _ in 0..len {
                cell.push(u32::from_le_bytes(
                    bytes.get(pos..pos + 4)?.try_into().ok()?,
                ));
                pos += 4;
            }
            cells.push(cell);
        }
        Some(Self { segments, cells })
    }
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
    mask_dir: PathBuf,
    entries: Vec<GrwlMaskEntry>,
    cache: Mutex<(HashMap<usize, Arc<Vec<u8>>>, VecDeque<usize>)>,
}

impl GrwlMaskStore {
    pub fn load_or_index(dir: &Path) -> Option<Self> {
        let mask_dir = dir.join("grwl_masks");
        if !mask_dir.is_dir() {
            return None;
        }
        let idx_path = dir.join("grwl_masks_idx.json");
        if let Ok(bytes) = fs::read(&idx_path)
            && let Ok(entries) = serde_json::from_slice::<Vec<GrwlMaskEntry>>(&bytes)
            && !entries.is_empty()
        {
            return Some(Self {
                mask_dir,
                entries,
                cache: Mutex::new((HashMap::new(), VecDeque::new())),
            });
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(&mask_dir).ok()?.flatten() {
            let p = entry.path();
            if !p
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("tif") || s.eq_ignore_ascii_case("tiff"))
            {
                continue;
            }
            let Ok(file) = fs::File::open(&p) else {
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
            let _ = fs::write(&idx_path, encoded);
        }
        Some(Self {
            mask_dir,
            entries,
            cache: Mutex::new((HashMap::new(), VecDeque::new())),
        })
    }

    /// Returns `Some(true)` for water, `Some(false)` for land/island inside a 30 m mask tile,
    /// or `None` if no mask tile covers `(lat, lon)`.
    pub fn sample_water(&self, lat: f64, lon: f64) -> Option<bool> {
        let (idx, entry) = self.entries.iter().enumerate().find(|(_, e)| {
            lon >= e.min_lon && lon <= e.max_lon && lat >= e.min_lat && lat <= e.max_lat
        })?;
        let data = {
            let mut guard = self.cache.lock().ok()?;
            if let Some(hit) = guard.0.get(&idx) {
                hit.clone()
            } else {
                drop(guard);
                let file = fs::File::open(self.mask_dir.join(&entry.file)).ok()?;
                let mut dec = tiff::decoder::Decoder::new(BufReader::new(file))
                    .ok()?
                    .with_limits(tiff::decoder::Limits::unlimited());
                let tiff::decoder::DecodingResult::U8(bytes) = dec.read_image().ok()? else {
                    return None;
                };
                let arc = Arc::new(bytes);
                let mut guard = self.cache.lock().ok()?;
                if guard.1.len() >= 6
                    && let Some(old) = guard.1.pop_front()
                {
                    guard.0.remove(&old);
                }
                guard.1.push_back(idx);
                guard.0.insert(idx, arc.clone());
                arc
            }
        };
        let px = (((lon - entry.min_lon) / (entry.max_lon - entry.min_lon)) * entry.width as f64)
            .floor() as usize;
        let py = (((entry.max_lat - lat) / (entry.max_lat - entry.min_lat)) * entry.height as f64)
            .floor() as usize;
        if px < entry.width as usize && py < entry.height as usize {
            data.get(py * entry.width as usize + px).map(|&v| v > 0)
        } else {
            None
        }
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
    pub fn load_or_build(dir: &Path) -> Option<Self> {
        let legacy = dir.join("rivers_stra3.bin");
        if legacy.is_file() {
            let _ = fs::remove_file(&legacy);
        }
        let cache_path = dir.join("rivers_stra3_v2.bin");
        if let Ok(bytes) = fs::read(&cache_path)
            && let Some(idx) = Self::decode_bin(&bytes)
        {
            return Some(idx);
        }
        let rivers_dir = dir.join("hydrorivers");
        let shp_path = find_first_with_ext(&rivers_dir, &["shp"])?;
        let dbf_path = shp_path.with_extension("dbf");
        let built = Self::build_from_shp(&shp_path, &dbf_path)?;
        if let Ok(encoded) = built.encode_bin() {
            let _ = fs::write(&cache_path, encoded);
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
        if let Ok(dbf_file) = fs::File::open(dbf_path) {
            let mut rdr = BufReader::new(dbf_file);
            let mut hdr = [0u8; 32];
            if rdr.read_exact(&mut hdr).is_ok() {
                let header_len = u16::from_le_bytes([hdr[8], hdr[9]]) as usize;
                let rec_len = u16::from_le_bytes([hdr[10], hdr[11]]) as usize;
                let n_recs = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
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

        let shp_file = fs::File::open(shp_path).ok()?;
        let mut rdr = BufReader::new(shp_file);
        let mut hdr = [0u8; 100];
        rdr.read_exact(&mut hdr).ok()?;
        let mut segments = Vec::new();
        let mut cells = vec![Vec::new(); 180 * 90];
        let mut rec_hdr = [0u8; 8];
        let mut rec_idx = 0usize;
        while rdr.read_exact(&mut rec_hdr).is_ok() {
            let content_words = i32::from_be_bytes(rec_hdr[4..8].try_into().ok()?) as usize;
            let content_bytes = content_words.saturating_mul(2);
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
            let num_parts = i32::from_le_bytes(buf[36..40].try_into().ok()?) as usize;
            let num_points = i32::from_le_bytes(buf[40..44].try_into().ok()?) as usize;
            let pts_off = 44 + num_parts.saturating_mul(4);
            if num_parts == 0 || num_points < 2 || pts_off + num_points * 16 > buf.len() {
                continue;
            }
            let step = (num_points / 24).max(1);
            let mut points = Vec::with_capacity((num_points / step).min(24) + 1);
            for i in (0..num_points).step_by(step) {
                let p = pts_off + i * 16;
                let lon = f64::from_le_bytes(buf[p..p + 8].try_into().ok()?) as f32;
                let lat = f64::from_le_bytes(buf[p + 8..p + 16].try_into().ok()?) as f32;
                points.push([lon, lat]);
            }
            if let Some(last) = num_points.checked_sub(1) {
                let p = pts_off + last * 16;
                let lon = f64::from_le_bytes(buf[p..p + 8].try_into().ok()?) as f32;
                let lat = f64::from_le_bytes(buf[p + 8..p + 16].try_into().ok()?) as f32;
                if points.last().copied() != Some([lon, lat]) {
                    points.push([lon, lat]);
                }
            }
            let seg_id = segments.len() as u32;
            let xmin = f64::from_le_bytes(buf[4..12].try_into().ok()?);
            let ymin = f64::from_le_bytes(buf[12..20].try_into().ok()?);
            let xmax = f64::from_le_bytes(buf[20..28].try_into().ok()?);
            let ymax = f64::from_le_bytes(buf[28..36].try_into().ok()?);
            let gx0 = (((xmin + 180.0) / 2.0).floor() as i32).clamp(0, 179) as usize;
            let gx1 = (((xmax + 180.0) / 2.0).floor() as i32).clamp(0, 179) as usize;
            let gy0 = (((ymin + 90.0) / 2.0).floor() as i32).clamp(0, 89) as usize;
            let gy1 = (((ymax + 90.0) / 2.0).floor() as i32).clamp(0, 89) as usize;
            for gy in gy0..=gy1 {
                for gx in gx0..=gx1 {
                    cells[gy * 180 + gx].push(seg_id);
                }
            }
            segments.push(RiverSegment {
                ord_stra: ord,
                dis_cms: dis,
                points,
            });
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
        let mut segments = Vec::with_capacity(seg_count);
        for _ in 0..seg_count {
            let ord_stra = *bytes.get(pos)?;
            pos += 1;
            let dis_cms = f32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?);
            pos += 4;
            let n = u16::from_le_bytes(bytes.get(pos..pos + 2)?.try_into().ok()?) as usize;
            pos += 2;
            let mut points = Vec::with_capacity(n);
            for _ in 0..n {
                let lon = f32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?);
                let lat = f32::from_le_bytes(bytes.get(pos + 4..pos + 8)?.try_into().ok()?);
                pos += 8;
                points.push([lon, lat]);
            }
            segments.push(RiverSegment {
                ord_stra,
                dis_cms,
                points,
            });
        }
        let mut cells = Vec::with_capacity(180 * 90);
        for _ in 0..(180 * 90) {
            let len = u32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?) as usize;
            pos += 4;
            let mut cell = Vec::with_capacity(len);
            for _ in 0..len {
                cell.push(u32::from_le_bytes(
                    bytes.get(pos..pos + 4)?.try_into().ok()?,
                ));
                pos += 4;
            }
            cells.push(cell);
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
        let file = fs::File::open(tif_path).ok()?;
        let mut decoder = tiff::decoder::Decoder::new(BufReader::new(file))
            .ok()?
            .with_limits(tiff::decoder::Limits::unlimited());
        let mut levels = Vec::new();
        for idx in 0..16usize {
            if idx > 0 && decoder.seek_to_image(idx).is_err() {
                break;
            }
            if let Ok((width, height)) = decoder.dimensions() {
                let (chunk_w, chunk_h) = decoder.chunk_dimensions();
                if width > 0 && height > 0 && chunk_w > 0 && chunk_h > 0 {
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
                let arc = Arc::new(vals);
                if guard.order.len() >= 128
                    && let Some(old) = guard.order.pop_front()
                {
                    guard.cache.remove(&old);
                }
                guard.order.push_back(key);
                guard.cache.insert(key, arc.clone());
                arc
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
        let file = fs::File::open(tif_path).ok()?;
        let mut decoder = tiff::decoder::Decoder::new(BufReader::new(file))
            .ok()?
            .with_limits(tiff::decoder::Limits::unlimited());
        let (width, height) = decoder.dimensions().ok()?;
        let (_chunk_w, chunk_h) = decoder.chunk_dimensions();
        if width < 8192 || chunk_h == 0 {
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
                let arc = Arc::new(bytes);
                if guard.order.len() >= 1024
                    && let Some(old) = guard.order.pop_front()
                {
                    guard.cache.remove(&old);
                }
                guard.order.push_back(chunk_idx);
                guard.cache.insert(chunk_idx, arc.clone());
                arc
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
    pub fn sample(&self, lat: f64, lon: f64) -> f32 {
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
        top * (1.0 - fy) + bot * fy
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
        self.has_ne2 || self.has_cities
    }
}

impl BasemapAssets {
    /// Inspects what offline assets are currently present on disk without loading large files.
    pub fn probe_status(assets_dir: &Path, terrain_override_dir: Option<&Path>) -> AssetStatus {
        let mut status = AssetStatus::default();
        status.has_ne2 = find_first_with_ext(assets_dir, &["png", "jpg", "jpeg", "tif", "tiff"])
            .is_some()
            || assets_dir.join("ne2_overview.rgb").is_file();
        status.has_cities = assets_dir.join("cities_all.json").is_file()
            || assets_dir.join("cities_250k.json").is_file()
            || assets_dir
                .join("ne_10m_populated_places_simple.geojson")
                .is_file();
        status.has_borders = (assets_dir.join("borders_admin0.json").is_file()
            || assets_dir
                .join("ne_10m_admin_0_boundary_lines_land.geojson")
                .is_file())
            && (assets_dir.join("borders_admin1.json").is_file()
                || assets_dir
                    .join("ne_10m_admin_1_states_provinces_lines.geojson")
                    .is_file());
        status.has_roads_lakes = (assets_dir.join("roads_10m.json").is_file()
            || assets_dir.join("ne_10m_roads.geojson").is_file())
            && (assets_dir.join("lakes_10m.json").is_file()
                || assets_dir.join("ne_10m_lakes.geojson").is_file());
        status.has_grwl = assets_dir.join("grwl_summary.bin").is_file()
            || find_first_with_ext(&assets_dir.join("grwl"), &["shp"]).is_some();
        status.has_grwl_masks = assets_dir.join("grwl_masks_idx.json").is_file()
            || find_first_with_ext(&assets_dir.join("grwl_masks"), &["tif", "tiff"]).is_some();
        status.has_gshhg = find_gshhg_file(assets_dir).is_some();
        status.has_gebco = assets_dir.join("gebco_overview.i16").is_file()
            || find_first_with_ext(&assets_dir.join("gebco"), &["tif", "tiff", "nc", "i16"])
                .is_some();
        status.has_land_cover = assets_dir.join("land_cover.u8").is_file()
            || assets_dir
                .join("land_cover_lccs_class_2020_v2_1_1.nc")
                .is_file();
        status.has_hydrorivers = assets_dir.join("hydrorivers").is_dir();
        if let Some(dir) = terrain_override_dir
            && let Ok(entries) = fs::read_dir(dir)
        {
            status.local_hgt_count = entries
                .flatten()
                .filter(|e| {
                    e.path()
                        .extension()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s.eq_ignore_ascii_case("hgt"))
                })
                .count();
        }
        status
    }

    /// Loads available offline basemap assets from `assets_dir` and optional `terrain_override_dir`.
    pub fn load(assets_dir: &Path, terrain_override_dir: Option<&Path>) -> Self {
        let ne2 = load_ne2_overview(assets_dir);
        let ne2_full = find_first_with_ext(assets_dir, &["tif", "tiff"])
            .and_then(|p| Ne2FullSampler::open(&p));
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
        let roads = load_roads_from_dir(assets_dir);
        let lakes = load_lakes_from_dir(assets_dir);
        let coastlines = find_gshhg_file(assets_dir)
            .and_then(|p| fs::read(p).ok())
            .map(|bytes| parse_gshhg_b(&bytes, 12_000))
            .unwrap_or_default();
        let mut terrain_tiles = HashMap::new();
        if let Some(dir) = terrain_override_dir {
            load_hgt_dir(dir, &mut terrain_tiles);
        }
        load_hgt_dir(&assets_dir.join("terrain"), &mut terrain_tiles);

        let global_dem = load_global_dem(assets_dir);
        let cog_dem = find_first_with_ext(&assets_dir.join("gebco"), &["tif", "tiff"])
            .and_then(|p| CogDemPyramid::open(&p));
        let land_cover = load_land_cover(assets_dir);
        let rivers = HydroRiversIndex::load_or_build(assets_dir);
        let grwl_summary = GrwlSummaryIndex::load_or_build(assets_dir);
        let grwl_masks = GrwlMaskStore::load_or_index(assets_dir);
        let has_hydrorivers = rivers.is_some() || assets_dir.join("hydrorivers").is_dir();

        Self {
            assets_dir: assets_dir.to_path_buf(),
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
        let lat_f = lat.floor() as i32;
        let lon_f = lon.floor() as i32;
        if let Some(tile) = self.terrain_tiles.get(&(lat_f, lon_f))
            && let Some(h) = tile.sample(lat, lon)
        {
            return Some(h);
        }
        if target_global_width > 5400.0
            && let Some(cog) = &self.cog_dem
            && let Some(h) = cog.sample_at(lat, lon, target_global_width)
        {
            return Some(h);
        }
        self.global_dem.as_ref().map(|g| g.sample(lat, lon))
    }

    /// Samples terrain elevation in meters at `(lat, lon)`.
    pub fn sample_elevation_m(&self, lat: f64, lon: f64) -> Option<f32> {
        self.sample_elevation_at(lat, lon, 5400.0)
    }

    /// Estimates elevation in meters, falling back to a smooth coarse approximation derived from
    /// Natural Earth II shading or continental/orogenic hints when GEBCO is not yet downloaded.
    pub fn sample_elevation_or_estimate(&self, lat: f64, lon: f64) -> (f32, bool) {
        self.sample_elevation_or_estimate_at(lat, lon, 5400.0)
    }

    /// Resolution-aware elevation lookup with fallback estimate.
    pub fn sample_elevation_or_estimate_at(
        &self,
        lat: f64,
        lon: f64,
        target_global_width: f64,
    ) -> (f32, bool) {
        if let Some(m) = self.sample_elevation_at(lat, lon, target_global_width) {
            return (m, true);
        }
        if let Some(ne2) = &self.ne2 {
            let [r, g, b] = ne2.sample_rgb(lat, lon);
            // Natural Earth II ocean pixels are strongly blue-dominant.
            if b > r.saturating_add(16) && b > g.saturating_add(6) && b > 140 {
                return (0.0, false);
            }
            // Approximate relative relief from brightness/snow/highland cues so Oilslick remains
            // inspectable before the full 7.2 GB GEBCO download.
            let luma = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0;
            let highland = (1.0 - (g as f32 / 255.0)).max(0.0);
            let elev = (luma * 1400.0 + highland * 2200.0 - 250.0).max(15.0);
            return (elev, false);
        }
        // Zero-asset fallback: subtle latitude/longitude harmonic relief above sea level.
        let s = (lat.to_radians() * 3.0).sin() * (lon.to_radians() * 2.0).cos();
        let elev = ((s * 0.5 + 0.5) * 1600.0) as f32;
        (elev, false)
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

    /// Computes a normalized northwest hillshade factor (`0.0..=1.0`) at `(lat, lon)`.
    pub fn sample_hillshade(&self, lat: f64, lon: f64, step_deg: f64) -> f32 {
        self.sample_hillshade_at(lat, lon, step_deg, 5400.0)
    }

    /// Resolution-aware northwest hillshade factor (`0.0..=1.0`).
    pub fn sample_hillshade_at(
        &self,
        lat: f64,
        lon: f64,
        step_deg: f64,
        target_global_width: f64,
    ) -> f32 {
        let d = step_deg.clamp(0.002, 0.5);
        let (e_w, _) = self.sample_elevation_or_estimate_at(lat, lon - d, target_global_width);
        let (e_e, _) = self.sample_elevation_or_estimate_at(lat, lon + d, target_global_width);
        let (e_s, _) = self.sample_elevation_or_estimate_at(lat - d, lon, target_global_width);
        let (e_n, _) = self.sample_elevation_or_estimate_at(lat + d, lon, target_global_width);
        let meters_per_deg = 111_320.0 * lat.to_radians().cos().abs().max(0.15) as f32;
        let dx = (e_e - e_w) / (2.0 * d as f32 * meters_per_deg);
        let dy = (e_n - e_s) / (2.0 * d as f32 * 111_320.0);
        // Northwest sun (azimuth 315°, altitude 45°)
        let nx = -dx * 2.4;
        let ny = -dy * 2.4;
        let nz = 1.0_f32;
        let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-5);
        let lx = -0.5_f32;
        let ly = 0.5_f32;
        let lz = 0.7071_f32;
        ((nx * lx + ny * ly + nz * lz) / len).clamp(0.12, 1.0)
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

        if blend <= 0.001 {
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

    /// Inverse-projects viewport pixel `(px, py)` back to `(lat, lon)` in degrees.
    pub fn unproject(&self, px: f32, py: f32, width: f32, height: f32) -> Option<(f64, f64)> {
        let blend = self.globe_blend();
        let scale = self.scale_px(width, height).max(1.0);
        let nx = (px as f64 - width as f64 * 0.5) / scale;
        let ny = -(py as f64 - height as f64 * 0.5) / scale;

        if blend < 0.5 {
            let cos_c = self.center_lat.to_radians().cos().abs().max(0.25);
            let deg_per_rad = 180.0 / std::f64::consts::PI;
            let lat = (self.center_lat + ny * deg_per_rad).clamp(-89.9, 89.9);
            let lon = wrap_lon(self.center_lon + (nx / cos_c) * deg_per_rad);
            return Some((lat, lon));
        }

        let r2 = nx * nx + ny * ny;
        if r2 > 1.0 {
            return None;
        }
        let rho = r2.sqrt();
        if rho < 1e-8 {
            return Some((self.center_lat, self.center_lon));
        }
        let c = rho.asin();
        let sin_c = c.sin();
        let cos_c = c.cos();
        let phi0 = self.center_lat.to_radians();
        let lat = (cos_c * phi0.sin() + (ny * sin_c * phi0.cos()) / rho).asin();
        let lon = self.center_lon.to_radians()
            + (nx * sin_c).atan2(rho * phi0.cos() * cos_c - ny * phi0.sin() * sin_c);
        Some((lat.to_degrees(), wrap_lon(lon.to_degrees())))
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

/// Rendered basemap frame with local elevation range and active contour interval metadata.
pub struct BasemapFrame {
    pub rgba: Vec<u8>,
    pub contour_interval_m: Option<f32>,
    pub local_min_m: f32,
    pub local_max_m: f32,
}

/// Picks a clean round topographic contour interval in meters for the visible elevation span and zoom.
pub fn choose_contour_interval_m(span_m: f32, zoom: f64) -> f32 {
    let target = (span_m.max(20.0) / 14.0) / (zoom as f32 / 6.0).clamp(0.6, 2.5);
    for &step in &[10.0_f32, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0] {
        if target <= step {
            return step;
        }
    }
    1000.0
}

/// Maps a Natural Earth II pixel into a clean, smudge-free biome color (water, forest, grass,
/// scrub/desert, rock, snow/ice) so close-zoom terrain stays crisp without blurry baked rivers.
fn classify_ne2_biome_rgb(
    ne2_rgb: [u8; 3],
    elev_m: f32,
    lat: f64,
    allow_ne2_water: bool,
) -> [u8; 3] {
    if elev_m <= 0.0 {
        return LandCoverClass::Ocean.rgb();
    }
    let [r, g, b] = ne2_rgb;
    if allow_ne2_water && b > r.saturating_add(12) && b > g.saturating_add(4) {
        return LandCoverClass::Ocean.rgb();
    }
    let luma = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0;
    if elev_m > 2400.0 && luma > 0.78 || elev_m > 4200.0 || (lat.abs() > 62.0 && luma > 0.78) {
        return LandCoverClass::SnowIce.rgb();
    }
    if r > g.saturating_add(14) && r > b.saturating_add(25) {
        return LandCoverClass::BareGround.rgb();
    }
    if g >= r && g > b.saturating_add(10) {
        if luma < 0.52 {
            return LandCoverClass::Forest.rgb();
        }
        return LandCoverClass::GrassCrops.rgb();
    }
    if r >= g && g > b {
        return LandCoverClass::Scrub.rgb();
    }
    LandCoverClass::GrassCrops.rgb()
}

/// Renders the offline basemap raster (Natural / Oilslick / Hillshade) for the given camera and
/// viewport dimensions into `width * height * 4` RGBA8 bytes.
pub fn render_basemap_rgba(
    assets: Option<&BasemapAssets>,
    camera: &GlobeCamera,
    style: BasemapStyle,
    width: u32,
    height: u32,
    bg_rgb: [u8; 3],
) -> Vec<u8> {
    render_basemap_frame(assets, camera, style, false, width, height, bg_rgb).rgba
}

/// Renders the offline basemap raster with optional topographic contour lines and local
/// min/max elevation gradient shading.
pub fn render_basemap_frame(
    assets: Option<&BasemapAssets>,
    camera: &GlobeCamera,
    style: BasemapStyle,
    show_contours: bool,
    width: u32,
    height: u32,
    bg_rgb: [u8; 3],
) -> BasemapFrame {
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let step_deg =
        (180.0 / (camera.scale_px(width as f32, height as f32).max(32.0) * 2.0)).clamp(0.002, 0.35);
    let blend = camera.globe_blend();
    let scale = camera.scale_px(width as f32, height as f32).max(1.0);
    let target_global_width = (scale * 2.0 * std::f64::consts::PI).max(2048.0);
    let supersample =
        width >= 420 && (style == BasemapStyle::Oilslick || target_global_width > 4096.0);
    let cx = width as f64 * 0.5;
    let cy = height as f64 * 0.5;

    // Sample local min/max elevation across the visible viewport for gradient & contour scaling.
    let mut local_min_m = f32::INFINITY;
    let mut local_max_m = f32::NEG_INFINITY;
    let has_dem = assets.is_some_and(|a| {
        !a.terrain_tiles.is_empty() || a.global_dem.is_some() || a.cog_dem.is_some()
    });
    if let Some(assets) = assets
        && has_dem
    {
        for sy in (0..14).map(|i| (i as f32 + 0.5) * height as f32 / 14.0) {
            for sx in (0..20).map(|i| (i as f32 + 0.5) * width as f32 / 20.0) {
                if let Some((lat, lon)) = camera.unproject(sx, sy, width as f32, height as f32)
                    && let Some(e) = assets.sample_elevation_at(lat, lon, target_global_width)
                    && e > 0.0
                {
                    local_min_m = local_min_m.min(e);
                    local_max_m = local_max_m.max(e);
                }
            }
        }
    }
    if !local_min_m.is_finite() || !local_max_m.is_finite() {
        local_min_m = 0.0;
        local_max_m = 1000.0;
    }
    let local_span_m = (local_max_m - local_min_m).max(20.0);
    let contour_interval_m = (show_contours && has_dem && camera.zoom >= 3.5)
        .then(|| choose_contour_interval_m(local_span_m, camera.zoom));
    let biome_blend = ((camera.zoom - 5.0) / 4.0).clamp(0.0, 1.0) as f32;
    let px_deg = (180.0 / (scale * std::f64::consts::PI)).clamp(0.0005, 0.25);

    // River ribbon field:
    // Tier 2: GRWL summary centerlines using Landsat-measured `width_med / 2` on each side.
    // Fallback (when GRWL isn't downloaded): HydroRIVERS discharge-derived hydraulic ribbon.
    let mut river_field: Vec<f32> = Vec::new();
    if style != BasemapStyle::Oilslick
        && camera.zoom >= 3.5
        && let Some(assets) = assets
        && (assets.grwl_summary.is_some() || assets.rivers.is_some())
    {
        let mut r_min_lat = 90.0_f64;
        let mut r_max_lat = -90.0_f64;
        let mut r_min_lon = 180.0_f64;
        let mut r_max_lon = -180.0_f64;
        let mut any_c = false;
        for (cx, cy) in [
            (0.0, 0.0),
            (width as f32, 0.0),
            (0.0, height as f32),
            (width as f32, height as f32),
            (width as f32 * 0.5, height as f32 * 0.5),
        ] {
            if let Some((lat, lon)) = camera.unproject(cx, cy, width as f32, height as f32) {
                any_c = true;
                r_min_lat = r_min_lat.min(lat);
                r_max_lat = r_max_lat.max(lat);
                r_min_lon = r_min_lon.min(lon);
                r_max_lon = r_max_lon.max(lon);
            }
        }
        if any_c && (r_max_lon - r_min_lon) < 120.0 {
            river_field = vec![2.0_f32; (width * height) as usize];
            let meters_per_px = (6_371_000.0 / scale).max(1.0);
            let mut splat_ribbon = |points: &[[f32; 2]], half_w_m: f64| {
                let rad_px = ((half_w_m / meters_per_px) as f32).clamp(0.75, 24.0);
                let rad_i = rad_px.ceil() as i32;
                for pair in points.windows(2) {
                    let [lon0, lat0] = pair[0];
                    let [lon1, lat1] = pair[1];
                    let Some([x0, y0]) =
                        camera.project(lat0 as f64, lon0 as f64, width as f32, height as f32)
                    else {
                        continue;
                    };
                    let Some([x1, y1]) =
                        camera.project(lat1 as f64, lon1 as f64, width as f32, height as f32)
                    else {
                        continue;
                    };
                    let min_x = (x0.min(x1).floor() as i32 - rad_i).max(0);
                    let max_x = (x0.max(x1).ceil() as i32 + rad_i).min(width as i32 - 1);
                    let min_y = (y0.min(y1).floor() as i32 - rad_i).max(0);
                    let max_y = (y0.max(y1).ceil() as i32 + rad_i).min(height as i32 - 1);
                    let dx = x1 - x0;
                    let dy = y1 - y0;
                    let len2 = (dx * dx + dy * dy).max(1e-5);
                    for py in min_y..=max_y {
                        for px in min_x..=max_x {
                            let t = (((px as f32 + 0.5 - x0) * dx + (py as f32 + 0.5 - y0) * dy)
                                / len2)
                                .clamp(0.0, 1.0);
                            let cx_p = x0 + dx * t;
                            let cy_p = y0 + dy * t;
                            let dist = ((px as f32 + 0.5 - cx_p).powi(2)
                                + (py as f32 + 0.5 - cy_p).powi(2))
                            .sqrt();
                            let frac = dist / rad_px;
                            if frac < 1.0 {
                                let cell =
                                    &mut river_field[(py as u32 * width + px as u32) as usize];
                                if frac < *cell {
                                    *cell = frac;
                                }
                            }
                        }
                    }
                }
            };

            if let Some(grwl) = &assets.grwl_summary {
                let min_w = if camera.zoom < 6.0 { 90.0 } else { 20.0 };
                let segs = grwl.query_bbox(
                    r_min_lon - 1.5,
                    r_min_lat - 1.5,
                    r_max_lon + 1.5,
                    r_max_lat + 1.5,
                    min_w,
                    3500,
                );
                for seg in segs {
                    splat_ribbon(&seg.points, (seg.width_med_m as f64) * 0.5);
                }
            } else if let Some(rivers) = &assets.rivers {
                let min_ord = if camera.zoom < 9.0 { 4 } else { 3 };
                let segs = rivers.query_bbox(
                    r_min_lon - 1.5,
                    r_min_lat - 1.5,
                    r_max_lon + 1.5,
                    r_max_lat + 1.5,
                    min_ord,
                    3000,
                );
                for seg in segs {
                    let half_w_m = (3.6 * seg.dis_cms.max(1.0).sqrt()).clamp(25.0, 1200.0) as f64;
                    splat_ribbon(&seg.points, half_w_m);
                }
            }
        }
    }

    let sample_style = |lat: f64, lon: f64| -> [u8; 3] {
        let mut rgb = match style {
            BasemapStyle::Natural => {
                if let Some(assets) = assets {
                    if let Some(lc) = &assets.land_cover {
                        let class = lc.sample(lat, lon);
                        let base = class.rgb();
                        if class == LandCoverClass::Ocean {
                            base
                        } else {
                            let hs =
                                assets.sample_hillshade_at(lat, lon, step_deg, target_global_width);
                            let shade = 0.58 + 0.42 * hs;
                            [
                                (base[0] as f32 * shade).clamp(0.0, 255.0) as u8,
                                (base[1] as f32 * shade).clamp(0.0, 255.0) as u8,
                                (base[2] as f32 * shade).clamp(0.0, 255.0) as u8,
                            ]
                        }
                    } else if let Some(ne2_rgb) =
                        assets.sample_ne2_at(lat, lon, target_global_width)
                    {
                        if let Some(elev) =
                            assets.sample_elevation_at(lat, lon, target_global_width)
                        {
                            if elev <= 0.0 && biome_blend > 0.2 {
                                let ocean = LandCoverClass::Ocean.rgb();
                                [
                                    (ne2_rgb[0] as f32 * (1.0 - biome_blend)
                                        + ocean[0] as f32 * biome_blend)
                                        as u8,
                                    (ne2_rgb[1] as f32 * (1.0 - biome_blend)
                                        + ocean[1] as f32 * biome_blend)
                                        as u8,
                                    (ne2_rgb[2] as f32 * (1.0 - biome_blend)
                                        + ocean[2] as f32 * biome_blend)
                                        as u8,
                                ]
                            } else if elev > 0.0
                                && (biome_blend > 0.01 || target_global_width > 4096.0)
                            {
                                let biome = classify_ne2_biome_rgb(
                                    ne2_rgb,
                                    elev,
                                    lat,
                                    assets.rivers.is_none(),
                                );
                                let base = [
                                    ne2_rgb[0] as f32 * (1.0 - biome_blend)
                                        + biome[0] as f32 * biome_blend,
                                    ne2_rgb[1] as f32 * (1.0 - biome_blend)
                                        + biome[1] as f32 * biome_blend,
                                    ne2_rgb[2] as f32 * (1.0 - biome_blend)
                                        + biome[2] as f32 * biome_blend,
                                ];
                                let hs = assets.sample_hillshade_at(
                                    lat,
                                    lon,
                                    step_deg,
                                    target_global_width,
                                );
                                let local_norm =
                                    ((elev - local_min_m) / local_span_m).clamp(0.0, 1.0);
                                let elev_grad =
                                    1.0 - biome_blend * 0.14 + biome_blend * 0.24 * local_norm;
                                let shade = (0.76 + 0.36 * (hs - 0.5)) * elev_grad;
                                [
                                    (base[0] * shade).clamp(0.0, 255.0) as u8,
                                    (base[1] * shade).clamp(0.0, 255.0) as u8,
                                    (base[2] * shade).clamp(0.0, 255.0) as u8,
                                ]
                            } else {
                                ne2_rgb
                            }
                        } else {
                            ne2_rgb
                        }
                    } else {
                        fallback_natural_rgb(lat, lon)
                    }
                } else {
                    fallback_natural_rgb(lat, lon)
                }
            }
            BasemapStyle::Hypsometric => {
                if let Some(assets) = assets {
                    let (elev, _) =
                        assets.sample_elevation_or_estimate_at(lat, lon, target_global_width);
                    let hs = assets.sample_hillshade_at(lat, lon, step_deg, target_global_width);
                    hypsometric_color(elev, hs)
                } else {
                    let s = (lat.to_radians() * 3.0).sin() * (lon.to_radians() * 2.0).cos();
                    let elev = ((s * 0.5 + 0.5) * 1600.0) as f32;
                    hypsometric_color(elev, 0.78)
                }
            }
            BasemapStyle::Oilslick => {
                if let Some(assets) = assets {
                    let (elev, _) =
                        assets.sample_elevation_or_estimate_at(lat, lon, target_global_width);
                    let hs = assets.sample_hillshade_at(lat, lon, step_deg, target_global_width);
                    oilslick_color(elev, hs)
                } else {
                    let s = (lat.to_radians() * 3.0).sin() * (lon.to_radians() * 2.0).cos();
                    let elev = ((s * 0.5 + 0.5) * 1600.0) as f32;
                    oilslick_color(elev, 0.78)
                }
            }
            BasemapStyle::Hillshade => {
                if let Some(assets) = assets {
                    let (elev, _) =
                        assets.sample_elevation_or_estimate_at(lat, lon, target_global_width);
                    if elev <= 0.0 {
                        [28, 52, 82]
                    } else {
                        let hs =
                            assets.sample_hillshade_at(lat, lon, step_deg, target_global_width);
                        let local_norm = ((elev - local_min_m) / local_span_m).clamp(0.0, 1.0);
                        let v = (48.0 + 170.0 * hs + 26.0 * local_norm).clamp(0.0, 255.0) as u8;
                        [v, v, v.saturating_add(4)]
                    }
                } else {
                    [76, 82, 92]
                }
            }
        };

        // Topographic contour lines from DEM using screen-space gradient for crisp 1px AA lines.
        if let Some(interval) = contour_interval_m
            && let Some(assets) = assets
            && let Some(e) = assets.sample_elevation_at(lat, lon, target_global_width)
            && e > 0.0
        {
            let ex = assets
                .sample_elevation_at(lat, lon + px_deg, target_global_width)
                .unwrap_or(e);
            let ey = assets
                .sample_elevation_at(lat + px_deg, lon, target_global_width)
                .unwrap_or(e);
            let grad = ((ex - e).powi(2) + (ey - e).powi(2)).sqrt().max(0.25);
            let phase = (e / interval).rem_euclid(1.0);
            let dist_m = phase.min(1.0 - phase) * interval;
            let dist_px = dist_m / grad;
            if dist_px < 1.15 {
                let is_major = ((e / interval).round() as i32).rem_euclid(5) == 0;
                let line_alpha =
                    ((1.15 - dist_px) / 1.15).clamp(0.0, 1.0) * if is_major { 0.58 } else { 0.30 };
                let line_rgb = if style == BasemapStyle::Oilslick {
                    [15.0_f32, 18.0, 24.0]
                } else {
                    [34.0_f32, 30.0, 26.0]
                };
                rgb[0] = (rgb[0] as f32 * (1.0 - line_alpha) + line_rgb[0] * line_alpha) as u8;
                rgb[1] = (rgb[1] as f32 * (1.0 - line_alpha) + line_rgb[1] * line_alpha) as u8;
                rgb[2] = (rgb[2] as f32 * (1.0 - line_alpha) + line_rgb[2] * line_alpha) as u8;
            }
        }

        rgb
    };

    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) as usize) * 4;
            let dx = (x as f64 + 0.5 - cx) / scale;
            let dy = (y as f64 + 0.5 - cy) / scale;
            let d = (dx * dx + dy * dy).sqrt();
            let halo_rgb = |dist: f64| -> [u8; 3] {
                if blend > 0.25 && dist >= 0.99 && dist < 1.06 {
                    let glow = ((1.06 - dist.max(1.0)) / 0.06).powi(2) * 0.45 * blend;
                    [
                        (bg_rgb[0] as f64 + (90.0 - bg_rgb[0] as f64) * glow) as u8,
                        (bg_rgb[1] as f64 + (150.0 - bg_rgb[1] as f64) * glow) as u8,
                        (bg_rgb[2] as f64 + (235.0 - bg_rgb[2] as f64) * glow) as u8,
                    ]
                } else {
                    bg_rgb
                }
            };

            let px_dist = (1.0 - d) * scale;
            let (coord, limb_cov) = if blend >= 0.5 && px_dist <= -0.75 {
                let outer = halo_rgb(d);
                rgba[idx] = outer[0];
                rgba[idx + 1] = outer[1];
                rgba[idx + 2] = outer[2];
                rgba[idx + 3] = 255;
                continue;
            } else if blend >= 0.5 && px_dist < 0.75 {
                let clamp_scale = (0.997 / d.max(1e-6)) as f32;
                let sx = cx as f32 + (x as f32 + 0.5 - cx as f32) * clamp_scale;
                let sy = cy as f32 + (y as f32 + 0.5 - cy as f32) * clamp_scale;
                let cov = ((px_dist + 0.75) / 1.5).clamp(0.0, 1.0) as f32;
                (camera.unproject(sx, sy, width as f32, height as f32), cov)
            } else {
                (
                    camera.unproject(x as f32 + 0.5, y as f32 + 0.5, width as f32, height as f32),
                    1.0_f32,
                )
            };
            let Some((lat, lon)) = coord else {
                let outer = halo_rgb(d);
                rgba[idx] = outer[0];
                rgba[idx + 1] = outer[1];
                rgba[idx + 2] = outer[2];
                rgba[idx + 3] = 255;
                continue;
            };

            let mut rgb = if supersample && limb_cov >= 0.999 {
                let mut acc = [0u32; 3];
                let mut count = 0u32;
                for (ox, oy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                    if let Some((slat, slon)) =
                        camera.unproject(x as f32 + ox, y as f32 + oy, width as f32, height as f32)
                    {
                        let c = sample_style(slat, slon);
                        acc[0] += c[0] as u32;
                        acc[1] += c[1] as u32;
                        acc[2] += c[2] as u32;
                        count += 1;
                    }
                }
                if count > 0 {
                    [
                        (acc[0] / count) as u8,
                        (acc[1] / count) as u8,
                        (acc[2] / count) as u8,
                    ]
                } else {
                    sample_style(lat, lon)
                }
            } else {
                sample_style(lat, lon)
            };

            if limb_cov < 0.999 {
                let outer = halo_rgb(d.max(1.0));
                rgba[idx] = (outer[0] as f32 + (rgb[0] as f32 - outer[0] as f32) * limb_cov) as u8;
                rgba[idx + 1] =
                    (outer[1] as f32 + (rgb[1] as f32 - outer[1] as f32) * limb_cov) as u8;
                rgba[idx + 2] =
                    (outer[2] as f32 + (rgb[2] as f32 - outer[2] as f32) * limb_cov) as u8;
                rgba[idx + 3] = 255;
            } else {
                let mask_sample = if style != BasemapStyle::Oilslick
                    && camera.zoom >= 6.0
                    && let Some(assets) = assets
                    && let Some(masks) = &assets.grwl_masks
                {
                    masks.sample_water(lat, lon)
                } else {
                    None
                };
                let water_alpha = match mask_sample {
                    Some(true) => 0.94_f32,
                    Some(false) => 0.0_f32,
                    None => {
                        if let Some(&dist_frac) = river_field.get((y * width + x) as usize)
                            && dist_frac < 1.0
                        {
                            ((1.0 - dist_frac) * 3.0).clamp(0.0, 0.94)
                        } else {
                            0.0
                        }
                    }
                };
                if water_alpha > 0.01 {
                    let w_rgb = [38.0_f32, 102.0, 172.0];
                    rgb[0] = (rgb[0] as f32 * (1.0 - water_alpha) + w_rgb[0] * water_alpha) as u8;
                    rgb[1] = (rgb[1] as f32 * (1.0 - water_alpha) + w_rgb[1] * water_alpha) as u8;
                    rgb[2] = (rgb[2] as f32 * (1.0 - water_alpha) + w_rgb[2] * water_alpha) as u8;
                }
                rgba[idx] = rgb[0];
                rgba[idx + 1] = rgb[1];
                rgba[idx + 2] = rgb[2];
                rgba[idx + 3] = 255;
            }
        }
    }
    BasemapFrame {
        rgba,
        contour_interval_m,
        local_min_m,
        local_max_m,
    }
}

fn fallback_natural_rgb(lat: f64, lon: f64) -> [u8; 3] {
    let checker = (((lat / 15.0).floor() as i32 + (lon / 15.0).floor() as i32) & 1) == 0;
    if checker { [34, 48, 66] } else { [30, 43, 60] }
}

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
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
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
    if let Ok(bytes) = fs::read(&compact)
        && let Ok(cities) = serde_json::from_slice::<Vec<CityPoint>>(&bytes)
    {
        return cities;
    }
    let raw = dir.join("ne_10m_populated_places_simple.geojson");
    if let Ok(bytes) = fs::read(&raw)
        && let Ok(cities) = parse_cities_geojson(&bytes)
    {
        if let Ok(encoded) = serde_json::to_vec(&cities) {
            let _ = fs::write(&compact, encoded);
        }
        return cities;
    }
    let legacy = dir.join("cities_250k.json");
    if let Ok(bytes) = fs::read(&legacy)
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
    if let Ok(bytes) = fs::read(&compact)
        && let Ok(lines) = serde_json::from_slice::<Vec<BorderLine>>(&bytes)
    {
        return lines;
    }
    let raw = dir.join(raw_name);
    if let Ok(bytes) = fs::read(&raw)
        && let Ok(lines) = parse_borders_geojson(&bytes)
    {
        if let Ok(encoded) = serde_json::to_vec(&lines) {
            let _ = fs::write(&compact, encoded);
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
    if let Ok(bytes) = fs::read(&compact)
        && let Ok(lines) = serde_json::from_slice::<Vec<RoadLine>>(&bytes)
    {
        return lines;
    }
    let raw = dir.join("ne_10m_roads.geojson");
    if let Ok(bytes) = fs::read(&raw)
        && let Ok(lines) = parse_roads_geojson(&bytes)
    {
        if let Ok(encoded) = serde_json::to_vec(&lines) {
            let _ = fs::write(&compact, encoded);
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
                out.push(LakePolygon {
                    bounds: b,
                    points: pts,
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
    let compact = dir.join("lakes_10m.json");
    if let Ok(bytes) = fs::read(&compact)
        && let Ok(polys) = serde_json::from_slice::<Vec<LakePolygon>>(&bytes)
    {
        return polys;
    }
    let raw = dir.join("ne_10m_lakes.geojson");
    if let Ok(bytes) = fs::read(&raw)
        && let Ok(polys) = parse_lakes_geojson(&bytes)
    {
        if let Ok(encoded) = serde_json::to_vec(&polys) {
            let _ = fs::write(&compact, encoded);
        }
        return polys;
    }
    Vec::new()
}

fn load_ne2_overview(dir: &Path) -> Option<EquirectTexture> {
    let cached = dir.join("ne2_overview.rgb");
    if let Ok(bytes) = fs::read(&cached)
        && bytes.len() >= 8
    {
        let w = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
        let h = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
        if bytes.len() == 8 + (w as usize) * (h as usize) * 3 {
            return Some(EquirectTexture {
                width: w,
                height: h,
                rgb: Arc::new(bytes[8..].to_vec()),
            });
        }
    }
    let img_path = find_first_with_ext(dir, &["png", "jpg", "jpeg", "tif", "tiff"])?;
    let mut reader = image::ImageReader::open(&img_path).ok()?;
    reader.limits(image::Limits::no_limits());
    let decoded = reader.decode().ok()?;
    let resized = decoded.resize_exact(4096, 2048, image::imageops::FilterType::Triangle);
    let rgb = resized.to_rgb8().into_raw();
    let mut blob = Vec::with_capacity(8 + rgb.len());
    blob.extend_from_slice(&4096u32.to_le_bytes());
    blob.extend_from_slice(&2048u32.to_le_bytes());
    blob.extend_from_slice(&rgb);
    let _ = fs::write(&cached, &blob);
    Some(EquirectTexture {
        width: 4096,
        height: 2048,
        rgb: Arc::new(rgb),
    })
}

fn load_hgt_dir(dir: &Path, out: &mut HashMap<(i32, i32), SrtmTile>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !p
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("hgt"))
        {
            continue;
        }
        if let Some((lat, lon)) = parse_hgt_name(stem)
            && let Ok(bytes) = fs::read(&p)
            && let Some(tile) = SrtmTile::from_hgt_bytes(lat, lon, &bytes)
        {
            out.insert((lat, lon), tile);
        }
    }
}

fn parse_hgt_name(stem: &str) -> Option<(i32, i32)> {
    let b = stem.as_bytes();
    if b.len() < 7 {
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
    Some((lat, lon))
}

fn load_global_dem(dir: &Path) -> Option<GlobalDemGrid> {
    let path = dir.join("gebco_overview.i16");
    if let Ok(bytes) = fs::read(&path)
        && bytes.len() >= 8
    {
        let width = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
        let height = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
        let expected = 8 + (width as usize) * (height as usize) * 2;
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
    let file = fs::File::open(&tif_path).ok()?;
    let mut decoder = tiff::decoder::Decoder::new(file)
        .ok()?
        .with_limits(tiff::decoder::Limits::unlimited());
    let mut best_ifd = 0usize;
    let mut best_diff = u32::MAX;
    for idx in 0..16usize {
        if idx > 0 && decoder.seek_to_image(idx).is_err() {
            break;
        }
        if let Ok((w, _h)) = decoder.dimensions() {
            let diff = w.abs_diff(5400);
            if w <= 6000 && diff < best_diff {
                best_diff = diff;
                best_ifd = idx;
            }
        }
    }
    decoder.seek_to_image(best_ifd).ok()?;
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
    let _ = fs::write(&path, &blob);
    Some(GlobalDemGrid {
        width,
        height,
        heights,
    })
}

fn load_land_cover(dir: &Path) -> Option<GlobalLandCoverGrid> {
    let path = dir.join("land_cover.u8");
    let bytes = fs::read(path).ok()?;
    if bytes.len() < 8 {
        return None;
    }
    let width = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    if bytes.len() != 8 + (width as usize) * (height as usize) {
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
