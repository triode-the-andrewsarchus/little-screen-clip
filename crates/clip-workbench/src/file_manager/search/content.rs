//! One literal matcher for current extraction and source-validated indexed text.
use super::explanation::{self, Explanation, Passage};

pub(super) type Match = (bool, bool, Option<Explanation>);

pub(super) struct Literal {
    needle: String,
    tail: String,
}
impl Literal {
    pub fn new(needle: &str) -> Self {
        Self {
            needle: needle
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase(),
            tail: String::new(),
        }
    }
    pub fn push(&mut self, text: &str, location: &str, coverage: String) -> Option<Explanation> {
        let preceding = self.tail.to_lowercase().len();
        if !self.tail.is_empty() {
            self.tail.push(' ');
        }
        self.tail
            .push_str(&text.split_whitespace().collect::<Vec<_>>().join(" "));
        if let Some(position) = self.tail.to_lowercase().find(&self.needle) {
            return Some(Explanation {
                unavailable: false,
                coverage,
                passages: vec![Passage {
                    kind: "Content match".into(),
                    location: if position < preceding {
                        format!("Across extraction chunks, ending at {location}")
                    } else {
                        location.into()
                    },
                    text: explanation::excerpt(&self.tail, &self.needle),
                }],
            });
        }
        self.tail = self
            .tail
            .chars()
            .rev()
            .take(self.needle.chars().count())
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_literals_keep_case_expansion_whitespace_and_cross_part_evidence() {
        let mut literal = Literal::new("CAFÉ\n月");
        assert!(
            literal
                .push("İstanbul\tCafé", "page 1", "partial".into())
                .is_none()
        );
        let result = literal
            .push("\n月\tend", "page 2", "partial".into())
            .unwrap();
        assert_eq!(
            result.passages[0].location,
            "Across extraction chunks, ending at page 2"
        );
        assert!(result.passages[0].text.contains("Café 月"));
        assert_eq!(result.coverage, "partial");
    }
}
