//! Bounded, read-only RFC 5322/MIME message inspection. No attachments are opened or extracted.
use crate::html_preview::{self, Block};
use mail_parser::{AddressList, MessageParser, PartFlags};

/// Encoded message opening budget.
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
const BODY_CHARS: usize = 256 * 1024;

/// Declared attachment metadata, without retaining decoded payloads.
#[derive(Debug)]
pub struct Attachment {
    /// Decoded display filename, never used as a filesystem path.
    pub name: String,
    /// Declared media type.
    pub content_type: String,
    /// Transfer-decoded size reported by the MIME reader.
    pub bytes: usize,
    /// Whether the MIME reader classifies the part as inline.
    pub inline: bool,
}
/// Display data for one saved message.
#[derive(Debug)]
pub struct Document {
    /// Decoded subject.
    pub subject: String,
    /// Present address, date and identifier headers.
    pub headers: Vec<(String, String)>,
    /// Inert reflowed HTML, or paragraphs from the plain text alternative.
    pub body: Vec<Block>,
    /// Attachment list; payloads remain in the original file.
    pub attachments: Vec<Attachment>,
    /// Bounded original-source preview, with replacement characters for non-UTF-8 bytes.
    pub source: String,
    /// Whether the original-source preview is incomplete.
    pub source_truncated: bool,
    /// Whether the source preview required replacement characters.
    pub source_lossy: bool,
    /// Actual structural, decoding or viewing-limit problems.
    pub problems: Vec<String>,
}
impl Document {
    /// Reads decoded headers, a body alternative and attachment metadata.
    ///
    /// # Errors
    /// Rejects oversized inputs or messages whose content cannot be read as text.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("Email exceeds the 16 MiB opening limit".into());
        }
        let message = MessageParser::new()
            .max_depth(32)
            .max_parts(1024)
            .max_encoded_nesting(2)
            .parse(bytes)
            .ok_or("Could not parse an email message")?;
        let mut problems = Vec::new();
        if message.parts().any(|part| {
            part.flags()
                .intersects(PartFlags::LIMIT_REACHED | PartFlags::NESTING_LIMIT)
        }) {
            problems.push("Some MIME parts exceed the reader limits and were omitted.".into());
        } else if message.parts().any(|part| !part.flags().is_empty()) {
            problems.push("The message has damaged or incomplete MIME structure.".into());
        }
        let subject = field(message.subject().unwrap_or("(No subject)"));
        let headers = message_headers(&message);
        let body = message_body(&message, &mut problems)?;
        let attachments = message
            .attachments()
            .map(|part| {
                let content_type = part
                    .content_type()
                    .map(|mime| {
                        format!(
                            "{}/{}",
                            mime.ctype(),
                            mime.subtype().unwrap_or("octet-stream")
                        )
                    })
                    .unwrap_or_else(|| "application/octet-stream".into());
                Attachment {
                    name: field(part.attachment_name().unwrap_or("Unnamed attachment")),
                    content_type,
                    bytes: part.decoded_len(),
                    inline: part.is_inline(),
                }
            })
            .collect();
        let raw = String::from_utf8_lossy(bytes);
        let source_lossy = matches!(raw, std::borrow::Cow::Owned(_));
        let source = clip(&raw, 512 * 1024);
        let source_truncated = source.len() < raw.len();
        Ok(Self {
            subject,
            headers,
            body,
            attachments,
            source,
            source_truncated,
            source_lossy,
            problems,
        })
    }
    /// Returns a short text excerpt for folder rows.
    pub fn excerpt(&self) -> String {
        self.body
            .iter()
            .filter_map(|block| match block {
                Block::Heading(_, text) | Block::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .flat_map(|text| text.chars().chain(std::iter::once(' ')))
            .take(240)
            .collect()
    }
}
fn message_headers(message: &mail_parser::Message<'_>) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    for (name, list) in [
        ("From", message.from()),
        ("To", message.to()),
        ("Cc", message.cc()),
        ("Bcc", message.bcc()),
        ("Reply-To", message.reply_to()),
    ] {
        if let Some(list) = list {
            headers.push((name.into(), addresses(list)));
        }
    }
    if let Some(date) = message.date() {
        headers.push(("Date".into(), date.to_string()));
    }
    if let Some(id) = message.message_id() {
        headers.push(("Message-ID".into(), field(id)));
    }
    headers
}
fn message_body(
    message: &mail_parser::Message<'_>,
    problems: &mut Vec<String>,
) -> Result<Vec<Block>, String> {
    let html = message
        .html_body()
        .find(|part| part.is_content_type("text", "html"));
    if let Some(part) = html {
        let value = decoded_body(part, problems)?;
        match html_preview::parse(&value, |_| None) {
            Ok(blocks) if !blocks.is_empty() => return Ok(blocks),
            Ok(_) => (),
            Err(error) => problems.push(format!("The HTML body could not be displayed: {error}.")),
        }
    }
    let body = if let Some(part) = message
        .text_body()
        .find(|part| !part.is_content_type("text", "html"))
    {
        let value = decoded_body(part, problems)?;
        let normalized = value.replace("\r\n", "\n");
        let mut paragraphs: Vec<_> = normalized
            .split("\n\n")
            .filter(|s| !s.trim().is_empty())
            .take(4097)
            .map(|s| Block::Text(s.into()))
            .collect();
        if paragraphs.len() > 4096 {
            paragraphs.truncate(4096);
            problems.push("The body preview ends after 4,096 paragraphs.".into());
        }
        paragraphs
    } else {
        Vec::new()
    };
    Ok(body)
}
fn decoded_body(
    part: mail_parser::MessagePart<'_>,
    problems: &mut Vec<String>,
) -> Result<String, String> {
    let (text, issues) = part.text_checked().ok_or("Email body is not text")?;
    if !issues.is_empty() {
        problems.push("Some body text could not be decoded completely.".into());
    }
    let limited = clip(&text, BODY_CHARS);
    if limited.len() < text.len() {
        problems.push("The body preview ends after 262,144 characters.".into());
    }
    Ok(limited)
}
fn addresses(list: AddressList<'_>) -> String {
    let mut labels = list
        .mailboxes()
        .take(101)
        .map(|mailbox| match (mailbox.name(), mailbox.address()) {
            (Some(name), Some(address)) => format!("{} <{}>", field(name), field(address)),
            (Some(name), None) => field(name),
            (_, Some(address)) => field(address),
            _ => String::new(),
        })
        .collect::<Vec<_>>();
    if labels.len() > 100 {
        labels.truncate(100);
        labels.push("… additional addresses in source".into());
    }
    labels.join(", ")
}
fn field(text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut limited = clip(&normalized, 4096);
    if limited.len() < normalized.len() {
        limited.push('…');
    }
    limited
}
fn clip(text: &str, max_chars: usize) -> String {
    let end = text
        .char_indices()
        .nth(max_chars)
        .map_or(text.len(), |(index, _)| index);
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    const MAIL: &[u8] = include_bytes!("../tests/fixtures/email/harbor.eml");
    #[test]
    fn encoded_headers_html_alternatives_and_attachment_metadata_are_decoded() {
        let message = Document::parse(MAIL).unwrap();
        assert_eq!(message.subject, "Harbor meeting – café");
        assert!(
            message
                .headers
                .iter()
                .any(|(name, value)| name == "From" && value == "Dana Café <dana@example.test>")
        );
        assert!(
            message
                .headers
                .iter()
                .any(|(name, value)| name == "Date" && value == "2026-11-08T09:30:00-06:00")
        );
        assert!(
            message
                .body
                .contains(&Block::Heading(1, "Harbor plan".into()))
        );
        assert!(message.excerpt().contains("blue pier & café"));
        assert!(!format!("{:?}", message.body).contains("HIDDEN"));
        assert!(!format!("{:?}", message.body).contains("tracker.png"));
        assert_eq!(
            message
                .attachments
                .iter()
                .map(|a| (a.name.as_str(), a.content_type.as_str(), a.bytes))
                .collect::<Vec<_>>(),
            [
                ("route.txt", "text/plain", 19),
                ("tide-data.bin", "application/octet-stream", 16)
            ]
        );
        assert!(message.problems.is_empty(), "{:?}", message.problems);
        assert!(!message.source_truncated);
        assert!(!message.source_lossy);
    }
    #[test]
    fn eight_bit_charset_decoding_is_separate_from_the_original_source_preview() {
        let message = Document::parse(b"From: a@example.test\r\nSubject: =?ISO-8859-1?Q?Caf=E9?=\r\nContent-Type: text/plain; charset=iso-8859-1\r\nContent-Transfer-Encoding: 8bit\r\n\r\nCaf\xe9 rendezvous.\r\n").unwrap();
        assert_eq!(message.subject, "Café");
        assert!(message.excerpt().contains("Café rendezvous."));
        assert!(message.source_lossy);
        assert!(message.problems.is_empty(), "{:?}", message.problems);
    }
    #[test]
    fn full_encoding_decodes_shift_jis_bodies() {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode([0x8a, 0x43, 0x95, 0xd3]);
        let raw = format!(
            "From: a@example.test\r\nContent-Type: text/plain; charset=shift_jis\r\nContent-Transfer-Encoding: base64\r\n\r\n{encoded}\r\n"
        );
        let message = Document::parse(raw.as_bytes()).unwrap();
        assert!(message.excerpt().contains("海辺"), "{}", message.excerpt());
    }
    #[test]
    fn related_images_are_classified_as_inline_without_an_explicit_disposition() {
        let raw=b"From: a@example.test\r\nContent-Type: multipart/related; boundary=b\r\n\r\n--b\r\nContent-Type: text/html\r\n\r\n<p>Inline gallery.</p><img src='cid:owned-image'>\r\n--b\r\nContent-Type: image/png\r\nContent-ID: <owned-image>\r\nContent-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n--b--\r\n";
        let message = Document::parse(raw).unwrap();
        assert_eq!(message.attachments.len(), 1);
        assert!(message.attachments[0].inline);
        assert_eq!(message.attachments[0].bytes, 8);
    }
    #[test]
    fn an_overly_complex_html_alternative_falls_back_to_plain_text() {
        let html = format!("{}content{}", "<div>".repeat(140), "</div>".repeat(140));
        let raw = format!(
            "From: a@example.test\r\nContent-Type: multipart/alternative; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nFallback plain text.\r\n--b\r\nContent-Type: text/html\r\n\r\n{html}\r\n--b--\r\n"
        );
        let message = Document::parse(raw.as_bytes()).unwrap();
        assert!(message.excerpt().contains("Fallback plain text."));
        assert!(message.problems.iter().any(|p| p.contains("HTML body")));
    }
    #[test]
    fn structural_limits_are_disclosed_and_the_byte_budget_is_enforced() {
        let mut raw = String::from(
            "From: a@example.test\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n",
        );
        for _ in 0..1030 {
            raw.push_str("--b\r\nContent-Type: text/plain\r\n\r\npart\r\n");
        }
        raw.push_str("--b--\r\n");
        let message = Document::parse(raw.as_bytes()).unwrap();
        assert!(
            message.problems.iter().any(|p| p.contains("reader limits")),
            "{:?}",
            message.problems
        );
        assert!(Document::parse(&vec![b'x'; MAX_BYTES + 1]).is_err());
    }
    #[test]
    fn body_previews_preserve_unicode_boundaries_and_disclose_truncation() {
        let raw = format!(
            "From: a@example.test\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{}",
            "é".repeat(BODY_CHARS + 1)
        );
        let message = Document::parse(raw.as_bytes()).unwrap();
        let Block::Text(body) = &message.body[0] else {
            panic!()
        };
        assert_eq!(body.chars().count(), BODY_CHARS);
        assert!(message.problems.iter().any(|p| p.contains("characters")));
    }
}
