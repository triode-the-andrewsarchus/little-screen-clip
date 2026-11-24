//! Manual end-to-end EDA raster timing on a representative local file.
use little_clip_core::eda::{
    Document, RasterRegion,
    comparison::{compare_artwork, compare_artwork_region},
    kicad::PcbLayerBucket,
    parse_bytes, read_document,
};
use std::{fmt::Write, path::Path, sync::atomic::AtomicBool, time::Instant};

fn main() {
    let cancel = AtomicBool::new(false);
    let mut args = std::env::args_os().skip(1).peekable();
    if args.peek().is_some_and(|arg| arg == "--artwork") {
        args.next();
        let files: Vec<_> = args.collect();
        assert!(files.len() == 2, "pass two artwork files");
        let docs = [&files[0], &files[1]].map(|path| {
            let Document::Gerber(doc) = read_document(Path::new(path), &cancel).unwrap() else {
                panic!("expected Gerber or Excellon artwork");
            };
            doc
        });
        for tolerance in [0.0, 0.05] {
            let start = Instant::now();
            let result = compare_artwork([&docs[0], &docs[1]], tolerance, &cancel).unwrap();
            eprintln!(
                "{} mm tolerance: {} additions, {} removals at {} mm/px in {:?}",
                tolerance,
                result.added_pixels,
                result.removed_pixels,
                result.pitch_mm,
                start.elapsed()
            );
        }
        let bounds = docs[0].bounds_mm;
        let density = 128.0;
        let region = RasterRegion {
            origin: [(bounds[0] + bounds[2]) * 0.5, (bounds[1] + bounds[3]) * 0.5],
            size: [1100, 800],
            pixels_per_mm: density,
            y_down: false,
        };
        for tolerance in [0.0, 0.05] {
            let start = Instant::now();
            let detail =
                compare_artwork_region([&docs[0], &docs[1]], region, tolerance, &cancel).unwrap();
            eprintln!(
                "Detail with {tolerance} mm tolerance: {} additions, {} removals at {} mm/px in {:?}",
                detail.added_pixels,
                detail.removed_pixels,
                detail.pitch_mm,
                start.elapsed()
            );
        }
        return;
    }
    for item in args {
        let path = Path::new(&item);
        let start = Instant::now();
        let doc = read_document(path, &cancel).unwrap();
        eprintln!("{path:?} read and parse: {:?}", start.elapsed());
        profile(&doc, &cancel);
    }
    let mut board =
        String::from("(kicad_pcb (gr_rect (start 0 0) (end 300 200) (layer \"Edge.Cuts\"))");
    for index in 0..60_000 {
        let x = index % 300;
        let y = index / 300;
        write!(
            board,
            "(segment (start {x} {y}) (end {x}.8 {y}.7) (width 0.2) (layer \"F.Cu\"))"
        )
        .unwrap();
    }
    board.push(')');
    let doc = parse_bytes(Path::new("dense.kicad_pcb"), board.as_bytes(), &cancel).unwrap();
    eprintln!("60,000 copper segments");
    profile(&doc, &cancel);
}

fn profile(doc: &Document, cancel: &AtomicBool) {
    if let Document::SpiceRaw(raw) = doc {
        for plot in &raw.plots {
            let trace = plot
                .traces
                .iter()
                .find(|trace| trace.name.eq_ignore_ascii_case("v(out)"));
            eprintln!(
                "{}: {} rows, V(out) final {:?}",
                plot.plot_name,
                plot.total_points,
                trace.and_then(|trace| trace.samples().last().map(|sample| sample.value))
            );
        }
    }
    for size in [[1400, 1000], [2048, 2048]] {
        let start = Instant::now();
        let image = doc.render_thumbnail(size, cancel).unwrap();
        eprintln!(
            "{size:?}: {}x{}, {:?}, {} bytes",
            image.width,
            image.height,
            start.elapsed(),
            image.premultiplied_rgba.len()
        );
    }
    let dpi = 128.0;
    let start = Instant::now();
    let detail = match doc {
        Document::KicadPcb(pcb) => {
            let [x0, y0, x1, y1] = pcb.bounds_mm;
            pcb.render_region(
                RasterRegion {
                    origin: [(x0 + x1) * 0.5, (y0 + y1) * 0.5],
                    size: [1100, 800],
                    pixels_per_mm: dpi,
                    y_down: true,
                },
                &PcbLayerBucket::ALL.into_iter().collect(),
                cancel,
            )
        }
        Document::KicadSchematic(sch) => {
            let [x0, y0, x1, y1] = sch.bounds_mm;
            sch.render_region(
                RasterRegion {
                    origin: [(x0 + x1) * 0.5, (y0 + y1) * 0.5],
                    size: [1100, 800],
                    pixels_per_mm: dpi,
                    y_down: sch
                        .scene
                        .as_ref()
                        .is_none_or(|scene| scene.items.is_empty() || scene.y_down),
                },
                cancel,
            )
        }
        _ => return,
    }
    .unwrap();
    eprintln!(
        "Detail {}x{} at {dpi} px/mm: {:?}, {} bytes",
        detail.width,
        detail.height,
        start.elapsed(),
        detail.premultiplied_rgba.len()
    );
}
