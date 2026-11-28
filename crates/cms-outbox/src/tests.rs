//! Unit tests for the outbox statements and row parsing.

use cms_db::TablePrefix;
use cms_db::{Cell, Column, Dialect, Reply, ResultSet, Row as DbRow, Scripted, ServerError, Sql};
use serde_json::{Map, Value};

use super::{Aggregate, Change, Event, OutboxError, Row, delete, fetch, write};

const INSERT: &str = "INSERT INTO \"fp_outbox\" (\"aggregate\", \"aggregate_id\", \"change\", \"payload\") VALUES ('post', 12, 'saved', '{\"old\":\"it''s\"}'::jsonb)";
const SELECT: &str = "SELECT \"id\"::text, floor(extract(epoch from \"created_at\") * 1000000)::bigint, \"aggregate\", \"aggregate_id\", \"change\", \"payload\"::text FROM \"fp_outbox\" ORDER BY \"created_at\", \"id\" LIMIT 100";
const DELETE: &str = "DELETE FROM \"fp_outbox\" WHERE \"id\"::text IN ('a', 'b')";

type Outcome = Result<(), cms_db::NameError>;

fn prefix() -> Result<TablePrefix, cms_db::NameError> {
    TablePrefix::new("fp_")
}

fn pg(steps: Vec<(&str, Reply)>) -> Scripted {
    Scripted::with_dialect(
        Dialect::Postgres,
        steps
            .into_iter()
            .map(|(sql, reply)| (Sql::from(sql), reply)),
    )
}

fn done() -> Reply {
    Reply::Done {
        affected: 1,
        insert_id: 0,
    }
}

fn rows(cells: Vec<Vec<Cell>>) -> Reply {
    let columns: Vec<Column> = [
        "id",
        "created",
        "aggregate",
        "aggregate_id",
        "change",
        "payload",
    ]
    .into_iter()
    .map(Column::new)
    .collect();
    Reply::Rows(ResultSet::new(
        columns,
        cells.into_iter().map(DbRow::new).collect::<Vec<_>>(),
    ))
}

fn cells(aggregate: &str, change: &str, payload: &str) -> Vec<Cell> {
    vec![
        Cell::bytes("a"),
        Cell::bytes("1764244800000000"),
        Cell::bytes(aggregate),
        Cell::bytes("12"),
        Cell::bytes(change),
        Cell::bytes(payload),
    ]
}

#[test]
fn names_round_trip_through_their_stored_spelling() {
    for aggregate in Aggregate::ALL.map(|(aggregate, _)| aggregate) {
        assert_eq!(Aggregate::parse(aggregate.as_str()), Some(aggregate));
    }
    for change in Change::ALL.map(|(change, _)| change) {
        assert_eq!(Change::parse(change.as_str()), Some(change));
    }
    assert_eq!(Aggregate::parse("widget"), None);
    assert_eq!(Change::parse("moved"), None);
    assert_eq!(Change::RebuildAll.as_str(), "rebuild-all");
}

#[test]
fn events_carry_their_fields() {
    let event = Event::rebuild_all();
    assert_eq!(event.aggregate(), Aggregate::Site);
    assert_eq!(event.aggregate_id(), 0);
    assert!(event.payload().is_empty());
}

#[test]
fn write_inserts_one_row_with_a_jsonb_payload() -> Outcome {
    let mut payload = Map::new();
    payload.insert("old".into(), Value::from("it's"));
    let event = Event::new(Aggregate::Post, 12, Change::Saved).with_payload(payload);
    let mut store = pg(vec![(INSERT, done())]);
    assert_eq!(write(&mut store, &prefix()?, &event), Ok(()));
    assert_eq!(store.seen().len(), 1);
    Ok(())
}

#[test]
fn the_mysql_dialect_has_no_outbox() -> Outcome {
    let mut store = Scripted::new([]);
    let event = Event::rebuild_all();
    assert_eq!(
        write(&mut store, &prefix()?, &event),
        Err(OutboxError::Dialect)
    );
    assert_eq!(fetch(&mut store, &prefix()?, 1), Err(OutboxError::Dialect));
    assert_eq!(
        delete(&mut store, &prefix()?, &["a".into()]),
        Err(OutboxError::Dialect)
    );
    Ok(())
}

#[test]
fn server_store_and_literal_failures_are_reported() -> Outcome {
    let event = Event::new(Aggregate::Post, 12, Change::Saved);
    let mut failing = pg(vec![(
        "INSERT INTO \"fp_outbox\" (\"aggregate\", \"aggregate_id\", \"change\", \"payload\") VALUES ('post', 12, 'saved', '{}'::jsonb)",
        Reply::Failed(ServerError::new(1, "conflict")),
    )]);
    assert_eq!(
        write(&mut failing, &prefix()?, &event),
        Err(OutboxError::Server("conflict".into()))
    );
    let mut empty = pg(vec![]);
    assert!(matches!(
        write(&mut empty, &prefix()?, &event),
        Err(OutboxError::Store(_))
    ));
    // An id holding NUL cannot be a postgres literal, so nothing reaches the server.
    assert!(matches!(
        delete(&mut empty, &prefix()?, &["a\0".into()]),
        Err(OutboxError::Literal(_))
    ));
    Ok(())
}

#[test]
fn fetch_parses_rows_oldest_first() -> Outcome {
    let mut store = pg(vec![(
        SELECT,
        rows(vec![cells("post", "saved", "{\"k\":1}")]),
    )]);
    let fetched = fetch(&mut store, &prefix()?, 100);
    let mut payload = Map::new();
    payload.insert("k".into(), Value::from(1));
    assert_eq!(
        fetched,
        Ok(vec![Row {
            id: "a".into(),
            created_micros: 1_764_244_800_000_000,
            event: Event::new(Aggregate::Post, 12, Change::Saved).with_payload(payload),
        }])
    );
    Ok(())
}

#[test]
fn malformed_rows_are_rejected() -> Outcome {
    let cases = [
        cells("widget", "saved", "{}"),
        cells("post", "moved", "{}"),
        cells("post", "saved", "[]"),
        cells("post", "saved", "{"),
        vec![Cell::bytes("a"), Cell::bytes("x")],
        vec![Cell::Null],
        vec![Cell::bytes([0xff])],
    ];
    for case in cases {
        let mut store = pg(vec![(SELECT, rows(vec![case]))]);
        assert!(matches!(
            fetch(&mut store, &prefix()?, 100),
            Err(OutboxError::Row(_))
        ));
    }
    let mut store = pg(vec![(SELECT, done())]);
    assert!(matches!(
        fetch(&mut store, &prefix()?, 100),
        Err(OutboxError::Row(_))
    ));
    Ok(())
}

#[test]
fn delete_removes_the_listed_ids_and_skips_an_empty_list() -> Outcome {
    let mut store = pg(vec![(DELETE, done())]);
    assert_eq!(
        delete(&mut store, &prefix()?, &["a".into(), "b".into()]),
        Ok(())
    );
    let mut untouched = pg(vec![]);
    assert_eq!(delete(&mut untouched, &prefix()?, &[]), Ok(()));
    assert!(untouched.seen().is_empty());
    Ok(())
}

#[test]
fn errors_render_their_cause() {
    assert_eq!(
        OutboxError::Dialect.to_string(),
        "the outbox needs the postgres dialect"
    );
    assert_eq!(
        OutboxError::Row("x".into()).to_string(),
        "malformed outbox row: x"
    );
}
