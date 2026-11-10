//! Guarded, streaming, no-overwrite publication for the built-in save node.

use super::types::PortValue;
use little_clip_core::file_source::{FileSource, open_regular_file};
use little_clip_core::kernel_interface::create_content_directory;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

pub(super) fn save(
    value: &PortValue,
    destination: &Path,
    completed: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let directory = create_content_directory(destination)?;
    match value {
        PortValue::Text(text) => {
            let name = format!("clip_{}.txt", uuid::Uuid::new_v4());
            write_file(&directory, OsStr::new(&name), completed, |file| {
                file.write_all(text.as_bytes())
            })
        }
        PortValue::FilePath(path) | PortValue::ImagePath(path) | PortValue::AudioPath(path) => {
            copy_file(&directory, path, completed)
        }
        PortValue::FileBatch(paths) => {
            if paths.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "No files to save",
                ));
            }
            for path in paths {
                copy_file(&directory, path, completed)?;
            }
            Ok(())
        }
    }
}

fn copy_file(directory: &File, path: &Path, completed: &mut Vec<PathBuf>) -> io::Result<()> {
    let result = (|| {
        let mut input = open_regular_file(path, true)?;
        let observed = FileSource::capture_file(&input).map_err(io::Error::other)?;
        let metadata = input.metadata()?;
        let filename = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing input filename"))?;
        write_file(directory, filename, completed, |output| {
            // Bound the read to the observed regular-file size, even if another
            // writer keeps extending the input. Publication requires that revision.
            let copied = io::copy(&mut (&mut input).take(metadata.len()), output)?;
            if copied != metadata.len()
                || FileSource::capture_file(&input).map_err(io::Error::other)? != observed
            {
                return Err(io::Error::other("Pipeline input changed while saving"));
            }
            output.set_permissions(metadata.permissions())
        })
    })();
    result.map_err(|error: io::Error| {
        io::Error::new(
            error.kind(),
            format!("Cannot save {}: {error}", path.display()),
        )
    })
}

fn write_file(
    directory: &File,
    filename: &OsStr,
    completed: &mut Vec<PathBuf>,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let anchor = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let mut staged = tempfile::NamedTempFile::new_in(&anchor)?;
    write(staged.as_file_mut())?;
    staged.as_file().sync_all()?;
    let expected = std::fs::read_link(&anchor)?.join(filename);
    let installed = staged.persist_noclobber(anchor.join(filename))?;
    // Record installation even if the subsequent observation fails. Accepted
    // writes must not disappear from a partial-execution error.
    match FileSource::capture_file(&installed) {
        Ok(observed) => {
            completed.push(observed.path().to_owned());
            Ok(())
        }
        Err(error) => {
            completed.push(expected);
            Err(io::Error::other(error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn saves_text_and_lossless_file_names_through_new_output_directories() {
        use std::os::unix::{ffi::OsStringExt, fs::PermissionsExt};
        let fixture = tempfile::tempdir().unwrap();
        let output = fixture.path().join("new/nested/output");
        let input = fixture
            .path()
            .join(std::ffi::OsString::from_vec(b"input_\xff.bin".to_vec()));
        fs::write(&input, b"file bytes").unwrap();
        fs::set_permissions(&input, fs::Permissions::from_mode(0o751)).unwrap();
        let mut completed = Vec::new();
        save(
            &PortValue::Text("text bytes".into()),
            &output,
            &mut completed,
        )
        .unwrap();
        save(&PortValue::FilePath(input.clone()), &output, &mut completed).unwrap();
        assert_eq!(completed.len(), 2);
        assert_eq!(fs::read(&completed[0]).unwrap(), b"text bytes");
        assert_eq!(completed[1], output.join(input.file_name().unwrap()));
        assert_eq!(fs::read(&completed[1]).unwrap(), b"file bytes");
        assert_eq!(
            fs::metadata(&completed[1]).unwrap().permissions().mode() & 0o777,
            0o751
        );
        assert_eq!(fs::read_dir(output).unwrap().count(), 2);
    }

    #[test]
    fn publication_follows_held_directory_after_its_path_is_replaced() {
        let fixture = tempfile::tempdir().unwrap();
        let original = fixture.path().join("output");
        let moved = fixture.path().join("moved");
        let directory = create_content_directory(&original).unwrap();
        let mut completed = Vec::new();
        write_file(
            &directory,
            OsStr::new("saved.txt"),
            &mut completed,
            |file| {
                file.write_all(b"accepted bytes")?;
                fs::rename(&original, &moved)?;
                fs::create_dir(&original)?;
                fs::write(original.join("saved.txt"), b"replacement bytes")
            },
        )
        .unwrap();
        assert_eq!(completed, [moved.join("saved.txt")]);
        assert_eq!(fs::read(&completed[0]).unwrap(), b"accepted bytes");
        assert_eq!(
            fs::read(original.join("saved.txt")).unwrap(),
            b"replacement bytes"
        );
        assert_eq!(fs::read_dir(moved).unwrap().count(), 1);
    }

    #[test]
    fn failed_staging_is_removed_without_installing_a_partial_file() {
        let fixture = tempfile::tempdir().unwrap();
        let directory = create_content_directory(fixture.path()).unwrap();
        let mut completed = Vec::new();
        let result = write_file(
            &directory,
            OsStr::new("saved.txt"),
            &mut completed,
            |file| {
                file.write_all(b"partial")?;
                Err(io::Error::other("writer failed"))
            },
        );
        assert!(result.is_err());
        assert!(completed.is_empty());
        assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 0);
    }

    #[test]
    fn protected_inputs_and_occupied_symlink_destinations_remain_untouched() {
        let fixture = tempfile::tempdir().unwrap();
        let output = fixture.path().join("output");
        let input = fixture.path().join("input.bin");
        std::os::unix::fs::symlink("/dev/null", &input).unwrap();
        let mut completed = Vec::new();
        assert!(save(&PortValue::FilePath(input.clone()), &output, &mut completed).is_err());
        assert_eq!(fs::read_dir(&output).unwrap().count(), 0);
        fs::remove_file(&input).unwrap();
        fs::write(&input, b"new bytes").unwrap();
        let existing = fixture.path().join("existing.bin");
        fs::write(&existing, b"existing bytes").unwrap();
        std::os::unix::fs::symlink(&existing, output.join("input.bin")).unwrap();
        assert!(save(&PortValue::FilePath(input), &output, &mut completed).is_err());
        assert!(completed.is_empty());
        assert_eq!(fs::read(existing).unwrap(), b"existing bytes");
        assert!(
            fs::symlink_metadata(output.join("input.bin"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_dir(output).unwrap().count(), 1);
    }
}
