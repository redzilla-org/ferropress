//! Query strings: reading one var and writing one back.

/// The decoded value of `name` in `raw` (no leading `?`), last occurrence winning as in the
/// reference's request parsing. `+` is a space; a malformed `%` escape stays literal.
pub(crate) fn param(raw: &str, name: &str) -> Option<Vec<u8>> {
    raw.split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(key) == name.as_bytes()).then(|| decode(value))
        })
        .next_back()
}

fn decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let escaped = (bytes[at] == b'%')
            .then(|| bytes.get(at + 1..at + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match (bytes[at], escaped) {
            (_, Some(byte)) => {
                out.push(byte);
                at += 3;
            }
            (b'+', None) => {
                out.push(b' ');
                at += 1;
            }
            (byte, None) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    out
}

/// `value` encoded for a query string: unreserved bytes kept, space as `+`, the rest `%XX`.
pub(crate) fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(char::from(HEX[usize::from(byte >> 4)]));
                out.push(char::from(HEX[usize::from(byte & 0xf)]));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{encode, param};

    #[test]
    fn params_decode_and_last_wins() {
        assert_eq!(
            param("s=red+boat&x=1", "s").as_deref(),
            Some(&b"red boat"[..])
        );
        assert_eq!(param("s=a&s=b", "s").as_deref(), Some(&b"b"[..]));
        assert_eq!(param("s", "s").as_deref(), Some(&b""[..]));
        assert_eq!(
            param("s=%22x%22%2By", "s").as_deref(),
            Some(&b"\"x\"+y"[..])
        );
        assert_eq!(param("s=100%", "s").as_deref(), Some(&b"100%"[..]));
        assert_eq!(param("s=%zz", "s").as_deref(), Some(&b"%zz"[..]));
        assert_eq!(
            param("s=caf%C3%A9", "s").as_deref(),
            Some("caf\u{e9}".as_bytes())
        );
        assert_eq!(param("%73=1", "s").as_deref(), Some(&b"1"[..]));
        assert_eq!(param("paged=2", "s"), None);
        assert_eq!(param("", "s"), None);
    }

    #[test]
    fn values_encode_for_links() {
        assert_eq!(encode("red boat"), "red+boat");
        assert_eq!(encode("a&b=\"c\""), "a%26b%3D%22c%22");
        assert_eq!(encode("caf\u{e9}-x_y.z"), "caf%C3%A9-x_y.z");
    }
}
