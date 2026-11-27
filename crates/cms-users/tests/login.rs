//! Lookups by login and email, and the password write used by login rehashing.

use cms_db::{Cell, Column, Reply, ResultSet, Row, Scripted, ServerError, Sql, TablePrefix};
use cms_users::{UserId, Users, Write};

fn row(id: &str, login: &str) -> Reply {
    Reply::Rows(ResultSet::new(
        [Column::new("ID"), Column::new("user_login")],
        [Row::new([Cell::bytes(id), Cell::bytes(login)])],
    ))
}

fn open(steps: Vec<(Sql, Reply)>) -> Result<Users<Scripted>, cms_users::Error> {
    let prefix = TablePrefix::new("wp_")?;
    Users::new(Scripted::new(steps), &prefix)
}

#[test]
fn login_then_email_lookups_fill_the_id_cache() {
    let by_login = Sql::from("SELECT * FROM wp_users WHERE user_login = 'ada' LIMIT 1");
    let by_email = Sql::from("SELECT * FROM wp_users WHERE user_email = 'b@example.test' LIMIT 1");
    let mut users = open(vec![
        (by_login, row("7", "ada")),
        (by_email, row("8", "bob")),
    ])
    .unwrap();
    let ada = users.by_login(b"ada").unwrap().unwrap();
    assert_eq!(ada.id().as_u64(), 7);
    let bob = users.by_email(b"b@example.test").unwrap().unwrap();
    assert_eq!(bob.login(), b"bob");
    // Served from the cache: the script has no more steps.
    let cached = users.get(UserId::new(7).unwrap()).unwrap().unwrap();
    assert_eq!(cached.login(), b"ada");
}

#[test]
fn lookup_misses_and_failures_are_none_and_outages_are_errors() {
    let empty = Reply::Rows(ResultSet::new([Column::new("ID")], Vec::<Row>::new()));
    let sql = |login: &str| {
        Sql::from(format!("SELECT * FROM wp_users WHERE user_login = '{login}' LIMIT 1").as_str())
    };
    let mut users = open(vec![
        (sql("a"), empty),
        (sql("b"), Reply::Failed(ServerError::new(1146, "missing"))),
    ])
    .unwrap();
    assert!(users.by_login(b"a").unwrap().is_none());
    assert!(users.by_login(b"b").unwrap().is_none());
    assert!(users.by_login(b"c").unwrap_err().is_unavailable());
}

#[test]
fn set_password_rewrites_the_hash_and_drops_the_cached_row() {
    let get = Sql::from("SELECT * FROM wp_users WHERE ID = '7' LIMIT 1");
    let write = Sql::from(
        "UPDATE `wp_users` SET `user_pass` = 'h', `user_activation_key` = '' WHERE `ID` = 7",
    );
    let done = |affected| Reply::Done {
        affected,
        insert_id: 0,
    };
    let mut users = open(vec![
        (get.clone(), row("7", "ada")),
        (write.clone(), done(1)),
        (get, row("7", "ada2")),
        (write, done(0)),
    ])
    .unwrap();
    let id = UserId::new(7).unwrap();
    assert!(users.get(id).unwrap().is_some());
    assert_eq!(users.set_password(id, b"h").unwrap(), Write::Replaced);
    assert_eq!(users.get(id).unwrap().unwrap().login(), b"ada2");
    assert_eq!(users.set_password(id, b"h").unwrap(), Write::Unchanged);
    assert!(users.set_password(id, b"h").unwrap_err().is_unavailable());
}
