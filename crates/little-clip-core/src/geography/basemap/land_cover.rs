//! One-time, bounded conversion of the Copernicus NetCDF categorical layer.
use super::{LandCoverClass, storage::AssetDirectory, tiff_limits};
use crate::{CommandRequest, file_source::FileSource};
use std::{
    ffi::OsStr,
    fs::File,
    io::{BufReader, Seek, SeekFrom},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub const CONVERSION_SPACE: u64 = 72 * 1024 * 1024;
const WIDTH: u32 = 4096;
const HEIGHT: u32 = 2048;

/// Converts `lccs_class` using nearest-neighbor resampling into the renderer's
/// north-up global grid. Inputs and staging output remain owned descriptors.
/// Existing compact data is replaced only after successful conversion.
pub fn import_netcdf(
    directory: &AssetDirectory,
    source: &File,
    available_bytes: u64,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    if available_bytes < CONVERSION_SPACE {
        return Err("Insufficient geography disk budget for land-cover conversion".into());
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err("Land-cover conversion cancelled".into());
    }
    let identity = FileSource::capture_file(source)?;
    let staged = directory.stage().map_err(|error| error.to_string())?;
    let (request, input) = CommandRequest::new("gdalwarp")
        .args([
            "-q",
            "-of",
            "GTiff",
            "-ot",
            "Byte",
            "-srcband",
            "1",
            "-r",
            "near",
            "-t_srs",
            "EPSG:4326",
            "-te",
            "-180",
            "-90",
            "180",
            "90",
            "-ts",
            "4096",
            "2048",
            "-dstnodata",
            "0",
            "-wm",
            "32",
            "-co",
            "TILED=YES",
            "-co",
            "COMPRESS=NONE",
            "--config",
            "GDAL_CACHEMAX",
            "32",
            "--config",
            "GDAL_PAM_ENABLED",
            "NO",
            "--config",
            "GDAL_DISABLE_READDIR_ON_OPEN",
            "EMPTY_DIR",
        ])
        .timeout(Duration::from_secs(300))
        .max_output_bytes(16 * 1024)
        .resource_limit(rustix::process::Resource::As, 2 * 1024 * 1024 * 1024)
        .resource_limit(rustix::process::Resource::Fsize, 64 * 1024 * 1024)
        .own_process_group()
        .die_with_parent()
        .inherit_handle(source)
        .map_err(|error| error.to_string())?;
    let (request, output) = request
        .inherit_handle(staged.as_file())
        .map_err(|error| error.to_string())?;
    let result = request
        .arg(format!("NETCDF:\"/proc/self/fd/{input}\":lccs_class"))
        .arg(format!("/proc/self/fd/{output}"))
        .run_cancellable(cancelled)
        .map_err(|error| {
            format!("Land-cover conversion needs GDAL 3.7+ with NetCDF support (gdalwarp): {error}")
        })?;
    if !result.status.success() {
        return Err(format!(
            "Land-cover conversion failed: {}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    if cancelled.load(Ordering::Relaxed) || FileSource::capture_file(source)? != identity {
        return Err("Land-cover source changed or conversion was cancelled".into());
    }
    let mut file = staged
        .as_file()
        .try_clone()
        .map_err(|error| error.to_string())?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let mut decoder = tiff::decoder::Decoder::new(BufReader::new(file))
        .map_err(|error| error.to_string())?
        .with_limits(tiff_limits(16 * 1024 * 1024));
    if decoder.dimensions().map_err(|error| error.to_string())? != (WIDTH, HEIGHT)
        || decoder.colortype().map_err(|error| error.to_string())? != tiff::ColorType::Gray(8)
    {
        return Err("Land-cover converter returned an invalid categorical grid".into());
    }
    let tiff::decoder::DecodingResult::U8(pixels) =
        decoder.read_image().map_err(|error| error.to_string())?
    else {
        return Err("Land-cover converter returned an unsupported pixel type".into());
    };
    if pixels.len() != WIDTH as usize * HEIGHT as usize || pixels.iter().all(|&p| p == 0) {
        return Err("Land-cover conversion produced no classified pixels".into());
    }
    let mut compact = Vec::with_capacity(8 + pixels.len());
    compact.extend_from_slice(&WIDTH.to_le_bytes());
    compact.extend_from_slice(&HEIGHT.to_le_bytes());
    compact.extend(
        pixels
            .into_iter()
            .map(|code| LandCoverClass::from_esa_cci(code) as u8),
    );
    if cancelled.load(Ordering::Relaxed) {
        return Err("Land-cover conversion cancelled".into());
    }
    directory
        .write(OsStr::new("land_cover.u8"), &compact)
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires GDAL 3.7+ with the NetCDF driver"]
    fn imports_real_netcdf_with_south_to_north_coordinates_and_preserves_categories() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("fixture.nc");
        std::fs::write(
            &source,
            include_bytes!("../../../tests/fixtures/geography/land_cover.nc"),
        )
        .unwrap();
        let directory = AssetDirectory::open(root.path()).unwrap();
        let input = directory.open_file(OsStr::new("fixture.nc")).unwrap();
        import_netcdf(
            &directory,
            &input,
            CONVERSION_SPACE,
            &AtomicBool::new(false),
        )
        .unwrap();
        let grid = super::super::load_land_cover(root.path()).unwrap();
        assert_eq!(grid.sample(45.0, -90.0), LandCoverClass::Forest);
        assert_eq!(grid.sample(45.0, 90.0), LandCoverClass::GrassCrops);
        assert_eq!(grid.sample(-45.0, -90.0), LandCoverClass::SnowIce);
        assert_eq!(grid.sample(-45.0, 90.0), LandCoverClass::Ocean);
        let before = std::fs::read(root.path().join("land_cover.u8")).unwrap();
        assert!(
            import_netcdf(&directory, &input, CONVERSION_SPACE, &AtomicBool::new(true)).is_err()
        );
        assert_eq!(
            std::fs::read(root.path().join("land_cover.u8")).unwrap(),
            before
        );
        assert!(import_netcdf(&directory, &input, 0, &AtomicBool::new(false)).is_err());
    }
}
