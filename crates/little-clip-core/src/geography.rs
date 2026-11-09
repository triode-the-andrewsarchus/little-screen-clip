//! Bounded WGS84 geometry and offline map/profile preparation.
use chrono::DateTime;
use roxmltree::{Document as Xml, Node};

/// Maximum encoded geographic document size.
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_POINTS: usize = 200_000;
const MAX_PATHS: usize = 4096;
const DRAW_POINTS: usize = 8192;
/// A geographic shape, without inferring a navigable road or terrain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// Independent locations.
    Point,
    /// Ordered line paths with explicit segment breaks.
    Line,
}
/// One supplied WGS84 position.
#[derive(Clone, Debug)]
pub struct Point {
    /// Longitude in degrees.
    pub lon: f64,
    /// Latitude in degrees.
    pub lat: f64,
    /// Supplied elevation in meters, not inferred terrain height.
    pub elevation: Option<f64>,
    /// Original supplied time, without assuming a missing time zone.
    pub time: String,
}
/// One bounded drawing sample, retaining an actual input position.
#[derive(Clone, Debug)]
pub struct Sample {
    /// Supplied position.
    pub point: Point,
    /// Offline equirectangular coordinates, north positive, in degrees.
    pub xy: [f64; 2],
    /// Cumulative spherical distance, excluding segment gaps.
    pub distance_m: f64,
    /// Elevation lines must not connect to the previous sample across missing values.
    pub profile_break: bool,
}
/// One named feature and its sampled, explicitly separated paths.
#[derive(Clone)]
pub struct Feature {
    /// Supplied name or a type-specific placeholder.
    pub name: String,
    /// Declared geometry kind.
    pub kind: Kind,
    /// Actual drawing samples; separate paths are never connected.
    pub paths: Vec<Vec<Sample>>,
    /// Number of original positions before drawing reduction.
    pub points: usize,
    /// Spherical distance over all original adjacent positions, not a road distance.
    pub distance_m: f64,
    /// Elevation extrema over all supplied readable elevations.
    pub elevation: Option<[f64; 2]>,
    /// Supplied text properties.
    pub properties: Vec<(String, String)>,
}
/// Prepared offline geography shared by folder previews and the viewer.
pub struct Document {
    /// Supplied document name.
    pub title: String,
    /// Features in source order.
    pub features: Vec<Feature>,
    /// Projected bounds [minimum x, minimum y, maximum x, maximum y].
    pub bounds: [f64; 4],
    /// Actual partial-data or unsupported-data disclosures.
    pub notes: Vec<String>,
    /// Total original positions.
    pub points: usize,
    /// Number of drawing samples.
    pub samples: usize,
}
struct RawFeature {
    name: String,
    kind: Kind,
    paths: Vec<Vec<Point>>,
    properties: Vec<(String, String)>,
}
impl Document {
    /// Reads GPX 1.0/1.1 tracks, routes and waypoints without following external resources.
    ///
    /// # Errors
    /// Rejects invalid XML, non-GPX roots, unreadable positions and exceeded input budgets.
    pub fn gpx(bytes: &[u8]) -> Result<Self, String> {
        let xml = gpx_xml(bytes)?;
        let root = xml.root_element();
        let namespace = root.tag_name().namespace();
        let mut raw = Vec::new();
        let mut total = 0;
        let mut paths = 0;
        for node in root
            .children()
            .filter(|n| n.is_element() && n.tag_name().namespace() == namespace)
        {
            let kind = node.tag_name().name();
            if !matches!(kind, "wpt" | "rte" | "trk") {
                continue;
            }
            if raw.len() >= 4096 {
                return Err("GPX exceeds the 4,096-feature limit".into());
            }
            let feature = gpx_feature(node, kind)?;
            total += feature.paths.iter().map(Vec::len).sum::<usize>();
            paths += feature.paths.len();
            if total > MAX_POINTS || paths > MAX_PATHS {
                return Err("GPX exceeds its 200,000-position or 4,096-segment limit".into());
            }
            raw.push(feature);
        }
        let title = children(root, "metadata")
            .next()
            .and_then(|m| child_text(m, "name"))
            .or_else(|| child_text(root, "name"))
            .unwrap_or_default();
        let extensions = xml
            .descendants()
            .filter(|n| {
                n.is_element()
                    && n.tag_name().name() == "extensions"
                    && n.tag_name().namespace() == namespace
            })
            .count();
        let mut notes = Vec::new();
        if extensions > 0 {
            notes.push(format!(
                "{extensions} extension containers are not interpreted."
            ));
        }
        prepare(title, raw, notes)
    }
}
fn gpx_xml(bytes: &[u8]) -> Result<Xml<'_>, String> {
    if bytes.len() > MAX_BYTES {
        return Err("GPX exceeds the 16 MiB opening limit".into());
    }
    crate::xml_content::validate(bytes, 1_000_000, 128)?;
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let xml = Xml::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 800_000,
        },
    )
    .map_err(|e| e.to_string())?;
    let root = xml.root_element();
    let namespace = root.tag_name().namespace();
    if root.tag_name().name() != "gpx"
        || !matches!(
            namespace,
            Some("http://www.topografix.com/GPX/1/1" | "http://www.topografix.com/GPX/1/0")
        )
    {
        return Err("Expected a GPX 1.0 or 1.1 document with its GPX namespace".into());
    }
    Ok(xml)
}
fn gpx_feature(node: Node<'_, '_>, kind: &str) -> Result<RawFeature, String> {
    let groups = match kind {
        "wpt" => vec![vec![point(node)?]],
        "rte" => vec![positions(node, "rtept")?],
        _ => children(node, "trkseg")
            .map(|segment| positions(segment, "trkpt"))
            .collect::<Result<Vec<_>, _>>()?,
    };
    let properties = children_text(node, &["desc", "cmt", "src", "type", "number", "sym"]);
    Ok(RawFeature {
        name: child_text(node, "name").unwrap_or_else(|| {
            match kind {
                "wpt" => "Waypoint",
                "rte" => "Route",
                _ => "Track",
            }
            .into()
        }),
        kind: if kind == "wpt" {
            Kind::Point
        } else {
            Kind::Line
        },
        paths: groups,
        properties,
    })
}
fn children<'a>(node: Node<'a, 'a>, name: &'a str) -> impl Iterator<Item = Node<'a, 'a>> {
    node.children().filter(move |n| {
        n.is_element()
            && n.tag_name().namespace() == node.tag_name().namespace()
            && n.tag_name().name() == name
    })
}
fn child_text(node: Node<'_, '_>, name: &str) -> Option<String> {
    node.children()
        .find(|n| {
            n.is_element()
                && n.tag_name().namespace() == node.tag_name().namespace()
                && n.tag_name().name() == name
        })
        .and_then(|n| n.text())
        .map(|s| clip(s, 2048))
}
fn children_text(node: Node<'_, '_>, names: &[&str]) -> Vec<(String, String)> {
    names
        .iter()
        .filter_map(|name| child_text(node, name).map(|value| ((*name).into(), value)))
        .collect()
}
fn positions(node: Node<'_, '_>, name: &str) -> Result<Vec<Point>, String> {
    children(node, name)
        .take(MAX_POINTS + 1)
        .map(point)
        .collect()
}
fn point(node: Node<'_, '_>) -> Result<Point, String> {
    let lon = node
        .attribute("lon")
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && (-180.0..=180.0).contains(v))
        .ok_or("Unreadable GPX longitude")?;
    let lat = node
        .attribute("lat")
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && (-90.0..=90.0).contains(v))
        .ok_or("Unreadable GPX latitude")?;
    let elevation = match child_text(node, "ele") {
        Some(text) => Some(
            text.parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or("Unreadable GPX elevation")?,
        ),
        None => None,
    };
    Ok(Point {
        lon,
        lat,
        elevation,
        time: child_text(node, "time").unwrap_or_default(),
    })
}
fn clip(value: &str, limit: usize) -> String {
    if value.len() > limit {
        format!("{}…", &value[..value.floor_char_boundary(limit)])
    } else {
        value.into()
    }
}
fn prepare(
    title: String,
    raw: Vec<RawFeature>,
    mut notes: Vec<String>,
) -> Result<Document, String> {
    let points: usize = raw.iter().flat_map(|f| &f.paths).map(Vec::len).sum();
    if points == 0 {
        return Err("No readable GPX positions".into());
    }
    let (origin, cosine) = projection(&raw, points);
    let mut bounds = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    let mut features = Vec::new();
    let mut samples = 0;
    for feature in raw {
        let feature = prepare_feature(feature, points, origin, cosine, &mut bounds);
        samples += feature.paths.iter().map(Vec::len).sum::<usize>();
        features.push(feature);
    }
    if samples < points {
        notes.push(format!("Map/profile drawing uses {samples} of {points} positions; distances and elevation bounds use all positions."));
    }
    if features
        .iter()
        .flat_map(|f| &f.paths)
        .flatten()
        .any(|s| !s.point.time.is_empty() && DateTime::parse_from_rfc3339(&s.point.time).is_err())
    {
        notes.push(
            "Some supplied times are not readable RFC3339 values; their original text is retained."
                .into(),
        );
    }
    Ok(Document {
        title,
        features,
        bounds,
        notes,
        points,
        samples,
    })
}
fn projection(raw: &[RawFeature], points: usize) -> (f64, f64) {
    let mut longitudes: Vec<_> = raw
        .iter()
        .flat_map(|f| &f.paths)
        .flatten()
        .map(|p| p.lon.rem_euclid(360.0))
        .collect();
    longitudes.sort_by(f64::total_cmp);
    let mut gap = (0.0, 0.0);
    for index in 0..longitudes.len() {
        let next = longitudes[(index + 1) % longitudes.len()]
            + if index + 1 == longitudes.len() {
                360.0
            } else {
                0.0
            };
        if next - longitudes[index] > gap.0 {
            gap = (
                next - longitudes[index],
                longitudes[(index + 1) % longitudes.len()],
            );
        }
    }
    let origin = gap.1;
    let latitude = raw
        .iter()
        .flat_map(|f| &f.paths)
        .flatten()
        .map(|p| p.lat)
        .sum::<f64>()
        / points as f64;
    let cosine = latitude.to_radians().cos().abs().max(0.01);
    (origin, cosine)
}
fn prepare_feature(
    feature: RawFeature,
    points: usize,
    origin: f64,
    cosine: f64,
    bounds: &mut [f64; 4],
) -> Feature {
    let count: usize = feature.paths.iter().map(Vec::len).sum();
    let stride = count.div_ceil((count * DRAW_POINTS / points).max(2)).max(1);
    let mut distance = 0.0;
    let mut elevation: Option<[f64; 2]> = None;
    let mut paths = Vec::new();
    for path in feature.paths {
        let mut prepared = Vec::new();
        let mut profile_gap = true;
        for (index, point) in path.iter().enumerate() {
            if index > 0 && feature.kind == Kind::Line {
                distance += distance_m(&path[index - 1], point);
            }
            if let Some(value) = point.elevation {
                let range = elevation.get_or_insert([value, value]);
                range[0] = range[0].min(value);
                range[1] = range[1].max(value);
            }
            let xy = [
                (point.lon.rem_euclid(360.0) - origin).rem_euclid(360.0) * cosine,
                point.lat,
            ];
            for axis in 0..2 {
                bounds[axis] = bounds[axis].min(xy[axis]);
                bounds[axis + 2] = bounds[axis + 2].max(xy[axis]);
            }
            profile_gap |= point.elevation.is_none();
            if index % stride == 0 || index + 1 == path.len() {
                prepared.push(Sample {
                    point: point.clone(),
                    xy,
                    distance_m: distance,
                    profile_break: profile_gap,
                });
                profile_gap = point.elevation.is_none();
            }
        }
        if !prepared.is_empty() {
            paths.push(prepared);
        }
    }
    Feature {
        name: feature.name,
        kind: feature.kind,
        paths,
        points: count,
        distance_m: distance,
        elevation,
        properties: feature.properties,
    }
}
/// Spherical great-circle distance in meters, not a terrain-following distance.
pub fn distance_m(a: &Point, b: &Point) -> f64 {
    let lat = (b.lat - a.lat).to_radians();
    let lon = (b.lon - a.lon).to_radians();
    let h = (lat / 2.0).sin().powi(2)
        + a.lat.to_radians().cos() * b.lat.to_radians().cos() * (lon / 2.0).sin().powi(2);
    6_371_008.8 * 2.0 * h.clamp(0.0, 1.0).sqrt().asin()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tracks_routes_and_waypoints_preserve_segment_gaps_and_supplied_values() {
        let doc = Document::gpx(include_bytes!("../tests/fixtures/geography/harbor.gpx")).unwrap();
        assert_eq!(doc.title, "Harbor walk");
        assert_eq!(doc.features.len(), 3);
        assert_eq!(doc.points, 8);
        let track = &doc.features[2];
        assert_eq!(track.paths.len(), 2);
        assert_eq!(track.elevation, Some([12.0, 45.0]));
        let mut expected = 0.0;
        for path in &track.paths {
            for pair in path.windows(2) {
                expected += distance_m(&pair[0].point, &pair[1].point);
            }
        }
        assert!((track.distance_m - expected).abs() < 1e-6);
        assert!(track.distance_m > 700.0 && track.distance_m < 1100.0);
        assert_eq!(
            track.paths[1][0].distance_m,
            track.paths[0].last().unwrap().distance_m
        );
        assert!(track.paths[0][0].point.time.ends_with("-06:00"));
        assert_eq!(doc.features[0].distance_m, 0.0);
    }
    #[test]
    fn longitude_wrap_uses_a_compact_dateline_view_and_short_distance() {
        let doc=Document::gpx(br#"<gpx xmlns="http://www.topografix.com/GPX/1/1"><rte><rtept lat="0" lon="179"/><rtept lat="0" lon="-179"/></rte></gpx>"#).unwrap();
        assert!((doc.bounds[2] - doc.bounds[0] - 2.0).abs() < 1e-8);
        assert!((doc.features[0].distance_m - 222_390.16).abs() < 2.0);
    }
    #[test]
    fn invalid_geometry_entities_foreign_points_and_nesting_are_rejected() {
        for text in [
            r#"<gpx xmlns="http://www.topografix.com/GPX/1/1"><wpt lat="NaN" lon="0"/></gpx>"#,
            r#"<gpx xmlns="http://www.topografix.com/GPX/1/1"><wpt lat="91" lon="0"/></gpx>"#,
            r#"<!DOCTYPE gpx [<!ENTITY x "payload">]><gpx xmlns="http://www.topografix.com/GPX/1/1">&x;</gpx>"#,
            r#"<gpx xmlns="http://www.topografix.com/GPX/1/1"><wpt xmlns="urn:other" lat="0" lon="0"/></gpx>"#,
        ] {
            assert!(Document::gpx(text.as_bytes()).is_err());
        }
        let deep = format!("{}{}", "<p>".repeat(129), "</p>".repeat(129));
        assert!(Document::gpx(deep.as_bytes()).is_err());
    }
    #[test]
    fn missing_elevation_breaks_survive_sampling() {
        let points = (0..20_000)
            .map(|i| Point {
                lon: i as f64 / 100_000.0,
                lat: 0.0,
                elevation: (i != 1).then_some(i as f64),
                time: String::new(),
            })
            .collect();
        let doc = prepare(
            String::new(),
            vec![RawFeature {
                name: "Gap".into(),
                kind: Kind::Line,
                paths: vec![points],
                properties: vec![],
            }],
            vec![],
        )
        .unwrap();
        assert!(doc.features[0].paths[0][1].profile_break);
        assert!(!doc.features[0].paths[0][2].profile_break);
    }
    #[test]
    fn drawing_reduction_keeps_endpoints_and_full_resolution_metrics() {
        let points = (0..20_000)
            .map(|i| Point {
                lon: i as f64 / 100_000.0,
                lat: 0.0,
                elevation: Some(i as f64),
                time: String::new(),
            })
            .collect();
        let doc = prepare(
            String::new(),
            vec![RawFeature {
                name: "Dense".into(),
                kind: Kind::Line,
                paths: vec![points],
                properties: vec![],
            }],
            vec![],
        )
        .unwrap();
        assert!(doc.samples <= DRAW_POINTS + 2);
        assert_eq!(doc.points, 20_000);
        assert_eq!(
            doc.features[0].paths[0].last().unwrap().point.elevation,
            Some(19_999.0)
        );
        assert_eq!(doc.features[0].elevation, Some([0.0, 19_999.0]));
        assert!(!doc.notes.is_empty());
    }
}
