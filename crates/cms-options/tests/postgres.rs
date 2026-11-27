//! Option reads and writes on a `PostgreSQL` store (serverless profile, ADR-0010).

use cms_db::{Cell, Column, Dialect, Reply, ResultSet, Row, Scripted, Sql, TablePrefix};
use cms_options::{Autoload, Error, OptionName, Options, Write};

fn opened(steps: Vec<(Sql, Reply)>) -> Option<Options<Scripted>> {
    let prefix = TablePrefix::new("fp_").ok()?;
    Options::new(Scripted::with_dialect(Dialect::Postgres, steps), &prefix).ok()
}

fn autoloaded() -> (Sql, Reply) {
    (
        Sql::from(
            "SELECT \"option_name\", \"option_value\" FROM \"fp_options\" WHERE \"autoload\" IN ( 'yes', 'on', 'auto-on', 'auto' )",
        ),
        Reply::Rows(ResultSet::new(
            [Column::new("option_name"), Column::new("option_value")],
            [Row::new([Cell::bytes("blogname"), Cell::bytes("Old")])],
        )),
    )
}

#[test]
fn postgres_add_looks_up_case_insensitively_then_upserts() {
    let miss = Sql::from(
        "SELECT \"option_value\" FROM \"fp_options\" WHERE lower(\"option_name\") = lower('k') LIMIT 1",
    );
    let insert = Sql::from(
        "INSERT INTO \"fp_options\" (\"option_name\", \"option_value\", \"autoload\") VALUES ('k', 'v', 'auto') ON CONFLICT (lower(\"option_name\")) DO UPDATE SET \"option_name\" = EXCLUDED.\"option_name\", \"option_value\" = EXCLUDED.\"option_value\", \"autoload\" = EXCLUDED.\"autoload\"",
    );
    let bag = opened(vec![
        autoloaded(),
        (
            miss,
            Reply::Rows(ResultSet::new(
                [Column::new("option_value")],
                Vec::<Row>::new(),
            )),
        ),
        (
            insert,
            Reply::Done {
                affected: 1,
                insert_id: 0,
            },
        ),
    ]);
    let key = OptionName::new("k");
    assert!(bag.is_some() && key.is_ok());
    if let (Some(mut options), Ok(key)) = (bag, key) {
        assert!(
            options
                .add(&key, b"v", Autoload::Auto)
                .is_ok_and(|write| write == Write::Changed)
        );
        assert!(
            options
                .get(&key)
                .is_ok_and(|value| value.as_deref() == Some(&b"v"[..]))
        );
    }
}

#[test]
fn postgres_refuses_nul_before_the_server() {
    let bag = opened(vec![autoloaded()]);
    let name = OptionName::new("blogname");
    assert!(bag.is_some() && name.is_ok());
    if let (Some(mut options), Ok(name)) = (bag, name) {
        assert!(matches!(
            options.update(&name, b"a\0b", Some(Autoload::On)),
            Err(Error::Literal(_))
        ));
    }
}
