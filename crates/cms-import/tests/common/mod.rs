//! In-memory fakes for the import tests: a store that executes the `PostgreSQL` statements the
//! import emits against tables held in memory, and the three ports.

use std::collections::BTreeMap;

use cms_db::{Cell, Column, Dialect, Reply, ResultSet, Row, ServerError, Sql, Store, StoreError};
use cms_import::{Checkpoint, Fetcher, PortError, UploadSink};

/// One stored row: column name to bytes.
pub(crate) type Record = BTreeMap<String, Vec<u8>>;
/// Every table's rows.
pub(crate) type Tables = BTreeMap<String, Vec<Record>>;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Ident(String),
    Text(Vec<u8>),
    Number(u64),
    Word(String),
    Symbol(u8),
}

fn tokenize(sql: &[u8]) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < sql.len() {
        let byte = sql[at];
        match byte {
            b' ' => at += 1,
            b'"' | b'\'' => {
                // Quoted identifier or literal: the quote doubles inside.
                let mut out = Vec::new();
                at += 1;
                loop {
                    let current = *sql.get(at)?;
                    if current == byte {
                        if sql.get(at + 1) == Some(&byte) {
                            out.push(byte);
                            at += 2;
                            continue;
                        }
                        at += 1;
                        break;
                    }
                    out.push(current);
                    at += 1;
                }
                tokens.push(if byte == b'"' {
                    Token::Ident(String::from_utf8(out).ok()?)
                } else {
                    Token::Text(out)
                });
            }
            b'0'..=b'9' => {
                let start = at;
                while sql.get(at).is_some_and(u8::is_ascii_digit) {
                    at += 1;
                }
                tokens.push(Token::Number(
                    std::str::from_utf8(&sql[start..at]).ok()?.parse().ok()?,
                ));
            }
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => {
                let start = at;
                while sql
                    .get(at)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                {
                    at += 1;
                }
                tokens.push(Token::Word(
                    String::from_utf8(sql[start..at].to_vec()).ok()?,
                ));
            }
            other => {
                tokens.push(Token::Symbol(other));
                at += 1;
            }
        }
    }
    Some(tokens)
}

struct Cursor {
    tokens: Vec<Token>,
    at: usize,
}

impl Cursor {
    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        token
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn word(&mut self, word: &str) -> Option<()> {
        (self.next()? == Token::Word(word.to_owned())).then_some(())
    }

    fn symbol(&mut self, symbol: u8) -> Option<()> {
        (self.next()? == Token::Symbol(symbol)).then_some(())
    }

    fn ident(&mut self) -> Option<String> {
        match self.next()? {
            Token::Ident(name) => Some(name),
            _ => None,
        }
    }

    fn value(&mut self) -> Option<Vec<u8>> {
        match self.next()? {
            Token::Text(bytes) => Some(bytes),
            Token::Number(number) => Some(number.to_string().into_bytes()),
            _ => None,
        }
    }

    /// `x` or, when `lower(` comes first, `lower(x)`; the flag says which.
    fn folded<T>(&mut self, mut item: impl FnMut(&mut Self) -> Option<T>) -> Option<(T, bool)> {
        if self.peek() != Some(&Token::Word("lower".to_owned())) {
            return Some((item(self)?, false));
        }
        self.at += 1;
        self.symbol(b'(')?;
        let inner = item(self)?;
        self.symbol(b')')?;
        Some((inner, true))
    }

    /// `c = v` pairs. A `lower(c) = lower(v)` pair keeps `v` lower-cased and is flagged, so
    /// it matches the way DSQL's case-folded lookups do.
    fn pairs(&mut self, separator: &str) -> Option<Vec<(String, Vec<u8>, bool)>> {
        let mut pairs = Vec::new();
        loop {
            let (column, fold) = self.folded(Self::ident)?;
            self.symbol(b'=')?;
            let (mut value, fold_value) = self.folded(Self::value)?;
            if fold != fold_value {
                return None;
            }
            if fold {
                value.make_ascii_lowercase();
            }
            pairs.push((column, value, fold));
            let more = match (self.peek(), separator) {
                (Some(Token::Symbol(b',')), ",") => true,
                (Some(Token::Word(word)), "AND") => word == "AND",
                _ => false,
            };
            if !more {
                return Some(pairs);
            }
            self.at += 1;
        }
    }

    fn list<T>(&mut self, mut item: impl FnMut(&mut Self) -> Option<T>) -> Option<Vec<T>> {
        self.symbol(b'(')?;
        let mut out = vec![item(self)?];
        while self.peek() == Some(&Token::Symbol(b',')) {
            self.at += 1;
            out.push(item(self)?);
        }
        self.symbol(b')')?;
        Some(out)
    }
}

/// The primary-key columns per table stem, enforced like the server would.
fn key_columns(table: &str) -> &'static [&'static str] {
    match table.trim_start_matches("fp_") {
        "users" | "posts" => &["ID"],
        "comments" => &["comment_ID"],
        "terms" => &["term_id"],
        "term_taxonomy" => &["term_taxonomy_id"],
        "term_relationships" => &["object_id", "term_taxonomy_id"],
        _ => &["meta_id"],
    }
}

/// A store over in-memory tables with `BEGIN`/`COMMIT`, a crash switch and a fault injector.
#[derive(Debug, Default)]
pub(crate) struct FakeStore {
    pub(crate) tables: Tables,
    snapshot: Option<Tables>,
    /// Statements executed so far.
    pub(crate) statements: usize,
    /// Fail with [`StoreError::Unavailable`] on this (1-based) statement.
    pub(crate) fail_at: Option<usize>,
    /// Row inserts executed.
    pub(crate) inserts: usize,
    /// Identity-advance statements, as text.
    pub(crate) advanced: Vec<String>,
}

impl FakeStore {
    /// Adds a committed row.
    pub(crate) fn seed(&mut self, table: &str, cells: &[(&str, &str)]) {
        let row = cells
            .iter()
            .map(|(column, value)| ((*column).to_owned(), value.as_bytes().to_vec()))
            .collect();
        self.tables.entry(table.to_owned()).or_default().push(row);
    }

    /// Drops the open transaction, as a lost connection would.
    pub(crate) fn crash(&mut self) {
        if let Some(saved) = self.snapshot.take() {
            self.tables = saved;
        }
    }

    /// Every table with rows sorted, for comparisons that ignore insert order.
    pub(crate) fn dump(&self) -> Tables {
        let mut tables = self.tables.clone();
        for rows in tables.values_mut() {
            rows.sort();
        }
        tables
    }

    fn rows(&self, table: &str) -> &[Record] {
        self.tables.get(table).map_or(&[], Vec::as_slice)
    }

    fn execute(&mut self, sql: &[u8]) -> Option<Reply> {
        let text = String::from_utf8_lossy(sql).into_owned();
        if text == "BEGIN" {
            self.snapshot = Some(self.tables.clone());
            return Some(done(0));
        }
        if text == "COMMIT" {
            self.snapshot = None;
            return Some(done(0));
        }
        if text.starts_with("SELECT setval(") {
            self.advanced.push(text);
            return Some(Reply::single_column("setval", [Cell::bytes("1")]));
        }
        let mut cursor = Cursor {
            tokens: tokenize(sql)?,
            at: 0,
        };
        match cursor.next()? {
            Token::Word(word) if word == "SELECT" => self.select(&mut cursor),
            Token::Word(word) if word == "INSERT" => self.insert(&mut cursor),
            Token::Word(word) if word == "UPDATE" => self.update(&mut cursor),
            _ => None,
        }
    }

    fn select(&self, cursor: &mut Cursor) -> Option<Reply> {
        let max = cursor.peek() == Some(&Token::Word("MAX".to_owned()));
        let column = if max {
            cursor.at += 1;
            cursor.symbol(b'(')?;
            let column = cursor.ident()?;
            cursor.symbol(b')')?;
            column
        } else {
            cursor.ident()?
        };
        cursor.word("FROM")?;
        let table = cursor.ident()?;
        let conditions = if cursor.peek() == Some(&Token::Word("WHERE".to_owned())) {
            cursor.at += 1;
            cursor.pairs("AND")?
        } else {
            Vec::new()
        };
        let mut limit = usize::MAX;
        if cursor.peek() == Some(&Token::Word("LIMIT".to_owned())) {
            cursor.at += 1;
            limit = usize::try_from(match cursor.next()? {
                Token::Number(number) => number,
                _ => return None,
            })
            .ok()?;
        }
        let matching = self
            .rows(&table)
            .iter()
            .filter(|row| matches(row, &conditions));
        if max {
            let top = matching
                .filter_map(|row| {
                    std::str::from_utf8(row.get(&column)?)
                        .ok()?
                        .parse::<u64>()
                        .ok()
                })
                .max();
            let cell = top.map_or(Cell::Null, |top| Cell::bytes(top.to_string()));
            return Some(Reply::single_column("max", [cell]));
        }
        let rows: Vec<Row> = matching
            .take(limit)
            .map(|row| Row::new(vec![row.get(&column).map_or(Cell::Null, Cell::bytes)]))
            .collect();
        Some(Reply::Rows(ResultSet::new(
            vec![Column::new(&column)],
            rows,
        )))
    }

    fn insert(&mut self, cursor: &mut Cursor) -> Option<Reply> {
        cursor.word("INTO")?;
        let table = cursor.ident()?;
        let columns = cursor.list(Cursor::ident)?;
        cursor.word("VALUES")?;
        let values = cursor.list(Cursor::value)?;
        if columns.len() != values.len() {
            return None;
        }
        let row: Record = columns.into_iter().zip(values).collect();
        let keys = key_columns(&table);
        let duplicate = self
            .rows(&table)
            .iter()
            .any(|other| keys.iter().all(|key| other.get(*key) == row.get(*key)));
        if duplicate {
            return Some(Reply::Failed(ServerError::new(23505, "duplicate key")));
        }
        self.tables.entry(table).or_default().push(row);
        self.inserts += 1;
        Some(done(1))
    }

    fn update(&mut self, cursor: &mut Cursor) -> Option<Reply> {
        let table = cursor.ident()?;
        cursor.word("SET")?;
        let sets = cursor.pairs(",")?;
        cursor.word("WHERE")?;
        let conditions = cursor.pairs("AND")?;
        let mut affected = 0;
        for row in self.tables.get_mut(&table)? {
            if matches(row, &conditions) {
                for (column, value, _) in &sets {
                    row.insert(column.clone(), value.clone());
                }
                affected += 1;
            }
        }
        Some(done(affected))
    }
}

/// Whether `row` meets every condition; flagged ones compare lower-cased.
fn matches(row: &Record, conditions: &[(String, Vec<u8>, bool)]) -> bool {
    conditions.iter().all(|(column, value, fold)| {
        row.get(column).is_some_and(|cell| {
            if *fold {
                cell.to_ascii_lowercase() == *value
            } else {
                cell == value
            }
        })
    })
}

fn done(affected: u64) -> Reply {
    Reply::Done {
        affected,
        insert_id: 0,
    }
}

impl Store for FakeStore {
    // The fake parses the Postgres spelling (double-quoted identifiers), as DSQL speaks it.
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    fn query(&mut self, sql: &Sql) -> Result<Reply, StoreError> {
        self.statements += 1;
        if self.fail_at == Some(self.statements) {
            return Err(StoreError::Unavailable);
        }
        // A statement the fake does not understand is a test bug, surfaced as a server error.
        Ok(self.execute(sql.as_bytes()).unwrap_or_else(|| {
            Reply::Failed(ServerError::new(
                42601,
                String::from_utf8_lossy(sql.as_bytes()).into_owned(),
            ))
        }))
    }
}

/// A checkpoint held in memory.
#[derive(Debug, Default)]
pub(crate) struct MemoryCheckpoint {
    pub(crate) records: Option<u64>,
    pub(crate) fail: bool,
}

impl Checkpoint for MemoryCheckpoint {
    fn load(&mut self) -> Result<Option<u64>, PortError> {
        if self.fail {
            return Err(PortError::new("checkpoint down"));
        }
        Ok(self.records)
    }

    fn save(&mut self, records: u64) -> Result<(), PortError> {
        self.records = Some(records);
        Ok(())
    }
}

/// Serves fixed bytes per URL.
#[derive(Debug, Default)]
pub(crate) struct MapFetcher {
    pub(crate) files: BTreeMap<String, Vec<u8>>,
    pub(crate) fetched: Vec<String>,
}

impl Fetcher for MapFetcher {
    fn fetch(&mut self, url: &str) -> Result<Vec<u8>, PortError> {
        self.fetched.push(url.to_owned());
        self.files
            .get(url)
            .cloned()
            .ok_or_else(|| PortError::new("404"))
    }
}

/// Keeps uploaded objects in memory.
#[derive(Debug, Default)]
pub(crate) struct MemoryUploads {
    pub(crate) objects: BTreeMap<String, Vec<u8>>,
    pub(crate) fail: bool,
}

impl UploadSink for MemoryUploads {
    fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), PortError> {
        if self.fail {
            return Err(PortError::new("bucket down"));
        }
        self.objects.insert(path.to_owned(), bytes.to_vec());
        Ok(())
    }
}
