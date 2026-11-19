//! Bounded, Hugo-aware local asset resolution for rendered Markdown documents.
use std::{
    borrow::Cow,
    path::{Component, Path, PathBuf},
};

const MAX_HUGO_ANCESTORS: usize = 8;
const MAX_THEME_DIRS: usize = 16;
const MAX_DIR_SCAN: usize = 1024;

/// Resolves a local Markdown or HTML asset reference (e.g. `<img>` `src` or
/// `![](path)`) relative to `doc_path`.
///
/// Direct/relative paths are checked beside the document first; when missing or
/// site-root-relative (`/...`), up to [`MAX_HUGO_ANCESTORS`] parent directories are
/// searched for Hugo `static/`, `assets/`, `public/`, and `themes/*/static/`
/// roots. External URIs (`http:`, `https:`, `data:`, `file:`, etc.) and
/// traversal escapes above `static/` return `None`.
fn document_base_dir(doc_path: &Path) -> &Path {
    if doc_path.is_dir() {
        doc_path
    } else {
        doc_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
    }
}

pub fn resolve_document_asset(doc_path: &Path, reference: &str) -> Option<PathBuf> {
    let candidates = clean_reference_variants(reference)?;
    let doc_dir = document_base_dir(doc_path);

    for cleaned in candidates {
        if let Some(found) = resolve_cleaned(doc_dir, &cleaned) {
            return Some(found);
        }
    }
    None
}

/// Resolves an internal Markdown or HTML document link relative to `doc_path`.
///
/// Relative links check beside the document first (clamped to the enclosing Hugo
/// site root when present), supporting omitted Markdown extensions, case-insensitive
/// Hugo segments, and `_index.md` / `index.md` / `_index.html` section bundles.
/// Site-root links (`/...`) and rootless Hugo `ref`/`relref` targets search up to
/// [`MAX_HUGO_ANCESTORS`] `content/` directories. Self-links, external URIs,
/// fragment-only links, and traversal escapes return `None`.
pub fn resolve_document_link(doc_path: &Path, reference: &str) -> Option<PathBuf> {
    let candidates = clean_reference_variants(reference)?;
    let doc_dir = document_base_dir(doc_path);
    let hugo_roots = collect_hugo_content_roots(doc_dir);
    let site_ceiling = hugo_roots
        .iter()
        .find(|(_, content)| doc_dir.starts_with(content))
        .map(|(site, _)| site.as_path());
    let self_identity = (!doc_path.is_dir()).then(|| canonical_or_normalized(doc_path));

    for cleaned in candidates {
        let found = if cleaned.starts_with('/') {
            hugo_roots.iter().find_map(|(_, content_dir)| {
                resolve_content_target(content_dir, content_dir, &cleaned)
            })
        } else {
            resolve_relative_target(doc_dir, site_ceiling, &cleaned).or_else(|| {
                hugo_roots.iter().find_map(|(_, content_dir)| {
                    resolve_content_target(content_dir, content_dir, &cleaned)
                })
            })
        };
        if let Some(path) = found
            && self_identity
                .as_ref()
                .is_none_or(|self_id| canonical_or_normalized(&path) != *self_id)
        {
            return Some(path);
        }
    }
    None
}

fn clean_reference_variants(reference: &str) -> Option<Vec<String>> {
    let mut trimmed = reference.trim();
    if trimmed.starts_with('<') && trimmed.ends_with('>') && trimmed.len() >= 2 {
        trimmed = trimmed[1..trimmed.len() - 1].trim();
    }
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.contains('\0') {
        return None;
    }
    if is_blocked_uri_scheme(trimmed) {
        return None;
    }

    let without_fragment = trimmed.split('#').next().unwrap_or(trimmed);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);

    let mut out = Vec::new();
    for raw in [without_query, trimmed] {
        let decoded = percent_decode(raw);
        let decoded = decoded.trim();
        if decoded.is_empty()
            || decoded.starts_with('#')
            || decoded.contains('\0')
            || is_blocked_uri_scheme(decoded)
        {
            continue;
        }
        if !out.iter().any(|existing: &String| existing == decoded) {
            out.push(decoded.to_owned());
        }
    }
    (!out.is_empty()).then_some(out)
}

fn is_blocked_uri_scheme(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("//") {
        return true;
    }
    const BLOCKED_PREFIXES: &[&str] = &[
        "http:",
        "https:",
        "ftp:",
        "ftps:",
        "file:",
        "data:",
        "blob:",
        "javascript:",
        "vbscript:",
        "mailto:",
        "about:",
        "tel:",
    ];
    if BLOCKED_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        return true;
    }
    if let Some((scheme, _)) = lower.split_once("://") {
        return !scheme.is_empty()
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
    }
    false
}

fn percent_decode(input: &str) -> Cow<'_, str> {
    if !input.as_bytes().contains(&b'%') {
        return Cow::Borrowed(input);
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    match String::from_utf8(out) {
        Ok(decoded) => Cow::Owned(decoded),
        Err(_) => Cow::Borrowed(input),
    }
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn try_regular_file(path: PathBuf) -> Option<PathBuf> {
    let canon = std::fs::canonicalize(&path).ok()?;
    if is_system_path(&canon) {
        return None;
    }
    crate::file_source::open_regular_file(&path, true)
        .ok()
        .map(|_| path)
}

fn safe_rel_under(root: &Path, raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim_start_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let mut stack = Vec::new();
    for component in Path::new(trimmed).components() {
        match component {
            Component::CurDir | Component::RootDir => {}
            Component::ParentDir => {
                stack.pop()?;
            }
            Component::Normal(part) => stack.push(part),
            Component::Prefix(_) => return None,
        }
    }
    if stack.is_empty() {
        return None;
    }
    let mut out = root.to_path_buf();
    for part in stack {
        out.push(part);
    }
    Some(out)
}

fn lexical_join(base: &Path, rel: &str) -> Option<PathBuf> {
    let mut path = base.to_path_buf();
    for component in Path::new(rel).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !path.pop() {
                    return None;
                }
            }
            Component::Normal(part) => path.push(part),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(path)
}

fn resolve_in_hugo_ancestors(doc_dir: &Path, rel: &str) -> Option<PathBuf> {
    let rel_trimmed = rel.trim_start_matches('/');
    for ancestor in doc_dir.ancestors().take(MAX_HUGO_ANCESTORS + 1) {
        for folder in ["static", "assets", "public"] {
            if let Some(candidate) = safe_rel_under(&ancestor.join(folder), rel_trimmed)
                && let Some(found) = try_regular_file(candidate)
            {
                return Some(found);
            }
        }
        if (rel_trimmed.starts_with("static/") || rel_trimmed.starts_with("assets/"))
            && let Some(candidate) = safe_rel_under(ancestor, rel_trimmed)
            && let Some(found) = try_regular_file(candidate)
        {
            return Some(found);
        }
        let themes = ancestor.join("themes");
        if themes.is_dir()
            && let Ok(entries) = std::fs::read_dir(&themes)
        {
            for entry in entries.flatten().take(MAX_THEME_DIRS) {
                let theme_dir = entry.path();
                if theme_dir.is_dir() {
                    for folder in ["static", "assets"] {
                        if let Some(candidate) =
                            safe_rel_under(&theme_dir.join(folder), rel_trimmed)
                            && let Some(found) = try_regular_file(candidate)
                        {
                            return Some(found);
                        }
                    }
                }
            }
        }
    }
    None
}

fn normalize_absolute(raw: &str) -> Option<PathBuf> {
    let mut parts = Vec::new();
    for component in Path::new(raw).components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(part) => parts.push(part),
            Component::Prefix(_) => return None,
        }
    }
    if parts.is_empty() {
        return None;
    }
    let mut out = PathBuf::from("/");
    for part in parts {
        out.push(part);
    }
    Some(out)
}

fn is_system_path(path: &Path) -> bool {
    const SYSTEM_PREFIXES: &[&str] = &[
        "/etc", "/proc", "/sys", "/dev", "/run", "/var", "/bin", "/sbin", "/usr", "/boot", "/lib",
        "/lib64",
    ];
    SYSTEM_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
}

fn is_direct_absolute_allowed(doc_dir: &Path, candidate: &Path) -> bool {
    if is_system_path(candidate) {
        return false;
    }
    if Path::new(candidate)
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return false;
    }
    if candidate.starts_with(doc_dir) {
        return true;
    }
    let Some(ext) = candidate.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let lower = ext.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "webp"
            | "gif"
            | "bmp"
            | "svg"
            | "svgz"
            | "avif"
            | "heif"
            | "heic"
            | "jxl"
            | "tif"
            | "tiff"
            | "tga"
    )
}

fn resolve_cleaned(doc_dir: &Path, cleaned: &str) -> Option<PathBuf> {
    if cleaned.starts_with('/') {
        if let Some(found) = resolve_in_hugo_ancestors(doc_dir, cleaned) {
            return Some(found);
        }
        // Only allow a direct filesystem absolute path when it contains no `..`
        // traversal segments and points to an explicit non-system image/local asset.
        if !Path::new(cleaned)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
            && let Some(normalized) = normalize_absolute(cleaned)
            && is_direct_absolute_allowed(doc_dir, &normalized)
            && let Some(found) = try_regular_file(normalized)
        {
            return Some(found);
        }
        return None;
    }

    if let Some(candidate) = lexical_join(doc_dir, cleaned)
        && !is_system_path(&candidate)
        && let Some(found) = try_regular_file(candidate)
    {
        return Some(found);
    }
    resolve_in_hugo_ancestors(doc_dir, cleaned)
}

fn collect_hugo_content_roots(doc_dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut roots = Vec::new();
    for ancestor in doc_dir.ancestors().take(MAX_HUGO_ANCESTORS + 1) {
        let content = ancestor.join("content");
        if content.is_dir() {
            roots.push((ancestor.to_path_buf(), content));
        }
    }
    roots
}

fn canonical_or_normalized(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        let mut out = PathBuf::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    out.pop();
                }
                other => out.push(other.as_os_str()),
            }
        }
        out
    })
}

fn resolve_relative_target(doc_dir: &Path, ceiling: Option<&Path>, rel: &str) -> Option<PathBuf> {
    if let Some(root) = ceiling {
        return resolve_content_target(doc_dir, root, rel);
    }
    let mut joined = doc_dir.to_path_buf();
    for component in Path::new(rel).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !joined.pop() {
                    return None;
                }
            }
            Component::Normal(part) => joined.push(part),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if is_system_path(&joined) {
        return None;
    }
    let parent = joined.parent()?;
    let leaf = joined.file_name()?.to_str()?;
    resolve_document_leaf(parent, leaf)
}

fn resolve_content_target(base: &Path, ceiling: &Path, rel: &str) -> Option<PathBuf> {
    let prefix = base.strip_prefix(ceiling).ok()?;
    let mut segments: Vec<String> = Vec::new();
    for component in prefix.components() {
        if let Component::Normal(part) = component {
            segments.push(part.to_str()?.to_owned());
        }
    }
    let trimmed = rel.trim_start_matches('/');
    for component in Path::new(trimmed).components() {
        match component {
            Component::CurDir | Component::RootDir => {}
            Component::ParentDir => {
                segments.pop()?;
            }
            Component::Normal(part) => {
                segments.push(part.to_str()?.to_owned());
            }
            Component::Prefix(_) => return None,
        }
    }
    if segments.is_empty() {
        return (!is_system_path(ceiling))
            .then(|| find_bundle_index(ceiling))
            .flatten();
    }
    let mut dir = ceiling.to_path_buf();
    for seg in &segments[..segments.len() - 1] {
        dir = find_subdir_ci(&dir, seg)?;
    }
    if is_system_path(&dir) {
        return None;
    }
    resolve_document_leaf(&dir, &segments[segments.len() - 1])
}

fn find_subdir_ci(dir: &Path, segment: &str) -> Option<PathBuf> {
    let exact = dir.join(segment);
    if exact.is_dir() && !is_system_path(&exact) {
        return Some(exact);
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten().take(MAX_DIR_SCAN) {
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|s| s.eq_ignore_ascii_case(segment))
        {
            let candidate = entry.path();
            if candidate.is_dir() && !is_system_path(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn find_regular_file_ci(dir: &Path, filename: &str) -> Option<PathBuf> {
    if let Some(exact) = try_regular_file(dir.join(filename)) {
        return Some(exact);
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten().take(MAX_DIR_SCAN) {
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|s| s.eq_ignore_ascii_case(filename))
            && let Some(found) = try_regular_file(entry.path())
        {
            return Some(found);
        }
    }
    None
}

fn find_bundle_index(dir: &Path) -> Option<PathBuf> {
    const INDEX_CANDIDATES: &[&str] = &[
        "_index.md",
        "index.md",
        "_index.markdown",
        "index.markdown",
        "_index.mdx",
        "index.mdx",
        "_index.html",
        "index.html",
        "_index.htm",
        "index.htm",
        "README.md",
        "readme.md",
    ];
    for name in INDEX_CANDIDATES {
        if let Some(found) = find_regular_file_ci(dir, name) {
            return Some(found);
        }
    }
    None
}

fn resolve_document_leaf(dir: &Path, leaf: &str) -> Option<PathBuf> {
    if is_system_path(dir) {
        return None;
    }
    if let Some(exact) = find_regular_file_ci(dir, leaf) {
        return Some(exact);
    }

    const DOC_EXTENSIONS: &[&str] = &["md", "markdown", "mdx", "html", "htm", "txt"];
    let stem = leaf
        .strip_suffix(".html")
        .or_else(|| leaf.strip_suffix(".HTML"))
        .or_else(|| leaf.strip_suffix(".htm"))
        .or_else(|| leaf.strip_suffix(".HTM"))
        .unwrap_or(leaf);
    for ext in DOC_EXTENSIONS {
        let candidate = format!("{stem}.{ext}");
        if let Some(found) = find_regular_file_ci(dir, &candidate) {
            return Some(found);
        }
    }

    if let Some(subdir) = find_subdir_ci(dir, stem) {
        return find_bundle_index(&subdir);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_hugo_static_assets_across_nested_content_dirs() {
        let root = tempfile::tempdir().unwrap();
        let nested_md = root
            .path()
            .join("content/Engineering/Networking/security.md");
        let bundle_md = root.path().join("content/projects/floppytron/index.md");
        std::fs::create_dir_all(nested_md.parent().unwrap()).unwrap();
        std::fs::create_dir_all(bundle_md.parent().unwrap()).unwrap();
        std::fs::write(&nested_md, "# Security\n").unwrap();
        std::fs::write(&bundle_md, "# Floppytron\n").unwrap();

        let static_img = root
            .path()
            .join("static/nonfree/permissiongranted/phyxius.webp");
        let space_img = root.path().join("static/design/my diagram.png");
        let asset_img = root.path().join("assets/diagrams/arch.svg");
        let bundle_img = root.path().join("content/projects/floppytron/floppy.jpg");
        std::fs::create_dir_all(static_img.parent().unwrap()).unwrap();
        std::fs::create_dir_all(space_img.parent().unwrap()).unwrap();
        std::fs::create_dir_all(asset_img.parent().unwrap()).unwrap();
        std::fs::write(&static_img, b"webp").unwrap();
        std::fs::write(&space_img, b"png").unwrap();
        std::fs::write(&asset_img, b"<svg/>").unwrap();
        std::fs::write(&bundle_img, b"jpg").unwrap();

        assert_eq!(
            resolve_document_asset(
                &nested_md,
                "/nonfree/permissiongranted/phyxius.webp?v=2#hero"
            ),
            Some(static_img.clone())
        );
        assert_eq!(
            resolve_document_asset(&nested_md, "nonfree/permissiongranted/phyxius.webp"),
            Some(static_img)
        );
        assert_eq!(
            resolve_document_asset(&nested_md, "/design/my%20diagram.png"),
            Some(space_img)
        );
        assert_eq!(
            resolve_document_asset(&nested_md, "/diagrams/arch.svg"),
            Some(asset_img)
        );
        assert_eq!(
            resolve_document_asset(&bundle_md, "floppy.jpg"),
            Some(bundle_img)
        );
    }

    #[test]
    fn rejects_external_schemes_and_traversal_escapes() {
        let root = tempfile::tempdir().unwrap();
        let md = root.path().join("site/content/post.md");
        std::fs::create_dir_all(md.parent().unwrap()).unwrap();
        std::fs::create_dir_all(root.path().join("site/static")).unwrap();
        std::fs::write(&md, "# Test\n").unwrap();
        let outside = root.path().join("secret.png");
        std::fs::write(&outside, b"png").unwrap();

        for blocked in [
            "https://example.com/a.png",
            "http://example.com/a.png",
            "file:///etc/passwd",
            "data:image/png;base64,AAAA",
            "javascript:alert(1)",
            "/../../../../etc/passwd",
            "/../../secret.png",
            "#section-anchor",
        ] {
            assert_eq!(
                resolve_document_asset(&md, blocked),
                None,
                "reference {blocked:?} must be rejected"
            );
        }
    }

    #[test]
    fn resolves_internal_hugo_and_relative_document_links_and_blocks_self_or_escapes() {
        let root = tempfile::tempdir().unwrap();
        let site = root.path().join("site");
        let content = site.join("content");
        let home = content.join("_index.md");
        let webdev = content.join("webdev.md");
        let eng_index = content.join("Engineering/_index.html");
        let eng_page = content.join("Engineering/Engineering.md");
        let plugin = content.join("Music/Software/plugin.md");
        let bundle = content.join("musiclog/uniquode/index.md");
        let toki = content.join("Other/Conlangs/tokipona.md");
        let outside = site.join("README.md");
        let parent_secret = root.path().join("secret.md");

        for p in [
            &home,
            &webdev,
            &eng_index,
            &eng_page,
            &plugin,
            &bundle,
            &toki,
            &outside,
            &parent_secret,
        ] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "# Doc\n").unwrap();
        }

        // Case-insensitive Hugo content routes, trailing slashes, anchors, and bundles.
        assert_eq!(
            resolve_document_link(&webdev, "/engineering/engineering"),
            Some(eng_page.clone())
        );
        assert_eq!(
            resolve_document_link(&webdev, "/engineering/#int"),
            Some(eng_index.clone())
        );
        assert_eq!(
            resolve_document_link(&webdev, "/music/software/plugin/"),
            Some(plugin.clone())
        );
        assert_eq!(
            resolve_document_link(&webdev, "/musiclog/uniquode/"),
            Some(bundle.clone())
        );
        assert_eq!(resolve_document_link(&webdev, "/"), Some(home.clone()));
        // Rootless `relref`-style paths resolve against `content/`.
        assert_eq!(
            resolve_document_link(&plugin, "Other/conlangs/tokipona#grammar"),
            Some(toki.clone())
        );
        // Relative navigation between docs inside the site.
        assert_eq!(
            resolve_document_link(&plugin, "../../Engineering/Engineering.md"),
            Some(eng_page.clone())
        );

        // Self-links, missing pages, external URLs, fragments, and site escapes return None.
        for non_navigable in [
            "/webdev",
            "/webdev/#section",
            "#section",
            "https://example.com/engineering",
            "mailto:user@example.com",
            "youtube.com",
            "/missing/page",
            "/../../../../etc/passwd",
            "../../../secret.md",
        ] {
            assert_eq!(
                resolve_document_link(&webdev, non_navigable),
                None,
                "reference {non_navigable:?} should not resolve internally"
            );
        }

        // Non-Hugo relative docs still resolve sibling and parent markdown files.
        let repo_dir = root.path().join("repo");
        let repo_readme = repo_dir.join("README.md");
        let repo_guide = repo_dir.join("docs/guide.md");
        std::fs::create_dir_all(repo_guide.parent().unwrap()).unwrap();
        std::fs::write(&repo_readme, "# Repo\n").unwrap();
        std::fs::write(&repo_guide, "# Guide\n").unwrap();
        assert_eq!(
            resolve_document_link(&repo_guide, "../README.md"),
            Some(repo_readme)
        );
        assert_eq!(resolve_document_link(&repo_guide, "guide.md"), None);
        assert_eq!(document_base_dir(Path::new("README.md")), Path::new("."));

        #[cfg(unix)]
        {
            let sys_link = repo_dir.join("docs/hosts.png");
            if std::path::Path::new("/etc/hosts").exists()
                && std::os::unix::fs::symlink("/etc/hosts", &sys_link).is_ok()
            {
                assert_eq!(resolve_document_asset(&repo_guide, "hosts.png"), None);
                assert_eq!(resolve_document_link(&repo_guide, "hosts.png"), None);
            }
        }
    }
}
