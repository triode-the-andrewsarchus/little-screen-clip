//! Bounded container access. ZIP members stay in memory and are never extracted by name.
use super::*;
use crate::notebook::container::{bounded, member};
use std::io::SeekFrom;

const MAX_SOURCE: u64 = 512 * 1024 * 1024;
pub(super) const MAX_XML: usize = 64 * 1024 * 1024;

pub(super) fn xml(
    input: &mut (impl Read + Seek),
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, bool), String> {
    if input
        .seek(SeekFrom::End(0))
        .map_err(|error| error.to_string())?
        > MAX_SOURCE
    {
        return Err("Xournal source exceeds 512 MiB".into());
    }
    input.rewind().map_err(|error| error.to_string())?;
    let mut header = [0; 4];
    input
        .read_exact(&mut header)
        .map_err(|error| error.to_string())?;
    input.rewind().map_err(|error| error.to_string())?;
    if header.starts_with(b"PK\x03\x04") {
        return member(input, "content.xml", MAX_XML, cancel).map(|xml| (xml, true));
    }
    let bytes = if header.starts_with(&[0x1f, 0x8b]) {
        bounded(
            &mut flate2::read::MultiGzDecoder::new(input),
            MAX_XML,
            cancel,
        )?
    } else {
        bounded(input, MAX_XML, cancel)?
    };
    Ok((bytes, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::container::{bounded, member};
    use std::io::{Cursor, Write};

    #[test]
    fn compressed_expansion_and_package_member_budgets_are_enforced() {
        let cancel = AtomicBool::new(false);
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&[b'a'; 8192]).unwrap();
        let gzip = gzip.finish().unwrap();
        assert!(
            bounded(
                &mut flate2::read::MultiGzDecoder::new(&gzip[..]),
                64,
                &cancel
            )
            .is_err()
        );
        assert!(xml(&mut Cursor::new(&gzip[..gzip.len() - 4]), &cancel).is_err());
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        archive.start_file("content.xml", options).unwrap();
        archive
            .write_all(b"<xournal><page width='10' height='20'/></xournal>")
            .unwrap();
        archive.start_file("../outside", options).unwrap();
        archive.write_all(b"not extracted").unwrap();
        let mut source = archive.finish().unwrap();
        assert!(member(&mut source, "content.xml", 1, &cancel).is_err());
        assert!(member(&mut source, "../outside", 1024, &cancel).is_err());
        assert!(member(&mut source, "missing", 1024, &cancel).is_err());
        assert!(member(&mut source, "content.xml", 1024, &cancel).is_ok());
    }
}
