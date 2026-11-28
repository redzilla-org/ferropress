//! The real composition root end to end: environment -> `DynamoDB`/S3 adapters -> Runtime API
//! loop, against two fake HTTP servers on 127.0.0.1 (one Runtime API, one AWS). Warm
//! invocations serve the home page, a post, a category archive, the feed and the sitemap, and
//! fetch the shared page shell once.

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

const SHELL: &str = "site/fragments/shell/s1.html";
const HTML: &str = "text/html; charset=UTF-8";

/// One published route: its path, item content, content type, body object key and bytes.
struct Published {
    path: &'static str,
    content_type: &'static str,
    body_key: &'static str,
    body: &'static str,
    page: bool,
}

// What the projector publishes for each kind of public URL the read path serves.
const SITE: [Published; 5] = [
    Published {
        path: "/blog/",
        content_type: HTML,
        body_key: "site/rendered/home.html",
        body: "<h1>Latest posts</h1>",
        page: true,
    },
    Published {
        path: "/blog/2026/11/hello/",
        content_type: HTML,
        body_key: "site/rendered/p1.html",
        body: "<h1>Hello</h1>",
        page: true,
    },
    Published {
        path: "/blog/category/news/",
        content_type: HTML,
        body_key: "site/rendered/category-news.html",
        body: "<h1>Category: News</h1>",
        page: true,
    },
    Published {
        path: "/blog/feed/",
        content_type: "application/rss+xml; charset=UTF-8",
        body_key: "site/rendered/feed.xml",
        body: "<rss version=\"2.0\"><channel><item><title>Hello</title></item></channel></rss>",
        page: false,
    },
    Published {
        path: "/blog/sitemap.xml",
        content_type: "application/xml; charset=UTF-8",
        body_key: "site/rendered/sitemap.xml",
        body: "<urlset><url><loc>https://example.test/blog/2026/11/hello/</loc></url></urlset>",
        page: false,
    },
];

impl Published {
    /// The `GetItem` reply holding this route item.
    fn item(&self) -> String {
        let body = self.body_key.into();
        let content = if self.page {
            Content::Page {
                shell: SHELL.into(),
                body,
            }
        } else {
            Content::Object { body }
        };
        let route = Route {
            status: 200,
            content_type: self.content_type.into(),
            etag: "\"e1\"".into(),
            last_modified: "Fri, 27 Nov 2026 00:00:00 GMT".into(),
            content,
        };
        json!({ "Item": infra_aws::encode_route(&RouteKey::from(self.path), &route) }).to_string()
    }

    /// The bytes a client receives.
    fn served(&self) -> String {
        if self.page {
            format!("<main>{}</main>", self.body)
        } else {
            self.body.to_owned()
        }
    }

    /// The Function URL event requesting this path.
    fn event(&self) -> String {
        json!({
            "version": "2.0",
            "rawPath": self.path,
            "headers": {},
            "requestContext": { "http": { "method": "GET" } },
        })
        .to_string()
    }
}

#[test]
fn warm_invocations_serve_every_route_kind_and_reuse_the_shell() {
    // AWS sees GetItem then the shell (first page only) and the body object per invocation.
    let ok = || "200 OK\r\n".to_owned();
    let mut aws_replies = Vec::new();
    let mut expected_aws = Vec::new();
    let mut api_replies = Vec::new();
    for (n, published) in SITE.iter().enumerate() {
        aws_replies.push((ok(), published.item()));
        expected_aws.push("POST / HTTP/1.1".to_owned());
        if n == 0 {
            aws_replies.push((ok(), "<main><!--fp:body--></main>".to_owned()));
            expected_aws.push(format!("GET /site-bucket/{SHELL} HTTP/1.1"));
        }
        aws_replies.push((ok(), published.body.to_owned()));
        expected_aws.push(format!("GET /site-bucket/{} HTTP/1.1", published.body_key));
        api_replies.push((
            format!("200 OK\r\nlambda-runtime-aws-request-id: req-{n}\r\n"),
            published.event(),
        ));
        api_replies.push(("202 Accepted\r\n".to_owned(), String::new()));
    }
    let (aws, aws_server) = fake(aws_replies);
    let (api, api_server) = fake(api_replies);

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
    // The fake Runtime API closes after the last invocation, which ends the loop.
    assert!(matches!(
        start(config),
        Err(StartError::Runtime(RuntimeError::Http(_)))
    ));

    // Each invocation's response carries the route's status, content type and assembled bytes.
    let api_seen = api_server.join().unwrap_or_default();
    assert_eq!(api_seen.len(), 2 * SITE.len());
    for (n, published) in SITE.iter().enumerate() {
        let posted = &api_seen[2 * n + 1];
        assert!(posted.0.starts_with(&format!(
            "POST /2018-06-01/runtime/invocation/req-{n}/response "
        )));
        let response: Value = serde_json::from_slice(&posted.1).unwrap_or(Value::Null);
        assert_eq!(response["statusCode"], 200, "{}", published.path);
        assert_eq!(response["headers"]["content-type"], published.content_type);
        assert_eq!(response["body"], published.served());
    }

    // The table is asked for exactly each requested route; the shell is fetched once.
    let aws_seen = aws_server.join().unwrap_or_default();
    let lines: Vec<&str> = aws_seen.iter().map(|(line, _)| line.trim_end()).collect();
    assert_eq!(lines, expected_aws);
    let asked: Vec<Value> = aws_seen
        .iter()
        .filter(|(line, _)| line.starts_with("POST "))
        .map(|(_, body)| serde_json::from_slice::<Value>(body).unwrap_or(Value::Null))
        .map(|request| request["Key"]["pk"]["S"].clone())
        .collect();
    let wanted: Vec<Value> = SITE
        .iter()
        .map(|published| Value::from(format!("ROUTE#{}", published.path)))
        .collect();
    assert_eq!(asked, wanted);
}

#[test]
fn missing_configuration_stops_startup() {
    assert!(matches!(
        Config::from_lookup(|_| None),
        Err(StartError::Config(_))
    ));
}
