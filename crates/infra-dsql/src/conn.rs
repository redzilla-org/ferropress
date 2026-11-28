//! One authenticated `PostgreSQL` session over any byte stream, speaking the simple Query protocol.

use std::io::{Read, Write};

use cms_db::{Cell, Column, Reply, ResultSet, Row};

use crate::Error;
use crate::sqlstate::{PgError, SqlState};
use crate::wire::{self, Body};

/// What one simple query produced, before the port's view is taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// A result set.
    Rows(ResultSet),
    /// A non-row acknowledgement.
    Done {
        /// Rows from the command tag.
        affected: u64,
        /// The `RETURNING` id of an insert, or `0`.
        insert_id: u64,
    },
    /// The server rejected the statement.
    Failed(PgError),
}

impl Outcome {
    /// The port's reply.
    pub(crate) fn into_reply(self) -> Reply {
        match self {
            Self::Rows(set) => Reply::Rows(set),
            Self::Done {
                affected,
                insert_id,
            } => Reply::Done {
                affected,
                insert_id,
            },
            Self::Failed(error) => Reply::Failed(error.to_server_error()),
        }
    }
}

/// Transaction state from the last `ReadyForQuery`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TxStatus {
    /// Not in a transaction block.
    Idle,
    /// Inside a transaction block.
    InBlock,
    /// Inside a failed transaction block; only ROLLBACK will succeed.
    Failed,
}

/// An authenticated session.
#[derive(Debug)]
pub(crate) struct Connection<S: Read + Write> {
    stream: S,
    status: TxStatus,
}

impl<S: Read + Write> Connection<S> {
    /// Runs startup and cleartext-password authentication on `stream`.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] when the server rejects the role, database or password;
    /// [`Error::UnsupportedAuth`] for any method but cleartext; I/O and protocol errors otherwise.
    pub(crate) fn open(
        mut stream: S,
        user: &str,
        database: &str,
        password: &str,
    ) -> Result<Self, Error> {
        wire::write_startup(&mut stream, user, database)?;
        loop {
            let message = wire::read_message(&mut stream)?;
            match message.kind {
                b'R' => {
                    let mut body = Body::new(&message.body);
                    match body.i32()? {
                        // AuthenticationOk.
                        0 => {}
                        // AuthenticationCleartextPassword: DSQL takes the IAM token here, over TLS.
                        3 => wire::write_cstr_message(&mut stream, b'p', password.as_bytes())?,
                        other => return Err(Error::UnsupportedAuth(other)),
                    }
                }
                // Server parameters, cancel key and notices carry nothing this client uses.
                b'S' | b'K' | b'N' => {}
                b'E' => {
                    let error = parse_error(&message.body)?;
                    return Err(Error::Refused {
                        state: error.state,
                        message: error.message,
                    });
                }
                b'Z' => {
                    let status = parse_status(&message.body)?;
                    return Ok(Self { stream, status });
                }
                _ => return Err(Error::Protocol("unexpected message during startup")),
            }
        }
    }

    /// The transaction state the server last reported.
    pub(crate) const fn status(&self) -> TxStatus {
        self.status
    }

    /// Sends `sql` as one simple Query and collects the reply up to `ReadyForQuery`.
    ///
    /// When the text holds several statements the last result wins, and any error wins over
    /// results, matching what the server committed or rolled back.
    ///
    /// # Errors
    ///
    /// I/O and protocol errors; the connection must then be dropped.
    pub(crate) fn query(&mut self, sql: &[u8]) -> Result<Outcome, Error> {
        wire::write_cstr_message(&mut self.stream, b'Q', sql)?;
        let mut columns: Option<Vec<Column>> = None;
        let mut rows: Vec<Row> = Vec::new();
        let mut last: Option<Outcome> = None;
        let mut failed: Option<PgError> = None;
        loop {
            let message = wire::read_message(&mut self.stream)?;
            match message.kind {
                b'T' => {
                    columns = Some(parse_columns(&message.body)?);
                    rows.clear();
                }
                b'D' => {
                    let width = columns
                        .as_ref()
                        .map(Vec::len)
                        .ok_or(Error::Protocol("data row without a row description"))?;
                    rows.push(parse_row(&message.body, width)?);
                }
                b'C' => {
                    let tag = Body::new(&message.body).cstr()?;
                    last = Some(complete(tag, columns.take(), std::mem::take(&mut rows)));
                }
                b'I' => {
                    last = Some(Outcome::Done {
                        affected: 0,
                        insert_id: 0,
                    });
                }
                b'E' => failed = Some(parse_error(&message.body)?),
                b'N' | b'S' => {}
                b'Z' => {
                    self.status = parse_status(&message.body)?;
                    return match (failed, last) {
                        (Some(error), _) => Ok(Outcome::Failed(error)),
                        (None, Some(outcome)) => Ok(outcome),
                        (None, None) => Err(Error::Protocol("query ended without a result")),
                    };
                }
                _ => return Err(Error::Protocol("unexpected message in a query reply")),
            }
        }
    }
}

impl<S: Read + Write> Drop for Connection<S> {
    fn drop(&mut self) {
        // Terminate is a courtesy so the server frees the session at once; a dead socket is fine.
        let _ignored = wire::write_message(&mut self.stream, b'X', &[]);
    }
}

// The rows count is the tag's last word ("INSERT 0 1", "UPDATE 3", "SELECT 2"); DDL tags have none.
fn complete(tag: &[u8], columns: Option<Vec<Column>>, rows: Vec<Row>) -> Outcome {
    let affected = tag
        .rsplit(|&byte| byte == b' ')
        .next()
        .and_then(|word| std::str::from_utf8(word).ok())
        .and_then(|word| word.parse::<u64>().ok())
        .unwrap_or(0);
    let Some(columns) = columns else {
        return Outcome::Done {
            affected,
            insert_id: 0,
        };
    };
    // `INSERT ... RETURNING "ID"` is how the serverless profile learns the new id: one numeric
    // column after an INSERT tag becomes the insert id, the way the reference reports it.
    if tag.starts_with(b"INSERT ") && columns.len() == 1 {
        let id = match rows.first().map(Row::cells) {
            None => Some(0),
            Some([Cell::Bytes(bytes)]) => std::str::from_utf8(bytes)
                .ok()
                .and_then(|text| text.parse::<u64>().ok()),
            Some(_) => None,
        };
        if let Some(insert_id) = id {
            return Outcome::Done {
                affected,
                insert_id,
            };
        }
    }
    Outcome::Rows(ResultSet::new(columns, rows))
}

fn parse_status(body: &[u8]) -> Result<TxStatus, Error> {
    match Body::new(body).byte()? {
        b'I' => Ok(TxStatus::Idle),
        b'T' => Ok(TxStatus::InBlock),
        b'E' => Ok(TxStatus::Failed),
        _ => Err(Error::Protocol("unknown transaction status")),
    }
}

fn parse_columns(body: &[u8]) -> Result<Vec<Column>, Error> {
    let mut body = Body::new(body);
    let count = usize::try_from(body.i16()?)
        .map_err(|_negative| Error::Protocol("negative column count"))?;
    let mut columns = Vec::with_capacity(count);
    for _ in 0..count {
        let name = body.cstr()?;
        // Table oid, attribute number, type oid, type size, type modifier, format code.
        body.take(4 + 2 + 4 + 2 + 4)?;
        if body.i16()? != 0 {
            return Err(Error::Protocol("simple query returned a binary column"));
        }
        columns.push(Column::new(name));
    }
    Ok(columns)
}

fn parse_row(body: &[u8], width: usize) -> Result<Row, Error> {
    let mut body = Body::new(body);
    let count = usize::try_from(body.i16()?)
        .map_err(|_negative| Error::Protocol("negative field count"))?;
    if count != width {
        return Err(Error::Protocol(
            "data row width differs from its description",
        ));
    }
    let mut cells = Vec::with_capacity(count);
    for _ in 0..count {
        let len = body.i32()?;
        // A length of -1 is SQL NULL; other negatives are invalid.
        let cell = if len == -1 {
            Cell::Null
        } else {
            let len = usize::try_from(len)
                .map_err(|_negative| Error::Protocol("negative field length"))?;
            Cell::bytes(body.take(len)?)
        };
        cells.push(cell);
    }
    Ok(Row::new(cells))
}

fn parse_error(body: &[u8]) -> Result<PgError, Error> {
    let mut body = Body::new(body);
    let mut state = None;
    let mut message = None;
    loop {
        let field = body.byte()?;
        if field == 0 {
            break;
        }
        let value = body.cstr()?;
        match field {
            b'C' => {
                let code: [u8; 5] = value
                    .try_into()
                    .map_err(|_wrong_len| Error::Protocol("SQLSTATE is not five bytes"))?;
                state = Some(SqlState::new(code));
            }
            b'M' => message = Some(String::from_utf8_lossy(value).into()),
            _ => {}
        }
    }
    match (state, message) {
        (Some(state), Some(message)) => Ok(PgError { state, message }),
        _ => Err(Error::Protocol("error response lacks a code or message")),
    }
}

#[cfg(test)]
mod tests {
    use super::{Outcome, TxStatus, complete, parse_columns, parse_error, parse_row, parse_status};
    use crate::Error;
    use cms_db::{Cell, Column, Reply, Row};

    #[test]
    fn tags_give_affected_rows_and_returning_gives_the_insert_id() {
        assert_eq!(
            complete(b"UPDATE 3", None, Vec::new()),
            Outcome::Done {
                affected: 3,
                insert_id: 0
            }
        );
        assert_eq!(
            complete(b"CREATE TABLE", None, Vec::new()),
            Outcome::Done {
                affected: 0,
                insert_id: 0
            }
        );
        let id = vec![Column::new("ID")];
        assert_eq!(
            complete(
                b"INSERT 0 1",
                Some(id.clone()),
                vec![Row::new(vec![Cell::bytes("42")])]
            ),
            Outcome::Done {
                affected: 1,
                insert_id: 42
            }
        );
        assert_eq!(
            complete(b"INSERT 0 0", Some(id.clone()), Vec::new()),
            Outcome::Done {
                affected: 0,
                insert_id: 0
            }
        );
        // A non-numeric RETURNING value stays a result set.
        assert!(matches!(
            complete(
                b"INSERT 0 1",
                Some(id),
                vec![Row::new(vec![Cell::bytes("abc")])]
            ),
            Outcome::Rows(_)
        ));
        assert!(matches!(
            complete(
                b"INSERT 0 1",
                Some(vec![Column::new("ID")]),
                vec![Row::new(vec![Cell::Null])]
            ),
            Outcome::Rows(_)
        ));
        assert!(matches!(
            complete(b"SELECT 0", Some(vec![Column::new("n")]), Vec::new()),
            Outcome::Rows(set) if set.rows().is_empty()
        ));
    }

    #[test]
    fn outcomes_become_port_replies() {
        assert_eq!(
            Outcome::Done {
                affected: 1,
                insert_id: 2
            }
            .into_reply()
            .as_done(),
            Some((1, 2))
        );
        let failed = parse_error(b"SERROR\0C42P01\0Mrelation \"x\" does not exist\0\0");
        assert!(failed.is_ok());
        if let Ok(error) = failed {
            let reply = Outcome::Failed(error).into_reply();
            assert_eq!(reply.as_failed().map(cms_db::ServerError::code), Some(1146));
        }
        assert!(matches!(
            Outcome::Rows(cms_db::ResultSet::new(Vec::new(), Vec::new())).into_reply(),
            Reply::Rows(_)
        ));
    }

    #[test]
    fn malformed_bodies_are_protocol_errors() {
        assert!(matches!(
            parse_error(b"C42P01\0\0"),
            Err(Error::Protocol(_))
        ));
        assert!(matches!(
            parse_error(b"C42\0Mx\0\0"),
            Err(Error::Protocol(_))
        ));
        assert!(matches!(parse_status(b"Q"), Err(Error::Protocol(_))));
        assert_eq!(parse_status(b"T").ok(), Some(TxStatus::InBlock));
        assert_eq!(parse_status(b"E").ok(), Some(TxStatus::Failed));
        assert!(matches!(
            parse_columns(b"\xff\xff"),
            Err(Error::Protocol(_))
        ));
        let binary =
            b"\x00\x01n\0\x00\x00\x00\x00\x00\x00\x00\x00\x00\x17\x00\x04\xff\xff\xff\xff\x00\x01";
        assert!(matches!(parse_columns(binary), Err(Error::Protocol(_))));
        assert!(matches!(parse_row(b"\x00\x02", 1), Err(Error::Protocol(_))));
        assert!(matches!(parse_row(b"\xff\xff", 1), Err(Error::Protocol(_))));
        assert!(matches!(
            parse_row(b"\x00\x01\xff\xff\xff\xfe", 1),
            Err(Error::Protocol(_))
        ));
        assert_eq!(
            parse_row(b"\x00\x02\xff\xff\xff\xff\x00\x00\x00\x01a", 2).ok(),
            Some(Row::new(vec![Cell::Null, Cell::bytes("a")]))
        );
    }
}
