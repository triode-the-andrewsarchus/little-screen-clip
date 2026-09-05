use base64::Engine;
use little_clip_core::{ClipItem, ClipPayload};
use std::collections::HashSet;
use thiserror::Error;

pub mod pipeline;
pub use pipeline::*;

pub mod diff;
pub use diff::*;

pub mod image_diff;
pub use image_diff::*;

pub mod binary_diff;
pub use binary_diff::*;


#[derive(Debug, Error)]
pub enum OpError {
    #[error("Incompatible payload: {0}")]
    IncompatiblePayload(String),
    #[error("Operation execution failed: {0}")]
    ExecutionFailed(String),
}

#[derive(Debug, Clone)]
pub struct OpDescriptor {
    pub name: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub is_binary: bool,
    pub category: &'static str,
}

pub const AVAILABLE_OPS: &[OpDescriptor] = &[
    OpDescriptor {
        name: "trim",
        label: "Trim Whitespace",
        description: "Trim leading/trailing whitespace & collapse multiple newlines",
        is_binary: false,
        category: "Text",
    },
    OpDescriptor {
        name: "clean-url",
        label: "Clean URL Tracking",
        description: "Strip utm_*, fbclid, si and other tracking parameters from URL",
        is_binary: false,
        category: "URL",
    },
    OpDescriptor {
        name: "json-format",
        label: "Format / Minify JSON",
        description: "Toggle pretty-printed vs compact minified JSON",
        is_binary: false,
        category: "Text",
    },
    OpDescriptor {
        name: "snake-case",
        label: "Convert to snake_case",
        description: "Convert string to snake_case format",
        is_binary: false,
        category: "Text",
    },
    OpDescriptor {
        name: "camel-case",
        label: "Convert to camelCase",
        description: "Convert string to camelCase format",
        is_binary: false,
        category: "Text",
    },
    OpDescriptor {
        name: "kebab-case",
        label: "Convert to kebab-case",
        description: "Convert string to kebab-case format",
        is_binary: false,
        category: "Text",
    },
    OpDescriptor {
        name: "uppercase",
        label: "Convert to UPPERCASE",
        description: "Convert all characters to uppercase",
        is_binary: false,
        category: "Text",
    },
    OpDescriptor {
        name: "lowercase",
        label: "Convert to lowercase",
        description: "Convert all characters to lowercase",
        is_binary: false,
        category: "Text",
    },
    OpDescriptor {
        name: "sort-lines",
        label: "Sort Lines",
        description: "Sort lines alphabetically",
        is_binary: false,
        category: "Text",
    },
    OpDescriptor {
        name: "dedupe-lines",
        label: "Deduplicate Lines",
        description: "Remove duplicate lines preserving order",
        is_binary: false,
        category: "Text",
    },
    OpDescriptor {
        name: "slugify",
        label: "Slugify Text",
        description: "Convert string to URL-safe hyphenated slug",
        is_binary: false,
        category: "Text",
    },
    OpDescriptor {
        name: "base64-encode",
        label: "Base64 Encode",
        description: "Encode text to standard Base64",
        is_binary: false,
        category: "Text",
    },
    OpDescriptor {
        name: "base64-decode",
        label: "Base64 Decode",
        description: "Decode Base64 string to plaintext",
        is_binary: false,
        category: "Text",
    },
    // Binary operations
    OpDescriptor {
        name: "concat",
        label: "Concatenate Items",
        description: "Join Hand item with selected Slot item",
        is_binary: true,
        category: "Multi-Slot",
    },
    OpDescriptor {
        name: "concat-space",
        label: "Join with Space",
        description: "Join items with a single space",
        is_binary: true,
        category: "Multi-Slot",
    },
    OpDescriptor {
        name: "concat-comma",
        label: "Join with Comma",
        description: "Join items with comma and space (, )",
        is_binary: true,
        category: "Multi-Slot",
    },
    OpDescriptor {
        name: "concat-reversed",
        label: "Concatenate (Reversed)",
        description: "Join selected Slot item followed by Hand item",
        is_binary: true,
        category: "Multi-Slot",
    },
    OpDescriptor {
        name: "diff",
        label: "Diff Comparison (Text & Image)",
        description: "Interactive visual text & picture diffing between slots, clipboard & files",
        is_binary: true,
        category: "Compare",
    },
    OpDescriptor {
        name: "binary-diff",
        label: "Binary Diff (Hex Alignment)",
        description: "Side-by-side hex and ASCII sequence alignment diff between any two items or files",
        is_binary: true,
        category: "Compare",
    },
    // Multi-item operations (3+ slots)
    OpDescriptor {
        name: "concat-all",
        label: "Concatenate All",
        description: "Join all selected items with newlines",
        is_binary: true,
        category: "Multi-Slot",
    },
    OpDescriptor {
        name: "join-spaces",
        label: "Join All with Spaces",
        description: "Join all selected items with space separator",
        is_binary: true,
        category: "Multi-Slot",
    },
    OpDescriptor {
        name: "join-commas",
        label: "Join All with Commas",
        description: "Join all selected items with comma separator (, )",
        is_binary: true,
        category: "Multi-Slot",
    },
    OpDescriptor {
        name: "sort-and-merge",
        label: "Merge & Sort All Lines",
        description: "Combine all items and sort lines alphabetically",
        is_binary: true,
        category: "Multi-Slot",
    },
    OpDescriptor {
        name: "dedupe-and-merge",
        label: "Merge & Deduplicate Lines",
        description: "Combine all items and remove duplicate lines",
        is_binary: true,
        category: "Multi-Slot",
    },
];

pub fn get_applicable_ops(items: &[&ClipItem]) -> Vec<OpDescriptor> {
    if items.is_empty() {
        return Vec::new();
    }

    if items.len() == 1 {
        let item = items[0];
        let mut ops = Vec::new();

        if let Ok(text) = get_item_text(item) {
            let trimmed = text.trim();

            // 1. URL-specific
            let is_url = matches!(item.payload, ClipPayload::Url(_))
                || trimmed.starts_with("http://")
                || trimmed.starts_with("https://");

            if is_url {
                ops.push(OpDescriptor {
                    name: "clean-url",
                    label: "Clean URL Tracking",
                    description: "Strip utm_*, fbclid, si and tracking parameters from URL",
                    is_binary: false,
                    category: "URL",
                });
            }

            // 2. JSON-specific
            let is_json = (trimmed.starts_with('{') && trimmed.ends_with('}'))
                || (trimmed.starts_with('[') && trimmed.ends_with(']'));
            if is_json && serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
                ops.push(OpDescriptor {
                    name: "json-format",
                    label: "Format / Minify JSON",
                    description: "Toggle pretty-printed vs compact minified JSON",
                    is_binary: false,
                    category: "JSON",
                });
            }

            // 3. Text Operations
            ops.push(OpDescriptor {
                name: "trim",
                label: "Trim Whitespace",
                description: "Trim leading/trailing whitespace & empty lines",
                is_binary: false,
                category: "Text",
            });
            ops.push(OpDescriptor {
                name: "uppercase",
                label: "Convert to UPPERCASE",
                description: "Convert all characters to uppercase",
                is_binary: false,
                category: "Text",
            });
            ops.push(OpDescriptor {
                name: "lowercase",
                label: "Convert to lowercase",
                description: "Convert all characters to lowercase",
                is_binary: false,
                category: "Text",
            });
            ops.push(OpDescriptor {
                name: "snake-case",
                label: "Convert to snake_case",
                description: "Convert string to snake_case format",
                is_binary: false,
                category: "Text",
            });
            ops.push(OpDescriptor {
                name: "camel-case",
                label: "Convert to camelCase",
                description: "Convert string to camelCase format",
                is_binary: false,
                category: "Text",
            });
            ops.push(OpDescriptor {
                name: "kebab-case",
                label: "Convert to kebab-case",
                description: "Convert string to kebab-case format",
                is_binary: false,
                category: "Text",
            });
            ops.push(OpDescriptor {
                name: "slugify",
                label: "Slugify Text",
                description: "Convert string to URL-safe hyphenated slug",
                is_binary: false,
                category: "Text",
            });

            if text.contains('\n') {
                ops.push(OpDescriptor {
                    name: "sort-lines",
                    label: "Sort Lines",
                    description: "Sort lines alphabetically",
                    is_binary: false,
                    category: "Text",
                });
                ops.push(OpDescriptor {
                    name: "dedupe-lines",
                    label: "Deduplicate Lines",
                    description: "Remove duplicate lines preserving order",
                    is_binary: false,
                    category: "Text",
                });
            }

            ops.push(OpDescriptor {
                name: "base64-encode",
                label: "Base64 Encode",
                description: "Encode text to standard Base64",
                is_binary: false,
                category: "Text",
            });

            ops.push(OpDescriptor {
                name: "diff",
                label: "Diff Comparison (Difftastic)",
                description: "Interactive structural syntax diff against Right Hand or Prior slot",
                is_binary: true,
                category: "Compare",
            });

            if base64::engine::general_purpose::STANDARD.decode(trimmed).is_ok() && trimmed.len() > 3 {
                ops.push(OpDescriptor {
                    name: "base64-decode",
                    label: "Base64 Decode",
                    description: "Decode Base64 string to plaintext",
                    is_binary: false,
                    category: "Text",
                });
            }
        }

        if resolve_diff_target(item).map(|t| t.is_image()).unwrap_or(false) {
            ops.push(OpDescriptor {
                name: "diff",
                label: "Diff Comparison (Picture Diff)",
                description: "Interactive visual image diff against Right Hand or Prior slot",
                is_binary: true,
                category: "Compare",
            });
        }

        // Binary diff is always applicable for any item against secondary slot or paired files
        ops.push(OpDescriptor {
            name: "binary-diff",
            label: "Binary Diff (Hex Alignment)",
            description: "Interactive side-by-side hex & ASCII alignment diff against secondary slot or files",
            is_binary: true,
            category: "Compare",
        });

        return ops;
    }

    if items.len() == 2 {
        let mut ops = Vec::new();

        let t0 = resolve_diff_target(items[0]);
        let t1 = resolve_diff_target(items[1]);
        if let (Some(t0), Some(t1)) = (t0, t1) {
            if t0.is_image() && t1.is_image() {
                ops.push(OpDescriptor {
                    name: "diff",
                    label: "Picture Diff Comparison",
                    description: "ODiff-style perceptual visual diff highlighting changed pixels",
                    is_binary: true,
                    category: "Compare",
                });
            }
        }

        let all_text = items.iter().all(|i| get_item_text(i).is_ok());
        if all_text {
            ops.extend_from_slice(&[
                OpDescriptor {
                    name: "concat",
                    label: "Concatenate Items",
                    description: "Join items with newline separator",
                    is_binary: true,
                    category: "Combine",
                },
                OpDescriptor {
                    name: "concat-space",
                    label: "Join with Space",
                    description: "Join items with single space",
                    is_binary: true,
                    category: "Combine",
                },
                OpDescriptor {
                    name: "concat-comma",
                    label: "Join with Comma",
                    description: "Join items with comma and space (, )",
                    is_binary: true,
                    category: "Combine",
                },
                OpDescriptor {
                    name: "concat-reversed",
                    label: "Concatenate (Reversed)",
                    description: "Join second item followed by first item",
                    is_binary: true,
                    category: "Combine",
                },
                OpDescriptor {
                    name: "diff",
                    label: "Diff Comparison",
                    description: "Generate unified diff comparing the two items",
                    is_binary: true,
                    category: "Compare",
                },
            ]);
        }

        // Any 2 items can be binary diffed!
        ops.push(OpDescriptor {
            name: "binary-diff",
            label: "Binary Diff (Hex Alignment)",
            description: "Side-by-side hex and ASCII sequence alignment diff comparing raw bytes",
            is_binary: true,
            category: "Compare",
        });

        return ops;
    }

    if items.len() >= 3 {
        let all_text = items.iter().all(|i| get_item_text(i).is_ok());
        if all_text {
            return vec![
                OpDescriptor {
                    name: "concat-all",
                    label: "Concatenate All",
                    description: "Join all selected items with newlines",
                    is_binary: true,
                    category: "Combine",
                },
                OpDescriptor {
                    name: "join-spaces",
                    label: "Join All with Spaces",
                    description: "Join all selected items with space separator",
                    is_binary: true,
                    category: "Combine",
                },
                OpDescriptor {
                    name: "join-commas",
                    label: "Join All with Commas",
                    description: "Join all selected items with comma separator (, )",
                    is_binary: true,
                    category: "Combine",
                },
                OpDescriptor {
                    name: "sort-and-merge",
                    label: "Merge & Sort All Lines",
                    description: "Combine all items and sort all lines alphabetically",
                    is_binary: true,
                    category: "Combine",
                },
                OpDescriptor {
                    name: "dedupe-and-merge",
                    label: "Merge & Deduplicate Lines",
                    description: "Combine all items and remove duplicate lines",
                    is_binary: true,
                    category: "Combine",
                },
                OpDescriptor {
                    name: "concat-reversed",
                    label: "Concatenate All (Reversed)",
                    description: "Join all selected items in reverse order",
                    is_binary: true,
                    category: "Combine",
                },
            ];
        }
    }

    Vec::new()
}

pub fn execute_unary_op(op_name: &str, item: &ClipItem) -> Result<ClipItem, OpError> {
    match op_name {
        "trim" => op_trim(item),
        "clean-url" => op_clean_url(item),
        "json-format" => op_json_format(item),
        "snake-case" => op_case_transform(item, Case::Snake),
        "camel-case" => op_case_transform(item, Case::Camel),
        "kebab-case" => op_case_transform(item, Case::Kebab),
        "uppercase" => op_case_transform(item, Case::Upper),
        "lowercase" => op_case_transform(item, Case::Lower),
        "sort-lines" => op_sort_lines(item),
        "dedupe-lines" => op_dedupe_lines(item),
        "slugify" => op_slugify(item),
        "base64-encode" => op_base64_encode(item),
        "base64-decode" => op_base64_decode(item),
        "binary-diff" => op_binary_diff_unary(item),
        _ => Err(OpError::ExecutionFailed(format!("Unknown unary operation: {}", op_name))),
    }
}

fn op_binary_diff_unary(item: &ClipItem) -> Result<ClipItem, OpError> {
    if let Some(((l_lbl, l_bytes), (r_lbl, r_bytes))) = resolve_two_files_from_single_item(item) {
        let res = compute_binary_diff(&l_lbl, &l_bytes, &r_lbl, &r_bytes, BinaryDiffMode::Aligned, 16);
        let summary = format!(
            "Binary Diff: {} ({} B) vs {} ({} B)\nSimilarity: {:.1}%\nMatches: {} B | Mismatches: {} B | Insertions: {} B | Deletions: {} B",
            l_lbl, l_bytes.len(), r_lbl, r_bytes.len(),
            res.stats.similarity_pct,
            res.stats.match_count,
            res.stats.mismatch_count,
            res.stats.right_only_count,
            res.stats.left_only_count,
        );
        let mut out = ClipItem::from_text(summary);
        out.title = format!("Binary Diff: {} vs {}", l_lbl, r_lbl);
        out.subtitle = format!("{:.1}% match • {} diff rows", res.stats.similarity_pct, res.diff_row_indices.len());
        out.handler_tag = "BINARY-DIFF".to_string();
        Ok(out)
    } else {
        Err(OpError::ExecutionFailed("Item does not contain two files or paths to diff".to_string()))
    }
}

pub fn execute_binary_op(
    op_name: &str,
    hand: &ClipItem,
    slot: &ClipItem,
    separator: Option<&str>,
) -> Result<ClipItem, OpError> {
    match op_name {
        "concat" => op_concat(hand, slot, separator.unwrap_or("\n")),
        "concat-space" => op_concat(hand, slot, " "),
        "concat-comma" => op_concat(hand, slot, ", "),
        "concat-reversed" => op_concat(slot, hand, separator.unwrap_or("\n")),
        "diff" => op_diff(hand, slot),
        "binary-diff" => op_binary_diff(hand, slot),
        _ => Err(OpError::ExecutionFailed(format!("Unknown binary operation: {}", op_name))),
    }
}

fn op_binary_diff(a: &ClipItem, b: &ClipItem) -> Result<ClipItem, OpError> {
    let (l_lbl, l_bytes) = get_item_binary_bytes(a);
    let (r_lbl, r_bytes) = get_item_binary_bytes(b);
    let res = compute_binary_diff(&l_lbl, &l_bytes, &r_lbl, &r_bytes, BinaryDiffMode::Aligned, 16);
    let summary = format!(
        "Binary Diff: {} ({} B) vs {} ({} B)\nSimilarity: {:.1}%\nMatches: {} B | Mismatches: {} B | Insertions: {} B | Deletions: {} B",
        l_lbl, l_bytes.len(), r_lbl, r_bytes.len(),
        res.stats.similarity_pct,
        res.stats.match_count,
        res.stats.mismatch_count,
        res.stats.right_only_count,
        res.stats.left_only_count,
    );
    let mut item = ClipItem::from_text(summary);
    item.title = format!("Binary Diff: {} vs {}", l_lbl, r_lbl);
    item.subtitle = format!("{:.1}% match • {} diff rows", res.stats.similarity_pct, res.diff_row_indices.len());
    item.handler_tag = "BINARY-DIFF".to_string();
    Ok(item)
}

pub fn execute_multi_op(
    op_name: &str,
    items: &[&ClipItem],
    separator: Option<&str>,
) -> Result<ClipItem, OpError> {
    if items.is_empty() {
        return Err(OpError::ExecutionFailed("No items provided".to_string()));
    }
    if items.len() == 1 {
        return execute_unary_op(op_name, items[0]);
    }
    if items.len() == 2 && (op_name == "concat" || op_name == "concat-space" || op_name == "concat-comma" || op_name == "concat-reversed" || op_name == "diff" || op_name == "binary-diff") {
        return execute_binary_op(op_name, items[0], items[1], separator);
    }

    match op_name {
        "concat-all" | "concat" => {
            let mut texts = Vec::new();
            for item in items {
                texts.push(get_item_text(item)?);
            }
            Ok(ClipItem::from_text(texts.join(separator.unwrap_or("\n"))))
        }
        "join-spaces" => {
            let mut texts = Vec::new();
            for item in items {
                texts.push(get_item_text(item)?);
            }
            Ok(ClipItem::from_text(texts.join(" ")))
        }
        "join-commas" => {
            let mut texts = Vec::new();
            for item in items {
                texts.push(get_item_text(item)?);
            }
            Ok(ClipItem::from_text(texts.join(", ")))
        }
        "concat-reversed" => {
            let mut texts = Vec::new();
            for item in items.iter().rev() {
                texts.push(get_item_text(item)?);
            }
            Ok(ClipItem::from_text(texts.join(separator.unwrap_or("\n"))))
        }
        "sort-and-merge" => {
            let mut lines = Vec::new();
            for item in items {
                let text = get_item_text(item)?;
                lines.extend(text.lines().map(|l| l.to_string()));
            }
            lines.sort();
            Ok(ClipItem::from_text(lines.join("\n")))
        }
        "dedupe-and-merge" => {
            let mut seen = HashSet::new();
            let mut unique_lines = Vec::new();
            for item in items {
                let text = get_item_text(item)?;
                for line in text.lines() {
                    if seen.insert(line.to_string()) {
                        unique_lines.push(line.to_string());
                    }
                }
            }
            Ok(ClipItem::from_text(unique_lines.join("\n")))
        }
        _ => Err(OpError::ExecutionFailed(format!("Unknown multi-item operation: {}", op_name))),
    }
}

fn get_item_text(item: &ClipItem) -> Result<String, OpError> {
    match &item.payload {
        ClipPayload::Text(t) => Ok(t.clone()),
        ClipPayload::Markdown(m) => Ok(m.clone()),
        ClipPayload::Url(u) => Ok(u.to_string()),
        ClipPayload::Color { hex, .. } => Ok(hex.clone()),
        ClipPayload::MultiImage { paths, .. } => Ok(paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("\n")),
        ClipPayload::FileBatch(files) => Ok(files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("\n")),
        _ => Err(OpError::IncompatiblePayload(
            "Operation requires textual payload".to_string(),
        )),
    }
}

fn op_trim(item: &ClipItem) -> Result<ClipItem, OpError> {
    let text = get_item_text(item)?;
    let mut lines: Vec<String> = text.lines().map(|l| l.trim_end().to_string()).collect();

    // Remove empty leading/trailing lines
    while lines.first().map(|s| s.is_empty()).unwrap_or(false) {
        lines.remove(0);
    }
    while lines.last().map(|s| s.is_empty()).unwrap_or(false) {
        lines.pop();
    }

    let result = lines.join("\n");
    Ok(ClipItem::from_text(result))
}

fn op_clean_url(item: &ClipItem) -> Result<ClipItem, OpError> {
    let url_str = get_item_text(item)?;
    let mut parsed = url::Url::parse(url_str.trim())
        .map_err(|e| OpError::ExecutionFailed(format!("Invalid URL: {}", e)))?;

    let tracking_keys: HashSet<&str> = [
        "utm_source",
        "utm_medium",
        "utm_campaign",
        "utm_term",
        "utm_content",
        "fbclid",
        "gclid",
        "si",
        "ref",
        "ref_src",
        "igshid",
        "_hsenc",
        "_hsmi",
        "mc_cid",
        "mc_eid",
    ]
    .into_iter()
    .collect();

    let clean_pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, _)| !tracking_keys.contains(k.as_ref()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    parsed.set_query(None);
    if !clean_pairs.is_empty() {
        let mut serializer = parsed.query_pairs_mut();
        for (k, v) in clean_pairs {
            serializer.append_pair(&k, &v);
        }
    }

    Ok(ClipItem::from_url(parsed))
}

fn op_json_format(item: &ClipItem) -> Result<ClipItem, OpError> {
    let text = get_item_text(item)?;
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| OpError::ExecutionFailed(format!("Invalid JSON: {}", e)))?;

    // If currently contains newlines, minify it. Otherwise, pretty-print.
    let output = if text.contains('\n') {
        serde_json::to_string(&parsed)
    } else {
        serde_json::to_string_pretty(&parsed)
    }
    .map_err(|e| OpError::ExecutionFailed(e.to_string()))?;

    Ok(ClipItem::from_text(output))
}

enum Case {
    Snake,
    Camel,
    Kebab,
    Upper,
    Lower,
}

fn op_case_transform(item: &ClipItem, case: Case) -> Result<ClipItem, OpError> {
    let text = get_item_text(item)?;
    let words: Vec<String> = text
        .split(|c: char| c.is_whitespace() || c == '_' || c == '-')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    let transformed = match case {
        Case::Upper => text.to_uppercase(),
        Case::Lower => text.to_lowercase(),
        Case::Snake => words.iter().map(|w| w.to_lowercase()).collect::<Vec<_>>().join("_"),
        Case::Kebab => words.iter().map(|w| w.to_lowercase()).collect::<Vec<_>>().join("-"),
        Case::Camel => {
            let mut res = String::new();
            for (i, w) in words.iter().enumerate() {
                if i == 0 {
                    res.push_str(&w.to_lowercase());
                } else {
                    let mut chars = w.chars();
                    if let Some(first) = chars.next() {
                        res.extend(first.to_uppercase());
                        res.push_str(&chars.as_str().to_lowercase());
                    }
                }
            }
            res
        }
    };

    Ok(ClipItem::from_text(transformed))
}

fn op_sort_lines(item: &ClipItem) -> Result<ClipItem, OpError> {
    let text = get_item_text(item)?;
    let mut lines: Vec<&str> = text.lines().collect();
    lines.sort();
    Ok(ClipItem::from_text(lines.join("\n")))
}

fn op_dedupe_lines(item: &ClipItem) -> Result<ClipItem, OpError> {
    let text = get_item_text(item)?;
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for line in text.lines() {
        if seen.insert(line) {
            result.push(line);
        }
    }
    Ok(ClipItem::from_text(result.join("\n")))
}

fn op_slugify(item: &ClipItem) -> Result<ClipItem, OpError> {
    let text = get_item_text(item)?;
    let slug: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();

    let mut clean_slug = String::new();
    let mut last_was_dash = false;
    for c in slug.trim_matches('-').chars() {
        if c == '-' {
            if !last_was_dash {
                clean_slug.push(c);
                last_was_dash = true;
            }
        } else {
            clean_slug.push(c);
            last_was_dash = false;
        }
    }

    Ok(ClipItem::from_text(clean_slug))
}

fn op_base64_encode(item: &ClipItem) -> Result<ClipItem, OpError> {
    let text = get_item_text(item)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    Ok(ClipItem::from_text(encoded))
}

fn op_base64_decode(item: &ClipItem) -> Result<ClipItem, OpError> {
    let text = get_item_text(item)?;
    let decoded_bytes = base64::engine::general_purpose::STANDARD
        .decode(text.trim().as_bytes())
        .map_err(|e| OpError::ExecutionFailed(format!("Base64 decode failed: {}", e)))?;
    let decoded_str = String::from_utf8(decoded_bytes)
        .map_err(|e| OpError::ExecutionFailed(format!("Invalid UTF-8 in decoded bytes: {}", e)))?;
    Ok(ClipItem::from_text(decoded_str))
}

fn op_concat(hand: &ClipItem, slot: &ClipItem, sep: &str) -> Result<ClipItem, OpError> {
    let text_a = get_item_text(hand)?;
    let text_b = get_item_text(slot)?;
    let combined = format!("{}{}{}", text_a, sep, text_b);
    Ok(ClipItem::from_text(combined))
}

fn op_diff(hand: &ClipItem, slot: &ClipItem) -> Result<ClipItem, OpError> {
    let lhs = resolve_diff_target(slot).ok_or_else(|| {
        OpError::IncompatiblePayload("Secondary slot payload is not readable text, file, or image".to_string())
    })?;
    let rhs = resolve_diff_target(hand).ok_or_else(|| {
        OpError::IncompatiblePayload("Left Hand payload is not readable text, file, or image".to_string())
    })?;

    if lhs.is_image() && rhs.is_image() {
        let options = ImageDiffOptions::default();
        let b1 = lhs.image_bytes().unwrap();
        let b2 = rhs.image_bytes().unwrap();
        let res = compare_image_bytes(b1, b2, &options)
            .map_err(|e| OpError::ExecutionFailed(format!("Picture diff failed: {}", e)))?;
        let mask_png = res.encode_mask_png()
            .map_err(|e| OpError::ExecutionFailed(format!("Encoding diff mask PNG failed: {}", e)))?;
        return ClipItem::from_image_bytes(mask_png, "image/png")
            .ok_or_else(|| OpError::ExecutionFailed("Failed to create ClipItem from diff mask PNG".to_string()));
    }

    if lhs.is_image() != rhs.is_image() {
        return Err(OpError::IncompatiblePayload("Both operands must be the same type (either both text/code or both images)".to_string()));
    }

    let res = compute_diff(&lhs, &rhs, DiffDisplayMode::Inline, 120);
    Ok(ClipItem::from_text(res.raw_ansi))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trim() {
        let item = ClipItem::from_text("  \n  hello world   \n\n  ");
        let res = execute_unary_op("trim", &item).unwrap();
        assert_eq!(get_item_text(&res).unwrap(), "  hello world");
    }

    #[test]
    fn test_clean_url() {
        let item = ClipItem::from_text("https://example.com/watch?v=123&utm_source=twitter&fbclid=xyz&si=abc");
        let res = execute_unary_op("clean-url", &item).unwrap();
        assert_eq!(get_item_text(&res).unwrap(), "https://example.com/watch?v=123");
    }

    #[test]
    fn test_case_transforms() {
        let item = ClipItem::from_text("hello_world_variable");
        let camel = execute_unary_op("camel-case", &item).unwrap();
        assert_eq!(get_item_text(&camel).unwrap(), "helloWorldVariable");

        let kebab = execute_unary_op("kebab-case", &item).unwrap();
        assert_eq!(get_item_text(&kebab).unwrap(), "hello-world-variable");
    }

    #[test]
    fn test_slugify() {
        let item = ClipItem::from_text("This is an Awesome Blog Post! (2026)");
        let res = execute_unary_op("slugify", &item).unwrap();
        assert_eq!(get_item_text(&res).unwrap(), "this-is-an-awesome-blog-post-2026");
    }

    #[test]
    fn test_concat() {
        let item1 = ClipItem::from_text("First");
        let item2 = ClipItem::from_text("Second");
        let res = execute_binary_op("concat", &item1, &item2, Some(" + ")).unwrap();
        assert_eq!(get_item_text(&res).unwrap(), "First + Second");
    }

    #[test]
    fn test_op_diff_images() {
        use image::{Rgba, RgbaImage};
        let mut img1 = RgbaImage::new(20, 20);
        for p in img1.pixels_mut() {
            *p = Rgba([255u8, 255, 255, 255]);
        }
        let mut b1 = std::io::Cursor::new(Vec::new());
        img1.write_to(&mut b1, image::ImageFormat::Png).unwrap();
        let bytes1 = b1.into_inner();

        let mut img2 = RgbaImage::new(20, 20);
        for (x, y, p) in img2.enumerate_pixels_mut() {
            if x < 10 && y < 10 {
                *p = Rgba([0u8, 0, 0, 255]); // 100 altered pixels
            } else {
                *p = Rgba([255u8, 255, 255, 255]);
            }
        }
        let mut b2 = std::io::Cursor::new(Vec::new());
        img2.write_to(&mut b2, image::ImageFormat::Png).unwrap();
        let bytes2 = b2.into_inner();

        let item1 = ClipItem::from_image_bytes(bytes1, "image/png").unwrap();
        let item2 = ClipItem::from_image_bytes(bytes2, "image/png").unwrap();

        // Check applicable ops (both visual image diff and binary hex diff are available)
        let ops = get_applicable_ops(&[&item1, &item2]);
        assert_eq!(ops.len(), 2);
        assert!(ops.iter().any(|op| op.name == "diff"));
        assert!(ops.iter().any(|op| op.name == "binary-diff"));

        // Execute diff operation
        let diff_item = execute_binary_op("diff", &item2, &item1, None).unwrap();
        match diff_item.payload {
            ClipPayload::Image { width, height, mime, .. } => {
                assert_eq!(width, 20);
                assert_eq!(height, 20);
                assert_eq!(mime, "image/png");
            }
            _ => panic!("Expected Image payload from diff operation"),
        }
    }
}

