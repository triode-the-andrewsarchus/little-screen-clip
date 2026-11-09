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
pub(super) fn serialize<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    encode(path).serialize(serializer)
}
pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
    Decoded::deserialize(deserializer).map(decode)
}
pub(super) mod optional {
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
pub(super) mod list {
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
