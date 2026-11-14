//! Streaming compressed TAR inventories with separate encoded/decoded budgets.
use super::{ARCHIVE_SIZE, Inventory, LIMIT, Members};
use std::{
    cell::Cell,
    fs::File,
    io::{self, Read},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const DECODED: u64 = 64 * 1024 * 1024;
const MEMORY: u64 = 64 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Format {
    Gzip,
    Bzip2,
    Xz,
    Zstd,
}
pub(super) fn format(path: &Path) -> Option<Format> {
    // Classification only: the original lossless path is used for all reads.
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    [
        (Format::Gzip, &[".tar.gz", ".tgz"][..]),
        (Format::Bzip2, &[".tar.bz2", ".tbz2", ".tbz"][..]),
        (Format::Xz, &[".tar.xz", ".txz"][..]),
        (Format::Zstd, &[".tar.zst", ".tzst"][..]),
    ]
    .into_iter()
    .find_map(|(format, suffixes)| {
        suffixes
            .iter()
            .any(|suffix| name.ends_with(suffix))
            .then_some(format)
    })
}
struct Limits<'a> {
    cancel: &'a AtomicBool,
    deadline: Instant,
    exceeded: Cell<Option<&'static str>>,
}
struct Bounded<'a, R> {
    inner: R,
    limits: &'a Limits<'a>,
    remaining: u64,
    label: &'static str,
}
impl<R: Read> Read for Bounded<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.limits.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other("Container preview cancelled"));
        }
        if Instant::now() >= self.limits.deadline {
            let reason = "Compressed archive preview reached its five-second budget";
            self.limits.exceeded.set(Some(reason));
            return Err(io::Error::new(io::ErrorKind::TimedOut, reason));
        }
        if self.remaining == 0 {
            // Distinguish an exact-budget EOF from additional decoded payload.
            if self.inner.read(&mut [0])? == 0 {
                return Ok(0);
            }
            self.limits.exceeded.set(Some(self.label));
            return Err(io::Error::other(self.label));
        }
        let limit = bytes.len().min(64 * 1024).min(self.remaining as usize);
        let read = self.inner.read(&mut bytes[..limit])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}
pub(super) fn load_with(
    file: &mut File,
    cancel: &AtomicBool,
    format: Format,
    members: Members,
) -> Result<Inventory, String> {
    let limits = Limits {
        cancel,
        deadline: Instant::now() + Duration::from_secs(5),
        exceeded: Cell::new(None),
    };
    let input = Bounded {
        inner: file,
        limits: &limits,
        remaining: ARCHIVE_SIZE,
        label: "512 MiB compressed source preview limit",
    };
    let decoder: Box<dyn Read + '_> = match format {
        Format::Gzip => Box::new(flate2::read::MultiGzDecoder::new(input)),
        Format::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(input)),
        Format::Xz => {
            let stream = xz2::stream::Stream::new_stream_decoder(MEMORY, xz2::stream::CONCATENATED)
                .map_err(|error| error.to_string())?;
            Box::new(xz2::read::XzDecoder::new_stream(input, stream))
        }
        Format::Zstd => {
            let mut decoder =
                zstd::stream::read::Decoder::new(input).map_err(|error| error.to_string())?;
            decoder
                .window_log_max(26)
                .map_err(|error| error.to_string())?;
            Box::new(decoder)
        }
    };
    inventory_with(decoder, &limits, DECODED, members)
}
#[cfg(test)]
fn inventory(input: impl Read, limits: &Limits<'_>, limit: u64) -> Result<Inventory, String> {
    inventory_with(input, limits, limit, Members::default())
}
fn inventory_with(
    input: impl Read,
    limits: &Limits<'_>,
    limit: u64,
    mut members: Members,
) -> Result<Inventory, String> {
    let mut input = Bounded {
        inner: input,
        limits,
        remaining: limit,
        label: "64 MiB decoded preview limit; remaining payloads were not verified",
    };
    let result = collect(&mut input, &mut members, limit, limits.cancel);
    match result {
        Ok(complete) => Ok(members.finish(complete)),
        Err(_) if limits.exceeded.get().is_some() => {
            members.notice = limits.exceeded.get().map(str::to_owned);
            Ok(members.finish(false))
        }
        Err(error) => Err(error),
    }
}
fn collect(
    input: &mut impl Read,
    members: &mut Members,
    limit: u64,
    cancel: &AtomicBool,
) -> Result<bool, String> {
    {
        let mut archive = tar::Archive::new(&mut *input);
        // Inspect concatenated TAR streams as well as their compressed members.
        archive.set_ignore_zeros(true);
        for (index, member) in archive
            .entries()
            .map_err(|error| error.to_string())?
            .enumerate()
        {
            let member = member.map_err(|error| error.to_string())?;
            let beyond_budget = member
                .raw_file_position()
                .checked_add(member.size())
                .is_none_or(|end| end > limit);
            members.add_tar(member, cancel)?;
            if beyond_budget || index >= members.display_limit.unwrap_or(LIMIT) {
                members.notice = Some(
                    if beyond_budget {
                        "64 MiB decoded preview limit; remaining payloads were not verified"
                    } else {
                        "Member count preview limit; remaining payloads were not verified"
                    }
                    .into(),
                );
                return Ok(false);
            }
        }
    }
    // Require decoder EOF/checksum validation before claiming a complete scan.
    io::copy(input, &mut io::sink()).map_err(|error| error.to_string())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tar_bytes(name: &str, size: usize) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(size as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, name, vec![42; size].as_slice())
            .unwrap();
        builder.into_inner().unwrap()
    }
    fn encode(format: Format, bytes: &[u8]) -> Vec<u8> {
        let mut encoder: Box<dyn Read + '_> = match format {
            Format::Gzip => Box::new(flate2::read::GzEncoder::new(
                bytes,
                flate2::Compression::fast(),
            )),
            Format::Bzip2 => Box::new(bzip2::read::BzEncoder::new(
                bytes,
                bzip2::Compression::fast(),
            )),
            Format::Xz => Box::new(xz2::read::XzEncoder::new(bytes, 1)),
            Format::Zstd => Box::new(zstd::stream::read::Encoder::new(bytes, 1).unwrap()),
        };
        let mut output = Vec::new();
        encoder.read_to_end(&mut output).unwrap();
        output
    }
    fn inventory_file(path: &Path) -> Result<Inventory, String> {
        let directory = crate::file_manager::filesystem::read_directory(
            path.parent().unwrap(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let entry = directory
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .unwrap();
        super::super::load(entry, &AtomicBool::new(false))
    }
    #[test]
    fn common_compressed_tar_formats_preserve_member_origin_without_extracting() {
        let directory = tempfile::tempdir().unwrap();
        let raw = tar_bytes("nested/file.txt", 4);
        for (kind, suffix) in [
            (Format::Gzip, "TAR.GZ"),
            (Format::Gzip, "tgz"),
            (Format::Bzip2, "tar.bz2"),
            (Format::Bzip2, "tbz2"),
            (Format::Xz, "txz"),
            (Format::Zstd, "tar.zst"),
        ] {
            let path = directory.path().join(format!("example.{suffix}"));
            let encoded = encode(kind, &raw);
            std::fs::write(&path, &encoded).unwrap();
            assert_eq!(format(&path), Some(kind));
            let ready = inventory_file(&path).unwrap();
            assert!(ready.archive && ready.complete);
            assert_eq!(ready.children[0].name(), "nested/file.txt");
            assert!(ready.notice.is_none());
            let source = super::super::FileSource::capture(&path).unwrap();
            let selected =
                super::super::read_archive(&source, Some(0), &AtomicBool::new(false)).unwrap();
            assert!(selected.verified);
            assert_eq!(selected.payload.as_deref(), Some([42; 4].as_slice()));
            assert_eq!(std::fs::read(&path).unwrap(), encoded);
            assert!(!directory.path().join("nested").exists());
        }
        use std::os::unix::ffi::OsStringExt;
        let path = directory
            .path()
            .join(std::ffi::OsString::from_vec(b"odd-\xff.tgz".to_vec()));
        std::fs::write(&path, encode(Format::Gzip, &raw)).unwrap();
        assert_eq!(
            inventory_file(&path).unwrap().children[0].name(),
            "nested/file.txt"
        );
        for name in ["notes.txt.gz", "disk.iso", "generic.zst", "fake.tar.gz.txt"] {
            assert!(format(Path::new(name)).is_none());
        }
    }
    #[test]
    fn complete_scans_validate_compressed_trailers_and_concatenated_members() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("collection.tgz");
        let mut bytes = encode(Format::Gzip, &tar_bytes("one.txt", 3));
        bytes.extend(encode(Format::Gzip, &tar_bytes("two.txt", 4)));
        std::fs::write(&path, &bytes).unwrap();
        let ready = inventory_file(&path).unwrap();
        assert!(ready.complete);
        assert_eq!(
            ready
                .children
                .iter()
                .map(|child| child.name())
                .collect::<Vec<_>>(),
            ["one.txt", "two.txt"]
        );
        let checksum = bytes.len() - 8;
        bytes[checksum] ^= 0x80;
        std::fs::write(&path, &bytes).unwrap();
        assert!(inventory_file(&path).is_err());
        bytes.truncate(bytes.len() - 4);
        std::fs::write(&path, &bytes).unwrap();
        assert!(inventory_file(&path).is_err());
    }
    #[test]
    fn partial_inventory_discloses_decoded_budget_and_exact_budget_eof_is_complete() {
        let raw = tar_bytes("file.txt", 2048);
        let cancel = AtomicBool::new(false);
        let limits = Limits {
            cancel: &cancel,
            deadline: Instant::now() + Duration::from_secs(5),
            exceeded: Cell::new(None),
        };
        let ready = inventory(raw.as_slice(), &limits, 1024).unwrap();
        assert!(!ready.complete);
        assert_eq!(ready.children[0].name(), "file.txt");
        assert!(ready.notice.unwrap().contains("decoded preview limit"));
        let ready = inventory(raw.as_slice(), &limits, raw.len() as u64).unwrap();
        assert!(ready.complete);
        assert!(ready.notice.is_none());
    }
    #[test]
    fn compressed_member_paths_and_duplicates_use_the_existing_tar_policy() {
        let directory = tempfile::tempdir().unwrap();
        let mut raw = tar_bytes("safe.txt", 4);
        let header = tar::Header::from_byte_slice(&raw[..512]);
        let mut changed = header.clone();
        changed.as_mut_bytes()[..100].fill(0);
        changed.as_mut_bytes()[..9].copy_from_slice(b"../escape");
        changed.set_cksum();
        raw[..512].copy_from_slice(changed.as_bytes());
        let path = directory.path().join("unsafe.tar.xz");
        std::fs::write(&path, encode(Format::Xz, &raw)).unwrap();
        assert!(
            inventory_file(&path)
                .unwrap_err()
                .contains("Unsafe archive member path")
        );
        let path = directory.path().join("duplicates.tgz");
        let raw = tar_bytes("same.txt", 4);
        let mut bytes = encode(Format::Gzip, &raw);
        bytes.extend(encode(Format::Gzip, &raw));
        std::fs::write(&path, bytes).unwrap();
        assert!(
            inventory_file(&path)
                .unwrap_err()
                .contains("Duplicate archive member path")
        );
    }
    #[test]
    fn member_card_budget_does_not_decode_the_rest_of_a_large_inventory() {
        let mut builder = tar::Builder::new(Vec::new());
        for index in 0..40 {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("item-{index:02}.txt"), io::empty())
                .unwrap();
        }
        let bytes = builder.into_inner().unwrap();
        let cancel = AtomicBool::new(false);
        let limits = Limits {
            cancel: &cancel,
            deadline: Instant::now() + Duration::from_secs(5),
            exceeded: Cell::new(None),
        };
        let ready = inventory(bytes.as_slice(), &limits, DECODED).unwrap();
        assert!(!ready.complete);
        assert_eq!(ready.observed, LIMIT + 1);
        assert_eq!(ready.children.len(), LIMIT);
        assert!(ready.notice.unwrap().contains("Member count"));
    }

    #[test]
    fn cancellation_and_deadlines_do_not_touch_the_underlying_reader() {
        struct Unreadable;
        impl Read for Unreadable {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("reader must not be touched")
            }
        }
        for cancelled in [true, false] {
            let cancel = AtomicBool::new(cancelled);
            let limits = Limits {
                cancel: &cancel,
                deadline: Instant::now() - Duration::from_secs(1),
                exceeded: Cell::new(None),
            };
            let mut input = Bounded {
                inner: Unreadable,
                limits: &limits,
                remaining: 1024,
                label: "test budget",
            };
            assert!(input.read(&mut [0]).is_err());
            assert_eq!(limits.exceeded.get().is_some(), !cancelled);
        }
    }
}
