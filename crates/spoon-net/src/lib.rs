//! A real HTTP transport for the `NetworkRequest` primitive.
//!
//! Every degree of freedom a caller has is bounded by host-owned policy: the
//! host must be in an exact allowlist, the scheme is `https` unless a specific
//! host is configured for plaintext, the port must be in a configured set, the
//! method must be in an allowlist, redirects are off until enabled and are
//! revalidated per hop, and the response body is capped while it streams.
//! Nothing here is selectable by a capability bundle.
//!
//! # DNS rebinding: the exact guarantee
//!
//! The adapter resolves each hop's host itself, refuses the request unless
//! *every* returned address passes the address policy, and hands that
//! validated set to the transport as a resolver override. The production
//! transport builds a fresh `reqwest` client per hop carrying only that hop's
//! override, and redirects are followed here rather than by `reqwest`, so
//! there is no hop whose address went unchecked. That the override, not a
//! second lookup, decides the destination is asserted directly by
//! `the_validated_address_decides_the_destination`, which succeeds against a
//! host in the `.invalid` TLD that no resolver can answer.
//!
//! What this does not give you. The check and the `connect` call remain two
//! steps: `reqwest` exposes no way to hand it an already-connected socket, so
//! the guarantee is that the connector is given only validated addresses, not
//! that a single kernel operation spans both. Two cases sit outside the
//! override entirely. A host that is itself an IP literal skips `hyper`'s
//! resolver, which is harmless because the literal is the validated address.
//! Anything resolving names below `reqwest`, in particular an HTTP proxy named
//! in the process environment, is not covered at all; run this adapter with no
//! proxy environment.

mod address;
mod target;
pub mod transport;

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use spoon_capability::{
    AdapterExecution, AuthorizedPrimitiveInvocation, CapabilityError, CapabilityInvocationAdapter,
    MAX_RESOURCE_BYTES, MAX_RESOURCE_MILLIS, MAX_RESOURCE_STEPS, NativePrimitive, PrimitivePolicy,
    PrimitiveRequest, ResourceBounds, ResourceUsage,
};

pub use address::AddressPolicy;
use target::{Target, default_port, request_target, resolve_location, valid_host};
pub use transport::{
    HttpTransport, MemoryTransport, OVERSIZED_MESSAGE, RequestLog, ReqwestTransport,
    TIMEOUT_MESSAGE, TransportRequest, TransportResponse,
};

/// Substituted for every secret-bearing header value that would otherwise
/// reach the structured output.
pub const REDACTED: &str = "[redacted]";

const MAX_REDIRECT_HOPS: u8 = 5;
const MAX_HEADERS: usize = 32;
const MAX_HEADER_VALUE_BYTES: usize = 1024;
const MIN_CONNECT: Duration = Duration::from_millis(1);
const MAX_CONNECT: Duration = Duration::from_millis(5_000);
const SUPPORTED_METHODS: [&str; 6] = ["DELETE", "GET", "HEAD", "PATCH", "POST", "PUT"];
const REDIRECT_STATUSES: [u16; 5] = [301, 302, 303, 307, 308];

/// Headers the transport owns. A caller setting one of these could contradict
/// the authorized target or frame a second request inside the first.
const RESERVED_HEADERS: [&str; 5] = [
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "upgrade",
];

/// Header names known to carry credentials.
const SECRET_HEADERS: [&str; 8] = [
    "authorization",
    "cookie",
    "set-cookie",
    "proxy-authorization",
    "proxy-authenticate",
    "www-authenticate",
    "x-api-key",
    "x-csrf-token",
];

/// Fragments that mark a header as secret-bearing without having to enumerate
/// every vendor's spelling first. A name matching one of these is redacted.
const SECRET_FRAGMENTS: [&str; 6] = [
    "secret",
    "token",
    "password",
    "credential",
    "api-key",
    "apikey",
];

/// Per-host egress rules. A host absent from the registry is unreachable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRule {
    pub host: String,
    /// Plaintext `http` is permitted for this host only when set.
    #[serde(default)]
    pub plaintext: bool,
    /// Ports a caller may select. Empty means the scheme default only.
    #[serde(default)]
    pub ports: BTreeSet<u16>,
}

impl HostRule {
    /// An https-only host on the scheme default port.
    pub fn secure(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            plaintext: false,
            ports: BTreeSet::new(),
        }
    }

    pub fn with_plaintext(mut self) -> Self {
        self.plaintext = true;
        self
    }

    pub fn with_ports(mut self, ports: impl IntoIterator<Item = u16>) -> Self {
        self.ports = ports.into_iter().collect();
        self
    }

    fn permitted_ports(&self, secure: bool) -> BTreeSet<u16> {
        if self.ports.is_empty() {
            BTreeSet::from([default_port(secure)])
        } else {
            self.ports.clone()
        }
    }
}

/// How many redirect hops are followed, and whether a hop may change origin.
/// The default refuses every redirect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedirectPolicy {
    /// Hops permitted after the first request. Zero refuses all redirects.
    pub max_hops: u8,
    /// Whether a hop may change scheme, host, or port. The new origin still
    /// has to pass every other check.
    pub cross_origin: bool,
}

/// Caller-supplied invocation input. Host policy decides what any of it is
/// allowed to be.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkInput {
    /// `https` when absent. `http` requires a host configured for plaintext.
    #[serde(default)]
    pub scheme: Option<String>,
    /// The scheme default when absent.
    #[serde(default)]
    pub port: Option<u16>,
    /// Absolute path beginning with `/`. Empty means `/`.
    #[serde(default)]
    pub path: String,
    /// Query without the leading `?`.
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: String,
}

/// A concrete host adapter for the network primitive.
///
/// Constructing it is an explicit host action. The authorization policy is
/// derived from the registered host rules, so the rule registry is the single
/// source of truth for what is reachable.
pub struct NetworkAdapter {
    hosts: BTreeMap<String, HostRule>,
    methods: BTreeSet<String>,
    addresses: AddressPolicy,
    redirects: RedirectPolicy,
    policy: PrimitivePolicy,
    transport: Box<dyn HttpTransport>,
}

impl NetworkAdapter {
    /// `transport` is the sole host-interaction seam: [`ReqwestTransport`] in
    /// production, [`MemoryTransport`] offline.
    pub fn new<M: Into<String>>(
        bounds: ResourceBounds,
        hosts: Vec<HostRule>,
        methods: impl IntoIterator<Item = M>,
        transport: Box<dyn HttpTransport>,
    ) -> Result<Self, CapabilityError> {
        validate_bounds(&bounds)?;
        let methods: BTreeSet<String> = methods.into_iter().map(Into::into).collect();
        if methods.is_empty() || methods.iter().any(|method| !supported_method(method)) {
            return Err(CapabilityError::Invalid(
                "network method allowlist must be a non-empty set of supported methods".into(),
            ));
        }
        let mut registry = BTreeMap::new();
        for rule in hosts {
            let host = rule.host.to_ascii_lowercase();
            if !valid_host(&host) {
                return Err(CapabilityError::Invalid(
                    "network host must be a bare host name or IPv4 literal".into(),
                ));
            }
            if rule.ports.contains(&0) {
                return Err(CapabilityError::Invalid(
                    "network host port set must not contain port zero".into(),
                ));
            }
            let key = host.clone();
            if registry.insert(key, HostRule { host, ..rule }).is_some() {
                return Err(CapabilityError::Invalid(
                    "network host rules must be unique".into(),
                ));
            }
        }
        Ok(Self {
            policy: PrimitivePolicy {
                network_hosts: registry.keys().cloned().collect(),
                bounds,
                ..PrimitivePolicy::default()
            },
            hosts: registry,
            methods,
            addresses: AddressPolicy::default(),
            redirects: RedirectPolicy::default(),
            transport,
        })
    }

    /// Widen the address policy. Host-owned, and never selectable by input.
    pub fn with_address_policy(mut self, addresses: AddressPolicy) -> Self {
        self.addresses = addresses;
        self
    }

    pub fn with_redirect_policy(
        mut self,
        redirects: RedirectPolicy,
    ) -> Result<Self, CapabilityError> {
        if redirects.max_hops > MAX_REDIRECT_HOPS {
            return Err(CapabilityError::Invalid(
                "network redirect hop cap exceeds the supported maximum".into(),
            ));
        }
        self.redirects = redirects;
        Ok(self)
    }

    /// The immutable server-local policy envelope to pass to capability
    /// authorization. Callers clone this per invocation rather than accepting
    /// a policy from a request.
    pub fn policy(&self) -> &PrimitivePolicy {
        &self.policy
    }

    /// Host allowlist, scheme, and port for one hop. Re-run on every hop so a
    /// redirect cannot reach a target the first request could not.
    fn authorize_target(&self, target: &Target) -> Result<(), CapabilityError> {
        let rule = self.hosts.get(&target.host).ok_or_else(|| {
            CapabilityError::PermissionRequired(format!("network host {}", target.host))
        })?;
        if !target.secure && !rule.plaintext {
            return Err(CapabilityError::PermissionRequired(format!(
                "network plaintext http for host {}",
                target.host
            )));
        }
        if !rule.permitted_ports(target.secure).contains(&target.port) {
            return Err(CapabilityError::PermissionRequired(format!(
                "network port {} for host {}",
                target.port, target.host
            )));
        }
        Ok(())
    }

    /// Resolve one hop and validate every address it returned.
    fn dial_addresses(&mut self, target: &Target) -> Result<Vec<IpAddr>, CapabilityError> {
        self.authorize_target(target)?;
        let addresses = self.transport.resolve(&target.host, target.port)?;
        let policy = self.addresses;
        // Refusing the whole answer rather than filtering it is what defeats a
        // rebinding response that mixes a public address with a private one.
        if !addresses.iter().all(|address| policy.permits(address)) {
            return Err(CapabilityError::PermissionRequired(format!(
                "network address class for host {} under the {} address policy",
                target.host,
                policy.label()
            )));
        }
        Ok(addresses)
    }
}

impl CapabilityInvocationAdapter for NetworkAdapter {
    fn policy(&self, primitive: &NativePrimitive) -> Option<PrimitivePolicy> {
        matches!(primitive, NativePrimitive::NetworkRequest).then(|| self.policy.clone())
    }

    fn execute(
        &mut self,
        invocation: &AuthorizedPrimitiveInvocation,
    ) -> Result<AdapterExecution, CapabilityError> {
        if invocation.primitive != NativePrimitive::NetworkRequest {
            return Err(CapabilityError::AdapterViolation(
                "network adapter does not support the requested primitive".into(),
            ));
        }
        // Authorizing against the host policy is what enforces the exact-host
        // allowlist and the declared byte bound.
        let receipt = self.policy.authorize(&invocation.request)?;
        let PrimitiveRequest::Network {
            host,
            method,
            body_bytes,
        } = &invocation.request
        else {
            return Err(CapabilityError::AdapterViolation(
                "network adapter requires a Network request".into(),
            ));
        };
        // A deserialization message can render a rejected value, and a caller
        // puts credentials in this structure, so only the classification is
        // reported.
        let input: NetworkInput = serde_json::from_value(invocation.input.clone())
            .map_err(|_| CapabilityError::Invalid("network input is not a valid request".into()))?;

        if !self.methods.contains(method) {
            return Err(CapabilityError::PermissionRequired(
                "network method is outside the host method allowlist".into(),
            ));
        }
        let secure = match input.scheme.as_deref() {
            None | Some("https") => true,
            Some("http") => false,
            Some(_) => {
                return Err(CapabilityError::Invalid(
                    "network scheme must be http or https".into(),
                ));
            }
        };
        let mut target = Target {
            secure,
            host: host.to_ascii_lowercase(),
            port: input.port.unwrap_or_else(|| default_port(secure)),
            request_target: request_target(&input.path, &input.query)?,
        };

        // The caller's bounds may only narrow the host envelope.
        let max_bytes = invocation
            .bounds
            .max_bytes
            .min(self.policy.bounds.max_bytes);
        let max_millis = invocation
            .bounds
            .max_millis
            .min(self.policy.bounds.max_millis);
        if max_millis == 0 {
            return Err(CapabilityError::Invalid(
                "network wall-clock bound must be positive".into(),
            ));
        }
        let body = input.body.into_bytes();
        let body_length = body.len() as u64;
        if body_length > *body_bytes || body_length > max_bytes {
            return Err(CapabilityError::Invalid(
                "network request body exceeds its declared byte bound".into(),
            ));
        }
        let mut headers = validate_headers(&input.headers)?;

        let started = Instant::now();
        let deadline = started + Duration::from_millis(max_millis);
        let max_body_bytes = usize::try_from(max_bytes).unwrap_or(usize::MAX);
        let mut hops = 0u64;
        let (response, addresses) = loop {
            let addresses = self.dial_addresses(&target)?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(CapabilityError::Invalid(TIMEOUT_MESSAGE.into()));
            }
            let response = self.transport.send(&TransportRequest {
                url: target.url(),
                host: target.host.clone(),
                port: target.port,
                method: method.clone(),
                addresses: addresses.clone(),
                headers: headers.clone(),
                body: body.clone(),
                max_body_bytes,
                connect_timeout: connect_timeout(remaining),
                total_timeout: remaining,
            })?;
            let location = redirect_location(&response).map(str::to_owned);
            let Some(location) = location else {
                break (response, addresses);
            };

            if hops >= u64::from(self.redirects.max_hops) {
                return Err(CapabilityError::PermissionRequired(
                    "network redirect exceeds the host redirect hop policy".into(),
                ));
            }
            // Replaying a body or silently rewriting the method are both ways
            // to send a caller's request somewhere it never agreed to, so only
            // the safe methods are followed.
            if !matches!(method.as_str(), "GET" | "HEAD") {
                return Err(CapabilityError::PermissionRequired(
                    "network redirect is followed only for GET and HEAD".into(),
                ));
            }
            let next = resolve_location(&target, &location)?;
            if next.origin() != target.origin() {
                if !self.redirects.cross_origin {
                    return Err(CapabilityError::PermissionRequired(
                        "network cross-origin redirect is not permitted".into(),
                    ));
                }
                // A credential is scoped to the origin it was supplied for;
                // carrying it to a new origin leaks it even when that origin
                // is allowlisted.
                headers.retain(|(name, _)| !is_secret_header(name));
            }
            target = next;
            hops += 1;
        };

        let mut response_headers = BTreeMap::new();
        let mut redacted = BTreeSet::new();
        for (name, value) in &response.headers {
            let name = name.to_ascii_lowercase();
            if is_secret_header(&name) {
                redacted.insert(name.clone());
                response_headers.insert(name, REDACTED.to_owned());
            } else {
                response_headers.insert(name, value.clone());
            }
        }
        let mut dialed: Vec<String> = addresses.iter().map(IpAddr::to_string).collect();
        dialed.sort();

        Ok(AdapterExecution {
            effect: receipt.effect,
            output: serde_json::json!({
                "url": target.url(),
                "scheme": target.scheme(),
                "host": target.host,
                "port": target.port,
                "method": method,
                "addresses": dialed,
                "addressPolicy": self.addresses.label(),
                "status": response.status,
                "headers": response_headers,
                "redactedHeaders": redacted,
                "body": String::from_utf8_lossy(&response.body),
                "bodyBytes": response.body.len(),
                "hops": hops,
            }),
            usage: ResourceUsage {
                bytes: body_length.saturating_add(response.body.len() as u64),
                steps: hops.saturating_add(1),
                millis: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            },
        })
    }
}

/// Build the invocation input a caller passes to the adapter.
pub fn network_input(input: &NetworkInput) -> Result<Value, CapabilityError> {
    serde_json::to_value(input).map_err(CapabilityError::from)
}

/// Whether a header carries a credential and must never reach a receipt, an
/// output, an error message, or a new origin.
pub fn is_secret_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SECRET_HEADERS.contains(&name.as_str())
        || SECRET_FRAGMENTS
            .iter()
            .any(|fragment| name.contains(fragment))
}

fn redirect_location(response: &TransportResponse) -> Option<&str> {
    if !REDIRECT_STATUSES.contains(&response.status) {
        return None;
    }
    response.header("location")
}

/// A connect budget below the total leaves room for the response to arrive,
/// and the ceiling keeps a generous total from becoming a long stall on an
/// address that never answers.
fn connect_timeout(total: Duration) -> Duration {
    (total / 4).max(MIN_CONNECT).min(MAX_CONNECT).min(total)
}

fn supported_method(method: &str) -> bool {
    SUPPORTED_METHODS.contains(&method)
}

/// Header names and values are caller data and can themselves be sensitive, so
/// a rejection names the rule rather than the offending header.
fn validate_headers(
    headers: &BTreeMap<String, String>,
) -> Result<Vec<(String, String)>, CapabilityError> {
    if headers.len() > MAX_HEADERS {
        return Err(CapabilityError::Invalid(
            "network request declares more headers than are permitted".into(),
        ));
    }
    let mut validated = Vec::with_capacity(headers.len());
    for (name, value) in headers {
        let name = name.to_ascii_lowercase();
        if name.is_empty()
            || !name.bytes().all(is_token_byte)
            || RESERVED_HEADERS.contains(&name.as_str())
        {
            return Err(CapabilityError::Invalid(
                "network request header name is not a permitted HTTP token".into(),
            ));
        }
        if value.len() > MAX_HEADER_VALUE_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_graphic() || byte == b' ')
        {
            return Err(CapabilityError::Invalid(
                "network request header value must be bounded printable ASCII".into(),
            ));
        }
        validated.push((name, value.clone()));
    }
    Ok(validated)
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn validate_bounds(bounds: &ResourceBounds) -> Result<(), CapabilityError> {
    if bounds.max_bytes == 0
        || bounds.max_millis == 0
        || bounds.max_bytes > MAX_RESOURCE_BYTES
        || bounds.max_steps > MAX_RESOURCE_STEPS
        || bounds.max_millis > MAX_RESOURCE_MILLIS
    {
        return Err(CapabilityError::Invalid(
            "network resource bounds are outside the supported envelope".into(),
        ));
    }
    Ok(())
}
