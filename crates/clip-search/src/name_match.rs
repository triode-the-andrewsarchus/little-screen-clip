//! Bounded literal and progressive filename ranking, independent of input focus.
/// Literal matching only: exact, prefix, then substring. Ties keep display order.
pub fn name_rank(name: &str, query: &str) -> Option<u8> {
    let name = if name
        .bytes()
        .all(|byte| byte.is_ascii() && !byte.is_ascii_uppercase())
    {
        std::borrow::Cow::Borrowed(name)
    } else {
        std::borrow::Cow::Owned(name.to_lowercase())
    };
    if name == query {
        Some(0)
    } else if name.starts_with(query) {
        Some(1)
    } else if name.contains(query) {
        Some(2)
    } else {
        None
    }
}

/// Progressive stages belong to navigation, not explicit literal commands.
/// The first tuple field preserves stage priority; the second prefers compact
/// subsequences. Equal scores retain the browser's current display order.
pub fn navigation_rank(name: &str, query: &str) -> Option<(u8, usize)> {
    if let Some(rank) = name_rank(name, query) {
        return Some((rank, 0));
    }
    let name = name.to_lowercase();
    if let Some((stem, extension)) = query.rsplit_once('.')
        && !stem.is_empty()
        && !extension.is_empty()
        && let Some((actual_stem, actual_extension)) = name.rsplit_once('.')
        && extension == actual_extension
        && actual_stem.starts_with(stem)
    {
        return Some((3, actual_stem.len() - stem.len()));
    }
    if let Some(score) = subsequence_score(&name, query) {
        return Some((4, score));
    }
    // Bound UI-thread work even for pasted queries and pathological names.
    // Short queries never get typo correction: that would match too broadly.
    let query: Vec<_> = query.chars().take(129).collect();
    let name: Vec<_> = name.chars().take(129).collect();
    if !(4..=128).contains(&query.len()) || name.len() > 128 {
        return None;
    }
    if one_edit_apart(&name, &query) {
        return Some((5, 0));
    }
    let stem = name.iter().rposition(|c| *c == '.').unwrap_or(name.len());
    one_edit_apart(&name[..stem], &query).then_some((5, 1))
}

fn subsequence_score(name: &str, query: &str) -> Option<usize> {
    // Like the PDF contents finder, ignore whitespace between typed fragments.
    let mut remaining = name.chars().enumerate();
    let mut score = 0;
    let mut count = 0;
    for wanted in query.chars().filter(|c| !c.is_whitespace()) {
        let (position, _) = remaining.find(|(_, actual)| *actual == wanted)?;
        score += position;
        count += 1;
    }
    (count > 0).then_some(score)
}

/// One insertion, deletion, substitution or adjacent transposition, in Unicode
/// scalar values. Linear work and no edit-distance matrix on the UI thread.
fn one_edit_apart(left: &[char], right: &[char]) -> bool {
    if left.len().abs_diff(right.len()) > 1 {
        return false;
    }
    let common = left.iter().zip(right).take_while(|(a, b)| a == b).count();
    let left = &left[common..];
    let right = &right[common..];
    match left.len().cmp(&right.len()) {
        std::cmp::Ordering::Greater => left[1..] == *right,
        std::cmp::Ordering::Less => *left == right[1..],
        std::cmp::Ordering::Equal => {
            left.is_empty()
                || left[1..] == right[1..]
                || (left.len() >= 2
                    && left[0] == right[1]
                    && left[1] == right[0]
                    && left[2..] == right[2..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_names_rank_exact_then_prefix_then_substring() {
        assert_eq!(name_rank("Report", "report"), Some(0));
        assert_eq!(name_rank("REPORT.txt", "report"), Some(1));
        assert_eq!(name_rank("old-report.txt", "report"), Some(2));
        assert_eq!(name_rank("r_e_p_o_r_t", "report"), None);
        assert_eq!(name_rank("Résumé.txt", "résumé"), Some(1));
    }

    #[test]
    fn progressive_stages_preserve_literal_priority_and_relax_extensions() {
        assert_eq!(navigation_rank("foo.md", "foo.md"), Some((0, 0)));
        assert_eq!(navigation_rank("foo_2.md", "foo.md"), Some((3, 2)));
        assert_eq!(navigation_rank("foo_2.txt", "foo.md"), None);
        assert!(navigation_rank("a_foo_b.md", "a_ foo").is_some());
        assert_eq!(navigation_rank("r_e_p_o_r_t", "report").unwrap().0, 4);
        assert_eq!(navigation_rank("Résumé.txt", "résmé").unwrap().0, 4);
        assert_eq!(navigation_rank("report.txt", "reprot").unwrap().0, 5);
        assert_eq!(navigation_rank("report.txt", "repirt").unwrap().0, 5);
        assert_eq!(navigation_rank("report.txt", "reports").unwrap().0, 5);
        assert_eq!(navigation_rank("report.txt", "repxxrt"), None);
        assert_eq!(navigation_rank("cat.txt", "cot"), None);
        assert_eq!(navigation_rank("file.txt", &"x".repeat(10000)), None);
        // Explicit command queries retain their literal semantics.
        assert_eq!(name_rank("report.txt", "reprot"), None);
    }
}
