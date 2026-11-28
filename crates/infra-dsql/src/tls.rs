//! TLS connector: `SSLRequest`, then a verified rustls session (sslmode=require semantics).

use std::fmt;
use std::io::Read;
use std::net::TcpStream;
use std::sync::Arc;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use crate::ports::{Connector, Endpoint};
use crate::{Error, wire};

/// Connects over TCP and upgrades to TLS before startup, refusing plaintext.
#[derive(Clone)]
pub struct Tls {
    config: Arc<ClientConfig>,
}

impl fmt::Debug for Tls {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Tls").finish_non_exhaustive()
    }
}

impl Tls {
    /// Verifies servers against the compiled-in Mozilla roots, which cover DSQL's public CA.
    ///
    /// # Errors
    ///
    /// [`Error::Tls`] when the crypto provider rejects the default protocol versions.
    pub fn new() -> Result<Self, Error> {
        Self::with_roots(RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        })
    }

    /// Verifies servers against `roots` only (private CAs, tests).
    ///
    /// # Errors
    ///
    /// [`Error::Tls`] when the crypto provider rejects the default protocol versions.
    pub fn with_roots(roots: RootCertStore) -> Result<Self, Error> {
        // ring keeps the build free of a C toolchain, matching the workspace's TLS stack.
        let config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|error| Error::Tls(error.to_string().into()))?
                .with_root_certificates(roots)
                .with_no_client_auth();
        Ok(Self {
            config: Arc::new(config),
        })
    }
}

impl Connector for Tls {
    type Stream = StreamOwned<ClientConnection, TcpStream>;

    fn connect(&self, endpoint: &Endpoint) -> Result<Self::Stream, Error> {
        let name = ServerName::try_from(endpoint.host().to_owned())
            .map_err(|error| Error::Tls(error.to_string().into()))?;
        let mut tcp = TcpStream::connect((endpoint.host(), endpoint.port()))?;
        // Small request/response frames: Nagle would add a round trip of latency per statement.
        tcp.set_nodelay(true)?;
        wire::write_ssl_request(&mut tcp)?;
        let mut answer = [0_u8; 1];
        tcp.read_exact(&mut answer)?;
        match answer[0] {
            b'S' => {}
            b'N' => return Err(Error::TlsRefused),
            _ => return Err(Error::Protocol("unexpected answer to the TLS request")),
        }
        let mut session = ClientConnection::new(Arc::clone(&self.config), name)
            .map_err(|error| Error::Tls(error.to_string().into()))?;
        // Handshake now so certificate failures surface as connect errors, not on first query.
        while session.is_handshaking() {
            session
                .complete_io(&mut tcp)
                .map_err(|error| Error::Tls(error.to_string().into()))?;
        }
        Ok(StreamOwned::new(session, tcp))
    }
}
