//! The request target the adapter dials, plus strict `Location` resolution.
//!
//! A caller never supplies a URL. A target is assembled from host-owned policy
//! and validated path/query fragments, which removes the usual class of bug
//! where a permissive URL parser disagrees with an allowlist check about what
//! the host is.

use spoon_capability::CapabilityError;

const MAX_TARGET_BYTES: usize = 2048;
const MAX_HOST_BYTES: usize = 253;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub secure: bool,
    pub host: String,
    pub port: u16,
    /// Path and optional query, always beginning with `/`.
    pub request_target: String,
}

impl Target {
    pub fn scheme(&self) -> &'static str {
        if self.secure { "https" } else { "http" }
    }

    /// The port is always explicit so the dialed authority is unambiguous in
    /// the output and in a redirect comparison.
    pub fn url(&self) -> String {
        format!(
            "{}://{}:{}{}",
            self.scheme(),
            self.host,
            self.port,
            self.request_target
        )
    }

    /// Scheme, host, and port together. A redirect changing any of the three
    /// is cross-origin.
    pub fn origin(&self) -> (bool, &str, u16) {
        (self.secure, self.host.as_str(), self.port)
    }
}

pub fn default_port(secure: bool) -> u16 {
    if secure { 443 } else { 80 }
}

pub fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= MAX_HOST_BYTES
        && !host.starts_with('.')
        && !host.starts_with('-')
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
}

/// Assemble the path-and-query portion from caller input.
pub fn request_target(path: &str, query: &str) -> Result<String, CapabilityError> {
    let path = if path.is_empty() { "/" } else { path };
    if !path.starts_with('/') || path.starts_with("//") || !path.bytes().all(is_path_byte) {
        return Err(CapabilityError::Invalid(
            "network path must be a single-slash absolute path of printable ASCII".into(),
        ));
    }
    let target = if query.is_empty() {
        path.to_owned()
    } else {
        if !query.bytes().all(is_query_byte) {
            return Err(CapabilityError::Invalid(
                "network query must be printable ASCII without a fragment".into(),
            ));
        }
        format!("{path}?{query}")
    };
    if target.len() > MAX_TARGET_BYTES {
        return Err(CapabilityError::Invalid(
            "network request target exceeds the supported length".into(),
        ));
    }
    Ok(target)
}

/// Resolve a `Location` header against the target that produced it. Only an
/// absolute `http`/`https` URL or a root-relative reference is accepted;
/// anything else is refused rather than guessed at, because a wrong guess here
/// is an egress bypass.
pub fn resolve_location(base: &Target, location: &str) -> Result<Target, CapabilityError> {
    if location.is_empty()
        || location.len() > MAX_TARGET_BYTES
        || !location.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(CapabilityError::Invalid(
            "network redirect location is not a printable ASCII reference".into(),
        ));
    }
    if let Some(remainder) = location.strip_prefix("http://") {
        return absolute(false, remainder);
    }
    if let Some(remainder) = location.strip_prefix("https://") {
        return absolute(true, remainder);
    }
    // A protocol-relative `//host/path` changes the authority while looking
    // like a path, so it is refused along with every scheme-less form.
    if location.starts_with('/') && !location.starts_with("//") {
        let (path, query) = split_query(location);
        return Ok(Target {
            secure: base.secure,
            host: base.host.clone(),
            port: base.port,
            request_target: request_target(path, query)?,
        });
    }
    Err(CapabilityError::Invalid(
        "network redirect location must be an absolute http(s) URL or a root-relative path".into(),
    ))
}

fn absolute(secure: bool, remainder: &str) -> Result<Target, CapabilityError> {
    let boundary = remainder.find(['/', '?']).unwrap_or(remainder.len());
    let authority = &remainder[..boundary];
    let (path, query) = match remainder[boundary..].strip_prefix('?') {
        Some(query) => ("/", query),
        None => split_query(&remainder[boundary..]),
    };
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (
            host,
            port.parse::<u16>().map_err(|_| {
                CapabilityError::Invalid("network redirect port is not a port number".into())
            })?,
        ),
        None => (authority, default_port(secure)),
    };
    // Userinfo and bracketed IPv6 literals are refused outright: both are
    // places where a parser can be talked into reading the wrong host.
    let host = host.to_ascii_lowercase();
    if !valid_host(&host) {
        return Err(CapabilityError::Invalid(
            "network redirect authority must be a bare host name or IPv4 literal".into(),
        ));
    }
    Ok(Target {
        secure,
        host,
        port,
        request_target: request_target(path, query)?,
    })
}

fn split_query(value: &str) -> (&str, &str) {
    match value.split_once('?') {
        Some((path, query)) => (path, query),
        None => (value, ""),
    }
}

/// `?`, `#`, and `\` are excluded so caller input cannot smuggle a query, a
/// fragment, or a Windows-style separator into the path.
fn is_path_byte(byte: u8) -> bool {
    byte.is_ascii_graphic() && !matches!(byte, b'?' | b'#' | b'\\')
}

fn is_query_byte(byte: u8) -> bool {
    byte.is_ascii_graphic() && !matches!(byte, b'#' | b'\\')
}
