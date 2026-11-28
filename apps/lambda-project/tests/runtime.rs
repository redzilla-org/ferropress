//! Two invocations through a fake Runtime API on localhost: the empty nudge and the scheduled
//! event each run the job once and get a summary back; then the API goes away and `run` ends.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use cms_projector::{Clock, Report};
use lambda_project::{RuntimeError, run};

const SCHEDULED: &str = r#"{"version":"0","detail-type":"Scheduled Event","source":"aws.scheduler","detail":{}}"#;

struct Fixed;

impl Clock for Fixed {
    fn now_micros(&self) -> i64 {
        1_500_000
    }
}

// Reads one HTTP/1.1 request and returns its request line and body.
fn read_request(stream: &TcpStream) -> (String, Vec<u8>) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let mut first = String::new();
    let mut length = 0_usize;
    while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
        if first.is_empty() {
            first.clone_from(&line);
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
        if line == "\r\n" {
            break;
        }
        line.clear();
    }
    let mut body = vec![0; length];
    assert!(reader.read_exact(&mut body).is_ok());
    (first, body)
}

fn reply(mut stream: &TcpStream, status: &str, extra: &str, body: &str) {
    let text = format!(
        "HTTP/1.1 {status}\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    assert!(stream.write_all(text.as_bytes()).is_ok());
}

#[test]
fn the_nudge_and_the_schedule_each_run_the_job_once() {
    let Ok(listener) = TcpListener::bind("127.0.0.1:0") else {
        panic_free_fail("bind");
        return;
    };
    let Ok(api) = listener.local_addr().map(|addr| addr.to_string()) else {
        panic_free_fail("addr");
        return;
    };
    let server = thread::spawn(move || {
        let mut seen = Vec::new();
        let payloads = ["", SCHEDULED];
        let mut polls = 0;
        for _ in 0..4 {
            let Ok((stream, _)) = listener.accept() else {
                return seen;
            };
            let (line, body) = read_request(&stream);
            if line.starts_with("GET ") {
                let id = format!("lambda-runtime-aws-request-id: req-{polls}\r\n");
                reply(&stream, "200 OK", &id, payloads[polls]);
                polls += 1;
            } else {
                reply(&stream, "202 Accepted", "", "");
            }
            seen.push((line, body));
        }
        seen
    });

    let mut runs = 0;
    let mut metrics = Vec::new();
    let ended = run(&api, &Fixed, &mut metrics, || {
        runs += 1;
        Ok(Report {
            rows: 1,
            age_seconds: 3,
            ..Report::default()
        })
    });
    assert!(matches!(ended, Err(RuntimeError::Http(_))));
    assert_eq!(runs, 2);
    let lines = String::from_utf8_lossy(&metrics).lines().count();
    assert_eq!(lines, 2);

    let Ok(seen) = server.join() else {
        panic_free_fail("join");
        return;
    };
    assert_eq!(seen.len(), 4);
    for (index, id) in [(1, "req-0"), (3, "req-1")] {
        assert!(
            seen[index]
                .0
                .starts_with(&format!("POST /2018-06-01/runtime/invocation/{id}/response ")),
            "{}",
            seen[index].0
        );
        let posted: serde_json::Value =
            serde_json::from_slice(&seen[index].1).unwrap_or(serde_json::Value::Null);
        assert_eq!(posted["rows"], 1);
        assert_eq!(posted["ageSeconds"], 3);
    }
}

// The workspace denies `panic!`; a failed precondition is a failed assertion instead.
fn panic_free_fail(what: &str) {
    assert!(what.is_empty(), "test setup failed: {what}");
}
