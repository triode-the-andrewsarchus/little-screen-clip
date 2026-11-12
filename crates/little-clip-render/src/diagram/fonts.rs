//! Bundled metric-compatible SVG text faces, without system-font discovery.
use std::sync::LazyLock;

pub(super) fn metric_fonts() -> &'static resvg::usvg::fontdb::Database {
    static FONTS: LazyLock<resvg::usvg::fontdb::Database> = LazyLock::new(|| {
        let mut database = super::default_fonts().clone();
        for bytes in little_clip_core::font_assets::GRAPHVIZ_FONT_BYTES {
            database.load_font_data(bytes.to_vec());
        }
        // The SVG names the metric families (Helvetica/Times/Courier), while
        // the bundled faces use their own family names. Keep both names on
        // each style/weight so explicit requests do not fall back to Tinos.
        let aliases: &[(&str, &[&str])] = &[
            ("Tinos", &["Times", "Times-Roman", "Times New Roman"]),
            ("Arimo", &["Helvetica", "Arial"]),
            ("Cousine", &["Courier", "Courier New"]),
        ];
        let faces: Vec<_> = database.faces().cloned().collect();
        for mut face in faces {
            let names = aliases
                .iter()
                .find(|(name, _)| face.families.iter().any(|(family, _)| family == name));
            if let Some((_, names)) = names {
                for name in *names {
                    face.families.push((
                        (*name).into(),
                        resvg::usvg::fontdb::Language::English_UnitedStates,
                    ));
                }
                database.remove_face(face.id);
                database.push_face_info(face);
            }
        }
        database.set_serif_family("Tinos");
        database.set_sans_serif_family("Arimo");
        database.set_monospace_family("Cousine");
        database
    });
    &FONTS
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn svg_font_family_names_resolve_to_the_metric_compatible_faces() {
        use resvg::usvg::fontdb::{Family, Query};
        let database = metric_fonts();
        for (requested, expected) in [
            ("Helvetica", "Arimo"),
            ("Times-Roman", "Tinos"),
            ("Courier", "Cousine"),
        ] {
            let id = database
                .query(&Query {
                    families: &[Family::Name(requested)],
                    ..Default::default()
                })
                .unwrap();
            assert!(
                database
                    .face(id)
                    .unwrap()
                    .families
                    .iter()
                    .any(|(name, _)| name == expected)
            );
        }
    }
}
