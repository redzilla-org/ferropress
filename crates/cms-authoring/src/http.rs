//! Requests and responses at the Function URL boundary, plus form and cookie parsing.

/// The request method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// `GET`.
    Get,
    /// `HEAD`.
    Head,
    /// `POST`.
    Post,
    /// `PUT`.
    Put,
    /// `DELETE`.
    Delete,
    /// Anything else.
    Other,
}

impl Method {
    /// Parses an HTTP method token.
    #[must_use]
    pub fn parse(token: &str) -> Self {
        match token {
            "GET" => Self::Get,
            "HEAD" => Self::Head,
            "POST" => Self::Post,
            "PUT" => Self::Put,
            "DELETE" => Self::Delete,
            _ => Self::Other,
        }
    }

    /// Whether the method only reads.
    #[must_use]
    pub const fn is_read(self) -> bool {
        matches!(self, Self::Get | Self::Head)
    }
}

/// One admin or REST request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// Method.
    pub method: Method,
    /// Raw path, base path included.
    pub path: String,
    /// Raw query string without `?`.
    pub query: String,
    /// Headers with lower-case names.
    pub headers: Vec<(String, String)>,
    /// `name=value` cookie pairs.
    pub cookies: Vec<String>,
    /// Decoded body bytes.
    pub body: Vec<u8>,
}

impl Request {
    /// The first header named `name` (lower case).
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The value of cookie `name`.
    #[must_use]
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies
            .iter()
            .flat_map(|pair| pair.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value)
    }
}

/// One response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    /// Status code.
    pub status: u16,
    /// Headers.
    pub headers: Vec<(&'static str, String)>,
    /// Body bytes.
    pub body: Vec<u8>,
}

impl Response {
    /// A response that no cache may store; every admin and REST answer is one.
    #[must_use]
    pub fn new(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: vec![
                ("content-type", content_type.to_owned()),
                ("cache-control", "no-store".to_owned()),
            ],
            body,
        }
    }

    /// A redirect to `location`.
    #[must_use]
    pub fn redirect(status: u16, location: &str) -> Self {
        let mut response = Self::new(status, "text/plain; charset=UTF-8", Vec::new());
        response.headers.push(("location", location.to_owned()));
        response
    }

    /// The first header named `name`.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// `application/x-www-form-urlencoded` pairs (also query strings), in order.
#[must_use]
pub fn form(bytes: &[u8]) -> Vec<(String, String)> {
    bytes
        .split(|&byte| byte == b'&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let mut halves = pair.splitn(2, |&byte| byte == b'=');
            let key = halves.next().unwrap_or_default();
            let value = halves.next().unwrap_or_default();
            (decode(key), decode(value))
        })
        .collect()
}

/// The first value of `name` in `pairs`.
#[must_use]
pub fn field<'p>(pairs: &'p [(String, String)], name: &str) -> Option<&'p str> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// Percent-encodes `text` for a query value.
#[must_use]
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    out
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn decode(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => out.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                match (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                    (Some(high), Some(low)) => {
                        out.push(high << 4 | low);
                        index += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            other => out.push(other),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

const fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Method, Request, Response, encode, field, form};

    #[test]
    fn methods_parse_and_classify() {
        for (token, method) in [
            ("GET", Method::Get),
            ("HEAD", Method::Head),
            ("POST", Method::Post),
            ("PUT", Method::Put),
            ("DELETE", Method::Delete),
            ("PATCH", Method::Other),
        ] {
            assert_eq!(Method::parse(token), method);
        }
        assert!(Method::Get.is_read() && Method::Head.is_read());
        assert!(!Method::Post.is_read());
    }

    #[test]
    fn forms_decode_plus_percent_and_repeats() {
        let pairs = form(b"title=Hello+World&category=1&category=2&x=%E2%80%94&bad=%zz&k&tail=%4");
        assert_eq!(field(&pairs, "title"), Some("Hello World"));
        assert_eq!(field(&pairs, "x"), Some("\u{2014}"));
        assert_eq!(field(&pairs, "bad"), Some("%zz"));
        assert_eq!(field(&pairs, "k"), Some(""));
        assert_eq!(field(&pairs, "tail"), Some("%4"));
        assert_eq!(field(&pairs, "missing"), None);
        assert_eq!(pairs.iter().filter(|(key, _)| key == "category").count(), 2);
        assert!(form(b"").is_empty());
        assert_eq!(encode("/blog/a b?c"), "/blog/a%20b%3Fc");
    }

    #[test]
    fn headers_and_cookies_are_found_by_name() {
        let request = Request {
            method: Method::Get,
            path: "/".into(),
            query: String::new(),
            headers: vec![("x-fp-nonce".into(), "n".into())],
            cookies: vec!["a=1".into(), "fp_session=tok; other=2".into()],
            body: Vec::new(),
        };
        assert_eq!(request.header("x-fp-nonce"), Some("n"));
        assert_eq!(request.header("missing"), None);
        assert_eq!(request.cookie("fp_session"), Some("tok"));
        assert_eq!(request.cookie("other"), Some("2"));
        assert_eq!(request.cookie("none"), None);
        let response = Response::redirect(303, "/blog/admin/");
        assert_eq!(response.header("location"), Some("/blog/admin/"));
        assert_eq!(response.header("cache-control"), Some("no-store"));
        assert_eq!(response.header("nope"), None);
    }
}
