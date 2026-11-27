//! The real composition root end to end: environment -> `DynamoDB`/S3 adapters -> Runtime API
//! loop, against two fake HTTP servers on 127.0.0.1 (one Runtime API, one AWS). Two warm
//! invocations of the same post fetch its shell once.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread::{self, JoinHandle};

use cms_readmodel::{Content, Route, RouteKey};
use lambda_read::{Config, RuntimeError, StartError, start};
use serde_json::{Value, json};

/// Request line and body as received.
type Seen = Vec<(String, Vec<u8>)>;

/// Answers one canned reply per connection and records what it received.
fn fake(replies: Vec<(String, String)>) -> (String, JoinHandle<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0");
    assert!(listener.is_ok());
    let Ok(listener) = listener else {
        return (String::new(), thread::spawn(Vec::new));
    };
    let addr = listener
        .local_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_default();
    let handle = thread::spawn(move || {
        let mut seen = Vec::new();
        for (head, body) in replies {
            let Ok((stream, _)) = listener.accept() else {
                break;
            };
            let mut reader = BufReader::new(&stream);
            let (mut line, mut first, mut length) = (String::new(), String::new(), 0);
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
                if first.is_empty() {
                    first.clone_from(&line);
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
                line.clear();
            }
            let mut request = vec![0; length];
            assert!(reader.read_exact(&mut request).is_ok());
            let reply = format!(
                "HTTP/1.1 {head}content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            assert!((&stream).write_all(reply.as_bytes()).is_ok());
            seen.push((first, request));
        }
        seen
    });
    (addr, handle)
}

const EVENT: &str = r#"{"version":"2.0","rawPath":"/blog/hello/","headers":{},"requestContext":{"http":{"method":"GET"}}}"#;

#[test]
fn warm_invocations_serve_a_post_and_reuse_the_shell() {
    let route = Route {
        status: 200,
        content_type: "text/html; charset=UTF-8".into(),
        etag: "\"e1\"".into(),
        last_modified: "Fri, 27 Nov 2026 00:00:00 GMT".into(),
        content: Content::Page {
            shell: "site/fragments/shell/s1.html".into(),
            body: "site/rendered/p1.html".into(),
        },
    };
    let item = json!({ "Item": infra_aws::encode_route(&RouteKey::from("/blog/hello/"), &route) })
        .to_string();
    let ok = || "200 OK\r\n".to_owned();
    let (aws, aws_server) = fake(vec![
        (ok(), item.clone()),
        (ok(), "<main><!--fp:body--></main>".to_owned()),
        (ok(), "<h1>Hello</h1>".to_owned()),
        (ok(), item),
        (ok(), "<h1>Hello</h1>".to_owned()),
    ]);
    let next = || {
        (
            "200 OK\r\nlambda-runtime-aws-request-id: req-1\r\n".to_owned(),
            EVENT.to_owned(),
        )
    };
    let accepted = || ("202 Accepted\r\n".to_owned(), String::new());
    let (api, api_server) = fake(vec![next(), accepted(), next(), accepted()]);

    let endpoint = format!("http://{aws}");
    let config = Config::from_lookup(|name| {
        let value = match name {
            "AWS_ACCESS_KEY_ID" => "AKID",
            "AWS_SECRET_ACCESS_KEY" => "secret",
            "AWS_SESSION_TOKEN" => "token",
            "AWS_REGION" => "us-east-1",
            "AWS_LAMBDA_RUNTIME_API" => &api,
            "FP_BASE_PATH" => "/blog/",
            "FP_TABLE" => "fp-read",
            "FP_BUCKET" => "site-bucket",
            "FP_DYNAMODB_ENDPOINT" | "FP_S3_ENDPOINT" => &endpoint,
            _ => return None,
        };
        Some(value.to_owned())
    });
    assert!(config.is_ok());
    let Ok(config) = config else {
        return;
    };
    // The fake Runtime API closes after two invocations, which ends the loop.
    assert!(matches!(
        start(config),
        Err(StartError::Runtime(RuntimeError::Http(_)))
    ));

    let api_seen = api_server.join().unwrap_or_default();
    assert_eq!(api_seen.len(), 4);
    for posted in [&api_seen[1], &api_seen[3]] {
        assert!(
            posted
                .0
                .starts_with("POST /2018-06-01/runtime/invocation/req-1/response ")
        );
        let response: Value = serde_json::from_slice(&posted.1).unwrap_or(Value::Null);
        assert_eq!(response["statusCode"], 200);
        assert_eq!(response["body"], "<main><h1>Hello</h1></main>");
    }
    let aws_seen = aws_server.join().unwrap_or_default();
    let lines: Vec<&str> = aws_seen.iter().map(|(line, _)| line.trim_end()).collect();
    assert_eq!(
        lines,
        [
            "POST / HTTP/1.1",
            "GET /site-bucket/site/fragments/shell/s1.html HTTP/1.1",
            "GET /site-bucket/site/rendered/p1.html HTTP/1.1",
            "POST / HTTP/1.1",
            "GET /site-bucket/site/rendered/p1.html HTTP/1.1",
        ]
    );
}

#[test]
fn missing_configuration_stops_startup() {
    assert!(matches!(
        Config::from_lookup(|_| None),
        Err(StartError::Config(_))
    ));
}
