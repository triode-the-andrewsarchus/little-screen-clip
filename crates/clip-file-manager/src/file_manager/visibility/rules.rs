//! Versioned visibility opinions; compilation and source reads run on a worker.
use globset::{GlobBuilder, GlobMatcher};
use little_clip_core::{
    file_source::{self, FileSource},
    kernel_interface,
};
use serde::Deserialize;
use std::{
    fs::File,
    io::{ErrorKind, Read},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

const FILE_LIMIT: usize = 32 * 1024;
const RULE_LIMIT: usize = 128;
const TOTAL_LIMIT: usize = 256 * 1024;
const DEPTH_LIMIT: usize = 32;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    patterns: Vec<String>,
}
#[derive(Debug)]
struct Pattern {
    matcher: GlobMatcher,
    basename: bool,
    directories: bool,
    cancel: bool,
}
#[derive(Debug)]
struct Layer {
    base: PathBuf,
    source: PathBuf,
    show: bool,
    patterns: Vec<Pattern>,
}
#[derive(Debug, Default)]
pub(in crate::file_manager) struct Rules {
    pub root: PathBuf,
    layers: Vec<Layer>,
}
pub(in crate::file_manager) struct Effect<'a> {
    pub show: bool,
    pub source: &'a Path,
}

impl Layer {
    fn parse(base: PathBuf, source: PathBuf, show: bool, bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
        let document: Document = toml::from_str(text).map_err(|e| e.to_string())?;
        if document.version != 1 {
            return Err("Unsupported visibility version; expected version = 1".into());
        }
        if document.patterns.len() > RULE_LIMIT {
            return Err("Visibility file exceeds 128 patterns".into());
        }
        let patterns = document
            .patterns
            .iter()
            .map(|p| Pattern::parse(p))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            base,
            source,
            show,
            patterns,
        })
    }
    fn matches(&self, path: &Path, directory: bool) -> bool {
        let Ok(relative) = path.strip_prefix(&self.base) else {
            return false;
        };
        let mut opinion = false;
        for rule in &self.patterns {
            if relative
                .ancestors()
                .filter(|p| !p.as_os_str().is_empty())
                .any(|candidate| {
                    if rule.directories && candidate == relative && !directory {
                        return false;
                    }
                    let target = if rule.basename {
                        candidate.file_name().map(Path::new).unwrap_or(candidate)
                    } else {
                        candidate
                    };
                    rule.matcher.is_match(target)
                })
            {
                opinion = !rule.cancel;
            }
        }
        opinion
    }
}
impl Pattern {
    fn parse(raw: &str) -> Result<Self, String> {
        if raw.is_empty() || raw.len() > 256 || raw.contains('\0') {
            return Err("Visibility patterns must contain 1–256 bytes without NUL".into());
        }
        let cancel = raw.starts_with('!');
        let text = raw.strip_prefix('!').unwrap_or(raw);
        let anchored = text.starts_with('/');
        let directories = text.ends_with('/');
        let text = text.trim_start_matches('/').trim_end_matches('/');
        if text.is_empty() || text.split('/').any(|p| matches!(p, "." | "..")) {
            return Err("Visibility patterns must stay relative to their owning directory".into());
        }
        let matcher = GlobBuilder::new(text)
            .literal_separator(true)
            .backslash_escape(true)
            .build()
            .map_err(|e| e.to_string())?
            .compile_matcher();
        Ok(Self {
            matcher,
            basename: !anchored && !text.contains('/'),
            directories,
            cancel,
        })
    }
}
impl Rules {
    pub fn count(&self) -> usize {
        self.layers.iter().map(|l| l.patterns.len()).sum()
    }
    pub fn sources(&self) -> impl Iterator<Item = &Path> {
        self.layers.iter().map(|l| l.source.as_path())
    }
    pub fn effect(&self, path: &Path, directory: bool) -> Option<Effect<'_>> {
        if self.root.as_os_str().is_empty() {
            return None;
        }
        self.layers
            .iter()
            .rev()
            .find(|layer| layer.matches(path, directory))
            .map(|l| Effect {
                show: l.show,
                source: &l.source,
            })
    }
    pub fn load(
        path: &Path,
        object: Option<(u64, u64)>,
        mount: Option<u64>,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        let mut pin = kernel_interface::pin_content_directory(path).map_err(|e| e.to_string())?;
        validate_root(&pin, object, mount)?;
        let root_mount = kernel_interface::MountIdentity::pinned(&pin)
            .map_err(|e| e.to_string())?
            .mount;
        let mut base = path.to_path_buf();
        let mut layers = Vec::new();
        let mut total = 0;
        for depth in 0..DEPTH_LIMIT {
            if cancel.load(Ordering::Relaxed) {
                return Err("Visibility read cancelled".into());
            }
            let local = folder_layers(&pin, &base, &mut total)?;
            // Later, deeper directories win. Within a directory .show wins.
            layers.splice(0..0, local);
            if layers.iter().map(|l| l.patterns.len()).sum::<usize>() > RULE_LIMIT {
                return Err("Inherited visibility rules exceed 128 patterns".into());
            }
            let Some((next, parent)) = parent(&pin, &base, root_mount, depth)? else {
                break;
            };
            base = parent;
            pin = next;
        }
        // Do not publish an observation for a replaced browser directory.
        let current = kernel_interface::pin_content_directory(path).map_err(|e| e.to_string())?;
        validate_root(&current, object, mount)?;
        Ok(Self {
            root: path.into(),
            layers,
        })
    }
}
// Source collection and parent traversal have different bounds and ownership.
fn folder_layers(pin: &File, base: &Path, total: &mut usize) -> Result<Vec<Layer>, String> {
    let mut local = Vec::new();
    for (name, show) in [(".hide", false), (".show", true)] {
        if let Some(bytes) = read(pin, name)? {
            *total += bytes.len();
            if *total > TOTAL_LIMIT {
                return Err("Inherited visibility sources exceed 256 KiB".into());
            }
            let source = base.join(name);
            local.push(
                Layer::parse(base.into(), source.clone(), show, &bytes)
                    .map_err(|e| format!("{}: {e}", source.display()))?,
            );
        }
    }
    Ok(local)
}
fn parent(
    pin: &File,
    base: &Path,
    mount: u64,
    depth: usize,
) -> Result<Option<(File, PathBuf)>, String> {
    let Some(parent) = base.parent() else {
        return Ok(None);
    };
    if depth + 1 == DEPTH_LIMIT {
        return Err("Visibility ancestry exceeds 32 directories".into());
    }
    let next = kernel_interface::pin(&held(pin).join(".."), true).map_err(|e| e.to_string())?;
    let observed = kernel_interface::MountIdentity::pinned(&next).map_err(|e| e.to_string())?;
    if observed.mount != mount {
        return Ok(None);
    }
    if kernel_interface::classify_handle(&next)
        .map_err(|e| e.to_string())?
        .protected()
    {
        return Err("Visibility inheritance reached a protected ancestor".into());
    }
    Ok(Some((next, parent.into())))
}
fn held(file: &File) -> PathBuf {
    format!("/proc/self/fd/{}", file.as_raw_fd()).into()
}
fn validate_root(
    file: &File,
    object: Option<(u64, u64)>,
    mount: Option<u64>,
) -> Result<(), String> {
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    let observed = kernel_interface::MountIdentity::pinned(file).map_err(|e| e.to_string())?;
    if object != Some((metadata.dev(), metadata.ino())) || mount != Some(observed.mount) {
        return Err(
            "Visibility directory identity changed or is unavailable; refresh the folder".into(),
        );
    }
    Ok(())
}
fn read(directory: &File, name: &str) -> Result<Option<Vec<u8>>, String> {
    let path = held(directory).join(name);
    let mut file = match file_source::open_regular_file(&path, false) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{name}: {e}")),
    };
    let source = FileSource::capture_file(&file)?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(FILE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > FILE_LIMIT {
        return Err(format!("{name} exceeds 32 KiB"));
    }
    source.check_file(&file)?;
    source.check()?;
    Ok(Some(bytes))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn layer(base: &str, show: bool, patterns: &[&str]) -> Layer {
        let text = format!(
            "version=1\npatterns={}\n",
            serde_json::to_string(patterns).unwrap()
        );
        Layer::parse(
            base.into(),
            Path::new(base).join(if show { ".show" } else { ".hide" }),
            show,
            text.as_bytes(),
        )
        .unwrap()
    }
    fn load(path: &Path) -> Result<Rules, String> {
        let directory =
            crate::file_manager::filesystem::read_directory(path, &AtomicBool::new(false))?;
        Rules::load(
            &directory.path,
            directory.object,
            directory.mount,
            &AtomicBool::new(false),
        )
    }
    #[test]
    fn ordinary_shared_memory_mount_stops_before_the_protected_parent() {
        let Ok(root) = tempfile::tempdir_in("/dev/shm") else {
            return;
        };
        std::fs::write(root.path().join(".show"), "version=1\npatterns=['.env']").unwrap();
        assert_eq!(load(root.path()).unwrap().count(), 1);
    }
    #[test]
    fn wildcard_matching_keeps_non_utf8_unix_names_lossless() {
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_vec(b"/root/odd-\xff.tmp".to_vec()));
        let rules = Rules {
            root: "/root".into(),
            layers: vec![layer("/root", false, &["*.tmp"])],
        };
        assert!(!rules.effect(&path, false).unwrap().show);
    }
    #[test]
    fn guarded_inheritance_selective_dotfiles_and_recovery_use_real_sources() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".hide"), "version=1\npatterns=['*.tmp']").unwrap();
        let child = root.path().join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(
            child.join(".show"),
            "version=1\npatterns=['keep.tmp','.env']",
        )
        .unwrap();
        for name in ["keep.tmp", "drop.tmp", ".env", ".secret"] {
            std::fs::write(child.join(name), name).unwrap();
        }
        let rules = load(&child).unwrap();
        assert_eq!(rules.count(), 3);
        let directory =
            crate::file_manager::filesystem::read_directory(&child, &AtomicBool::new(false))
                .unwrap();
        for entry in &directory.entries {
            let expected = matches!(entry.name.as_str(), "keep.tmp" | ".env");
            assert_eq!(
                rules.visible(entry, false, false),
                expected,
                "{}",
                entry.name
            );
            assert!(
                rules.visible(entry, true, true),
                "recovery must expose {}",
                entry.name
            );
        }
        let future = b"version=99\npatterns=[]";
        std::fs::write(child.join(".show"), future).unwrap();
        assert!(load(&child).is_err());
        assert_eq!(std::fs::read(child.join(".show")).unwrap(), future);
    }
    #[test]
    fn aliases_nonregular_oversized_and_replaced_directories_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("rules");
        std::fs::write(&source, "version=1\npatterns=[]").unwrap();
        std::os::unix::fs::symlink(&source, root.path().join(".hide")).unwrap();
        assert!(load(root.path()).is_err());
        std::fs::remove_file(root.path().join(".hide")).unwrap();
        std::fs::create_dir(root.path().join(".hide")).unwrap();
        assert!(load(root.path()).is_err());
        std::fs::remove_dir(root.path().join(".hide")).unwrap();
        std::fs::write(root.path().join(".hide"), vec![b'x'; FILE_LIMIT + 1]).unwrap();
        assert!(load(root.path()).is_err());
        let child = root.path().join("folder");
        std::fs::create_dir(&child).unwrap();
        let directory =
            crate::file_manager::filesystem::read_directory(&child, &AtomicBool::new(false))
                .unwrap();
        std::fs::rename(&child, root.path().join("old")).unwrap();
        std::fs::create_dir(&child).unwrap();
        assert!(
            Rules::load(
                &child,
                directory.object,
                directory.mount,
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
    #[test]
    fn precedence_negation_anchoring_and_directory_patterns_are_explicit() {
        let rules = Rules {
            root: "/root/child".into(),
            layers: vec![
                layer(
                    "/root",
                    false,
                    &["*.tmp", "!keep.tmp", "cache/", "/top.txt"],
                ),
                layer("/root", true, &[".env"]),
                layer("/root/child", false, &[".env"]),
            ],
        };
        assert!(
            !rules
                .effect(Path::new("/root/child/a.tmp"), false)
                .unwrap()
                .show
        );
        assert!(
            rules
                .effect(Path::new("/root/child/keep.tmp"), false)
                .is_none()
        );
        assert!(
            !rules
                .effect(Path::new("/root/child/cache/file"), false)
                .unwrap()
                .show
        );
        assert!(
            rules
                .effect(Path::new("/root/child/top.txt"), false)
                .is_none()
        );
        assert!(
            !rules
                .effect(Path::new("/root/child/.env"), false)
                .unwrap()
                .show
        );
        assert!(rules.effect(Path::new("/outside/a.tmp"), false).is_none());
        let same = Rules {
            root: "/root".into(),
            layers: vec![
                layer("/root", false, &["*"]),
                layer("/root", true, &[".env"]),
            ],
        };
        assert!(same.effect(Path::new("/root/.env"), false).unwrap().show);
        assert!(!same.effect(Path::new("/root/.secret"), false).unwrap().show);
    }
    #[test]
    fn malformed_future_oversized_and_escaping_patterns_fail() {
        for text in [
            "version=2\npatterns=[]",
            "version=1\npatterns=['[']",
            "version=1\npatterns=['../secret']",
            "version=1\npatterns=['']",
            "version=1\npatterns=[]\nextra=1",
        ] {
            assert!(
                Layer::parse("/root".into(), "/root/.hide".into(), false, text.as_bytes()).is_err(),
                "{text}"
            );
        }
        assert!(Pattern::parse(&"x".repeat(257)).is_err());
    }
}
