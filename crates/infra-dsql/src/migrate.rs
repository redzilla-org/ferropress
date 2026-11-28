//! Schema migrations: one statement per transaction, recorded in `fp_schema_migrations`.

use std::collections::HashSet;
use std::path::Path;

use cms_db::{Cell, Dialect, ServerError, Sql, Store};

use crate::conn::Outcome;
use crate::ports::{Connector, TokenSource};
use crate::transaction::TransactionError;
use crate::{Dsql, Error};

/// One migration file: its version (the file stem) and its statements in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migration {
    version: Box<str>,
    statements: Vec<Sql>,
}

/// Why a migration run stopped.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MigrateError {
    /// A migration file could not be read.
    #[error("cannot read migration {path}: {source}")]
    Read {
        /// The file.
        path: Box<str>,
        /// The I/O failure.
        source: std::io::Error,
    },
    /// A file ends inside a statement (no `;` at the end of its last line).
    #[error("migration {0} ends inside a statement")]
    Unterminated(Box<str>),
    /// A version or file name cannot be used.
    #[error("migration version {0:?} is not usable")]
    BadVersion(Box<str>),
    /// The applied-migrations table returned something unreadable.
    #[error("fp_schema_migrations returned an unexpected reply")]
    BadLedger,
    /// A statement failed on the server; later statements were not run.
    #[error("migration {version} statement {statement} failed: {}", .error.message())]
    Statement {
        /// The migration version.
        version: Box<str>,
        /// The 1-based statement number.
        statement: usize,
        /// The server's error.
        error: ServerError,
    },
    /// The connection failed or kept conflicting.
    #[error("migration transaction failed: {0}")]
    Transaction(String),
    /// The connection failed outside a transaction.
    #[error(transparent)]
    Store(#[from] Error),
}

/// What a run did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Statements executed and recorded now.
    pub applied: usize,
    /// Statements already recorded, left alone.
    pub skipped: usize,
}

impl Migration {
    /// Splits `text` into statements. A statement ends at a line whose last non-blank character
    /// is `;`; lines starting with `--` are comments and dropped, which is the convention the
    /// files under `deploy/aws/dsql/` follow.
    ///
    /// # Errors
    ///
    /// [`MigrateError::Unterminated`] when text remains after the last `;`, and
    /// [`MigrateError::BadVersion`] for an empty version.
    pub fn parse(version: &str, text: &str) -> Result<Self, MigrateError> {
        if version.is_empty() {
            return Err(MigrateError::BadVersion(version.into()));
        }
        let mut statements = Vec::new();
        let mut current = String::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("--") {
                continue;
            }
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line.trim_end());
            if let Some(body) = current.strip_suffix(';') {
                statements.push(Sql::from(body));
                current.clear();
            }
        }
        if !current.is_empty() {
            return Err(MigrateError::Unterminated(version.into()));
        }
        Ok(Self {
            version: version.into(),
            statements,
        })
    }

    /// Every `*.sql` file in `dir`, in file-name order, versioned by file stem.
    ///
    /// # Errors
    ///
    /// [`MigrateError::Read`] for I/O failures and the errors of [`Self::parse`].
    pub fn load_dir(dir: &Path) -> Result<Vec<Self>, MigrateError> {
        let read_error = |path: &Path, source| MigrateError::Read {
            path: path.display().to_string().into(),
            source,
        };
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(dir).map_err(|source| read_error(dir, source))? {
            let path = entry.map_err(|source| read_error(dir, source))?.path();
            if path.extension().is_some_and(|ext| ext == "sql") {
                paths.push(path);
            }
        }
        paths.sort();
        paths
            .iter()
            .map(|path| {
                let version = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .ok_or_else(|| MigrateError::BadVersion(path.display().to_string().into()))?;
                let text =
                    std::fs::read_to_string(path).map_err(|source| read_error(path, source))?;
                Self::parse(version, &text)
            })
            .collect()
    }

    /// The version, e.g. `0001-core`.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The statements in file order.
    #[must_use]
    pub fn statements(&self) -> &[Sql] {
        &self.statements
    }
}

impl<C: Connector, K: TokenSource> Dsql<C, K> {
    /// Applies every statement of `migrations` not yet recorded in `fp_schema_migrations`.
    ///
    /// Each statement runs in its own transaction (DSQL allows one DDL statement per transaction
    /// and no DDL/DML mix), then its `(version, statement)` row is inserted in a second one.
    /// The first statement of the first migration creates that table.
    ///
    /// # Errors
    ///
    /// [`MigrateError`]; statements after the failing one are not run.
    pub fn migrate(&mut self, migrations: &[Migration]) -> Result<Applied, MigrateError> {
        let done = self.applied_set()?;
        let mut applied = Applied::default();
        for migration in migrations {
            let version = Dialect::Postgres
                .literal(migration.version.as_bytes())
                .map_err(|_nul| MigrateError::BadVersion(migration.version.clone()))?;
            for (index, statement) in migration.statements.iter().enumerate() {
                let number = index + 1;
                if done.contains(&(migration.version.to_string(), number)) {
                    applied.skipped += 1;
                    continue;
                }
                let failed = |error| MigrateError::Statement {
                    version: migration.version.clone(),
                    statement: number,
                    error,
                };
                self.write_once(statement).map_err(|error| match error {
                    TransactionError::Closure(error) => failed(error),
                    other => MigrateError::Transaction(other.to_string()),
                })?;
                let mut record =
                    b"INSERT INTO \"fp_schema_migrations\" (\"version\", \"statement\") VALUES ("
                        .to_vec();
                record.extend_from_slice(&version);
                record.extend_from_slice(format!(", {number})").as_bytes());
                self.write_once(&Sql::new(record))
                    .map_err(|error| MigrateError::Transaction(error.to_string()))?;
                applied.applied += 1;
            }
        }
        Ok(applied)
    }

    // One statement in its own transaction; a server failure is the closure's error.
    fn write_once(&mut self, statement: &Sql) -> Result<(), TransactionError<ServerError>> {
        self.with_transaction(|tx| match tx.query(statement) {
            Ok(cms_db::Reply::Failed(error)) => Err(error),
            Ok(_) => Ok(()),
            // The transaction layer sees the dropped connection on ROLLBACK and reports it.
            Err(_unavailable) => Err(ServerError::new(2013, "lost connection to the server")),
        })
    }

    // Recorded (version, statement) pairs; a missing table means nothing has run yet.
    fn applied_set(&mut self) -> Result<HashSet<(String, usize)>, MigrateError> {
        let sql = b"SELECT \"version\", \"statement\" FROM \"fp_schema_migrations\"";
        let set = match self.run(sql)? {
            Outcome::Failed(error) if error.state.as_bytes() == b"42P01" => {
                return Ok(HashSet::new());
            }
            Outcome::Rows(set) => set,
            Outcome::Failed(_) | Outcome::Done { .. } => return Err(MigrateError::BadLedger),
        };
        set.rows()
            .iter()
            .map(|row| match row.cells() {
                [Cell::Bytes(version), Cell::Bytes(number)] => {
                    let version =
                        std::str::from_utf8(version).map_err(|_utf8| MigrateError::BadLedger)?;
                    let number = std::str::from_utf8(number)
                        .ok()
                        .and_then(|text| text.parse::<usize>().ok())
                        .ok_or(MigrateError::BadLedger)?;
                    Ok((version.to_owned(), number))
                }
                _ => Err(MigrateError::BadLedger),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{MigrateError, Migration};

    #[test]
    fn statements_split_at_line_end_semicolons_and_drop_comments() {
        let text = "-- header\nCREATE TABLE \"a\" (\n    \"x\" text -- inline stays\n);\n\nCREATE INDEX ASYNC \"i\" ON \"a\" (\"x\");  \n";
        let parsed = Migration::parse("0001-core", text);
        assert!(parsed.is_ok());
        if let Ok(migration) = parsed {
            assert_eq!(migration.version(), "0001-core");
            let statements: Vec<&[u8]> = migration
                .statements()
                .iter()
                .map(cms_db::Sql::as_bytes)
                .collect();
            assert_eq!(
                statements,
                [
                    &b"CREATE TABLE \"a\" (\n    \"x\" text -- inline stays\n)"[..],
                    &b"CREATE INDEX ASYNC \"i\" ON \"a\" (\"x\")"[..],
                ]
            );
        }
    }

    #[test]
    fn unterminated_text_and_empty_versions_are_rejected() {
        assert!(matches!(
            Migration::parse("0002", "CREATE TABLE \"a\" (\"x\" text)"),
            Err(MigrateError::Unterminated(_))
        ));
        assert!(matches!(
            Migration::parse("", "SELECT 1;"),
            Err(MigrateError::BadVersion(_))
        ));
    }

    #[test]
    fn the_core_schema_parses_into_one_statement_per_ddl() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/aws/dsql");
        let loaded = Migration::load_dir(&dir);
        assert!(loaded.is_ok());
        if let Ok(migrations) = loaded {
            assert_eq!(migrations[0].version(), "0001-core");
            let statements = migrations[0].statements();
            // Counted from deploy/aws/dsql/0001-core.sql: 14 CREATE TABLE and 30 CREATE INDEX.
            assert_eq!(statements.len(), 44);
            assert!(
                statements[0]
                    .as_bytes()
                    .starts_with(b"CREATE TABLE \"fp_schema_migrations\"")
            );
            assert!(
                statements
                    .iter()
                    .all(|sql| sql.as_bytes().starts_with(b"CREATE "))
            );
        }
        assert!(matches!(
            Migration::load_dir(std::path::Path::new("/nonexistent-migrations")),
            Err(MigrateError::Read { .. })
        ));
    }
}
