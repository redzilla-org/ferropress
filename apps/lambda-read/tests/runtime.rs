//! One invocation through a fake Runtime API on localhost: a published post comes back as the
//! Function URL response, then the API goes away and the loop returns its error.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use cms_readmodel::{BasePath, BlobKey, Blobs, Content, PortError, Route, RouteKey, RouteTable};
use lambda_read::{RuntimeError, run};

struct Routes(HashMap<RouteKey, Route>);

impl RouteTable for Routes {
    fn route(&mut self, key: &RouteKey) -> Result<Option<Route>, PortError> {
        Ok(self.0.get(key).cloned())
    }
}

struct Objects(HashMap<BlobKey, Vec<u8>>);

impl Blobs for Objects {
    fn get(&mut self, key: &BlobKey) -> Result<Option<Vec<u8>>, PortError> {
        Ok(self.0.get(key).cloned())
    }
}

const EVENT: &str = r#"{"version":"2.0","rawPath":"/blog/hello/","headers":{},"requestContext":{"http":{"method":"GET"}}}"#;

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
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
        if line == "\r\n" {
            break;
        }
        line.clear();
    }
    let mut body = vec![0; length];
    let read = reader.read_exact(&mut body);
    assert!(read.is_ok());
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
fn a_post_invocation_is_answered_through_the_runtime_api() {
    let listener = TcpListener::bind("127.0.0.1:0");
    assert!(listener.is_ok());
    let Ok(listener) = listener else {
        return;
    };
    let api = listener.local_addr().map(|addr| addr.to_string());
    assert!(api.is_ok());
    let Ok(api) = api else {
        return;
    };
    let server = thread::spawn(move || {
        let mut seen = Vec::new();
        for _ in 0..2 {
            let Ok((stream, _)) = listener.accept() else {
                return seen;
            };
            let (line, body) = read_request(&stream);
            if line.starts_with("GET ") {
                reply(
                    &stream,
                    "200 OK",
                    "lambda-runtime-aws-request-id: req-1\r\n",
                    EVENT,
                );
            } else {
                reply(&stream, "202 Accepted", "", "");
            }
            seen.push((line, body));
        }
        // Dropping the listener makes the next poll fail, which ends `run`.
        seen
    });

    let mut routes = Routes(HashMap::from([(
        RouteKey::from("/blog/hello/"),
        Route {
            status: 200,
            content_type: "text/html; charset=UTF-8".into(),
            etag: "\"e1\"".into(),
            last_modified: "Fri, 27 Nov 2026 00:00:00 GMT".into(),
            content: Content::Page {
                shell: "fragments/shell/s1.html".into(),
                body: "rendered/p1.html".into(),
            },
        },
    )]));
    let mut objects = Objects(HashMap::from([
        (
            BlobKey::from("fragments/shell/s1.html"),
            b"<main><!--fp:body--></main>".to_vec(),
        ),
        (
            BlobKey::from("rendered/p1.html"),
            b"<h1>Hello</h1>".to_vec(),
        ),
    ]));
    let base = BasePath::new("/blog/");
    assert!(base.is_ok());
    let Ok(base) = base else {
        return;
    };

    let ended = run(&api, &base, &mut routes, &mut objects);
    assert!(matches!(ended, Err(RuntimeError::Http(_))));

    let seen = server.join();
    assert!(seen.is_ok());
    let Ok(seen) = seen else {
        return;
    };
    assert_eq!(seen.len(), 2);
    assert!(
        seen[0]
            .0
            .starts_with("GET /2018-06-01/runtime/invocation/next ")
    );
    assert!(
        seen[1]
            .0
            .starts_with("POST /2018-06-01/runtime/invocation/req-1/response ")
    );
    let posted: serde_json::Value =
        serde_json::from_slice(&seen[1].1).unwrap_or(serde_json::Value::Null);
    assert_eq!(posted["statusCode"], 200);
    assert_eq!(posted["body"], "<main><h1>Hello</h1></main>");
    assert_eq!(posted["headers"]["etag"], "\"e1\"");
}
