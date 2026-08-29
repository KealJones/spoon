//! The single host-interaction seam: name resolution and one HTTP exchange.
//!
//! Both implementations honor this contract exactly, including the streaming
//! byte cap and the wall-clock deadline, so an offline test exercises the same
//! adapter logic a real request does. Redirects are deliberately outside the
//! seam: the adapter follows them itself so every hop is revalidated.

use std::collections::BTreeMap;
use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spoon_capability::CapabilityError;

/// Stable prefixes so a caller can classify these two outcomes without
/// parsing a whole message.
pub const OVERSIZED_MESSAGE: &str = "network response exceeded the byte bound";
pub const TIMEOUT_MESSAGE: &str = "network request exceeded the wall-clock bound";

const READ_CHUNK_BYTES: usize = 8 * 1024;

/// One fully validated exchange. Every field is decided by the adapter; a
/// transport adds nothing of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportRequest {
    pub url: String,
    pub host: String,
    pub port: u16,
    pub method: String,
    /// Every address the adapter validated. A transport must connect only to
    /// one of these and must never resolve the host again.
    pub addresses: Vec<IpAddr>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub max_body_bytes: usize,
    pub connect_timeout: Duration,
    pub total_timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl TransportResponse {
    pub fn new(status: u16, headers: Vec<(String, String)>, body: Vec<u8>) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }

    /// First value for a header name, matched case-insensitively as HTTP
    /// requires.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub trait HttpTransport: Send {
    /// Every address the name currently maps to. The adapter validates all of
    /// them before anything is dialed.
    fn resolve(&mut self, host: &str, port: u16) -> Result<Vec<IpAddr>, CapabilityError>;

    /// Perform exactly one exchange, without following redirects.
    fn send(&mut self, request: &TransportRequest) -> Result<TransportResponse, CapabilityError>;
}

/// The production transport.
///
/// A fresh client is built per exchange so the resolver override carries only
/// the addresses validated for that exchange. That costs a TLS handshake per
/// request and gives up connection reuse, which is the intended trade: a
/// long-lived client would either cache overrides across hosts or fall back to
/// the system resolver on a hop the adapter has not validated.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReqwestTransport;

impl HttpTransport for ReqwestTransport {
    fn resolve(&mut self, host: &str, port: u16) -> Result<Vec<IpAddr>, CapabilityError> {
        let addresses: Vec<IpAddr> = (host, port)
            .to_socket_addrs()
            .map_err(|error| {
                CapabilityError::Invalid(format!("network host resolution failed: {error}"))
            })?
            .map(|address| address.ip())
            .collect();
        if addresses.is_empty() {
            return Err(CapabilityError::Invalid(
                "network host resolved to no addresses".into(),
            ));
        }
        Ok(addresses)
    }

    fn send(&mut self, request: &TransportRequest) -> Result<TransportResponse, CapabilityError> {
        let sockets: Vec<SocketAddr> = request
            .addresses
            .iter()
            .map(|address| SocketAddr::new(*address, request.port))
            .collect();
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(request.connect_timeout)
            .timeout(request.total_timeout)
            .resolve_to_addrs(&request.host, &sockets)
            .build()
            .map_err(transport_failure)?;
        let method = reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|_| {
            CapabilityError::Invalid("network method is not a valid HTTP token".into())
        })?;
        let mut builder = client.request(method, &request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if !request.body.is_empty() {
            builder = builder.body(request.body.clone());
        }

        let deadline = Instant::now() + request.total_timeout;
        let response = builder.send().map_err(transport_failure)?;
        let status = response.status().as_u16();
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        let body = read_bounded(response, request.max_body_bytes, deadline)?;
        Ok(TransportResponse {
            status,
            headers,
            body,
        })
    }
}

/// A deterministic transport answering from a canned table.
///
/// It enforces the same streaming cap and deadline the production transport
/// does, so a test that passes here describes real adapter behavior. It has no
/// socket, so `addresses` is recorded rather than dialed; the adapter has
/// already refused anything unroutable by the time a request arrives.
#[derive(Debug, Clone, Default)]
pub struct MemoryTransport {
    addresses: BTreeMap<String, Vec<IpAddr>>,
    responses: BTreeMap<(String, String), TransportResponse>,
    sent: RequestLog,
}

/// A handle to what a [`MemoryTransport`] was actually asked to send. It is
/// shared rather than owned because the transport is moved into an adapter
/// before any request happens.
pub type RequestLog = Arc<Mutex<Vec<TransportRequest>>>;

impl MemoryTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_addresses(mut self, host: &str, addresses: Vec<IpAddr>) -> Self {
        self.addresses.insert(host.to_owned(), addresses);
        self
    }

    pub fn with_response(mut self, method: &str, url: &str, response: TransportResponse) -> Self {
        self.responses
            .insert((method.to_owned(), url.to_owned()), response);
        self
    }

    /// Every request handed to this transport, in order. Tests use it to prove
    /// a secret header really did reach the wire while staying out of the
    /// receipt, the output, and every error message.
    pub fn log(&self) -> RequestLog {
        Arc::clone(&self.sent)
    }
}

impl HttpTransport for MemoryTransport {
    fn resolve(&mut self, host: &str, _port: u16) -> Result<Vec<IpAddr>, CapabilityError> {
        self.addresses.get(host).cloned().ok_or_else(|| {
            CapabilityError::Invalid(format!(
                "network host resolution failed: no address for {host}"
            ))
        })
    }

    fn send(&mut self, request: &TransportRequest) -> Result<TransportResponse, CapabilityError> {
        // A poisoned log costs the recording, never the contract.
        if let Ok(mut sent) = self.sent.lock() {
            sent.push(request.clone());
        }
        let key = (request.method.clone(), request.url.clone());
        let response = self.responses.get(&key).cloned().ok_or_else(|| {
            // The URL can carry a caller-supplied query, so it stays out of
            // the message even in a test-only transport.
            CapabilityError::Invalid("network transport has no response for the request".into())
        })?;
        let body = read_bounded(
            response.body.as_slice(),
            request.max_body_bytes,
            Instant::now() + request.total_timeout,
        )?;
        Ok(TransportResponse { body, ..response })
    }
}

/// Read a body while holding at most `max_bytes`. The cap is checked before
/// each chunk is appended, so an oversized response is abandoned mid-download
/// and the connection is dropped rather than buffered and then rejected.
fn read_bounded<R: Read>(
    mut source: R,
    max_bytes: usize,
    deadline: Instant,
) -> Result<Vec<u8>, CapabilityError> {
    let mut body = Vec::new();
    let mut buffer = [0u8; READ_CHUNK_BYTES];
    loop {
        if Instant::now() >= deadline {
            return Err(CapabilityError::Invalid(TIMEOUT_MESSAGE.into()));
        }
        let read = match source.read(&mut buffer) {
            Ok(read) => read,
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(CapabilityError::Invalid(TIMEOUT_MESSAGE.into()));
                }
                // Only the error kind is reported: the underlying transport
                // error can render the request URL, and a caller-supplied
                // query may carry a secret.
                return Err(CapabilityError::Invalid(format!(
                    "network response read failed: {:?}",
                    error.kind()
                )));
            }
        };
        if read == 0 {
            return Ok(body);
        }
        if body.len().saturating_add(read) > max_bytes {
            return Err(CapabilityError::Invalid(format!(
                "{OVERSIZED_MESSAGE} of {max_bytes} bytes"
            )));
        }
        body.extend_from_slice(&buffer[..read]);
    }
}

/// A `reqwest` error can render the request URL, and a caller-supplied query
/// may carry a secret, so the URL is stripped before any message is formatted.
fn transport_failure(error: reqwest::Error) -> CapabilityError {
    if error.is_timeout() {
        return CapabilityError::Invalid(TIMEOUT_MESSAGE.into());
    }
    CapabilityError::Invalid(format!("network transport failed: {}", error.without_url()))
}
