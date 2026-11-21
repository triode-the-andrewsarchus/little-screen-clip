//! Windowed reads of the global JPL/NASA PDS LOLA 256 px/degree elevation grid.
use little_clip_core::{
    CommandRequest,
    geography::basemap::tiles::{EDGE, PADDED_EDGE, SourceTile, TileId},
    kernel_interface::open_content,
};
use rustix::process::Resource;
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

pub(in crate::file_manager) const DIRECTORY: &str = "lunar-relief-v1";
const WIDTH: usize = 46_080;
const HEIGHT: usize = WIDTH / 2;
const SAMPLES_PER_DEGREE: f64 = 256.0;
const OVERVIEW_REDUCTION: u8 = 5;
pub(super) const MAX_LEVEL: u8 = 8;

// Northern quadrants in source longitude order, followed by southern quadrants.
macro_rules! pds_quadrant {
    ($stem:literal, $bytes:literal) => {
        (
            concat!($stem, ".jp2"),
            concat!("https://pds-geosciences.wustl.edu/lro/lro-l-lola-3-rdr-v1/lrolol_1xxx/data/lola_gdr/cylindrical/jp2/", $stem, ".jp2"),
            $bytes,
        )
    };
}

pub(in crate::file_manager) const FILES: [(&str, &str, u64); 4] = [
    pds_quadrant!("ldem_256_00n_90n_000_180", 427_545_233),
    pds_quadrant!("ldem_256_00n_90n_180_360", 418_539_506),
    pds_quadrant!("ldem_256_90s_00s_000_180", 500_914_193),
    pds_quadrant!("ldem_256_90s_00s_180_360", 500_968_345),
];
pub(in crate::file_manager) const BYTES: u64 = 1_847_967_277;

pub(super) fn installed(root: &Path) -> bool {
    FILES.iter().all(|(name, _, size)| {
        open_content(&root.join(DIRECTORY).join(name), false)
            .ok()
            .and_then(|file| file.metadata().ok())
            .is_some_and(|metadata| metadata.len() == *size)
    })
}

pub(super) struct Source {
    paths: [PathBuf; 4],
    overview: [Window; 4],
}

#[derive(Clone, Copy)]
struct Bounds {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

struct Window {
    bounds: Bounds,
    reduction: u8,
    samples: Vec<i16>,
}

impl Source {
    pub fn load(root: &Path, cancel: &AtomicBool) -> Result<Option<Arc<Self>>, String> {
        if !installed(root) {
            return Ok(None);
        }
        let paths = FILES.map(|(name, _, _)| root.join(DIRECTORY).join(name));
        let overview = paths
            .iter()
            .map(|path| {
                decode(
                    path,
                    Bounds {
                        x: 0,
                        y: 0,
                        width: WIDTH,
                        height: HEIGHT,
                    },
                    OVERVIEW_REDUCTION,
                    cancel,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(Arc::new(Self {
            paths,
            overview: overview
                .try_into()
                .map_err(|_| "Expected four LOLA quadrants")?,
        })))
    }

    pub fn prepare(&self, id: TileId, cancel: &AtomicBool) -> Result<SourceTile, String> {
        let [west, north, side] = id.bounds();
        let start_x = west.rem_euclid(360.0) * SAMPLES_PER_DEGREE;
        let start_y = (90.0 - north) * SAMPLES_PER_DEGREE;
        let stride = side * SAMPLES_PER_DEGREE / EDGE as f64;
        let detail = if id.level > 3 {
            let (index, [x, y]) = quadrant(start_x + 0.5, start_y + 0.5);
            let reduction = 7u8.saturating_sub(id.level);
            let step = 1usize << reduction;
            let window = |start: usize, edge: usize| {
                let min = start.saturating_sub(step * 2) / step * step;
                let max = (start + (side * SAMPLES_PER_DEGREE).ceil() as usize + step * 2)
                    .next_multiple_of(step)
                    .min(edge);
                (min, max - min)
            };
            let (x, width) = window(x as usize, WIDTH);
            let (y, height) = window(y as usize, HEIGHT);
            Some((
                index,
                decode(
                    &self.paths[index],
                    Bounds {
                        x,
                        y,
                        width,
                        height,
                    },
                    reduction,
                    cancel,
                )?,
            ))
        } else {
            None
        };
        let mut elevation = Vec::with_capacity(PADDED_EDGE * PADDED_EDGE * 2);
        let mut elevation_range = [f32::INFINITY, f32::NEG_INFINITY];
        for y in 0..PADDED_EDGE {
            for x in 0..PADDED_EDGE {
                let (index, xy) = quadrant(
                    start_x + (x as f64 - 0.5) * stride,
                    start_y + (y as f64 - 0.5) * stride,
                );
                let window = detail
                    .as_ref()
                    .filter(|(detail_index, _)| *detail_index == index)
                    .map_or(&self.overview[index], |(_, window)| window);
                let value = window.sample(xy);
                elevation_range[0] = elevation_range[0].min(value);
                elevation_range[1] = elevation_range[1].max(value);
                elevation.extend_from_slice(&[value, 1.0]);
            }
        }
        Ok(SourceTile {
            id,
            color: Vec::new(),
            elevation,
            water: Vec::new(),
            elevation_range: Some(elevation_range),
        })
    }
}

fn quadrant(x: f64, y: f64) -> (usize, [f64; 2]) {
    let x = x.rem_euclid(2.0 * WIDTH as f64);
    let y = y.clamp(0.0, 2.0 * HEIGHT as f64 - 0.0001);
    let east = (x / WIDTH as f64) as usize;
    let south = (y / HEIGHT as f64) as usize;
    (
        east + 2 * south,
        [x - (WIDTH * east) as f64, y - (HEIGHT * south) as f64],
    )
}

impl Window {
    fn sample(&self, [x, y]: [f64; 2]) -> f32 {
        let step = (1u32 << self.reduction) as f64;
        let width = self.bounds.width.div_ceil(step as usize);
        let height = self.bounds.height.div_ceil(step as usize);
        let fx = ((x - self.bounds.x as f64) / step - 0.5).clamp(0.0, (width - 1) as f64);
        let fy = ((y - self.bounds.y as f64) / step - 0.5).clamp(0.0, (height - 1) as f64);
        let (x, y) = (fx as usize, fy as usize);
        let (right, below) = ((x + 1).min(width - 1), (y + 1).min(height - 1));
        let sample = |x, y| f64::from(self.samples[y * width + x]);
        let top = sample(x, y) + (sample(right, y) - sample(x, y)) * fx.fract();
        let bottom = sample(x, below) + (sample(right, below) - sample(x, below)) * fx.fract();
        ((top + (bottom - top) * fy.fract()) * 0.5) as f32
    }
}

fn decode(
    path: &Path,
    bounds: Bounds,
    reduction: u8,
    cancel: &AtomicBool,
) -> Result<Window, String> {
    let find_error = |error: std::io::Error| format!("LOLA decoder: {error}");
    let source = open_content(path, false).map_err(find_error)?;
    let stage = tempfile::tempdir().map_err(find_error)?;
    let input = stage.path().join("input.jp2");
    let output = stage.path().join("elevation.pgx");
    // OpenJPEG selects the input decoder by filename extension. The private
    // alias keeps that extension and points at the inherited ordinary file.
    let (request, fd) = CommandRequest::new("opj_decompress")
        .inherit_handle(&source)
        .map_err(find_error)?;
    std::os::unix::fs::symlink(format!("/proc/self/fd/{fd}"), &input).map_err(find_error)?;
    let area = format!(
        "{},{},{},{}",
        bounds.x,
        bounds.y,
        bounds.x + bounds.width,
        bounds.y + bounds.height
    );
    let result = request
        .args(["-quiet", "-threads", "1", "-i"])
        .arg(input)
        .args(["-r", &reduction.to_string(), "-d", &area, "-o"])
        .arg_content_output(&output)
        .map_err(find_error)?
        .resource_limit(Resource::As, 1536 * 1024 * 1024)
        .resource_limit(Resource::Fsize, 8 * 1024 * 1024)
        .own_process_group()
        .die_with_parent()
        .timeout(Duration::from_secs(35))
        .run_cancellable(cancel)
        .map_err(|error| format!("LOLA elevation requires OpenJPEG's opj_decompress: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "LOLA window decode failed: {}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    read_pgx(
        File::open(stage.path().join("elevation_0.pgx")).map_err(find_error)?,
        bounds,
        reduction,
    )
    .map_err(find_error)
}

fn read_pgx(file: impl Read, bounds: Bounds, reduction: u8) -> std::io::Result<Window> {
    let divider = 1usize << reduction;
    let width = bounds.width.div_ceil(divider);
    let height = bounds.height.div_ceil(divider);
    let mut reader = BufReader::new(file);
    let header = format!("PG ML - 16 {width} {height}\n");
    let mut actual = vec![0; header.len()];
    reader.read_exact(&mut actual)?;
    if actual != header.as_bytes() {
        return Err(std::io::Error::other("unexpected signed 16-bit PGX header"));
    }
    let mut data = vec![0u8; 2 * width * height];
    reader.read_exact(&mut data)?;
    if !reader.fill_buf()?.is_empty() {
        return Err(std::io::Error::other(
            "trailing pixels in PGX elevation window",
        ));
    }
    let samples = data
        .chunks_exact(2)
        .map(|n| i16::from_be_bytes([n[0], n[1]]))
        .collect();
    Ok(Window {
        bounds,
        reduction,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_elevation_is_in_half_meters_and_interpolates() {
        assert_eq!(BYTES, FILES.iter().map(|file| file.2).sum::<u64>());
        let mut bytes = b"PG ML - 16 2 2\n".to_vec();
        for height in [-200i16, 100, 200, 500] {
            bytes.extend_from_slice(&height.to_be_bytes());
        }
        let window = read_pgx(
            bytes.as_slice(),
            Bounds {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            0,
        )
        .unwrap();
        assert_eq!(window.sample([0.5, 0.5]), -100.0);
        assert_eq!(window.sample([1.0, 1.0]), 75.0);
        assert_eq!(quadrant(-1.0, 0.0), (1, [46_079.0, 0.0]));
        assert_eq!(quadrant(0.0, 23_040.5), (2, [0.0, 0.5]));
    }

    #[test]
    #[ignore = "requires the NASA LOLA pack and OpenJPEG tools"]
    fn official_lola_windows_load_at_global_and_local_scales() {
        let root = PathBuf::from(std::env::var_os("CLIP_TEST_LOLA_ROOT").unwrap());
        let cancel = AtomicBool::new(false);
        let source = Source::load(&root, &cancel).unwrap().unwrap();
        let [lat, lon] = [0.674, 23.473];
        for level in [0, 4, MAX_LEVEL] {
            let tile = source
                .prepare(TileId::at(level, lat, lon), &cancel)
                .unwrap();
            let height = tile.height_at(lat, lon).unwrap();
            assert!((-6_000.0..-400.0).contains(&height), "{level}: {height}");
            let [low, high] = tile.elevation_range.unwrap();
            assert!(high > low, "{level}: {low}..{high}");
        }
    }
}
