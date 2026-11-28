//! An in-process fake `PostgreSQL` server and test doubles. For tests only (`test-util`).

use std::cell::{Cell as StdCell, RefCell};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cms_db::{Cell, Reply};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{RootCertStore, ServerConfig, ServerConnection, StreamOwned};

use crate::Error;
use crate::ports::{Clock, Connector, Endpoint, Token, TokenSource};

// Test-only certificate chain, generated for this fake with openssl (P-256, 100-year validity):
// a CA, and a server certificate for `localhost` and `127.0.0.1` it signed. The key protects
// nothing but this in-process fake.
const CA_CERT: &[u8] = include_bytes!("../fixtures/test-ca.pem");
const SERVER_CERT: &[u8] = include_bytes!("../fixtures/test-server.pem");
const SERVER_KEY: &[u8] = include_bytes!("../fixtures/test-server.key");

/// A root store holding only the fake server's test CA, for [`crate::Tls::with_roots`].
///
/// # Errors
///
/// [`Error::Tls`] when the embedded certificate does not parse.
pub fn test_roots() -> Result<RootCertStore, Error> {
    let ca = CertificateDer::from_pem_slice(CA_CERT)
        .map_err(|error| Error::Tls(error.to_string().into()))?;
    let mut roots = RootCertStore::empty();
    roots
        .add(ca)
        .map_err(|error| Error::Tls(error.to_string().into()))?;
    Ok(roots)
}

/// Backend messages a fake session sends for one statement, ending with `ReadyForQuery`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frames(Vec<u8>);

impl Frames {
    /// No messages yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A `RowDescription` of text columns followed by one `DataRow` per row (`None` is NULL).
    #[must_use]
    pub fn rows(mut self, columns: &[&str], rows: &[&[Option<&str>]]) -> Self {
        let mut body = count16(columns.len());
        for name in columns {
            body.extend_from_slice(name.as_bytes());
            body.push(0);
            // Table oid, attribute, type oid 25 (text), size -1, modifier -1, text format.
            body.extend_from_slice(&[
                0, 0, 0, 0, 0, 0, 0, 0, 0, 25, 255, 255, 255, 255, 255, 255, 0, 0,
            ]);
        }
        self = self.raw(b'T', &body);
        for row in rows {
            let mut body = count16(row.len());
            for cell in *row {
                match cell {
                    None => body.extend_from_slice(&(-1_i32).to_be_bytes()),
                    Some(text) => {
                        body.extend_from_slice(&len32(text.len()));
                        body.extend_from_slice(text.as_bytes());
                    }
                }
            }
            self = self.raw(b'D', &body);
        }
        self
    }

    /// A `CommandComplete` with `tag`.
    #[must_use]
    pub fn complete(self, tag: &str) -> Self {
        self.raw(b'C', &cstr(tag))
    }

    /// An `ErrorResponse` with this SQLSTATE and message.
    #[must_use]
    pub fn error(self, state: &str, message: &str) -> Self {
        let mut body = b"SERROR\0C".to_vec();
        body.extend_from_slice(&cstr(state));
        body.push(b'M');
        body.extend_from_slice(&cstr(message));
        body.push(0);
        self.raw(b'E', &body)
    }

    /// An `EmptyQueryResponse`.
    #[must_use]
    pub fn empty(self) -> Self {
        self.raw(b'I', &[])
    }

    /// A `NoticeResponse`, which clients must skip.
    #[must_use]
    pub fn notice(self) -> Self {
        self.raw(b'N', b"SNOTICE\0C00000\0Mnotice\0\0")
    }

    /// `ReadyForQuery` with status `I` (idle), `T` (in block) or `E` (failed block).
    #[must_use]
    pub fn ready(self, status: u8) -> Self {
        self.raw(b'Z', &[status])
    }

    /// Any typed message.
    #[must_use]
    pub fn raw(mut self, kind: u8, body: &[u8]) -> Self {
        self.0.push(kind);
        self.0.extend_from_slice(&len32(body.len() + 4));
        self.0.extend_from_slice(body);
        self
    }

    /// The frames a DSQL server sends for `reply`, idle afterwards. A `Done` with an insert id
    /// comes back as `RETURNING "ID"`; a failure carries the SQLSTATE mapped to its code.
    #[must_use]
    pub fn from_reply(reply: &Reply) -> Self {
        let frames = Self::new();
        let frames = match reply {
            Reply::Rows(set) => {
                let columns: Vec<String> = set
                    .columns()
                    .iter()
                    .map(|column| String::from_utf8_lossy(column.as_bytes()).into_owned())
                    .collect();
                let names: Vec<&str> = columns.iter().map(String::as_str).collect();
                let cells: Vec<Vec<Option<String>>> = set
                    .rows()
                    .iter()
                    .map(|row| {
                        row.cells()
                            .iter()
                            .map(|cell| match cell {
                                Cell::Null => None,
                                Cell::Bytes(bytes) => {
                                    Some(String::from_utf8_lossy(bytes).into_owned())
                                }
                            })
                            .collect()
                    })
                    .collect();
                let borrowed: Vec<Vec<Option<&str>>> = cells
                    .iter()
                    .map(|row| row.iter().map(Option::as_deref).collect())
                    .collect();
                let rows: Vec<&[Option<&str>]> = borrowed.iter().map(Vec::as_slice).collect();
                frames
                    .rows(&names, &rows)
                    .complete(&format!("SELECT {}", set.rows().len()))
            }
            Reply::Done {
                affected,
                insert_id: 0,
            } => frames.complete(&format!("UPDATE {affected}")),
            Reply::Done {
                affected,
                insert_id,
            } => frames
                .rows(&["ID"], &[&[Some(&insert_id.to_string())]])
                .complete(&format!("INSERT 0 {affected}")),
            Reply::Failed(error) => {
                let state = crate::sqlstate::state_for_code(error.code());
                frames.error(&String::from_utf8_lossy(&state), error.message())
            }
            _ => frames.error("XX000", "fake cannot encode this reply"),
        };
        frames.ready(b'I')
    }
}

/// How a fake session answers the startup handshake.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Greeting {
    /// Ask for a cleartext password and accept exactly this one.
    Password(String),
    /// Answer the handshake with these frames instead (refusals, odd auth methods).
    Frames(Frames),
}

/// One connection's script: its greeting and `(statement, frames)` answers in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// The handshake.
    pub greeting: Greeting,
    /// Expected statements and the frames sent for each.
    pub steps: Vec<(Vec<u8>, Frames)>,
}

impl Session {
    /// A session accepting `password` and answering `steps`.
    #[must_use]
    pub fn new(password: &str, steps: Vec<(&str, Frames)>) -> Self {
        Self {
            greeting: Greeting::Password(password.to_owned()),
            steps: steps
                .into_iter()
                .map(|(sql, frames)| (sql.as_bytes().to_vec(), frames))
                .collect(),
        }
    }
}

/// What the fake saw, for assertions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    /// `(user, database, password)` per connection that authenticated or tried to.
    pub logins: Vec<(String, String, String)>,
    /// Every Query text received, across connections.
    pub statements: Vec<String>,
    /// `SSLRequest` packets received.
    pub ssl_requests: usize,
}

/// A fake server on 127.0.0.1 that serves each scripted session to the next connection.
#[derive(Debug)]
pub struct FakeServer {
    addr: SocketAddr,
    seen: Arc<Mutex<Seen>>,
}

impl FakeServer {
    /// Starts serving `sessions`, one per accepted connection, on a background thread.
    ///
    /// # Errors
    ///
    /// I/O errors from binding the listener.
    pub fn start(sessions: Vec<Session>) -> Result<Self, Error> {
        Self::spawn(sessions, None)
    }

    /// Like [`Self::start`], but accepts `SSLRequest` and serves TLS with the test certificate
    /// for `localhost` and `127.0.0.1`, issued by the CA in [`test_roots`].
    ///
    /// # Errors
    ///
    /// I/O errors from binding, [`Error::Tls`] when the test certificate does not load.
    pub fn start_tls(sessions: Vec<Session>) -> Result<Self, Error> {
        let tls = |error: rustls::Error| Error::Tls(error.to_string().into());
        let pem = |error: rustls::pki_types::pem::Error| Error::Tls(error.to_string().into());
        let chain = vec![CertificateDer::from_pem_slice(SERVER_CERT).map_err(pem)?];
        let key = PrivateKeyDer::from_pem_slice(SERVER_KEY).map_err(pem)?;
        let config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(tls)?
                .with_no_client_auth()
                .with_single_cert(chain, key)
                .map_err(tls)?;
        Self::spawn(sessions, Some(Arc::new(config)))
    }

    fn spawn(sessions: Vec<Session>, tls: Option<Arc<ServerConfig>>) -> Result<Self, Error> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let seen = Arc::new(Mutex::new(Seen::default()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for session in sessions {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                // A failed session only ends that connection; the client sees the I/O error.
                let _ended = serve(stream, &session, &log, tls.as_ref());
            }
        });
        Ok(Self { addr, seen })
    }

    /// An endpoint for this server.
    #[must_use]
    pub fn endpoint(&self) -> Endpoint {
        Endpoint::new("127.0.0.1", self.addr.port(), "app", "postgres")
    }

    /// A snapshot of what the server saw so far.
    #[must_use]
    pub fn seen(&self) -> Seen {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

fn record(log: &Mutex<Seen>, change: impl FnOnce(&mut Seen)) {
    change(&mut log.lock().unwrap_or_else(PoisonError::into_inner));
}

fn serve(
    mut tcp: TcpStream,
    session: &Session,
    log: &Mutex<Seen>,
    tls: Option<&Arc<ServerConfig>>,
) -> std::io::Result<()> {
    let first = read_untyped(&mut tcp)?;
    if first != [4, 210, 22, 47] {
        return converse(&mut tcp, &first, session, log);
    }
    record(log, |seen| seen.ssl_requests += 1);
    let Some(config) = tls else {
        // A plain fake declines TLS, which sslmode=require clients must treat as fatal.
        send(&mut tcp, b"N")?;
        let startup = read_untyped(&mut tcp)?;
        return converse(&mut tcp, &startup, session, log);
    };
    send(&mut tcp, b"S")?;
    let connection = ServerConnection::new(Arc::clone(config)).map_err(std::io::Error::other)?;
    let mut stream = StreamOwned::new(connection, tcp);
    let startup = read_untyped(&mut stream)?;
    converse(&mut stream, &startup, session, log)
}

fn send(stream: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    stream.write_all(bytes)?;
    stream.flush()
}

fn converse(
    stream: &mut (impl Read + Write),
    startup: &[u8],
    session: &Session,
    log: &Mutex<Seen>,
) -> std::io::Result<()> {
    let params = parse_params(startup.get(4..).unwrap_or_default());
    let param = |key: &str| {
        params
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    match &session.greeting {
        Greeting::Frames(frames) => {
            record(log, |seen| {
                seen.logins
                    .push((param("user"), param("database"), String::new()));
            });
            send(stream, &frames.0)?;
            return drain(stream);
        }
        Greeting::Password(expected) => {
            send(stream, &Frames::new().raw(b'R', &3_i32.to_be_bytes()).0)?;
            let (kind, body) = read_typed(stream)?;
            let password =
                String::from_utf8_lossy(body.strip_suffix(&[0]).unwrap_or(&body)).into_owned();
            record(log, |seen| {
                seen.logins
                    .push((param("user"), param("database"), password.clone()));
            });
            if kind != b'p' || password != *expected {
                return send(
                    stream,
                    &Frames::new()
                        .error("28P01", "password authentication failed")
                        .0,
                );
            }
            let ok = Frames::new()
                .raw(b'R', &0_i32.to_be_bytes())
                .raw(b'S', b"server_version\x0016.0\0")
                .raw(b'K', &[0, 0, 0, 1, 0, 0, 0, 2])
                .notice()
                .ready(b'I');
            send(stream, &ok.0)?;
        }
    }
    let mut steps: VecDeque<&(Vec<u8>, Frames)> = session.steps.iter().collect();
    loop {
        let (kind, body) = read_typed(stream)?;
        if kind != b'Q' {
            return Ok(());
        }
        let sql = body.strip_suffix(&[0]).unwrap_or(&body).to_vec();
        record(log, |seen| {
            seen.statements
                .push(String::from_utf8_lossy(&sql).into_owned());
        });
        match steps.pop_front() {
            Some((expected, frames)) if *expected == sql => send(stream, &frames.0)?,
            _ => send(
                stream,
                &Frames::new()
                    .error("XX000", "fake server got an unscripted statement")
                    .ready(b'I')
                    .0,
            )?,
        }
    }
}

// Read until the client closes or terminates, so it sees the frames before EOF.
fn drain(stream: &mut impl Read) -> std::io::Result<()> {
    let mut sink = Vec::new();
    stream.read_to_end(&mut sink).map(|_| ())
}

fn read_untyped(stream: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let mut len = [0_u8; 4];
    stream.read_exact(&mut len)?;
    let mut body = vec![
        0_u8;
        usize::try_from(i32::from_be_bytes(len))
            .unwrap_or(4)
            .saturating_sub(4)
    ];
    stream.read_exact(&mut body)?;
    Ok(body)
}

fn read_typed(stream: &mut impl Read) -> std::io::Result<(u8, Vec<u8>)> {
    let mut kind = [0_u8; 1];
    stream.read_exact(&mut kind)?;
    Ok((kind[0], read_untyped(stream)?))
}

fn parse_params(body: &[u8]) -> Vec<(String, String)> {
    let words: Vec<String> = body
        .split(|&byte| byte == 0)
        .map(|word| String::from_utf8_lossy(word).into_owned())
        .collect();
    words
        .chunks(2)
        .filter(|pair| pair.len() == 2 && !pair[0].is_empty())
        .map(|pair| (pair[0].clone(), pair[1].clone()))
        .collect()
}

fn count16(count: usize) -> Vec<u8> {
    i16::try_from(count)
        .unwrap_or(i16::MAX)
        .to_be_bytes()
        .to_vec()
}

fn len32(len: usize) -> [u8; 4] {
    i32::try_from(len).unwrap_or(i32::MAX).to_be_bytes()
}

fn cstr(text: &str) -> Vec<u8> {
    let mut out = text.as_bytes().to_vec();
    out.push(0);
    out
}

/// Plain TCP to the endpoint. Test-only: DSQL requires TLS, so production uses the TLS connector.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlainTcp;

impl Connector for PlainTcp {
    type Stream = TcpStream;

    fn connect(&self, endpoint: &Endpoint) -> Result<TcpStream, Error> {
        Ok(TcpStream::connect((endpoint.host(), endpoint.port()))?)
    }
}

/// Hands out the given secrets in order, counting calls.
#[derive(Clone, Debug)]
pub struct ListedTokens {
    secrets: Rc<RefCell<VecDeque<String>>>,
    lifetime: Duration,
    calls: Rc<StdCell<usize>>,
}

impl ListedTokens {
    /// Tokens with these secrets, each valid for `lifetime`.
    #[must_use]
    pub fn new(secrets: &[&str], lifetime: Duration) -> Self {
        Self {
            secrets: Rc::new(RefCell::new(
                secrets.iter().map(|secret| (*secret).to_owned()).collect(),
            )),
            lifetime,
            calls: Rc::new(StdCell::new(0)),
        }
    }

    /// How many tokens were requested.
    #[must_use]
    pub fn calls(&self) -> usize {
        self.calls.get()
    }
}

impl TokenSource for ListedTokens {
    fn token(&mut self, _endpoint: &Endpoint) -> Result<Token, Error> {
        self.calls.set(self.calls.get() + 1);
        let secret = self
            .secrets
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| Error::Token("no credentials left".into()))?;
        Ok(Token::new(secret, self.lifetime))
    }
}

/// A clock moved only by [`Self::advance`] and by `sleep`, which records each delay.
#[derive(Clone, Debug)]
pub struct ManualClock {
    now: Rc<StdCell<Instant>>,
    sleeps: Rc<RefCell<Vec<Duration>>>,
}

impl Default for ManualClock {
    fn default() -> Self {
        Self {
            now: Rc::new(StdCell::new(Instant::now())),
            sleeps: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

impl ManualClock {
    /// Moves time forward.
    pub fn advance(&self, by: Duration) {
        self.now.set(self.now.get() + by);
    }

    /// Every delay slept so far.
    #[must_use]
    pub fn sleeps(&self) -> Vec<Duration> {
        self.sleeps.borrow().clone()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        self.now.get()
    }

    fn sleep(&self, delay: Duration) {
        self.sleeps.borrow_mut().push(delay);
        self.advance(delay);
    }
}
