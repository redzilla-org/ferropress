//! User and usermeta writes on a `PostgreSQL` store (serverless profile, ADR-0010).

use cms_db::{Cell, Column, Dialect, Reply, ResultSet, Row, Scripted, Sql, TablePrefix};
use cms_users::{Draft, Error, Field, MetaId, MetaKey, UserId, UserMeta, Users};

fn store(steps: Vec<(Sql, Reply)>) -> Scripted {
    Scripted::with_dialect(Dialect::Postgres, steps)
}

fn returned(column: &str, id: &str) -> Reply {
    Reply::returned(
        1,
        &ResultSet::new([Column::new(column)], [Row::new([Cell::bytes(id)])]),
    )
}

#[test]
fn postgres_inserts_read_the_returned_ids() {
    let prefix = TablePrefix::new("fp_");
    let key = MetaKey::new("nickname");
    let user = UserId::new(3);
    assert!(prefix.is_ok() && key.is_ok() && user.is_ok());
    if let (Ok(prefix), Ok(key), Ok(user)) = (prefix, key, user) {
        let insert = Sql::from(
            "INSERT INTO \"fp_users\" (\"user_pass\", \"user_nicename\", \"user_email\", \"user_url\", \"user_registered\", \"user_activation_key\", \"display_name\", \"user_login\") VALUES ('', '', '', '', '', '', '', 'ada') RETURNING \"ID\"",
        );
        let users = Users::new(store(vec![(insert, returned("ID", "3"))]), &prefix);
        assert!(users.is_ok());
        if let Ok(mut users) = users {
            let mut draft = Draft::new();
            draft.set(Field::Login, b"ada");
            assert!(
                users
                    .insert(&draft)
                    .is_ok_and(|write| write.inserted_id().map(UserId::as_u64) == Some(3))
            );
        }
        let insert = Sql::from(
            "INSERT INTO \"fp_usermeta\" (\"user_id\", \"meta_key\", \"meta_value\") VALUES (3, 'nickname', 'Ada') RETURNING \"umeta_id\"",
        );
        let meta = UserMeta::new(store(vec![(insert, returned("umeta_id", "8"))]), &prefix);
        assert!(meta.is_ok());
        if let Ok(mut meta) = meta {
            assert!(
                meta.insert(user, &key, b"Ada")
                    .is_ok_and(|write| write.inserted_id().map(MetaId::as_u64) == Some(8))
            );
        }
    }
}

#[test]
fn postgres_refuses_nul_before_the_server() {
    let prefix = TablePrefix::new("fp_");
    let key = MetaKey::new("nickname");
    let user = UserId::new(3);
    assert!(prefix.is_ok() && key.is_ok() && user.is_ok());
    if let (Ok(prefix), Ok(key), Ok(user)) = (prefix, key, user) {
        let users = Users::new(store(Vec::new()), &prefix);
        assert!(users.is_ok());
        if let Ok(mut users) = users {
            let mut draft = Draft::new();
            draft.set(Field::Email, b"a\0b");
            assert!(matches!(users.insert(&draft), Err(Error::Literal(_))));
            assert!(matches!(users.update(user, &draft), Err(Error::Literal(_))));
        }
        let meta = UserMeta::new(store(Vec::new()), &prefix);
        assert!(meta.is_ok());
        if let Ok(mut meta) = meta {
            assert!(matches!(
                meta.insert(user, &key, b"a\0b"),
                Err(Error::Literal(_))
            ));
            assert!(matches!(
                meta.update(user, &key, b"a\0b"),
                Err(Error::Literal(_))
            ));
        }
    }
}
