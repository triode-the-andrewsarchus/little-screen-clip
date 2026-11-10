//! Helper responses preserve Unix path bytes while keeping UTF-8 paths readable.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};

#[derive(Serialize)]
#[serde(untagged)]
enum Encoded<'a> {
    Text(&'a str),
    Bytes(&'a [u8]),
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Decoded {
    Text(String),
    Bytes(Vec<u8>),
}
fn encode(path: &Path) -> Encoded<'_> {
    path.to_str().map_or_else(
        || Encoded::Bytes(path.as_os_str().as_bytes()),
        Encoded::Text,
    )
}
fn decode(path: Decoded) -> PathBuf {
    match path {
        Decoded::Text(text) => text.into(),
        Decoded::Bytes(bytes) => std::ffi::OsString::from_vec(bytes).into(),
    }
}
/// Serializes a path as Unicode text or its original Unix bytes.
pub fn serialize<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    encode(path).serialize(serializer)
}
/// Deserializes either the text or byte representation without lossy conversion.
pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
    Decoded::deserialize(deserializer).map(decode)
}
/// Serde adapter for `Option<PathBuf>` with the same lossless representation.
pub mod optional {
    use super::*;
    pub fn serialize<S: Serializer>(
        path: &Option<PathBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        path.as_ref().map(|path| encode(path)).serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PathBuf>, D::Error> {
        Option::<Decoded>::deserialize(deserializer).map(|path| path.map(decode))
    }
}
/// Serde adapter for path lists, preserving each path independently.
pub mod list {
    use super::*;
    pub fn serialize<S: Serializer>(paths: &[PathBuf], serializer: S) -> Result<S::Ok, S::Error> {
        paths
            .iter()
            .map(|path| encode(path))
            .collect::<Vec<_>>()
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<PathBuf>, D::Error> {
        Vec::<Decoded>::deserialize(deserializer)
            .map(|paths| paths.into_iter().map(decode).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Paths {
        #[serde(with = "super")]
        path: PathBuf,
        #[serde(with = "super::optional")]
        optional: Option<PathBuf>,
        #[serde(with = "super::list")]
        list: Vec<PathBuf>,
    }

    #[test]
    fn wire_format_keeps_unicode_as_text_and_invalid_names_as_original_bytes() {
        let text = PathBuf::from("/data/照片\\line\n");
        let raw = PathBuf::from(std::ffi::OsString::from_vec(b"/data/\xff".to_vec()));
        let value = Paths {
            path: raw.clone(),
            optional: Some(text.clone()),
            list: vec![text.clone(), raw.clone()],
        };
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(
            json["path"],
            serde_json::json!([47, 100, 97, 116, 97, 47, 255])
        );
        assert_eq!(json["optional"], serde_json::json!(text));
        assert_eq!(json["list"][0], json["optional"]);
        assert_eq!(json["list"][1], json["path"]);
        assert_eq!(serde_json::from_value::<Paths>(json).unwrap(), value);
        let none = Paths {
            path: text,
            optional: None,
            list: vec![],
        };
        let json = serde_json::to_value(&none).unwrap();
        assert!(json["optional"].is_null());
        assert_eq!(serde_json::from_value::<Paths>(json).unwrap(), none);
    }
}
