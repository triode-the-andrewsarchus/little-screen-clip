//! Index enrollment and content admission. Call only on background workers.
//!
//! Browser visibility is deliberately independent from indexing permission.
use std::{
    fs::File,
    os::fd::{AsRawFd, FromRawFd},
    path::{Component, Path, PathBuf},
};

use crate::file_manager::system::MountIdentity;
use little_clip_core::kernel_interface;

const PRIVATE_DIRECTORIES: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".kube",
    ".azure",
    ".docker",
    ".pki",
    ".password-store",
    ".config",
    "keyrings",
];
const GENERATED_DIRECTORIES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".cache",
    ".venv",
    "venv",
    "node_modules",
    "bower_components",
    "__pycache__",
    "site-packages",
    "target",
    "build",
    "dist",
    "coverage",
    "DerivedData",
    "Pods",
    "Carthage",
];
const SYSTEM_DIRECTORIES: &[&str] = &[
    "proc",
    "sys",
    "dev",
    "etc",
    "run",
    "var",
    "usr",
    "bin",
    "sbin",
    "lib",
    "lib64",
    "boot",
    "root",
    "lost+found",
];

/// A path check is useful for UI explanation, but never authorizes a content read.
pub(super) fn excluded(path: &Path) -> Option<&'static str> {
    // These exclusions are all children of /. Inspect the leading components
    // once rather than reparsing the same path for every protected root.
    let mut parts = path.components();
    if let (Some(Component::RootDir), Some(Component::Normal(name))) = (parts.next(), parts.next())
        && name
            .to_str()
            .is_some_and(|name| SYSTEM_DIRECTORIES.contains(&name))
    {
        return Some("System folders cannot be indexed");
    }
    for part in path.components() {
        if let Component::Normal(name) = part {
            let Some(name) = name.to_str() else { continue };
            if PRIVATE_DIRECTORIES.contains(&name) {
                return Some("Private configuration and credential folders cannot be indexed");
            }
            if GENERATED_DIRECTORIES.contains(&name) || name.ends_with(".noindex") {
                return Some("Repository internals, caches and generated folders are excluded");
            }
        }
    }
    None
}

pub(super) fn secret_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    name == ".env"
        || name.starts_with(".env.")
        || name == ".envrc"
        || name.starts_with("secrets.")
        || matches!(
            name.as_str(),
            "id_rsa"
                | "id_dsa"
                | "id_ecdsa"
                | "id_ed25519"
                | "identity"
                | ".netrc"
                | ".npmrc"
                | ".pypirc"
                | ".git-credentials"
                | "credentials"
                | "credentials.json"
                | "tokens.json"
        )
        || matches!(
            extension.as_str(),
            "pem" | "key" | "p12" | "pfx" | "keystore" | "kdbx"
        )
}

/// Visibility is relative to the chosen root, including non-UTF-8 names.
/// An explicitly opened/enrolled hidden root does not hide its ordinary children.
pub(super) fn hidden_descendant(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root).map_or(true, |relative| {
        relative.components().any(|part| {
            matches!(part,
            Component::Normal(name) if name.as_encoded_bytes().starts_with(b"."))
        })
    })
}

/// Parse literal addresses without opening targets or following aliases.
pub(super) fn exclusions(addresses: &[String]) -> Result<Vec<PathBuf>, String> {
    if addresses.len() > 128 {
        return Err("Use at most 128 excluded file/folder addresses".into());
    }
    let mut paths = Vec::with_capacity(addresses.len());
    for address in addresses {
        if address.len() > 8192 {
            return Err("An excluded address is too long".into());
        }
        let path = crate::file_manager::filesystem::decode_path(address)?;
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(
                "Excluded addresses must be absolute, without parent (..) components".into(),
            );
        }
        paths.push(path);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

pub(super) fn excluded_by_user(path: &Path, exclusions: &[PathBuf]) -> bool {
    exclusions.iter().any(|excluded| path.starts_with(excluded))
}

pub(super) fn excluded_from_root(path: &Path, root: &Path, hidden: bool) -> bool {
    !path.starts_with(root)
        || excluded(path).is_some()
        || secret_file(path)
        || (!hidden && hidden_descendant(path, root))
}

fn check_path(path: &Path) -> Result<(), String> {
    excluded(path).map_or(Ok(()), |reason| Err(reason.into()))
}

/// Inspect the actual pinned object, including aliases to excluded directories.
pub(super) fn check_handle(file: &File) -> Result<PathBuf, String> {
    if kernel_interface::classify_handle(file)
        .map_err(|e| e.to_string())?
        .protected()
    {
        return Err("Kernel interfaces and special files cannot be indexed".into());
    }
    let actual = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
        .map_err(|e| format!("Cannot verify index source: {e}"))?;
    if !actual.is_absolute()
        || actual
            .as_os_str()
            .as_encoded_bytes()
            .ends_with(b" (deleted)")
    {
        return Err("Index source is no longer available".into());
    }
    check_path(&actual)?;
    Ok(actual)
}

pub(super) struct Root {
    pub path: PathBuf,
    pub directory: File,
    pub device: u64,
    pub inode: u64,
    pub mount: u64,
}

impl Root {
    pub(super) fn open(path: &Path) -> Result<Self, String> {
        check_path(path)?;
        let directory = kernel_interface::pin_content_directory(path).map_err(|e| e.to_string())?;
        Self::from_directory(directory)
    }
    fn from_directory(directory: File) -> Result<Self, String> {
        let actual = check_handle(&directory)?;
        if !directory.metadata().map_err(|e| e.to_string())?.is_dir() {
            return Err("Search root is not an ordinary directory".into());
        }
        if matches!(
            actual.to_str(),
            Some("/" | "/home" | "/tmp" | "/mnt" | "/media")
        ) {
            return Err("Choose a specific content folder to index".into());
        }
        let identity = MountIdentity::pinned(&directory).map_err(|e| e.to_string())?;
        Ok(Self {
            path: actual,
            device: identity.device,
            inode: identity.inode,
            mount: identity.mount,
            directory,
        })
    }
    pub(super) fn child(&self, path: &Path) -> Result<Self, String> {
        Self::from_directory(self.pin(path)?)
    }
    fn relative<'a>(&self, path: &'a Path) -> Result<&'a Path, String> {
        check_path(path)?;
        let relative = path
            .strip_prefix(&self.path)
            .map_err(|_| "Path is outside its search root")?;
        Ok(if relative.as_os_str().is_empty() {
            Path::new(".")
        } else {
            relative
        })
    }
    fn pin(&self, path: &Path) -> Result<File, String> {
        self.revalidate()?;
        self.admit(path)?
            .ok_or_else(|| "Search does not cross aliases or nested mounts".into())
    }
    /// A mount/alias crossing is an intentional exclusion; I/O errors remain failures.
    pub(super) fn admit(&self, path: &Path) -> Result<Option<File>, String> {
        let pinned = match pin_beneath(&self.directory, self.relative(path)?) {
            Ok(file) => file,
            Err(error) if matches!(error.raw_os_error(), Some(libc::EXDEV | libc::ELOOP)) => {
                return Ok(None);
            }
            Err(error) => {
                return Err(format!(
                    "Cannot resolve search source within its original mount: {error}"
                ));
            }
        };
        if check_handle(&pinned)? != path {
            return Err("Search source moved or crosses an alias".into());
        }
        Ok(Some(pinned))
    }
    pub(super) fn confirms_absence(&self, path: &Path) -> Result<(), String> {
        self.revalidate()?;
        match pin_beneath(&self.directory, self.relative(path)?) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => self.revalidate(),
            _ => Err("Path absence was not confirmed within the indexed mount".into()),
        }
    }
    pub(super) fn open_content(&self, path: &Path) -> Result<File, String> {
        if secret_file(path) {
            return Err("Credential/key material is excluded from search".into());
        }
        let pinned = self.pin(path)?;
        // This descriptor label is application-owned, not file-supplied input.
        // Core reopens and classifies this exact pin before any payload access.
        let label = PathBuf::from(format!("/proc/self/fd/{}", pinned.as_raw_fd()));
        let file = kernel_interface::open_content(&label, true).map_err(|e| e.to_string())?;
        if check_handle(&file)? != path {
            return Err("Search source moved during open".into());
        }
        Ok(file)
    }

    pub(super) fn revalidate(&self) -> Result<(), String> {
        let actual = check_handle(&self.directory)?;
        if actual != self.path {
            return Err("Indexed root moved; choose its new location explicitly".into());
        }
        let current = kernel_interface::pin(&self.path, false).map_err(|e| e.to_string())?;
        let identity = MountIdentity::pinned(&current).map_err(|e| e.to_string())?;
        if (identity.device, identity.inode, identity.mount)
            != (self.device, self.inode, self.mount)
        {
            return Err("Indexed root was replaced or remounted".into());
        }
        Ok(())
    }
}

// Linux openat2 resolves the entire relative path under the retained root.
// A final mount-ID comparison alone misses paths that cross out and back in.
fn pin_beneath(directory: &File, path: &Path) -> std::io::Result<File> {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid search path")
    })?;
    let how = OpenHow {
        flags: (libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW) as u64,
        mode: 0,
        resolve: 0x01 | 0x04 | 0x08, // NO_XDEV | NO_SYMLINKS | BENEATH (linux/openat2.h)
    };
    // SAFETY: directory is live; both pointers describe initialized ABI structures.
    let descriptor = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            directory.as_raw_fd(),
            path.as_ptr(),
            &how,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful openat2 returned one new owned descriptor.
    Ok(unsafe { File::from_raw_fd(descriptor as i32) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_component_check_matches_prior_component_prefix_policy() {
        use std::os::unix::ffi::OsStringExt;
        let mut paths = Vec::new();
        for directory in SYSTEM_DIRECTORIES
            .iter()
            .chain(PRIVATE_DIRECTORIES)
            .chain(GENERATED_DIRECTORIES)
        {
            for prefix in ["", "/", "//", "/./", "./", "/ordinary/", "/ordinary/../"] {
                for suffix in [
                    "",
                    "/",
                    "/file.txt",
                    "/./file.txt",
                    "/../file.txt",
                    "-ordinary/file.txt",
                    "/.ssh/file.txt",
                    "/.git/objects",
                ] {
                    paths.push(PathBuf::from(format!("{prefix}{directory}{suffix}")));
                }
            }
        }
        paths.extend([
            PathBuf::new(),
            PathBuf::from("/"),
            PathBuf::from("./"),
            PathBuf::from("/ordinary/data.noindex/file"),
        ]);
        for bytes in [
            b"/proc/\xff".as_slice(),
            b"/\xff/.ssh/key",
            b"/pro\xff/file",
            b"proc/\xff",
        ] {
            paths.push(PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec())));
        }
        for path in paths {
            let legacy_system = SYSTEM_DIRECTORIES
                .iter()
                .any(|name| path.starts_with(format!("/{name}")));
            let expected = if legacy_system {
                Some("System folders cannot be indexed")
            } else {
                path.components().find_map(|part| {
                    let Component::Normal(name) = part else {
                        return None;
                    };
                    let name = name.to_str()?;
                    if PRIVATE_DIRECTORIES.contains(&name) {
                        Some("Private configuration and credential folders cannot be indexed")
                    } else if GENERATED_DIRECTORIES.contains(&name) || name.ends_with(".noindex") {
                        Some("Repository internals, caches and generated folders are excluded")
                    } else {
                        None
                    }
                })
            };
            assert_eq!(excluded(&path), expected, "{}", path.display());
        }
    }

    #[test]
    fn user_exclusions_are_literal_component_prefixes_with_reversible_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let unusual = PathBuf::from(std::ffi::OsString::from_vec(b"/data/private-\xff".to_vec()));
        let addresses = vec![
            "/data/ignore".into(),
            crate::file_manager::filesystem::display_path(&unusual),
        ];
        let paths = exclusions(&addresses).unwrap();
        assert!(excluded_by_user(Path::new("/data/ignore"), &paths));
        assert!(excluded_by_user(
            Path::new("/data/ignore/deep/file.txt"),
            &paths
        ));
        assert!(excluded_by_user(&unusual.join("notes.txt"), &paths));
        assert!(!excluded_by_user(
            Path::new("/data/ignored/file.txt"),
            &paths
        ));
        for invalid in [
            vec!["relative".into()],
            vec!["/data/../private".into()],
            vec!["/data".into(); 129],
        ] {
            assert!(exclusions(&invalid).is_err());
        }
        assert!(excluded_by_user(
            Path::new("/data/ordinary"),
            &exclusions(&["/".into()]).unwrap()
        ));
    }

    #[test]
    fn hidden_visibility_is_relative_and_preserves_non_utf8_paths() {
        use std::os::unix::ffi::OsStringExt;
        let root = Path::new("/home/a/.samples");
        assert!(!hidden_descendant(&root.join("kick.wav"), root));
        assert!(hidden_descendant(&root.join(".old/kick.wav"), root));
        assert!(hidden_descendant(&root.join("ordinary/.kick.wav"), root));
        assert!(hidden_descendant(Path::new("/other/kick.wav"), root));
        let invalid = root.join(std::ffi::OsString::from_vec(vec![b'.', 0xff]));
        assert!(hidden_descendant(&invalid.join("kick.wav"), root));
        assert!(!excluded_from_root(&root.join("kick.wav"), root, false));
        assert!(!excluded_from_root(&invalid.join("kick.wav"), root, true));
        assert!(excluded_from_root(&root.join(".ssh/key"), root, true));
        assert!(excluded_from_root(&root.join(".env"), root, true));
    }

    #[test]
    fn private_and_useless_roots_cannot_be_enrolled_even_when_explicit() {
        for path in [
            "/proc",
            "/sys/devices",
            "/dev/shm",
            "/home/a/.ssh",
            "/home/a/.git/objects",
            "/home/a/node_modules/pkg",
            "/home/a/.config/app",
            "/home/a/notes.noindex",
        ] {
            assert!(Root::open(Path::new(path)).is_err(), "{path}");
        }
        assert!(excluded(Path::new("/home/a/Documents/git/project")).is_none());
        assert!(excluded(Path::new("/home/a/Documents/type_testing")).is_none());
    }

    #[test]
    fn aliases_cannot_bypass_credential_content_or_root_checks() {
        let dir = tempfile::tempdir().unwrap();
        let private = dir.path().join(".ssh");
        std::fs::create_dir(&private).unwrap();
        std::fs::write(private.join("notes.txt"), "private").unwrap();
        let alias = dir.path().join("ordinary");
        std::os::unix::fs::symlink(&private, &alias).unwrap();
        assert!(Root::open(&alias).is_err());
        assert!(
            Root::open(dir.path())
                .unwrap()
                .open_content(&alias.join("notes.txt"))
                .is_err()
        );
        for name in [
            ".env",
            ".env.production",
            "id_ed25519",
            "private.pem",
            "vault.kdbx",
        ] {
            assert!(secret_file(Path::new(name)), "{name}");
        }
        assert!(!secret_file(Path::new("my-keyboard-notes.txt")));
    }

    #[test]
    fn root_handles_detect_replacement_and_leave_ordinary_mounts_usable() {
        let dir = tempfile::tempdir().unwrap();
        let root = Root::open(dir.path()).unwrap();
        root.revalidate().unwrap();
        let child = dir.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let root = Root::open(&child).unwrap();
        std::fs::rename(&child, dir.path().join("moved")).unwrap();
        std::fs::create_dir(&child).unwrap();
        assert!(root.revalidate().is_err());
    }
}
