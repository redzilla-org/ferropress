//! Narrow ports the admin needs from other phases. The composition root adapts the real
//! session store, projector invoker and clock to these.

use std::fmt;

use cms_auth::{Caps, RoleTable};
use cms_db::Store;
use cms_users::UserId;

use crate::Error;

/// Sessions and nonces (the auth phase owns tokens, the sessions table and the salts).
pub trait Sessions {
    /// The user a session token belongs to, or `None` when it is unknown or expired.
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when the sessions table cannot be read.
    fn user(&mut self, token: &str) -> Result<Option<UserId>, Error>;

    /// A nonce for `action`, bound to the session token.
    fn nonce(&self, token: &str, action: &str) -> String;

    /// Whether `nonce` is valid for `action` on this session (current or previous tick).
    fn verify(&self, token: &str, action: &str, nonce: &str) -> bool;
}

/// Primitive capability checks for a stored user.
pub trait Permissions {
    /// Whether `user` holds every primitive in `required` after role merging.
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when usermeta or the roles option cannot be read.
    fn has(&mut self, user: UserId, required: &[&[u8]]) -> Result<bool, Error>;
}

/// Wakes the projector after a commit. Delivery is best effort: the schedule is the safety net.
pub trait Projector {
    /// Sends one nudge.
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when the invoke failed.
    fn nudge(&mut self) -> Result<(), Error>;
}

/// Wall-clock seconds since the Unix epoch, UTC.
pub trait Clock {
    /// The current time.
    fn now(&self) -> i64;
}

/// [`Permissions`] over the stored capability maps in `cms-auth`.
pub struct CapsPermissions<S> {
    caps: Caps<S>,
    roles: RoleTable,
}

impl<S: Store> CapsPermissions<S> {
    /// Checks against `caps` with the site's role table.
    #[must_use]
    pub const fn new(caps: Caps<S>, roles: RoleTable) -> Self {
        Self { caps, roles }
    }
}

impl<S: Store> Permissions for CapsPermissions<S> {
    fn has(&mut self, user: UserId, required: &[&[u8]]) -> Result<bool, Error> {
        self.caps
            .has(user, &self.roles, required)
            .map_err(|_unavailable| Error::Unavailable)
    }
}

impl<S> fmt::Debug for CapsPermissions<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CapsPermissions").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{CapsPermissions, Permissions};
    use crate::Error;
    use cms_auth::{Caps, RoleTable};
    use cms_db::{Cell, Column, Reply, ResultSet, Row, Scripted, Sql, TablePrefix};
    use cms_users::UserId;

    #[test]
    fn stored_caps_answer_and_outages_are_unavailable() {
        let prefix = TablePrefix::new("fp_");
        let user = UserId::new(3);
        assert!(prefix.is_ok() && user.is_ok());
        let (Ok(prefix), Ok(user)) = (prefix, user) else {
            return;
        };
        let select = Sql::from(
            "SELECT user_id, meta_key, meta_value FROM fp_usermeta WHERE user_id IN (3) ORDER BY umeta_id ASC",
        );
        let stored = Reply::Rows(ResultSet::new(
            [
                Column::new("user_id"),
                Column::new("meta_key"),
                Column::new("meta_value"),
            ],
            [Row::new([
                Cell::bytes("3"),
                Cell::bytes("fp_capabilities"),
                Cell::bytes("a:1:{s:10:\"edit_posts\";b:1;}"),
            ])],
        ));
        let caps = Caps::new(Scripted::once(select, stored), &prefix);
        assert!(caps.is_ok());
        if let Ok(caps) = caps {
            let mut permissions = CapsPermissions::new(caps, RoleTable::empty());
            assert!(format!("{permissions:?}").contains("CapsPermissions"));
            assert_eq!(permissions.has(user, &[b"edit_posts"]), Ok(true));
            assert_eq!(permissions.has(user, &[b"publish_posts"]), Ok(false));
        }
        let empty = Caps::new(Scripted::new([]), &prefix);
        assert!(empty.is_ok());
        if let Ok(empty) = empty {
            let mut permissions = CapsPermissions::new(empty, RoleTable::empty());
            assert_eq!(
                permissions.has(user, &[b"edit_posts"]),
                Err(Error::Unavailable)
            );
        }
    }
}
