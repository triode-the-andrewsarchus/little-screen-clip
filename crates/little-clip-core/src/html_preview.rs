//! Inert, bounded HTML blocks shared by document readers. No scripts or resource fetching.
/// Inert document elements. Scripts, styles and embedded objects are omitted.
#[derive(Debug, PartialEq)]
pub enum Block {
    /// A heading and its level.
    Heading(u8, String),
    /// A paragraph of readable text.
    Text(String),
    /// A local image and its alternative text.
    Image(String, String),
}

/// Reads headings, paragraphs and caller-approved image references from HTML.
///
/// The resolver identifies local resources; this function never opens them.
///
/// # Errors
/// Rejects bodies exceeding byte, markup, node, nesting or display-block budgets.
pub fn parse(html: &str, resolve: impl Fn(&str) -> Option<String>) -> Result<Vec<Block>, String> {
    if html.len() > 2 * 1024 * 1024 || html.bytes().filter(|b| *b == b'<').count() > 16_384 {
        return Err("HTML exceeds the reader budget".into());
    }
    let doc = scraper::Html::parse_document(html);
    let root = doc.root_element();
    if root.descendants().take(100_001).count() > 100_000
        || root
            .descendants()
            .any(|node| node.ancestors().take(130).count() >= 130)
    {
        return Err("HTML is too large or deeply nested".into());
    }
    let body = root
        .descendent_elements()
        .find(|n| n.value().name() == "body")
        .ok_or("Missing HTML body")?;
    let mut blocks = Vec::new();
    content(body, &resolve, &mut blocks);
    if blocks.len() > 4096 {
        return Err("HTML has too many display blocks".into());
    }
    Ok(blocks)
}
fn html_text(node: scraper::ElementRef<'_>) -> String {
    node.descendants()
        .filter(|node| {
            !node
                .ancestors()
                .filter_map(scraper::ElementRef::wrap)
                .any(|parent| matches!(parent.value().name(), "script" | "style" | "head"))
        })
        .filter_map(|node| node.value().as_text())
        .map(|text| &**text)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn content(
    node: scraper::ElementRef<'_>,
    resolve: &impl Fn(&str) -> Option<String>,
    blocks: &mut Vec<Block>,
) {
    let name = node.value().name();
    match name {
        "script" | "style" | "iframe" | "object" | "audio" | "video" => (),
        "img" => {
            if let Some(src) = node.attr("src").and_then(|src| resolve(src)) {
                blocks.push(Block::Image(src, node.attr("alt").unwrap_or("").into()));
            }
        }
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            blocks.push(Block::Heading(name.as_bytes()[1] - b'0', html_text(node)))
        }
        "p" | "li" | "pre" | "blockquote" | "dt" | "dd" | "figcaption" => {
            let value = html_text(node);
            if !value.is_empty() {
                blocks.push(Block::Text(value));
            }
            for image in node
                .descendent_elements()
                .filter(|n| n.value().name() == "img")
            {
                content(image, resolve, blocks);
            }
        }
        _ => {
            for child in node.children() {
                if let Some(element) = scraper::ElementRef::wrap(child) {
                    content(element, resolve, blocks);
                } else if let Some(text) = child.value().as_text() {
                    if !text.trim().is_empty() {
                        blocks.push(Block::Text(text.trim().into()));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_html_is_inert_and_resources_need_explicit_resolution() {
        let html = "<h1>Heading</h1><p>sun<strong>lit</strong> &amp; sea<style>hidden</style><script>hidden</script><img src='file:///etc/passwd'>";
        let blocks = parse(html, |_| None).unwrap();
        assert!(blocks.contains(&Block::Heading(1, "Heading".into())));
        assert!(blocks.contains(&Block::Text("sunlit & sea".into())));
        assert!(!format!("{blocks:?}").contains("passwd"));
    }
    #[test]
    fn display_limits_reject_excessive_depth_and_markup() {
        let deep = format!("{}x{}", "<div>".repeat(140), "</div>".repeat(140));
        assert!(parse(&deep, |_| None).is_err());
        assert!(parse(&"<p>x</p>".repeat(9000), |_| None).is_err());
    }
}
