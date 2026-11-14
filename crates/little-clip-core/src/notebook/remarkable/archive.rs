//! Lazy reMarkable notebook indexing; document metadata, never ZIP enumeration, orders pages.
use super::package::Package;
use serde_json::Value;
use std::{
    collections::HashSet,
    io::{Read, Seek},
    sync::atomic::AtomicBool,
};

const MAX_METADATA: usize = 1024 * 1024;

/// A live page's source annotation and background references, in authored order.
#[derive(Debug)]
pub struct Page {
    pub id: String,
    pub template: Option<String>,
    /// One-based source PDF page; `None` means an inserted notebook page.
    pub pdf_page: Option<u32>,
}

/// Archive directory and small JSON snapshot, without decoded ink or backgrounds.
#[derive(Debug)]
pub struct Index {
    pub root: String,
    pub title: String,
    pub file_type: String,
    pub pages: Vec<Page>,
}

/// Indexes a bounded notebook export without extracting archive entries to disk.
///
/// # Errors
/// Rejects ambiguous roots, unsupported document types, malformed metadata,
/// unsafe page IDs, duplicate live pages and entry/page/source-size limits.
pub fn inspect(input: &mut (impl Read + Seek), cancel: &AtomicBool) -> Result<Index, String> {
    let mut package = Package::open(input, cancel)?;
    from_package(&mut package, cancel)
}

pub(super) fn from_package<R: Read + Seek>(
    package: &mut Package<R>,
    cancel: &AtomicBool,
) -> Result<Index, String> {
    let roots: Vec<_> = package
        .names()
        .filter_map(|name| name.strip_suffix(".metadata"))
        .filter(|name| component(name))
        .map(str::to_owned)
        .collect();
    let [root] = roots.as_slice() else {
        return Err("Expected one reMarkable document root".into());
    };
    let metadata: Value = serde_json::from_slice(
        &package
            .read(&format!("{root}.metadata"), MAX_METADATA, cancel)?
            .ok_or("Missing reMarkable document metadata")?,
    )
    .map_err(|error| format!("Invalid reMarkable metadata: {error}"))?;
    if metadata.get("type").and_then(Value::as_str) != Some("DocumentType") {
        return Err("Not a reMarkable document archive".into());
    }
    let content: Value = serde_json::from_slice(
        &package
            .read(&format!("{root}.content"), MAX_METADATA, cancel)?
            .ok_or("Missing reMarkable document content")?,
    )
    .map_err(|error| format!("Invalid reMarkable content: {error}"))?;
    let file_type = match content.get("fileType") {
        None => "",
        Some(value) => value.as_str().ok_or("Invalid reMarkable document type")?,
    };
    if !matches!(file_type, "" | "notebook" | "pdf" | "epub") {
        return Err(format!("Unsupported reMarkable document type: {file_type}"));
    }
    let mut pages = page_plan(&content, matches!(file_type, "pdf" | "epub"))?;
    if content.get("cPages").is_none() {
        legacy_templates(package, root, &mut pages, cancel)?;
    }

    Ok(Index {
        root: root.clone(),
        title: metadata
            .get("visibleName")
            .and_then(Value::as_str)
            .unwrap_or(root)
            .to_owned(),
        file_type: file_type.to_owned(),
        pages,
    })
}

fn legacy_templates<R: Read + Seek>(
    package: &mut Package<R>,
    root: &str,
    pages: &mut [Page],
    cancel: &AtomicBool,
) -> Result<(), String> {
    if let Some(data) = package.read(&format!("{root}.pagedata"), MAX_METADATA, cancel)? {
        let data = std::str::from_utf8(&data).map_err(|_| "Invalid reMarkable pagedata")?;
        for (page, template) in pages.iter_mut().zip(data.lines()) {
            if !matches!(template, "" | "Blank") {
                page.template = Some(template.to_owned());
            }
        }
    }
    Ok(())
}

fn component(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn page_plan(content: &Value, has_pdf: bool) -> Result<Vec<Page>, String> {
    let mut pages = if let Some(modern) = content.get("cPages") {
        modern_pages(modern)?
    } else {
        legacy_pages(content, has_pdf)?
    };
    if pages.is_empty() || pages.len() > 4096 {
        return Err("reMarkable page count is outside 1..=4096".into());
    }
    let mut ids = HashSet::new();
    for page in &mut pages {
        if !component(&page.id) || !ids.insert(page.id.clone()) {
            return Err("Invalid or duplicate reMarkable page identifier".into());
        }
        if page.template.as_deref() == Some("Blank") {
            page.template = None;
        }
    }
    Ok(pages)
}

fn register<'a>(record: &'a Value, name: &str) -> Result<Option<&'a Value>, String> {
    let Some(value) = record.get(name).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    value
        .get("value")
        .map(Some)
        .ok_or_else(|| format!("Invalid reMarkable {name} register"))
}

fn modern_pages(modern: &Value) -> Result<Vec<Page>, String> {
    let records = modern
        .get("pages")
        .and_then(Value::as_array)
        .ok_or("Missing reMarkable cPages page array")?;
    if records.len() > 8192 {
        return Err("reMarkable page records exceed the preview limit".into());
    }
    let mut pages = Vec::new();
    for record in records {
        if let Some(page) = modern_page(record)? {
            pages.push(page);
        }
    }

    Ok(pages)
}

fn modern_page(record: &Value) -> Result<Option<Page>, String> {
    let deleted = register(record, "deleted")?
        .map(|value| {
            value
                .as_i64()
                .ok_or("Invalid reMarkable page deletion flag")
        })
        .transpose()?
        .unwrap_or(0);
    match deleted {
        0 => {}
        1 => return Ok(None),
        _ => return Err("Unsupported reMarkable page deletion flag".into()),
    }

    let id = record
        .get("id")
        .and_then(Value::as_str)
        .ok_or("Missing reMarkable page identifier")?;
    let pdf_page = register(record, "redir")?
        .map(pdf_reference)
        .transpose()?
        .flatten();
    Ok(Some(Page {
        id: id.to_owned(),
        pdf_page,
        template: register(record, "template")?
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or("Invalid reMarkable template reference")
            })
            .transpose()?,
    }))
}

fn legacy_pages(content: &Value, has_pdf: bool) -> Result<Vec<Page>, String> {
    let declared = content
        .get("pageCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if declared > 4096 {
        return Err("reMarkable page count exceeds the preview limit".into());
    }
    let records = content
        .get("pages")
        .map(|value| {
            value
                .as_array()
                .ok_or("Invalid legacy reMarkable page array")
        })
        .transpose()?;
    let count = records.map_or(declared as usize, Vec::len);
    if count > 4096 {
        return Err("reMarkable page count exceeds the preview limit".into());
    }
    let redirections = content
        .get("redirectionPageMap")
        .map(|value| {
            value
                .as_array()
                .ok_or("Invalid legacy reMarkable PDF redirection array")
        })
        .transpose()?;
    (0..count)
        .map(|index| {
            let id = match records.and_then(|records| records.get(index)) {
                None => index.to_string(),
                Some(Value::String(id)) => id.clone(),
                Some(record) => record
                    .get("id")
                    .or_else(|| record.get("uuid"))
                    .and_then(Value::as_str)
                    .ok_or("Invalid legacy reMarkable page identifier")?
                    .to_owned(),
            };
            let pdf_page = if has_pdf {
                redirections
                    .and_then(|values| values.get(index))
                    .map(pdf_reference)
                    .transpose()?
                    .unwrap_or(Some(index as u32 + 1))
            } else {
                None
            };
            Ok(Page {
                id,
                pdf_page,
                template: None,
            })
        })
        .collect()
}

fn pdf_reference(value: &Value) -> Result<Option<u32>, String> {
    match value.as_i64() {
        Some(-1) => Ok(None),
        Some(page @ 0..=4095) => Ok(Some(page as u32 + 1)),
        _ => Err("Invalid reMarkable source PDF page reference".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::container::zip_member;
    use serde_json::json;
    use std::{
        io::{Cursor, Write},
        path::Path,
    };

    #[test]
    fn real_export_keeps_live_page_order_and_template_references() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/examples/notebooks/remarkable/archives/Test.rmdoc");
        let mut source = std::fs::File::open(path).unwrap();
        let index = inspect(&mut source, &AtomicBool::new(false)).unwrap();
        assert_eq!(index.title, "Test");
        assert_eq!(index.pages.len(), 2);
        assert_eq!(index.pages[0].id, "df87211e-12cc-46f7-9681-9840e995bea3");
        assert_eq!(index.pages[1].id, "e2f8b50f-3f86-46ed-aee7-81c2cf31a619");
        assert_eq!(index.pages[1].template.as_deref(), Some("P Dots S"));
        assert!(index.pages.iter().all(|page| page.pdf_page.is_none()));
        // The archive's raw pages must qualify independently of its filenames.
        let mut zip = zip::ZipArchive::new(&mut source).unwrap();
        for page in index.pages {
            let bytes = zip_member(
                &mut zip,
                &format!("{}/{}.rm", index.root, page.id),
                super::super::MAX_PAGE_BYTES,
                &AtomicBool::new(false),
            )
            .unwrap();
            super::super::parse(&bytes, &AtomicBool::new(false)).unwrap();
        }
    }

    #[test]
    fn authored_order_deletions_and_pdf_redirection_are_preserved() {
        let plan = page_plan(
            &json!({"cPages":{"pages":[
                {"id":"z","redir":{"value":4}},
                {"id":"gone","deleted":{"value":1}},
                {"id":"a","deleted":{"value":0},"template":{"value":"Blank"}}
            ]}}),
            true,
        )
        .unwrap();
        assert_eq!(
            plan.iter().map(|page| page.id.as_str()).collect::<Vec<_>>(),
            ["z", "a"]
        );
        assert_eq!(plan[0].pdf_page, Some(5));
        assert_eq!(plan[1].pdf_page, None);
        let legacy = page_plan(
            &json!({"pages":["b", "a"],"redirectionPageMap":[2,-1]}),
            true,
        )
        .unwrap();
        assert_eq!(legacy[0].pdf_page, Some(3));
        assert_eq!(legacy[1].pdf_page, None);
        assert!(page_plan(&json!({"cPages":{"pages":[]},"pages":["stale"]}), false).is_err());
        assert!(page_plan(&json!({"pages":["../outside"]}), false).is_err());
        assert!(page_plan(&json!({"pages":["same","same"]}), false).is_err());
        assert!(page_plan(&json!({"pageCount":u64::MAX}), true).is_err());
        assert!(page_plan(&json!({"pages":true,"pageCount":1}), false).is_err());
        assert!(
            page_plan(
                &json!({"cPages":{"pages":[{"id":"p0","deleted":{"value":"yes"}}]}}),
                false
            )
            .is_err()
        );
        assert!(
            page_plan(
                &json!({"cPages":{"pages":[{"id":"p0","redir":{"value":-2}}]}}),
                true
            )
            .is_err()
        );
    }

    #[test]
    fn ordinary_zip_and_directory_metadata_do_not_qualify_as_notebooks() {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("normal.metadata", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(br#"{"type":"CollectionType"}"#).unwrap();
        let mut bytes = zip.finish().unwrap();
        assert!(inspect(&mut bytes, &AtomicBool::new(false)).is_err());
    }
}
