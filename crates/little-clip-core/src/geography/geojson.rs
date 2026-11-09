//! RFC 7946 geometry, with bounded attributes and no external resource resolution.
use super::{Document, Kind, MAX_BYTES, MAX_PATHS, MAX_POINTS, Point, RawFeature, clip, prepare};
use serde_json::Value;
use std::collections::BTreeSet;

pub(super) fn parse(bytes: &[u8]) -> Result<Document, String> {
    if bytes.len() > MAX_BYTES {
        return Err("GeoJSON exceeds the 16 MiB opening limit".into());
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("Invalid GeoJSON: {e}"))?;
    let mut reader = Reader::default();
    check_crs(&value)?;
    match tag(&value)? {
        "FeatureCollection" => {
            let features = array(&value["features"])?;
            if features.len() > 4096 {
                return Err("GeoJSON exceeds the 4,096-shape limit".into());
            }
            for feature in features {
                reader.feature(feature)?;
            }
        }
        "Feature" => reader.feature(&value)?,
        _ => reader.geometry(&value, "Geometry", Vec::new(), 0)?,
    }
    let title = value["name"]
        .as_str()
        .map(|s| clip(s, 256))
        .unwrap_or_default();
    if reader.features.is_empty() {
        reader.features.push(RawFeature {
            name: "Empty collection".into(),
            kind: Kind::Empty,
            paths: Vec::new(),
            properties: Vec::new(),
        });
    }
    drop(value);
    prepare(title, reader.features, reader.notes.into_iter().collect())
}
fn tag(value: &Value) -> Result<&str, String> {
    value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "GeoJSON object has no type".into())
}
fn array(value: &Value) -> Result<&Vec<Value>, String> {
    value
        .as_array()
        .ok_or_else(|| "Expected a GeoJSON coordinate or object array".into())
}
fn check_crs(value: &Value) -> Result<(), String> {
    let Some(crs) = value.get("crs").filter(|v| !v.is_null()) else {
        return Ok(());
    };
    let name = crs["properties"]["name"].as_str().unwrap_or("");
    if crs["type"] == "name"
        && matches!(
            name,
            "EPSG:4326"
                | "urn:ogc:def:crs:EPSG::4326"
                | "urn:ogc:def:crs:OGC::CRS84"
                | "urn:ogc:def:crs:OGC:1.3:CRS84"
                | "OGC:CRS84"
        )
    {
        Ok(())
    } else {
        Err("This viewer requires WGS84 longitude/latitude; the declared CRS is unsupported".into())
    }
}
#[derive(Default)]
struct Reader {
    features: Vec<RawFeature>,
    points: usize,
    paths: usize,
    property_bytes: usize,
    notes: BTreeSet<String>,
}
impl Reader {
    fn feature(&mut self, value: &Value) -> Result<(), String> {
        if tag(value)? != "Feature" {
            return Err("FeatureCollection contains a non-Feature object".into());
        }
        check_crs(value)?;
        let properties = self.properties(value.get("properties"))?;
        let name = value["properties"]["name"]
            .as_str()
            .or_else(|| value["properties"]["title"].as_str())
            .unwrap_or("Feature");
        let mut properties = properties;
        if let Some(id) = value.get("id") {
            properties.push(("Feature ID".into(), display(id)));
        }
        self.geometry(
            value
                .get("geometry")
                .ok_or("Feature has no geometry member")?,
            name,
            properties,
            0,
        )
    }
    fn properties(&mut self, value: Option<&Value>) -> Result<Vec<(String, String)>, String> {
        let Some(value) = value.filter(|v| !v.is_null()) else {
            return Ok(Vec::new());
        };
        let object = value
            .as_object()
            .ok_or("GeoJSON properties must be an object or null")?;
        if object.len() > 32 {
            self.notes
                .insert("Only the first 32 attributes per shape are shown.".into());
        }
        let mut fields = Vec::new();
        for (key, value) in object.iter().take(32) {
            let key = clip(key, 128);
            let value = display(value);
            if value.ends_with('…') {
                self.notes
                    .insert("Long text attributes are clipped for display.".into());
            }
            fields.push((key, value));
        }
        Ok(fields)
    }
    fn geometry(
        &mut self,
        value: &Value,
        name: &str,
        mut properties: Vec<(String, String)>,
        depth: usize,
    ) -> Result<(), String> {
        if depth > 32 {
            return Err("GeoJSON geometry collections are too deeply nested".into());
        }
        check_crs(value)?;
        if value.is_null() {
            properties.push(("Geometry".into(), "Null".into()));
            return self.push(name, Kind::Empty, Vec::new(), properties);
        }
        let kind = tag(value)?;
        if kind == "GeometryCollection" {
            let parts = array(&value["geometries"])?;
            if parts.is_empty() {
                properties.push(("Geometry".into(), kind.into()));
                return self.push(name, Kind::Empty, Vec::new(), properties);
            }
            self.notes.insert("Geometry collections are split into selectable shapes; source attributes accompany each part.".into());
            for (index, part) in parts.iter().enumerate() {
                self.geometry(
                    part,
                    &format!("{} / {}", clip(name, 256), index + 1),
                    properties.clone(),
                    depth + 1,
                )?;
            }
            return Ok(());
        }
        let (shape, paths) = self.coordinates(kind, &value["coordinates"])?;
        properties.push(("Geometry".into(), kind.into()));
        self.push(name, shape, paths, properties)
    }
    fn coordinates(
        &mut self,
        kind: &str,
        value: &Value,
    ) -> Result<(Kind, Vec<Vec<Point>>), String> {
        let paths = match kind {
            "Point" => vec![vec![self.point(array(value)?)?]],
            "MultiPoint" => vec![self.path(value, false)?],
            "LineString" => vec![self.line(value)?],
            "MultiLineString" => array(value)?
                .iter()
                .map(|line| self.line(line))
                .collect::<Result<Vec<_>, _>>()?,
            "Polygon" => self.polygon(value)?,
            "MultiPolygon" => {
                let mut paths = Vec::new();
                for polygon in array(value)? {
                    paths.extend(self.polygon(polygon)?);
                }
                paths
            }
            _ => return Err(format!("Unsupported GeoJSON geometry: {}", clip(kind, 80))),
        };
        let kind = match kind {
            "Point" | "MultiPoint" => Kind::Point,
            "Polygon" | "MultiPolygon" => Kind::Polygon,
            _ => Kind::Line,
        };
        Ok((kind, paths))
    }
    fn path(&mut self, value: &Value, ring: bool) -> Result<Vec<Point>, String> {
        let values = array(value)?;
        if self.points.saturating_add(values.len()) > MAX_POINTS {
            return Err("GeoJSON exceeds the 200,000-position limit".into());
        }
        let points = values
            .iter()
            .map(|value| self.point(array(value)?))
            .collect::<Result<Vec<_>, _>>()?;
        if ring && !points.is_empty() {
            let first = &points[0];
            let last = points.last().unwrap();
            if points.len() < 4
                || first.lon != last.lon
                || first.lat != last.lat
                || first.elevation != last.elevation
            {
                return Err("A polygon ring needs at least four positions and an equivalent closing position".into());
            }
        }
        Ok(points)
    }
    fn line(&mut self, value: &Value) -> Result<Vec<Point>, String> {
        let points = self.path(value, false)?;
        if points.len() == 1 {
            return Err("A LineString needs at least two positions".into());
        }
        Ok(points)
    }
    fn polygon(&mut self, value: &Value) -> Result<Vec<Vec<Point>>, String> {
        array(value)?
            .iter()
            .map(|ring| self.path(ring, true))
            .collect()
    }
    fn point(&mut self, values: &[Value]) -> Result<Point, String> {
        if values.len() < 2 {
            return Err("A GeoJSON position needs longitude and latitude".into());
        }
        self.points += 1;
        if self.points > MAX_POINTS {
            return Err("GeoJSON exceeds the position limit".into());
        }
        let lon = number(&values[0])?;
        let lat = number(&values[1])?;
        if !(-180.0..=180.0).contains(&lon) || !(-90.0..=90.0).contains(&lat) {
            return Err("GeoJSON coordinates must be WGS84 longitude/latitude in degrees".into());
        }
        let elevation = values.get(2).map(number).transpose()?;
        if values.len() > 3 {
            self.notes
                .insert("Position elements beyond elevation are not interpreted.".into());
        }
        Ok(Point {
            lon,
            lat,
            elevation,
            time: String::new(),
        })
    }
    fn push(
        &mut self,
        name: &str,
        kind: Kind,
        paths: Vec<Vec<Point>>,
        properties: Vec<(String, String)>,
    ) -> Result<(), String> {
        self.paths += paths.len();
        if self.features.len() >= 4096 || self.paths > MAX_PATHS {
            return Err("GeoJSON exceeds the 4,096-shape/segment limit".into());
        }
        let mut retained = Vec::new();
        for (key, value) in properties {
            let size = key.len() + value.len();
            if self.property_bytes + size <= 4 * 1024 * 1024 {
                self.property_bytes += size;
                retained.push((key, value));
            } else {
                self.notes.insert(
                    "Additional attributes are omitted after the 4 MiB display budget.".into(),
                );
            }
        }
        self.features.push(RawFeature {
            name: clip(name, 256),
            kind,
            paths,
            properties: retained,
        });
        Ok(())
    }
}
fn number(value: &Value) -> Result<f64, String> {
    value
        .as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(|| "Unreadable GeoJSON position element".into())
}
fn display(value: &Value) -> String {
    match value {
        Value::String(value) => clip(value, 512),
        Value::Array(value) => format!("Array · {} values", value.len()),
        Value::Object(value) => format!("Object · {} fields", value.len()),
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shapes_keep_rings_attributes_null_records_and_supplied_heights() {
        let doc = parse(include_bytes!(
            "../../tests/fixtures/geography/harbor.geojson"
        ))
        .unwrap();
        assert_eq!(doc.title, "Harbor survey");
        assert_eq!(doc.features.len(), 4);
        assert_eq!(doc.points, 15);
        let park = &doc.features[0];
        assert_eq!(park.kind, Kind::Polygon);
        assert_eq!(park.paths.len(), 2);
        assert!(park.distance_m > 5000.0);
        assert!(
            park.properties
                .contains(&("survey".into(), "Object · 1 fields".into()))
        );
        assert_eq!(doc.features[1].elevation, Some([12.0, 28.0]));
        assert_eq!(doc.features[2].distance_m, 0.0);
        assert!(doc.features[3].paths.is_empty());
        assert!(doc.bounds.iter().all(|n| n.is_finite()));
    }
    #[test]
    fn collections_dateline_and_empty_metadata_are_readable() {
        let doc = parse(br#"{"type":"Feature","properties":{"name":"Island"},"geometry":{"type":"GeometryCollection","geometries":[{"type":"LineString","coordinates":[[179,0],[-179,0]]},{"type":"Point","coordinates":[180,1,3,9]}]}}"#).unwrap();
        assert_eq!(doc.features.len(), 2);
        assert!(doc.bounds[2] - doc.bounds[0] < 3.0);
        assert!(doc.notes.iter().any(|s| s.contains("beyond elevation")));
        let empty = parse(br#"{"type":"Feature","properties":{"name":"Pending"},"geometry":null}"#)
            .unwrap();
        assert_eq!(empty.features[0].name, "Pending");
        assert_eq!(empty.points, 0);
        assert!(empty.bounds.iter().all(|n| n.is_finite()));
    }
    #[test]
    fn malformed_positions_rings_crs_and_foreign_types_are_rejected() {
        for source in [
            r#"{"type":"Point","coordinates":[181,0]}"#,
            r#"{"type":"Point","coordinates":[0,91]}"#,
            r#"{"type":"Point","coordinates":["0",0]}"#,
            r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,1]]]}"#,
            r#"{"type":"LineString","coordinates":[[0,0]]}"#,
            r#"{"type":"Point","coordinates":[0,0],"crs":{"type":"name","properties":{"name":"EPSG:3857"}}}"#,
            r#"{"type":"FeatureCollection","features":[{"type":"Point","coordinates":[0,0]}]}"#,
        ] {
            assert!(parse(source.as_bytes()).is_err(), "{source}");
        }
        let mut value = serde_json::json!({"type":"Point","coordinates":[0,0]});
        for _ in 0..34 {
            value = serde_json::json!({"type":"GeometryCollection","geometries":[value]});
        }
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
        let coordinates = vec![serde_json::json!([0, 0]); MAX_POINTS + 1];
        assert!(
            parse(
                &serde_json::to_vec(
                    &serde_json::json!({"type":"MultiPoint","coordinates":coordinates})
                )
                .unwrap()
            )
            .is_err()
        );
    }
}
