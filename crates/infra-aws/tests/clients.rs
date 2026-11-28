//! Service clients against in-process fake HTTP servers on 127.0.0.1: each test asserts the
//! signed request on the wire and feeds back a canned reply. No AWS endpoint is contacted.

use std::io::{BufRead, BufReader, Read, Write as _};
use std::net::TcpListener;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cms_readmodel::{BlobKey, Blobs, Content, PortError, Route, RouteKey, RouteTable};
use infra_aws::sigv4::{PathStyle, Scope, Signable, sign};
use infra_aws::{
    Addressing, AmzDate, AwsClient, AwsError, CloudFront, Credentials, DynamoDb, Endpoint,
    Environment, ReadTable, S3, Write, sha256_hex,
};
use serde_json::{Value, json};

/// One request as the fake server saw it.
#[derive(Debug, Default)]
struct Seen {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Seen {
    fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map_or("", |(_, value)| value.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// Serves one canned `(status, body)` per connection, in order, and returns what it saw.
fn fake(replies: Vec<(u16, String)>) -> (Endpoint, JoinHandle<Vec<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0");
    assert!(listener.is_ok());
    let Ok(listener) = listener else {
        return (Endpoint::https("unreachable"), thread::spawn(Vec::new));
    };
    let port = listener.local_addr().map(|addr| addr.port()).unwrap_or(0);
    let endpoint = Endpoint::parse(&format!("http://127.0.0.1:{port}"));
    assert!(endpoint.is_ok());
    let handle = thread::spawn(move || {
        let mut seen = Vec::new();
        for (status, body) in replies {
            let Ok((stream, _)) = listener.accept() else {
                break;
            };
            let mut reader = BufReader::new(&stream);
            let mut request = Seen::default();
            let mut line = String::new();
            let mut length = 0;
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
                let text = line.trim_end();
                if request.method.is_empty() {
                    let mut words = text.split(' ');
                    words
                        .next()
                        .unwrap_or_default()
                        .clone_into(&mut request.method);
                    words
                        .next()
                        .unwrap_or_default()
                        .clone_into(&mut request.target);
                } else if let Some((name, value)) = text.split_once(':') {
                    let name = name.to_ascii_lowercase();
                    if name == "content-length" {
                        length = value.trim().parse().unwrap_or(0);
                    }
                    request.headers.push((name, value.trim().to_owned()));
                }
                line.clear();
            }
            request.body = vec![0; length];
            assert!(reader.read_exact(&mut request.body).is_ok());
            let reply = format!(
                "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            assert!((&stream).write_all(reply.as_bytes()).is_ok());
            seen.push(request);
        }
        seen
    });
    (
        endpoint.unwrap_or_else(|_| Endpoint::https("unreachable")),
        handle,
    )
}

fn client() -> AwsClient {
    AwsClient::new(
        Environment {
            credentials: Credentials::new("AKIDEXAMPLE", "secret", Some("session")),
            region: "us-west-2".into(),
        },
        Duration::from_secs(5),
    )
}

fn finish(handle: JoinHandle<Vec<Seen>>) -> Vec<Seen> {
    let seen = handle.join();
    assert!(seen.is_ok());
    seen.unwrap_or_default()
}

/// Recomputes the signature from the request as received and checks the wire header.
fn assert_signed(seen: &Seen, service: &str, region: &str, style: PathStyle) {
    let authorization = seen.header("authorization");
    let names: Vec<&str> = authorization
        .split("SignedHeaders=")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .unwrap_or_default()
        .split(';')
        .filter(|name| !matches!(*name, "x-amz-date" | "x-amz-security-token"))
        .collect();
    let headers: Vec<(&str, &str)> = names.iter().map(|n| (*n, seen.header(n))).collect();
    assert_eq!(seen.header("x-amz-content-sha256"), sha256_hex(&seen.body));
    assert_eq!(seen.header("x-amz-security-token"), "session");
    let date = seen.header("x-amz-date");
    let at = parse_stamp(date);
    let (path, query) = seen.target.split_once('?').unwrap_or((&seen.target, ""));
    assert!(query.is_empty(), "these clients send no query string");
    let credentials = Credentials::new("AKIDEXAMPLE", "secret", Some("session"));
    let expected = sign(
        &Scope {
            credentials: &credentials,
            region,
            service,
        },
        &Signable {
            method: &seen.method,
            path,
            query: &[],
            headers: &headers,
            payload_sha256: seen.header("x-amz-content-sha256"),
            path_style: style,
        },
        at,
    );
    assert_eq!(
        expected.last().map(|(_, value)| value.as_str()),
        Some(authorization)
    );
    assert!(authorization.contains(&format!("/{region}/{service}/aws4_request")));
}

// `YYYYMMDDTHHMMSSZ` back to Unix seconds (days by counting, fine for test dates).
fn parse_stamp(stamp: &str) -> AmzDate {
    let field = |range: std::ops::Range<usize>| -> u64 {
        stamp.get(range).and_then(|s| s.parse().ok()).unwrap_or(0)
    };
    let (year, month, day) = (field(0..4), field(4..6), field(6..8));
    let mut days = 0;
    for y in 1970..year {
        days += if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let lengths = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    days += lengths
        .iter()
        .take(usize::try_from(month).unwrap_or(1) - 1)
        .sum::<u64>();
    days += day - 1;
    AmzDate::from_unix(days * 86_400 + field(9..11) * 3600 + field(11..13) * 60 + field(13..15))
}

fn page_route() -> Route {
    Route {
        status: 200,
        content_type: "text/html; charset=UTF-8".into(),
        etag: "\"e1\"".into(),
        last_modified: "Fri, 27 Nov 2026 00:00:00 GMT".into(),
        content: Content::Page {
            shell: "fragments/shell/s1.html".into(),
            body: "rendered/p1.html".into(),
        },
    }
}

#[test]
fn route_items_are_read_with_a_signed_get_item() {
    let item = infra_aws::encode_route(&RouteKey::from("/blog/hello/"), &page_route());
    let (endpoint, server) = fake(vec![
        (200, json!({ "Item": item }).to_string()),
        (200, "{}".to_owned()),
        (
            400,
            r#"{"__type":"x#ResourceNotFoundException"}"#.to_owned(),
        ),
    ]);
    let mut table = ReadTable::new(DynamoDb::new(client(), endpoint), "fp-read");
    assert_eq!(
        table.route(&RouteKey::from("/blog/hello/")),
        Ok(Some(page_route()))
    );
    assert_eq!(table.route(&RouteKey::from("/blog/missing/")), Ok(None));
    assert_eq!(
        table.route(&RouteKey::from("/blog/x/")),
        Err(PortError::Store(
            "DynamoDB.GetItem returned HTTP 400 ResourceNotFoundException".into()
        ))
    );
    let seen = finish(server);
    assert_eq!(seen.len(), 3);
    assert_eq!(
        (seen[0].method.as_str(), seen[0].target.as_str()),
        ("POST", "/")
    );
    assert_eq!(seen[0].header("x-amz-target"), "DynamoDB_20120810.GetItem");
    assert_eq!(seen[0].header("content-type"), "application/x-amz-json-1.0");
    assert_eq!(
        seen[0].json(),
        json!({ "TableName": "fp-read",
                "Key": { "pk": { "S": "ROUTE#/blog/hello/" }, "sk": { "S": "R" } } })
    );
    assert_signed(&seen[0], "dynamodb", "us-west-2", PathStyle::Standard);
}

#[test]
fn projector_writes_are_version_gated() {
    let (endpoint, server) = fake(vec![
        (200, "{}".to_owned()),
        (
            400,
            r#"{"__type":"com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException"}"#
                .to_owned(),
        ),
        (500, r#"{"__type":"x#InternalServerError"}"#.to_owned()),
        (200, "{}".to_owned()),
    ]);
    let table = ReadTable::new(DynamoDb::new(client(), endpoint), "fp-read");
    let key = RouteKey::from("/blog/hello/");
    let version = "2026-11-27 10:00:00#0001";
    assert_eq!(
        table.put_route(&key, &page_route(), version),
        Ok(Write::Applied)
    );
    let mut attributes = infra_aws::Item::new();
    attributes.insert("title".to_owned(), json!({ "S": "Hello" }));
    assert_eq!(
        table.put_versioned("POST#7", "D", attributes.clone(), version),
        Ok(Write::Stale)
    );
    assert!(matches!(
        table.put_versioned("POST#7", "D", attributes, version),
        Err(AwsError::Service { status: 500, .. })
    ));
    assert_eq!(table.delete("ROUTE#/blog/old/", "R"), Ok(()));
    let seen = finish(server);
    let put = seen[0].json();
    assert_eq!(seen[0].header("x-amz-target"), "DynamoDB_20120810.PutItem");
    assert_eq!(
        put["ConditionExpression"],
        "attribute_not_exists(#v) OR #v < :new"
    );
    assert_eq!(put["ExpressionAttributeNames"], json!({ "#v": "version" }));
    assert_eq!(
        put["ExpressionAttributeValues"],
        json!({ ":new": { "S": version } })
    );
    assert_eq!(put["Item"]["version"], json!({ "S": version }));
    assert_eq!(put["Item"]["kind"], json!({ "S": "page" }));
    assert_eq!(seen[1].json()["Item"]["pk"], json!({ "S": "POST#7" }));
    assert_eq!(seen[1].json()["Item"]["title"], json!({ "S": "Hello" }));
    assert_eq!(
        seen[3].header("x-amz-target"),
        "DynamoDB_20120810.DeleteItem"
    );
    assert_eq!(
        seen[3].json()["Key"],
        json!({ "pk": { "S": "ROUTE#/blog/old/" }, "sk": { "S": "R" } })
    );
    assert_signed(&seen[1], "dynamodb", "us-west-2", PathStyle::Standard);
}

#[test]
fn term_listings_page_newest_first() {
    let row = json!({ "pk": { "S": "TERM#category#news" }, "sk": { "S": "2026-11-27#7" } });
    let (endpoint, server) = fake(vec![
        (
            200,
            json!({ "Items": [row], "LastEvaluatedKey": row }).to_string(),
        ),
        (200, json!({ "Items": [] }).to_string()),
        (200, json!({ "Items": [1] }).to_string()),
        (200, "{}".to_owned()),
        (
            200,
            json!({ "Items": [], "LastEvaluatedKey": 1 }).to_string(),
        ),
        (200, "[]".to_owned()),
    ]);
    let table = ReadTable::new(DynamoDb::new(client(), endpoint), "fp-read");
    let first = table.term_listing("category", "news", 10, None);
    assert!(first.as_ref().is_ok_and(|page| page.items.len() == 1));
    let next = first.ok().and_then(|page| page.next);
    assert!(next.is_some());
    let second = table.term_listing("category", "news", 10, next);
    assert!(second.is_ok_and(|page| page.items.is_empty() && page.next.is_none()));
    for what in [
        "an item is not a map",
        "Items is missing",
        "LastEvaluatedKey is not a map",
        "body is not a JSON object",
    ] {
        assert_eq!(
            table.term_listing("category", "news", 10, None),
            Err(AwsError::Malformed {
                operation: "DynamoDB.Query",
                what
            })
        );
    }
    let seen = finish(server);
    let query = seen[0].json();
    assert_eq!(query["KeyConditionExpression"], "pk = :pk");
    assert_eq!(
        query["ExpressionAttributeValues"],
        json!({ ":pk": { "S": "TERM#category#news" } })
    );
    assert_eq!(query["ScanIndexForward"], false);
    assert_eq!(query["Limit"], 10);
    assert!(query.get("ExclusiveStartKey").is_none());
    assert_eq!(
        seen[1].json()["ExclusiveStartKey"]["sk"],
        json!({ "S": "2026-11-27#7" })
    );
}

#[test]
fn malformed_get_item_replies_are_errors() {
    let (endpoint, server) = fake(vec![(200, json!({ "Item": 1 }).to_string())]);
    let db = DynamoDb::new(client(), endpoint);
    assert_eq!(
        db.get_item("t", &infra_aws::Item::new()),
        Err(AwsError::Malformed {
            operation: "DynamoDB.GetItem",
            what: "Item is not a map"
        })
    );
    finish(server);
}

#[test]
fn s3_objects_are_put_read_and_deleted() {
    let (endpoint, server) = fake(vec![
        (200, String::new()),
        (200, "<h1>Hi</h1>".to_owned()),
        (404, "<Error><Code>NoSuchKey</Code></Error>".to_owned()),
        (403, "<Error><Code>AccessDenied</Code></Error>".to_owned()),
        (204, String::new()),
    ]);
    let mut s3 = S3::new(client(), endpoint, "site-bucket", Addressing::Path);
    assert_eq!(
        s3.put_object(
            "site/rendered/a b.html",
            b"<h1>Hi</h1>",
            "text/html; charset=UTF-8",
            "public, max-age=31536000, immutable"
        ),
        Ok(())
    );
    assert_eq!(
        s3.get(&BlobKey::from("site/rendered/a b.html")),
        Ok(Some(b"<h1>Hi</h1>".to_vec()))
    );
    assert_eq!(s3.get_object("site/rendered/none.html"), Ok(None));
    assert_eq!(
        s3.get(&BlobKey::from("site/private")),
        Err(PortError::Store(
            "S3.GetObject returned HTTP 403 AccessDenied".into()
        ))
    );
    assert_eq!(s3.delete_object("site/rendered/a b.html"), Ok(()));
    assert_eq!(
        s3.delete_object(""),
        Err(AwsError::Input {
            operation: "S3.DeleteObject",
            what: "empty object key"
        })
    );
    let seen = finish(server);
    assert_eq!(
        (seen[0].method.as_str(), seen[0].target.as_str()),
        ("PUT", "/site-bucket/site/rendered/a%20b.html")
    );
    assert_eq!(seen[0].header("content-type"), "text/html; charset=UTF-8");
    assert_eq!(
        seen[0].header("cache-control"),
        "public, max-age=31536000, immutable"
    );
    assert_eq!(seen[0].body, b"<h1>Hi</h1>");
    assert_signed(&seen[0], "s3", "us-west-2", PathStyle::S3);
    assert_eq!(seen[1].method, "GET");
    assert_signed(&seen[1], "s3", "us-west-2", PathStyle::S3);
    assert_eq!(seen[4].method, "DELETE");
}

#[test]
fn regional_s3_uses_the_virtual_hosted_bucket() {
    // Nothing listens at the AWS host here; only the URL shape matters, so use an unroutable
    // fake endpoint through `new` for the virtual-hosted form.
    let (endpoint, server) = fake(vec![(200, "x".to_owned())]);
    let s3 = S3::new(client(), endpoint, "site-bucket", Addressing::VirtualHosted);
    assert_eq!(s3.get_object("k/1"), Ok(Some(b"x".to_vec())));
    let seen = finish(server);
    assert_eq!(seen[0].target, "/k/1");
    let _regional = S3::regional(client(), "site-bucket");
}

#[test]
fn invalidations_post_xml_signed_for_us_east_1() {
    let (endpoint, server) = fake(vec![
        (201, "<Invalidation><Id>I2J0I21PCUYOIK</Id></Invalidation>".to_owned()),
        (201, "<Invalidation></Invalidation>".to_owned()),
        (400, "<ErrorResponse><Error><Code>TooManyInvalidationsInProgress</Code></Error></ErrorResponse>".to_owned()),
    ]);
    let cloudfront = CloudFront::new(client(), endpoint);
    assert_eq!(
        cloudfront.create_invalidation("E2EXAMPLE", &["/blog/*", "/blog/a&b/"], "outbox-42"),
        Ok("I2J0I21PCUYOIK".into())
    );
    assert_eq!(
        cloudfront.create_invalidation("E2EXAMPLE", &["/blog/"], "r"),
        Err(AwsError::Malformed {
            operation: "CloudFront.CreateInvalidation",
            what: "no invalidation Id"
        })
    );
    assert!(matches!(
        cloudfront.create_invalidation("E2EXAMPLE", &["/blog/"], "r"),
        Err(AwsError::Service { status: 400, ref code, .. }) if &**code == "TooManyInvalidationsInProgress"
    ));
    assert_eq!(
        cloudfront.create_invalidation("E2EXAMPLE", &[], "r"),
        Err(AwsError::Input {
            operation: "CloudFront.CreateInvalidation",
            what: "no paths"
        })
    );
    let seen = finish(server);
    assert_eq!(
        (seen[0].method.as_str(), seen[0].target.as_str()),
        ("POST", "/2020-05-31/distribution/E2EXAMPLE/invalidation")
    );
    let body = String::from_utf8(seen[0].body.clone()).unwrap_or_default();
    assert!(body.contains(
        "<Paths><Quantity>2</Quantity><Items><Path>/blog/*</Path><Path>/blog/a&amp;b/</Path>\
         </Items></Paths><CallerReference>outbox-42</CallerReference>"
    ));
    assert_signed(&seen[0], "cloudfront", "us-east-1", PathStyle::Standard);
    assert_eq!(
        CloudFront::global_endpoint().base(),
        "https://cloudfront.amazonaws.com"
    );
    assert_eq!(
        DynamoDb::regional_endpoint("us-west-2").host(),
        "dynamodb.us-west-2.amazonaws.com"
    );
}

#[test]
fn unreachable_endpoints_are_transport_errors() {
    // Bind then drop a listener so the port is closed.
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .unwrap_or(1);
    let endpoint = Endpoint::parse(&format!("http://127.0.0.1:{port}"));
    assert!(endpoint.is_ok());
    let Ok(endpoint) = endpoint else {
        return;
    };
    let s3 = S3::new(client(), endpoint, "b", Addressing::Path);
    assert!(matches!(
        s3.get_object("k"),
        Err(AwsError::Transport {
            operation: "S3.GetObject",
            ..
        })
    ));
    assert_eq!(client().region(), "us-west-2");
}
