//! Automatic clipboard transformations executed on clipboard capture.
//!
//! Provides rules for stripping marketing/analytics tracking query parameters (`utm_*`, `fbclid`,
//! `gclid`, `si`, `igshid`, etc.) from incoming URLs.

use url::Url;

/// List of query parameter keys to strip from URLs.
const TRACKING_PARAMS: &[&str] = &[
    // Google Analytics / Urchin
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "utm_id",
    "utm_reader",
    "utm_name",
    "utm_cid",
    "_ga",
    "_gl",
    // Facebook
    "fbclid",
    // Google Ads
    "gclid",
    "gclsrc",
    "dclid",
    // Microsoft / Bing
    "msclkid",
    // Mailchimp
    "mc_eid",
    "mc_cid",
    // HubSpot
    "_hsenc",
    "_hsmi",
    "hsCtaTracking",
    // Twitter / X
    "twclid",
    // Instagram / Meta
    "igshid",
    "share_id",
    // YouTube / Spotify / Generic social share trackers
    "si",
    "ref",
    "ref_src",
    "ref_url",
    "feature",
];

/// Checks if a query key matches any known tracking parameter (case-insensitive prefix/exact).
fn is_tracking_param(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    if lower.starts_with("utm_") {
        return true;
    }
    TRACKING_PARAMS.iter().any(|&p| lower == p)
}

/// Trims tracking parameters from a URL string.
/// Returns `Some(cleaned_url)` if any tracking parameters were removed, or `None` if the URL had no tracking.
pub fn clean_url_tracking(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return None;
    }

    let parsed = Url::parse(trimmed).ok()?;
    if parsed.query().is_none() {
        return None;
    }

    let mut had_tracking = false;
    let mut retained_pairs = Vec::new();

    for (key, val) in parsed.query_pairs() {
        if is_tracking_param(&key) {
            had_tracking = true;
        } else {
            retained_pairs.push((key.into_owned(), val.into_owned()));
        }
    }

    if !had_tracking {
        return None;
    }

    let mut cleaned = parsed.clone();
    if retained_pairs.is_empty() {
        cleaned.set_query(None);
    } else {
        cleaned
            .query_pairs_mut()
            .clear()
            .extend_pairs(retained_pairs);
    }

    Some(cleaned.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_utm_parameters() {
        let url = "https://example.com/article?utm_source=twitter&utm_medium=social&utm_campaign=launch&id=42";
        let cleaned = clean_url_tracking(url);
        assert_eq!(
            cleaned,
            Some("https://example.com/article?id=42".to_string())
        );
    }

    #[test]
    fn test_clean_only_tracking_removes_question_mark() {
        let url = "https://example.com/product?fbclid=IwAR12345&utm_source=fb";
        let cleaned = clean_url_tracking(url);
        assert_eq!(cleaned, Some("https://example.com/product".to_string()));
    }

    #[test]
    fn test_clean_youtube_spotify_si() {
        let yt = "https://youtu.be/dQw4w9WgXcQ?si=abcdef123456";
        let cleaned = clean_url_tracking(yt);
        assert_eq!(cleaned, Some("https://youtu.be/dQw4w9WgXcQ".to_string()));
    }

    #[test]
    fn test_untracked_url_returns_none() {
        let url = "https://example.com/search?q=rust+lang&page=2";
        assert_eq!(clean_url_tracking(url), None);
    }

    #[test]
    fn test_non_url_returns_none() {
        let text = "Just some text with utm_source in it";
        assert_eq!(clean_url_tracking(text), None);
    }
}
