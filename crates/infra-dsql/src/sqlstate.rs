//! SQLSTATE codes and their mapping onto the store port's numeric server codes.

use std::fmt;

use cms_db::ServerError;

/// Reference code for an error without a closer equivalent (`ER_UNKNOWN_ERROR`).
const UNKNOWN: u16 = 1105;

// SQLSTATEs whose reference (MySQL) code callers already branch on, so domain code keeps one
// meaning per code across profiles. Codes are the reference server's documented error numbers.
const REFERENCE_CODES: &[(&[u8; 5], u16)] = &[
    (b"42P01", 1146), // undefined_table -> ER_NO_SUCH_TABLE
    (b"42703", 1054), // undefined_column -> ER_BAD_FIELD_ERROR
    (b"42601", 1064), // syntax_error -> ER_PARSE_ERROR
    (b"42P07", 1050), // duplicate_table -> ER_TABLE_EXISTS_ERROR
    (b"23505", 1062), // unique_violation -> ER_DUP_ENTRY
    (b"23502", 1048), // not_null_violation -> ER_BAD_NULL_ERROR
    (b"22001", 1406), // string_data_right_truncation -> ER_DATA_TOO_LONG
    (b"42501", 1142), // insufficient_privilege -> ER_TABLEACCESS_DENIED_ERROR
    (b"28000", 1045), // invalid_authorization_specification -> ER_ACCESS_DENIED_ERROR
    (b"28P01", 1045), // invalid_password -> ER_ACCESS_DENIED_ERROR
    (b"40001", 1213), // serialization_failure -> ER_LOCK_DEADLOCK (restart the transaction)
    (b"OC000", 1213), // DSQL optimistic concurrency conflict on data
    (b"OC001", 1213), // DSQL optimistic concurrency conflict on schema
];

/// The first SQLSTATE that maps to `code`, `XX000` (`internal_error`) for none; the fake server
/// uses it to send port-level scripts as a DSQL server would.
#[cfg(feature = "test-util")]
pub(crate) fn state_for_code(code: u16) -> [u8; 5] {
    REFERENCE_CODES
        .iter()
        .find(|(_, mapped)| *mapped == code)
        .map_or(*b"XX000", |(state, _)| **state)
}

/// A five-character SQLSTATE as the server sent it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SqlState([u8; 5]);

impl SqlState {
    /// The state with these five bytes.
    #[must_use]
    pub const fn new(code: [u8; 5]) -> Self {
        Self(code)
    }

    /// The five bytes as sent.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 5] {
        &self.0
    }

    /// Whether rerunning the whole transaction can succeed: `40001` or DSQL's `OC000`/`OC001`.
    #[must_use]
    pub fn is_conflict(self) -> bool {
        matches!(&self.0, b"40001" | b"OC000" | b"OC001")
    }

    /// Whether the server rejected the credentials, which for DSQL means the token expired or
    /// was signed for another role or host.
    #[must_use]
    pub fn is_auth_failure(self) -> bool {
        matches!(&self.0, b"28000" | b"28P01")
    }

    /// The reference server's code with the same meaning, or `1105` when there is none.
    #[must_use]
    pub fn reference_code(self) -> u16 {
        REFERENCE_CODES
            .iter()
            .find(|(state, _)| **state == self.0)
            .map_or(UNKNOWN, |(_, code)| *code)
    }
}

impl fmt::Display for SqlState {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(&String::from_utf8_lossy(&self.0))
    }
}

/// A server-reported SQL failure with its SQLSTATE.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PgError {
    /// The SQLSTATE.
    pub state: SqlState,
    /// The server's primary message.
    pub message: Box<str>,
}

impl PgError {
    /// The port's view: the reference code, with the SQLSTATE kept in the message because
    /// several states share one code.
    #[must_use]
    pub fn to_server_error(&self) -> ServerError {
        ServerError::new(
            self.state.reference_code(),
            format!("{} (SQLSTATE {})", self.message, self.state),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{PgError, SqlState};

    #[test]
    fn known_states_map_to_reference_codes() {
        let code = |state: &[u8; 5]| SqlState::new(*state).reference_code();
        assert_eq!(code(b"42P01"), 1146);
        assert_eq!(code(b"23505"), 1062);
        assert_eq!(code(b"42703"), 1054);
        assert_eq!(code(b"OC000"), 1213);
        assert_eq!(code(b"28P01"), 1045);
        assert_eq!(code(b"XX000"), 1105);
    }

    #[test]
    fn conflicts_and_auth_failures_are_classified() {
        for state in [b"40001", b"OC000", b"OC001"] {
            assert!(SqlState::new(*state).is_conflict());
        }
        assert!(!SqlState::new(*b"23505").is_conflict());
        assert!(SqlState::new(*b"28000").is_auth_failure());
        assert!(SqlState::new(*b"28P01").is_auth_failure());
        assert!(!SqlState::new(*b"40001").is_auth_failure());
    }

    #[test]
    fn server_errors_keep_the_state_in_the_message() {
        let error = PgError {
            state: SqlState::new(*b"42P01"),
            message: "relation \"missing\" does not exist".into(),
        };
        assert_eq!(error.state.as_bytes(), b"42P01");
        let server = error.to_server_error();
        assert_eq!(server.code(), 1146);
        assert_eq!(
            server.message(),
            "relation \"missing\" does not exist (SQLSTATE 42P01)"
        );
    }
}
