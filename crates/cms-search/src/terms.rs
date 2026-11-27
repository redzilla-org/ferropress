//! Splitting a search string into terms the way the reference's search query does.

/// The reference's default search stopwords, compared against lower-cased terms.
pub const STOPWORDS: &[&str] = &[
    "about", "an", "are", "as", "at", "be", "by", "com", "for", "from", "how", "in", "is", "it",
    "of", "on", "or", "that", "the", "this", "to", "was", "what", "when", "where", "who", "will",
    "with", "www",
];

// The reference matches the whole string as one sentence beyond this many terms.
const MAX_TERMS: usize = 9;

// Separators between unquoted terms.
const fn is_separator(byte: u8) -> bool {
    matches!(byte, b'\t' | b' ' | b',' | b'+')
}

/// A parsed search string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Terms {
    sentence: Box<str>,
    raw_count: usize,
    kept: Vec<Box<str>>,
}

impl Terms {
    /// Parses the stored `s` query var.
    ///
    /// Line breaks are removed. The string splits into double-quoted phrases (an unclosed quote runs
    /// to the end) and runs between spaces, tabs, commas and plus signs. Each piece loses its
    /// surrounding quotes (and spaces, unless it is a closed phrase); empty pieces, `0`, single ASCII
    /// letters, a lone `-` and stopwords are dropped. When nothing is left, or more than nine terms
    /// are, the whole string is the only term.
    #[must_use]
    pub fn parse(search: &str) -> Self {
        let sentence: String = search
            .chars()
            .filter(|c| !matches!(c, '\r' | '\n'))
            .collect();
        let pieces = split(&sentence);
        let mut kept: Vec<Box<str>> = pieces.iter().filter_map(|piece| keep(piece)).collect();
        if kept.is_empty() || kept.len() > MAX_TERMS {
            kept = vec![sentence.as_str().into()];
        }
        Self {
            raw_count: pieces.len(),
            sentence: sentence.into(),
            kept,
        }
    }

    /// The search string after line-break removal; the "sentence" of relevance ordering.
    #[must_use]
    pub fn sentence(&self) -> &str {
        &self.sentence
    }

    /// How many pieces the split found before any were dropped.
    #[must_use]
    pub const fn raw_count(&self) -> usize {
        self.raw_count
    }

    /// The terms that filter, in order; a leading `-` marks an excluded term.
    #[must_use]
    pub fn terms(&self) -> &[Box<str>] {
        &self.kept
    }

    /// Whether the search filters nothing (an empty `s` lists every document).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sentence.is_empty()
    }
}

// Pieces: a quoted phrase from `"` to the next `"` or the end, or a run of non-separators.
fn split(sentence: &str) -> Vec<&str> {
    let bytes = sentence.as_bytes();
    let mut pieces = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let end = if bytes[at] == b'"' {
            bytes[at + 1..]
                .iter()
                .position(|&b| b == b'"')
                .map_or(bytes.len(), |close| at + 1 + close + 1)
        } else if is_separator(bytes[at]) {
            at += 1;
            continue;
        } else {
            bytes[at..]
                .iter()
                .position(|&b| b == b'"' || is_separator(b))
                .map_or(bytes.len(), |stop| at + stop)
        };
        // Every boundary is an ASCII byte, so slicing stays on character boundaries.
        pieces.push(&sentence[at..end]);
        at = end;
    }
    pieces
}

// One piece after trimming, or `None` when the reference drops it.
fn keep(piece: &str) -> Option<Box<str>> {
    // A closed phrase keeps its inner spaces at the edges; anything else loses them too.
    let closed = piece.len() > 2 && piece.starts_with('"') && piece.ends_with('"');
    let term = if closed {
        piece.trim_matches(['"', '\''])
    } else {
        piece.trim_matches(['"', '\'', ' '])
    };
    let single = term.len() == 1 && term.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-');
    if term.is_empty() || term == "0" || single || STOPWORDS.contains(&&*term.to_lowercase()) {
        return None;
    }
    Some(term.into())
}

#[cfg(test)]
mod tests {
    use super::{Terms, keep, split};

    fn terms(search: &str) -> Vec<String> {
        Terms::parse(search)
            .terms()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn splits_on_separators_and_quotes() {
        assert_eq!(split("a b,c+d\te"), ["a", "b", "c", "d", "e"]);
        assert_eq!(split(r#"x "y z" w"#), ["x", r#""y z""#, "w"]);
        assert_eq!(split(r#"ab"cd"#), ["ab", r#""cd"#]);
        assert_eq!(split("  "), Vec::<&str>::new());
        assert_eq!(split("caf\u{e9} bar"), ["caf\u{e9}", "bar"]);
    }

    #[test]
    fn pieces_are_trimmed_and_filtered() {
        assert_eq!(keep(r#"" big ""#).as_deref(), Some(" big "));
        assert_eq!(keep(r#""'big""#).as_deref(), Some("big"));
        assert_eq!(keep(r#""big"#).as_deref(), Some("big"));
        assert_eq!(keep(r#""""#), None);
        assert_eq!(keep("0"), None);
        assert_eq!(keep("a"), None);
        assert_eq!(keep("-"), None);
        assert_eq!(keep("7").as_deref(), Some("7"));
        assert_eq!(keep("The"), None);
        assert_eq!(keep("-the").as_deref(), Some("-the"));
    }

    #[test]
    fn sentence_fallback_and_counts() {
        let parsed = Terms::parse("the a of");
        assert_eq!(parsed.raw_count(), 3);
        assert_eq!(terms("the a of"), ["the a of"]);
        assert_eq!(terms("1 2 3 4 5 6 7 8 9 10"), ["1 2 3 4 5 6 7 8 9 10"]);
        assert_eq!(
            terms("1 2 3 4 5 6 7 8 9"),
            ["1", "2", "3", "4", "5", "6", "7", "8", "9"]
        );
        let lines = Terms::parse("red\r\nboat");
        assert_eq!(lines.sentence(), "redboat");
        assert!(!lines.is_empty());
        assert!(Terms::parse("").is_empty());
        assert_eq!(Terms::parse("").terms().len(), 1);
    }
}
