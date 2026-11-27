//! `PostgreSQL` v3 frontend/backend message framing (protocol version 3.0).

use std::io::{Read, Write};

use crate::Error;

/// Protocol version 3.0 as sent in the startup packet.
const PROTOCOL_3_0: i32 = 196_608;
/// The startup-phase code that asks the server for TLS.
const SSL_REQUEST: i32 = 80_877_103;
/// Largest backend message accepted. DSQL rows are at most 2 MiB, so a bigger frame is a
/// corrupt stream, and refusing it bounds memory.
const MAX_MESSAGE: usize = 64 * 1024 * 1024;

/// One backend message: its type byte and body (without the length word).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Message {
    pub(crate) kind: u8,
    pub(crate) body: Vec<u8>,
}

/// Writes the untyped `SSLRequest` packet.
pub(crate) fn write_ssl_request(out: &mut impl Write) -> Result<(), Error> {
    let mut packet = Vec::with_capacity(8);
    packet.extend_from_slice(&8_i32.to_be_bytes());
    packet.extend_from_slice(&SSL_REQUEST.to_be_bytes());
    out.write_all(&packet)?;
    out.flush()?;
    Ok(())
}

/// Writes the untyped `StartupMessage` naming the role and database.
pub(crate) fn write_startup(out: &mut impl Write, user: &str, database: &str) -> Result<(), Error> {
    let mut body = Vec::with_capacity(32 + user.len() + database.len());
    body.extend_from_slice(&PROTOCOL_3_0.to_be_bytes());
    for (key, value) in [("user", user), ("database", database)] {
        put_cstr(&mut body, key.as_bytes())?;
        put_cstr(&mut body, value.as_bytes())?;
    }
    body.push(0);
    let mut packet = Vec::with_capacity(body.len() + 4);
    packet.extend_from_slice(&frame_len(body.len())?.to_be_bytes());
    packet.extend_from_slice(&body);
    out.write_all(&packet)?;
    out.flush()?;
    Ok(())
}

/// Writes a typed frontend message whose body is one NUL-terminated string.
pub(crate) fn write_cstr_message(out: &mut impl Write, kind: u8, text: &[u8]) -> Result<(), Error> {
    let mut body = Vec::with_capacity(text.len() + 1);
    put_cstr(&mut body, text)?;
    write_message(out, kind, &body)
}

/// Writes a typed frontend message.
pub(crate) fn write_message(out: &mut impl Write, kind: u8, body: &[u8]) -> Result<(), Error> {
    let mut packet = Vec::with_capacity(body.len() + 5);
    packet.push(kind);
    packet.extend_from_slice(&frame_len(body.len())?.to_be_bytes());
    packet.extend_from_slice(body);
    out.write_all(&packet)?;
    out.flush()?;
    Ok(())
}

/// Reads one typed backend message.
pub(crate) fn read_message(input: &mut impl Read) -> Result<Message, Error> {
    let mut head = [0_u8; 5];
    input.read_exact(&mut head)?;
    let len = i32::from_be_bytes([head[1], head[2], head[3], head[4]]);
    // The length counts itself, so anything under 4 cannot be a frame.
    let body_len = usize::try_from(len)
        .ok()
        .and_then(|len| len.checked_sub(4))
        .filter(|len| *len <= MAX_MESSAGE)
        .ok_or(Error::Protocol("backend message length is out of range"))?;
    let mut body = vec![0_u8; body_len];
    input.read_exact(&mut body)?;
    Ok(Message {
        kind: head[0],
        body,
    })
}

// A frame length covers the 4-byte length word plus the body.
fn frame_len(body: usize) -> Result<i32, Error> {
    body.checked_add(4)
        .and_then(|len| i32::try_from(len).ok())
        .ok_or(Error::Protocol("frontend message is too large"))
}

// Strings on the wire are NUL-terminated, so an embedded NUL would truncate them silently.
fn put_cstr(out: &mut Vec<u8>, text: &[u8]) -> Result<(), Error> {
    if text.contains(&0) {
        return Err(Error::Protocol("a wire string must not contain a nul byte"));
    }
    out.extend_from_slice(text);
    out.push(0);
    Ok(())
}

/// A cursor over a backend message body.
#[derive(Debug)]
pub(crate) struct Body<'a> {
    rest: &'a [u8],
}

impl<'a> Body<'a> {
    /// A cursor at the start of `body`.
    pub(crate) const fn new(body: &'a [u8]) -> Self {
        Self { rest: body }
    }

    /// The next `n` bytes.
    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.rest.len() < n {
            return Err(Error::Protocol("backend message is truncated"));
        }
        let (head, tail) = self.rest.split_at(n);
        self.rest = tail;
        Ok(head)
    }

    /// One byte.
    pub(crate) fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    /// A big-endian `i16`.
    pub(crate) fn i16(&mut self) -> Result<i16, Error> {
        let bytes = self.take(2)?;
        Ok(i16::from_be_bytes([bytes[0], bytes[1]]))
    }

    /// A big-endian `i32`.
    pub(crate) fn i32(&mut self) -> Result<i32, Error> {
        let bytes = self.take(4)?;
        Ok(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// A NUL-terminated string, without the terminator.
    pub(crate) fn cstr(&mut self) -> Result<&'a [u8], Error> {
        let end = self
            .rest
            .iter()
            .position(|&byte| byte == 0)
            .ok_or(Error::Protocol("backend string is not terminated"))?;
        let text = self.take(end)?;
        self.take(1)?;
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::{Body, Message, read_message, write_cstr_message, write_ssl_request, write_startup};
    use crate::Error;

    #[test]
    fn startup_packet_carries_version_user_and_database() {
        let mut out = Vec::new();
        assert!(write_startup(&mut out, "app", "postgres").is_ok());
        let expected_body = b"\x00\x03\x00\x00user\0app\0database\0postgres\0\0";
        assert_eq!(&out[4..], expected_body);
        assert_eq!(out[..4], i32::try_from(out.len()).unwrap_or(0).to_be_bytes());
        assert!(matches!(
            write_startup(&mut Vec::new(), "a\0b", "postgres"),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn ssl_request_is_the_fixed_eight_bytes() {
        let mut out = Vec::new();
        assert!(write_ssl_request(&mut out).is_ok());
        assert_eq!(out, [0, 0, 0, 8, 4, 210, 22, 47]);
    }

    #[test]
    fn typed_messages_round_trip_through_the_reader() {
        let mut out = Vec::new();
        assert!(write_cstr_message(&mut out, b'Q', b"SELECT 1").is_ok());
        assert_eq!(&out, b"Q\x00\x00\x00\x0dSELECT 1\0");
        let read = read_message(&mut out.as_slice());
        assert_eq!(
            read.ok(),
            Some(Message {
                kind: b'Q',
                body: b"SELECT 1\0".to_vec()
            })
        );
        assert!(matches!(
            write_cstr_message(&mut Vec::new(), b'Q', b"a\0"),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn bad_lengths_and_short_reads_are_errors() {
        assert!(matches!(
            read_message(&mut &b"Z\x00\x00\x00\x03"[..]),
            Err(Error::Protocol(_))
        ));
        assert!(matches!(
            read_message(&mut &b"Z\x7f\x00\x00\x00"[..]),
            Err(Error::Protocol(_))
        ));
        assert!(matches!(
            read_message(&mut &b"Z\xff\xff\xff\xff"[..]),
            Err(Error::Protocol(_))
        ));
        assert!(matches!(
            read_message(&mut &b"Z\x00\x00\x00\x06I"[..]),
            Err(Error::Io(_))
        ));
    }

    #[test]
    fn body_cursor_reads_fields_and_rejects_truncation() {
        let mut body = Body::new(b"ab\0\x01\x00\x02\x00\x00\x00\x03");
        assert_eq!(body.cstr().ok(), Some(&b"ab"[..]));
        assert_eq!(body.byte().ok(), Some(1));
        assert_eq!(body.i16().ok(), Some(2));
        assert_eq!(body.i32().ok(), Some(3));
        assert!(matches!(body.byte(), Err(Error::Protocol(_))));
        assert!(matches!(
            Body::new(b"no-nul").cstr(),
            Err(Error::Protocol(_))
        ));
    }
}
