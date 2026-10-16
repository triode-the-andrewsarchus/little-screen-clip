//! Shared Linux kernel-interface routing, before ordinary content readers run.
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    },
    path::Path,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Kind {
    Content,
    Character { major: u32, minor: u32 },
    Block { major: u32, minor: u32 },
    Kernel { filesystem: Filesystem },
    Special,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Filesystem {
    DeviceNodes,
    Devpts,
    Sysfs,
    Proc,
    Cgroup,
    Trace,
    Debug,
    Efi,
    Security,
    Config,
    Bpf,
    Pstore,
    ResourceControl,
    Binder,
    FuseControl,
    Namespace,
}
impl Kind {
    pub fn protected(self) -> bool {
        !matches!(self, Self::Content)
    }
    pub fn semantic(self) -> bool {
        matches!(
            self,
            Self::Character { .. } | Self::Block { .. } | Self::Kernel { .. }
        )
    }
}

/// Pins a pathname without invoking a device driver's open callback.
pub fn pin(path: &Path, follow: bool) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_CLOEXEC | if follow { 0 } else { libc::O_NOFOLLOW })
        .open(path)
}
pub fn classify(path: &Path) -> io::Result<Kind> {
    classify_handle(&pin(path, true)?)
}
pub fn classify_handle(file: &File) -> io::Result<Kind> {
    let metadata = file.metadata()?;
    let kind = metadata.file_type();
    if kind.is_char_device() {
        return Ok(Kind::Character {
            major: libc::major(metadata.rdev()),
            minor: libc::minor(metadata.rdev()),
        });
    }
    if kind.is_block_device() {
        return Ok(Kind::Block {
            major: libc::major(metadata.rdev()),
            minor: libc::minor(metadata.rdev()),
        });
    }
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: file owns a live descriptor and stats points to writable statfs storage.
    if unsafe { libc::fstatfs(file.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fstatfs initialized the structure.
    let magic = unsafe { stats.assume_init() }.f_type as u64;
    let filesystem = filesystem_for_magic(
        magic,
        magic == 0x1021994 && metadata.is_dir() && device_namespace_directory(&metadata),
    );
    if let Some(filesystem) = filesystem {
        return Ok(Kind::Kernel { filesystem });
    }
    Ok(
        if metadata.is_file() || metadata.is_dir() || kind.is_symlink() {
            Kind::Content
        } else {
            Kind::Special
        },
    )
}
/// Pure filesystem policy lookup; metadata pinning supplies the observed magic.
fn filesystem_for_magic(magic: u64, device_directory: bool) -> Option<Filesystem> {
    // Kernel magic constants are 32-bit, including signed f_type on 32-bit targets.
    match magic as u32 {
        0x1cd1 => Some(Filesystem::Devpts),
        0x1021994 if device_directory => Some(Filesystem::DeviceNodes),
        0x62656572 => Some(Filesystem::Sysfs),
        0x9fa0 => Some(Filesystem::Proc),
        0x27e0eb | 0x63677270 => Some(Filesystem::Cgroup),
        0x74726163 => Some(Filesystem::Trace),
        0x64626720 => Some(Filesystem::Debug),
        0xde5e81e4 => Some(Filesystem::Efi),
        0x62656570 => Some(Filesystem::Config),
        0xcafe4a11 => Some(Filesystem::Bpf),
        0x6165676c => Some(Filesystem::Pstore),
        0x07655821 => Some(Filesystem::ResourceControl),
        0x6c6f6f70 => Some(Filesystem::Binder),
        0x65735543 => Some(Filesystem::FuseControl),
        0x6e736673 => Some(Filesystem::Namespace),
        0x73636673 | 0xf97cff8c | 0x43415d53 | 0x5a3c69f0 => Some(Filesystem::Security),
        _ => None,
    }
}
fn device_namespace_directory(metadata: &std::fs::Metadata) -> bool {
    let Ok(root) = std::fs::metadata("/dev") else {
        return false;
    };
    if root.dev() != metadata.dev() {
        return false;
    }
    for directory in ["/dev/bus/usb", "/dev/cpu"] {
        if std::fs::read_dir(directory).is_ok_and(|entries| {
            entries.take(256).filter_map(Result::ok).any(|entry| {
                entry.metadata().is_ok_and(|candidate| {
                    candidate.dev() == metadata.dev()
                        && candidate.ino() == metadata.ino()
                        && candidate.is_dir()
                })
            })
        }) {
            return true;
        }
    }
    [
        "/dev",
        "/dev/pts",
        "/dev/dri",
        "/dev/dri/by-path",
        "/dev/input",
        "/dev/input/by-id",
        "/dev/input/by-path",
        "/dev/snd",
        "/dev/snd/by-id",
        "/dev/snd/by-path",
        "/dev/disk",
        "/dev/disk/by-id",
        "/dev/disk/by-uuid",
        "/dev/disk/by-label",
        "/dev/disk/by-path",
        "/dev/disk/by-partuuid",
        "/dev/disk/by-partlabel",
        "/dev/serial",
        "/dev/serial/by-id",
        "/dev/serial/by-path",
        "/dev/mapper",
        "/dev/bus",
        "/dev/bus/usb",
        "/dev/vfio",
        "/dev/cpu",
    ]
    .iter()
    .any(|path| {
        std::fs::metadata(path).is_ok_and(|candidate| {
            candidate.dev() == metadata.dev() && candidate.ino() == metadata.ino()
        })
    })
}

pub fn require_content(path: &Path) -> io::Result<()> {
    let kind = match classify(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            classify_handle(&pin(path, false)?)?
        }
        result => result?,
    };
    if kind.protected() {
        Err(protected_error())
    } else {
        Ok(())
    }
}
fn protected_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "Kernel interfaces are inspection-only; ordinary content access is disabled",
    )
}

/// Pins an ordinary directory for operations relative to its validated handle.
///
/// The caller must retain the returned handle throughout staging and installation.
/// Resolving the original pathname again would discard the pin's identity guarantee.
///
/// # Errors
/// Returns an error when metadata cannot be inspected, the directory belongs to a
/// protected kernel filesystem, or the target is not a directory.
pub fn pin_content_directory(path: &Path) -> io::Result<File> {
    let directory = pin(path, true)?;
    if classify_handle(&directory)?.protected() {
        return Err(protected_error());
    }
    if !directory.metadata()?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "Not an ordinary directory",
        ));
    }
    Ok(directory)
}

/// Creates missing ordinary directory components and returns a validated handle.
///
/// Starts at the closest existing ordinary directory, then uses a held parent
/// for each child lookup and creation. Replacing an earlier pathname cannot
/// redirect later creations. Existing aliases are allowed when their pinned
/// targets are ordinary directories, including data mounts such as `/dev/shm`.
/// The caller must retain the returned handle for subsequent output operations.
///
/// A failure can leave ordinary directories created by preceding components.
///
/// # Examples
/// ```
/// use little_clip_core::kernel_interface::create_content_directory;
/// let root = tempfile::tempdir()?;
/// let directory = create_content_directory(&root.path().join("exports/audio"))?;
/// assert!(directory.metadata()?.is_dir());
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
/// Returns an error for protected or non-directory anchors or child components,
/// unavailable metadata, or directory creation failures.
pub fn create_content_directory(path: &Path) -> io::Result<File> {
    let mut ancestor = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    loop {
        match pin_content_directory(ancestor) {
            Ok(directory) => {
                let remaining = if ancestor == Path::new(".") && !path.is_absolute() {
                    path
                } else {
                    path.strip_prefix(ancestor)
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?
                };
                return create_content_directory_in(directory, remaining);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && ancestor != Path::new(".") => {
                let parent = ancestor.parent().ok_or(error)?;
                ancestor = if parent.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    parent
                };
            }
            Err(error) => return Err(error),
        }
    }
}

fn create_content_directory_in(mut directory: File, path: &Path) -> io::Result<File> {
    use std::path::Component;
    for component in path.components() {
        let name = match component {
            Component::Normal(name) => name,
            Component::ParentDir => std::ffi::OsStr::new(".."),
            Component::CurDir | Component::RootDir => continue,
            Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Unsupported path prefix",
                ));
            }
        };
        let child = Path::new(&format!("/proc/self/fd/{}", directory.as_raw_fd())).join(name);
        directory = match pin_content_directory(&child) {
            Ok(child) => child,
            Err(error) if error.kind() == io::ErrorKind::NotFound && name != ".." => {
                match std::fs::create_dir(&child) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
                pin_content_directory(&child)?
            }
            Err(error) => return Err(error),
        };
    }
    Ok(directory)
}

/// Reopens an already pinned ordinary file, eliminating pathname replacement races.
pub fn open_content(path: &Path, follow: bool) -> io::Result<File> {
    let pinned = pin(path, follow)?;
    if classify_handle(&pinned)?.protected() || !pinned.metadata()?.is_file() {
        return Err(protected_error());
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(format!("/proc/self/fd/{}", pinned.as_raw_fd()))?;
    if classify_handle(&file)?.protected() || !file.metadata()?.is_file() {
        return Err(protected_error());
    }
    Ok(file)
}
pub fn read_content(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_content(path, true)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #[test]
    fn directory_creation_keeps_shared_memory_as_ordinary_output() {
        use super::*;
        let shared = Path::new("/dev/shm");
        if !matches!(classify(shared), Ok(Kind::Content)) {
            return;
        }
        let root = tempfile::tempdir_in(shared).unwrap();
        let existing = create_content_directory(root.path()).unwrap();
        assert_eq!(
            existing.metadata().unwrap().ino(),
            root.path().metadata().unwrap().ino()
        );
        let child = root.path().join("new/nested");
        let created = create_content_directory(&child).unwrap();
        assert_eq!(classify_handle(&created).unwrap(), Kind::Content);
        assert!(child.is_dir());
    }

    #[test]
    fn directory_creation_keeps_held_parents_and_rejects_protected_aliases() {
        use super::*;
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("parent");
        std::fs::create_dir(&parent).unwrap();
        let held = pin_content_directory(&parent).unwrap();
        let moved = root.path().join("moved");
        std::fs::rename(&parent, &moved).unwrap();
        let redirected = root.path().join("redirected");
        std::fs::create_dir(&redirected).unwrap();
        symlink(&redirected, &parent).unwrap();
        let created = create_content_directory_in(held, Path::new("child/grandchild")).unwrap();
        assert_eq!(
            created.metadata().unwrap().ino(),
            std::fs::metadata(moved.join("child/grandchild"))
                .unwrap()
                .ino()
        );
        assert_eq!(std::fs::read_dir(&redirected).unwrap().count(), 0);
        symlink("/proc", moved.join("kernel")).unwrap();
        assert_eq!(
            create_content_directory(&moved.join("kernel/forbidden-creation"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        std::fs::write(moved.join("file"), b"ordinary").unwrap();
        assert_eq!(
            create_content_directory(&moved.join("file/child"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotADirectory
        );
        let reused = create_content_directory(&moved.join("child/./grandchild/../second")).unwrap();
        assert_eq!(classify_handle(&reused).unwrap(), Kind::Content);
        assert!(moved.join("child/second").is_dir());
        assert_eq!(std::fs::read(moved.join("file")).unwrap(), b"ordinary");
    }

    #[test]
    fn ordinary_directory_pins_reject_protected_aliases_and_keep_the_original_directory() {
        use super::*;
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("directory");
        let moved = temp.path().join("moved");
        let alias = temp.path().join("alias");
        std::fs::create_dir(&directory).unwrap();
        symlink(&directory, &alias).unwrap();
        let pinned = pin_content_directory(&alias).unwrap();
        let identity = pinned.metadata().unwrap().ino();
        std::fs::rename(&directory, &moved).unwrap();
        std::fs::remove_file(&alias).unwrap();
        symlink("/proc", &alias).unwrap();
        assert_eq!(classify_handle(&pinned).unwrap(), Kind::Content);
        assert_eq!(pinned.metadata().unwrap().ino(), identity);
        assert_eq!(std::fs::metadata(&moved).unwrap().ino(), identity);
        assert_eq!(
            pin_content_directory(&alias).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        let file = temp.path().join("file");
        std::fs::write(&file, b"ordinary").unwrap();
        assert_eq!(
            pin_content_directory(&file).unwrap_err().kind(),
            io::ErrorKind::NotADirectory
        );
        assert_eq!(
            pin_content_directory(Path::new("/dev/null"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }
    #[test]
    fn object_control_and_persistent_log_filesystems_are_not_ordinary_content() {
        for (magic, filesystem) in [
            (0x62656570, super::Filesystem::Config),
            (0xcafe4a11, super::Filesystem::Bpf),
            (0x6165676c, super::Filesystem::Pstore),
            (0x07655821, super::Filesystem::ResourceControl),
            (0x6c6f6f70, super::Filesystem::Binder),
            (0x65735543, super::Filesystem::FuseControl),
            (0x6e736673, super::Filesystem::Namespace),
        ] {
            assert_eq!(super::filesystem_for_magic(magic, false), Some(filesystem));
            let kind = super::Kind::Kernel { filesystem };
            assert!(kind.protected() && kind.semantic());
        }
    }
    #[test]
    fn namespace_handles_and_misleading_aliases_cannot_enter_content_readers() {
        use super::*;
        let source = Path::new("/proc/self/ns/mnt");
        let expected = Kind::Kernel {
            filesystem: Filesystem::Namespace,
        };
        assert_eq!(classify(source).unwrap(), expected);
        let temp = tempfile::tempdir().unwrap();
        let alias = temp.path().join("namespace.png");
        std::os::unix::fs::symlink(source, &alias).unwrap();
        assert_eq!(classify(&alias).unwrap(), expected);
        for path in [source, alias.as_path()] {
            assert_eq!(
                require_content(path).unwrap_err().kind(),
                io::ErrorKind::PermissionDenied
            );
            assert_eq!(
                open_content(path, true).unwrap_err().kind(),
                io::ErrorKind::PermissionDenied
            );
        }
    }

    #[test]
    fn fuse_data_filesystems_keep_ordinary_content_policy() {
        assert_eq!(super::filesystem_for_magic(0x65735546, false), None);
    }

    #[test]
    fn mounted_fuse_control_aliases_cannot_enter_ordinary_readers() {
        use super::*;
        let root = Path::new("/sys/fs/fuse/connections");
        let expected = Kind::Kernel {
            filesystem: Filesystem::FuseControl,
        };
        if classify(root).ok() != Some(expected) {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let alias = temp.path().join("ordinary-looking-directory");
        std::os::unix::fs::symlink(root, &alias).unwrap();
        assert_eq!(classify(&alias).unwrap(), expected);
        assert!(require_content(&alias).is_err());
        assert!(open_content(&alias, true).is_err());
    }

    #[test]
    fn security_policy_filesystems_are_protected_independently_of_path_names() {
        for magic in [0x73636673, 0xf97cff8c, 0x43415d53, 0x5a3c69f0] {
            assert_eq!(
                super::filesystem_for_magic(magic, false),
                Some(super::Filesystem::Security)
            );
            assert!(
                super::Kind::Kernel {
                    filesystem: super::Filesystem::Security
                }
                .protected()
            );
        }
        assert_eq!(
            super::filesystem_for_magic(0xffff_ffff_f97c_ff8c, false),
            Some(super::Filesystem::Security)
        );
        assert_eq!(super::filesystem_for_magic(0x9123683e, false), None);
        assert_eq!(super::filesystem_for_magic(0x1021994, false), None);
    }

    use super::*;
    use std::os::unix::fs::symlink;
    #[test]
    fn ordinary_content_and_aliases_remain_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("content");
        std::fs::write(&path, b"ordinary bytes").unwrap();
        let alias = dir.path().join("alias");
        symlink(&path, &alias).unwrap();
        assert_eq!(classify(&alias).unwrap(), Kind::Content);
        assert_eq!(read_content(&alias).unwrap(), b"ordinary bytes");
        assert!(open_content(&alias, false).is_err());
    }
    #[test]
    fn kernel_attribute_alias_cannot_enter_content_reader() {
        let dir = tempfile::tempdir().unwrap();
        let alias = dir.path().join("ordinary-looking.txt");
        symlink("/proc/uptime", &alias).unwrap();
        assert_eq!(
            classify(&alias).unwrap(),
            Kind::Kernel {
                filesystem: Filesystem::Proc
            }
        );
        assert_eq!(
            open_content(&alias, true).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
    #[test]
    fn dangling_ordinary_links_remain_mutable_references() {
        let dir = tempfile::tempdir().unwrap();
        let alias = dir.path().join("alias");
        symlink(dir.path().join("missing"), &alias).unwrap();
        require_content(&alias).unwrap();
        assert!(open_content(&alias, true).is_err());
    }
    #[test]
    fn fifo_is_rejected_before_opening_its_stream() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fifo");
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a NUL-terminated pathname in a private temporary directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert_eq!(classify(&path).unwrap(), Kind::Special);
        assert!(open_content(&path, true).is_err());
    }
}
