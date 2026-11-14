//! Seekable notebook ZIP/TAR access. Entries are never extracted by their names.
use super::super::check_cancel as cancelled;
use crate::notebook::container::{bounded, zip_member};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    sync::atomic::AtomicBool,
};

pub(super) enum Package<R: Read + Seek> {
    Zip {
        archive: zip::ZipArchive<R>,
        names: BTreeMap<String, String>,
    },
    Tar {
        source: R,
        entries: BTreeMap<String, (u64, u64)>,
    },
}

impl<R: Read + Seek> Package<R> {
    pub fn open(mut source: R, cancel: &AtomicBool) -> Result<Self, String> {
        if source
            .seek(SeekFrom::End(0))
            .map_err(|error| error.to_string())?
            > 512 * 1024 * 1024
        {
            return Err("reMarkable archive exceeds 512 MiB".into());
        }
        source.rewind().map_err(|error| error.to_string())?;
        let mut header = [0; 4];
        source
            .read_exact(&mut header)
            .map_err(|error| error.to_string())?;
        source.rewind().map_err(|error| error.to_string())?;
        if header == *b"PK\x03\x04" {
            Self::zip(source, cancel)
        } else {
            Self::tar(source, cancel)
        }
    }

    fn zip(source: R, cancel: &AtomicBool) -> Result<Self, String> {
        let mut archive = zip::ZipArchive::new(source).map_err(|error| error.to_string())?;
        if archive.len() > 8192 {
            return Err("reMarkable archive exceeds the entry budget".into());
        }
        let mut names = BTreeMap::new();
        for index in 0..archive.len() {
            cancelled(cancel)?;
            let entry = archive.by_index(index).map_err(|error| error.to_string())?;
            if entry.is_dir() {
                continue;
            }
            if names
                .insert(canonical(entry.name())?, entry.name().to_owned())
                .is_some()
            {
                return Err("Duplicate reMarkable archive member".into());
            }
        }
        Ok(Self::Zip { archive, names })
    }

    fn tar(mut source: R, cancel: &AtomicBool) -> Result<Self, String> {
        let size = source
            .seek(SeekFrom::End(0))
            .map_err(|error| error.to_string())?;
        source.rewind().map_err(|error| error.to_string())?;
        let mut entries = BTreeMap::new();
        {
            let mut archive = tar::Archive::new(&mut source);
            for (index, entry) in archive
                .entries_with_seek()
                .map_err(|error| error.to_string())?
                .enumerate()
            {
                cancelled(cancel)?;
                if index >= 8192 {
                    return Err("reMarkable archive exceeds the entry budget".into());
                }
                let entry = entry.map_err(|error| error.to_string())?;
                if let Some((name, offset, length)) = tar_member(&entry, size)?
                    && entries.insert(name, (offset, length)).is_some()
                {
                    return Err("Duplicate reMarkable archive member".into());
                }
            }
        }
        Ok(Self::Tar { source, entries })
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        let names: Box<dyn Iterator<Item = &String> + '_> = match self {
            Self::Zip { names, .. } => Box::new(names.keys()),
            Self::Tar { entries, .. } => Box::new(entries.keys()),
        };
        names.map(String::as_str)
    }

    pub fn read(
        &mut self,
        name: &str,
        limit: usize,
        cancel: &AtomicBool,
    ) -> Result<Option<Vec<u8>>, String> {
        cancelled(cancel)?;
        match self {
            Self::Zip { archive, names } => names
                .get(name)
                .map(|original| zip_member(archive, original, limit, cancel))
                .transpose(),
            Self::Tar { source, entries } => {
                let Some(&(offset, size)) = entries.get(name) else {
                    return Ok(None);
                };
                if size > limit as u64 {
                    return Err("Notebook package member exceeds its read budget".into());
                }
                source
                    .seek(SeekFrom::Start(offset))
                    .map_err(|error| error.to_string())?;
                let bytes = bounded(&mut source.take(size), limit, cancel)?;
                if bytes.len() as u64 != size {
                    return Err("Truncated notebook package member".into());
                }
                Ok(Some(bytes))
            }
        }
    }
}

fn tar_member<R: Read>(
    entry: &tar::Entry<'_, R>,
    size: u64,
) -> Result<Option<(String, u64, u64)>, String> {
    if entry.header().entry_type().is_dir() {
        return Ok(None);
    }
    if !entry.header().entry_type().is_file() {
        return Err("reMarkable archive links/special members are unsupported".into());
    }
    let name =
        canonical(std::str::from_utf8(&entry.path_bytes()).map_err(|_| "Non-UTF8 archive name")?)?;
    let offset = entry.raw_file_position();
    let length = entry.size();
    if offset.checked_add(length).is_none_or(|end| end > size) {
        return Err("Truncated reMarkable archive member".into());
    }
    Ok(Some((name, offset, length)))
}

fn canonical(name: &str) -> Result<String, String> {
    let path = std::path::Path::new(name);
    if name.len() > 4096
        || path.is_absolute()
        || path.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err("Unsafe reMarkable archive member path".into());
    }
    Ok(path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    #[test]
    fn indexed_tar_entries_remain_bounded_and_links_are_rejected() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "./page.rm", &b"hello"[..])
            .unwrap();
        let bytes = builder.into_inner().unwrap();
        let cancel = AtomicBool::new(false);
        let mut package = Package::open(Cursor::new(&bytes), &cancel).unwrap();
        assert_eq!(package.names().collect::<Vec<_>>(), ["page.rm"]);
        assert_eq!(
            package.read("page.rm", 8, &cancel).unwrap().unwrap(),
            b"hello"
        );
        assert!(package.read("page.rm", 4, &cancel).is_err());
        assert!(package.read("missing", 8, &cancel).unwrap().is_none());
        let mut builder = tar::Builder::new(Vec::new());
        let mut link = tar::Header::new_gnu();
        link.set_size(0);
        link.set_mode(0o644);
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_link_name("/etc/passwd").unwrap();
        link.set_cksum();
        builder.append_data(&mut link, "page.rm", &b""[..]).unwrap();
        assert!(Package::open(Cursor::new(builder.into_inner().unwrap()), &cancel).is_err());
        assert!(canonical("../outside").is_err());
        assert!(canonical("/outside").is_err());
        assert!(Package::open(Cursor::new(bytes), &AtomicBool::new(true)).is_err());
    }

    #[test]
    fn zip_members_reuse_common_bounded_reader() {
        let mut builder = zip::ZipWriter::new(Cursor::new(Vec::new()));
        builder
            .start_file("./page.rm", zip::write::SimpleFileOptions::default())
            .unwrap();
        builder.write_all(b"hello").unwrap();
        let mut package =
            Package::open(builder.finish().unwrap(), &AtomicBool::new(false)).unwrap();
        assert_eq!(
            package
                .read("page.rm", 5, &AtomicBool::new(false))
                .unwrap()
                .unwrap(),
            b"hello"
        );
        assert!(package.read("page.rm", 4, &AtomicBool::new(false)).is_err());
    }
}
