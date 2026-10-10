use super::{Library, Material, Request};
use crate::{file_manager::media::PreviewData, file_source::FileSource, icons::Icon};
use little_clip_core::ImageThumbnail;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

impl Request {
    pub(in crate::file_manager) fn load(self, token: &AtomicBool) -> PreviewData {
        let mut data = PreviewData::new(String::new(), String::new(), Icon::Palette);
        match self {
            Self::Library(path) => {
                let result = (|| {
                    let source = FileSource::capture(&path)?;
                    if token.load(Ordering::Relaxed) {
                        return Err("Preview cancelled".into());
                    }
                    let materials = little_clip_core::model3d::materials::load(&path)?;
                    source.check()?;
                    if token.load(Ordering::Relaxed) {
                        return Err("Preview cancelled".into());
                    }
                    Ok::<Library, String>(Arc::new(materials.into_iter().map(Arc::new).collect()))
                })();
                match result {
                    Ok(materials) => {
                        data.detail = format!("{} materials", materials.len());
                        data.materials = Some(materials);
                    }
                    Err(error) => data.detail = error,
                }
            }
            Self::Sphere { material, edge } => {
                let (detail, missing) = details(&material);
                data.detail = detail;
                if missing > 0 {
                    data.text = format!("{missing} missing texture references");
                }
                data.image = sphere(&material, edge, token);
            }
        }
        data
    }
}

fn details(material: &Material) -> (String, usize) {
    let [dr, dg, db] = material.diffuse.map(|value| (value * 255.0) as u8);
    let [sr, sg, sb] = material.specular.map(|value| (value * 255.0) as u8);
    let mut details = format!(
        "{}\nDiffuse #{dr:02X}{dg:02X}{db:02X} · Specular #{sr:02X}{sg:02X}{sb:02X}\nShininess {:.0} · Opacity {:.0}% · IOR {:.2}",
        material.name.chars().take(60).collect::<String>(),
        material.shininess,
        material.opacity * 100.0,
        material.optical_density,
    );
    let mut missing = 0;
    for (kind, path) in &material.textures {
        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        let status = if path.is_file() {
            ""
        } else {
            missing += 1;
            " (missing)"
        };
        details.push_str(&format!(
            "\n{kind}: {}{status}",
            filename.chars().take(48).collect::<String>()
        ));
    }
    (details, missing)
}

fn sphere(material: &Material, edge: u32, token: &AtomicBool) -> Option<ImageThumbnail> {
    let edge = edge.clamp(32, 768);
    let mut pixels = vec![0; edge as usize * edge as usize * 4];
    for y in 0..edge {
        if token.load(Ordering::Relaxed) {
            return None;
        }
        let ny = 1.0 - (y as f32 + 0.5) / edge as f32 * 2.0;
        for x in 0..edge {
            let nx = (x as f32 + 0.5) / edge as f32 * 2.0 - 1.0;
            let radius = nx * nx + ny * ny;
            let checker = if ((x * 12 / edge) + (y * 12 / edge)) % 2 == 0 {
                0.24
            } else {
                0.30
            };
            let mut rgb = [checker; 3];
            if radius < 0.90 * 0.90 {
                let nx = nx / 0.90;
                let ny = ny / 0.90;
                let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
                let diffuse = (-nx * 0.45 + ny * 0.65 + nz * 0.61).max(0.0);
                let highlight = (-nx * 0.25 + ny * 0.36 + nz * 0.90)
                    .clamp(0.0, 1.0)
                    .powf(material.shininess.max(1.0));
                for channel in 0..3 {
                    let lit = (material.diffuse[channel] * (0.18 + 0.74 * diffuse)
                        + material.specular[channel] * highlight * 0.85)
                        .clamp(0.0, 1.0);
                    rgb[channel] = lit * material.opacity + checker * (1.0 - material.opacity);
                }
            }
            let offset = ((y * edge + x) * 4) as usize;
            pixels[offset..offset + 3].copy_from_slice(&rgb.map(|value| (value * 255.0) as u8));
            pixels[offset + 3] = 255;
        }
    }
    Some(ImageThumbnail {
        width: edge,
        height: edge,
        premultiplied_rgba: pixels,
    })
}
