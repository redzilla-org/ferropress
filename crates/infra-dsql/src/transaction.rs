//! Closure transactions rerun on optimistic-concurrency conflicts.

use cms_db::{Dialect, Reply, ServerError, Sql, Store, StoreError};

use crate::conn::{Outcome, TxStatus};
use crate::ports::{Connector, TokenSource};
use crate::sqlstate::PgError;
use crate::{Dsql, Error};

/// Why [`Dsql::with_transaction`] did not commit.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransactionError<E> {
    /// The closure returned an error; the transaction was rolled back.
    #[error("transaction closure failed")]
    Closure(E),
    /// Every attempt hit a conflict; this is the last conflict the server reported.
    #[error("transaction kept conflicting: {}", .0.message())]
    Conflict(ServerError),
    /// A statement failed outside the closure's view, or COMMIT failed for another reason.
    #[error("transaction aborted: {}", .0.message())]
    Aborted(ServerError),
    /// The connection failed; whether a COMMIT in flight landed is unknown, so nothing reruns.
    #[error("transaction lost its connection: {0}")]
    Store(#[source] Error),
}

/// The store a transaction closure writes through. Statements run inside the open block.
#[derive(Debug)]
pub struct Transaction<'a, C: Connector, K: TokenSource> {
    session: &'a mut Dsql<C, K>,
    conflict: Option<PgError>,
}

impl<C: Connector, K: TokenSource> Store for Transaction<'_, C, K> {
    // Same server as the owning session.
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    fn query(&mut self, sql: &Sql) -> Result<Reply, StoreError> {
        let outcome = self.session.run_in_block(sql.as_bytes())?;
        // A conflict dooms the block whatever the closure does next, so remember it for rerun.
        if let Outcome::Failed(error) = &outcome
            && error.state.is_conflict()
        {
            self.conflict = Some(error.clone());
        }
        Ok(outcome.into_reply())
    }
}

impl<C: Connector, K: TokenSource> Dsql<C, K> {
    /// Runs `work` between BEGIN and COMMIT, rerunning it from scratch when DSQL reports an
    /// optimistic-concurrency conflict (`40001`, `OC000`, `OC001`) on any statement or on COMMIT.
    ///
    /// `work` may run several times, so it must not have side effects outside the transaction.
    ///
    /// # Errors
    ///
    /// [`TransactionError`]: the closure's own error, exhausted retries, a non-conflict server
    /// failure that left the block unusable, or a lost connection.
    pub fn with_transaction<T, E>(
        &mut self,
        mut work: impl FnMut(&mut Transaction<'_, C, K>) -> Result<T, E>,
    ) -> Result<T, TransactionError<E>> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let conflict = match self.attempt(&mut work)? {
                Ok(value) => return Ok(value),
                Err(conflict) => conflict,
            };
            if attempt >= self.retry.attempts {
                return Err(TransactionError::Conflict(conflict.to_server_error()));
            }
            self.backoff(attempt);
        }
    }

    // One BEGIN..COMMIT run: `Ok(Ok)` committed, `Ok(Err)` conflicted and may rerun.
    fn attempt<T, E>(
        &mut self,
        work: &mut impl FnMut(&mut Transaction<'_, C, K>) -> Result<T, E>,
    ) -> Result<Result<T, PgError>, TransactionError<E>> {
        self.control(b"BEGIN")?
            .map_err(|error| TransactionError::Aborted(error.to_server_error()))?;
        let mut tx = Transaction {
            session: self,
            conflict: None,
        };
        let result = work(&mut tx);
        let conflict = tx.conflict;
        if let Some(conflict) = conflict {
            self.rollback()?;
            return Ok(Err(conflict));
        }
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                self.rollback()?;
                return Err(TransactionError::Closure(error));
            }
        };
        // COMMIT of a failed block reports success while rolling back, so check first.
        if self.status() == Some(TxStatus::Failed) {
            self.rollback()?;
            return Err(TransactionError::Aborted(ServerError::new(
                1105,
                "a statement failed inside the transaction (SQLSTATE 25P02)",
            )));
        }
        match self.control(b"COMMIT")? {
            Ok(()) => Ok(Ok(value)),
            // DSQL validates at commit and has already ended the block.
            Err(error) if error.state.is_conflict() => Ok(Err(error)),
            Err(error) => Err(TransactionError::Aborted(error.to_server_error())),
        }
    }

    fn rollback<E>(&mut self) -> Result<(), TransactionError<E>> {
        self.control(b"ROLLBACK")?
            .map_err(|error| TransactionError::Aborted(error.to_server_error()))
    }

    // BEGIN, COMMIT and ROLLBACK reply with a bare tag; a result set would be a broken server.
    fn control<E>(&mut self, sql: &[u8]) -> Result<Result<(), PgError>, TransactionError<E>> {
        let outcome = if sql == b"BEGIN" {
            self.run(sql)
        } else {
            self.run_in_block(sql)
        };
        match outcome.map_err(TransactionError::Store)? {
            Outcome::Done { .. } => Ok(Ok(())),
            Outcome::Failed(error) => Ok(Err(error)),
            Outcome::Rows(_) => Err(TransactionError::Store(Error::Protocol(
                "transaction control returned rows",
            ))),
        }
    }
}
