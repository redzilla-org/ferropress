//! SQL dialects: how identifiers and string literals are spelled per server.
//!
//! The `MySQL` spelling is the parity profile (ADR-0004) and must stay byte-identical to what the
//! reference sends. The `PostgreSQL` spelling serves the serverless profile on Aurora DSQL
//! (`docs/aws/architecture.md` section 8), where SQL text differs by design but result columns
//! keep the reference names because identifiers are always quoted.

/// A value that cannot be written as a literal in the chosen dialect.
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LiteralError {
    /// `PostgreSQL` text types cannot store a NUL byte.
    #[error("a PostgreSQL text literal must not contain a nul byte")]
    Nul,
}

/// The SQL spelling a [`crate::Store`] adapter expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// Backtick identifiers and backslash-escaped literals, as the reference sends them.
    MySql,
    /// Double-quoted identifiers and standard-conforming literals (Aurora DSQL).
    Postgres,
}

impl Dialect {
    /// `name` quoted as an identifier, with the quote character doubled inside.
    ///
    /// Quoting is unconditional so `PostgreSQL` keeps mixed-case reference names such as `ID`
    /// instead of folding them to lower case.
    #[must_use]
    pub fn ident(self, name: &[u8]) -> Vec<u8> {
        let mark = match self {
            Self::MySql => b'`',
            Self::Postgres => b'"',
        };
        let mut out = Vec::with_capacity(name.len() + 2);
        out.push(mark);
        for &byte in name {
            // Doubling is the only escape both servers accept inside a quoted identifier.
            if byte == mark {
                out.push(mark);
            }
            out.push(byte);
        }
        out.push(mark);
        out
    }

    /// `bytes` as a string literal.
    ///
    /// # Errors
    ///
    /// [`LiteralError::Nul`] for `PostgreSQL` when `bytes` contains a NUL, which its text types
    /// cannot store; failing here beats a server error after a partial write.
    pub fn literal(self, bytes: &[u8]) -> Result<Vec<u8>, LiteralError> {
        match self {
            Self::MySql => Ok(mysql_literal(bytes)),
            Self::Postgres => postgres_literal(bytes),
        }
    }
}

// Same escapes the reference applies before a statement runs; kept byte-exact for parity.
fn mysql_literal(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 2);
    out.push(b'\'');
    for &byte in bytes {
        match byte {
            0 => out.extend_from_slice(br"\0"),
            b'\n' => out.extend_from_slice(br"\n"),
            b'\r' => out.extend_from_slice(br"\r"),
            0x1a => out.extend_from_slice(br"\Z"),
            b'\\' | b'\'' | b'"' => {
                out.push(b'\\');
                out.push(byte);
            }
            other => out.push(other),
        }
    }
    out.push(b'\'');
    out
}

// standard_conforming_strings is on in DSQL: only the quote is doubled, backslash is literal.
fn postgres_literal(bytes: &[u8]) -> Result<Vec<u8>, LiteralError> {
    if bytes.contains(&0) {
        return Err(LiteralError::Nul);
    }
    let mut out = Vec::with_capacity(bytes.len() + 2);
    out.push(b'\'');
    for &byte in bytes {
        if byte == b'\'' {
            out.push(b'\'');
        }
        out.push(byte);
    }
    out.push(b'\'');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::Dialect;
    use super::LiteralError;

    #[test]
    fn identifiers_are_always_quoted_with_the_mark_doubled() {
        assert_eq!(Dialect::MySql.ident(b"ID"), b"`ID`");
        assert_eq!(Dialect::MySql.ident(b"a`b"), b"`a``b`");
        assert_eq!(Dialect::Postgres.ident(b"ID"), b"\"ID\"");
        assert_eq!(Dialect::Postgres.ident(b"a\"b"), b"\"a\"\"b\"");
        assert_eq!(Dialect::Postgres.ident(b"a`b"), b"\"a`b\"");
    }

    #[test]
    fn mysql_literals_match_the_reference_escapes() {
        let quote = |bytes: &[u8]| Dialect::MySql.literal(bytes).ok();
        assert_eq!(quote(b"home"), Some(br"'home'".to_vec()));
        assert_eq!(quote(b"a'b"), Some(br"'a\'b'".to_vec()));
        assert_eq!(quote(b"a\"b"), Some(b"'a\\\"b'".to_vec()));
        assert_eq!(quote(b"a\\b"), Some(br"'a\\b'".to_vec()));
        assert_eq!(quote(b"a\0b"), Some(br"'a\0b'".to_vec()));
        assert_eq!(quote(b"a\nb"), Some(br"'a\nb'".to_vec()));
        assert_eq!(quote(b"a\rb"), Some(br"'a\rb'".to_vec()));
        assert_eq!(quote(&[b'a', 0x1a, b'b']), Some(br"'a\Zb'".to_vec()));
        assert_eq!(quote(b"a\tb"), Some(b"'a\tb'".to_vec()));
    }

    #[test]
    fn postgres_literals_double_quotes_and_keep_backslashes() {
        let quote = |bytes: &[u8]| Dialect::Postgres.literal(bytes);
        assert_eq!(quote(b"home").ok(), Some(b"'home'".to_vec()));
        assert_eq!(quote(b"a'b").ok(), Some(b"'a''b'".to_vec()));
        assert_eq!(quote(b"a\\b").ok(), Some(b"'a\\b'".to_vec()));
        assert_eq!(quote(b"a\"b\nc").ok(), Some(b"'a\"b\nc'".to_vec()));
        assert!(matches!(quote(b"a\0b"), Err(LiteralError::Nul)));
    }

    proptest::proptest! {
        #[test]
        fn plain_ascii_is_wrapped_unchanged_in_both_dialects(text in "[A-Za-z0-9._-]{0,32}") {
            for dialect in [Dialect::MySql, Dialect::Postgres] {
                let quoted = dialect.literal(text.as_bytes());
                proptest::prop_assert!(quoted.is_ok());
                if let Ok(quoted) = quoted {
                    proptest::prop_assert_eq!(&quoted[1..quoted.len() - 1], text.as_bytes());
                }
            }
        }
    }
}
