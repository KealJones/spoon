//! Behavioral tests for the network adapter. Nothing here reaches the public
//! internet: policy behavior runs through the offline transport, and the cases
//! that need a real socket run against a loopback `TcpListener`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use spoon_capability::{
    AuthorizedPrimitiveInvocation, CapabilityError, CapabilityInvocationAdapter, Effect,
    NativePrimitive, PrimitiveRequest, ResourceBounds,
};
use spoon_net::{
    AddressPolicy, HostRule, HttpTransport, MemoryTransport, NetworkAdapter, NetworkInput,
    OVERSIZED_MESSAGE, REDACTED, RedirectPolicy, ReqwestTransport, TIMEOUT_MESSAGE,
    TransportRequest, TransportResponse, network_input,
};

const HOST: &str = "api.example.test";
const CDN: &str = "cdn.example.test";
const LOOPBACK: &str = "127.0.0.1";
const BODY: &[u8] = b"hello, spoon!";

fn bounds(max_bytes: u64, max_millis: u64) -> ResourceBounds {
    ResourceBounds {
        max_bytes,
        max_steps: 16,
        max_millis,
    }
}

fn public() -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))
}

fn get(path: &str) -> NetworkInput {
    NetworkInput {
        path: path.into(),
        ..NetworkInput::default()
    }
}

fn with_headers(path: &str, headers: &[(&str, &str)]) -> NetworkInput {
    NetworkInput {
        path: path.into(),
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        ..NetworkInput::default()
    }
}

fn plaintext(path: &str, port: u16) -> NetworkInput {
    NetworkInput {
        scheme: Some("http".into()),
        port: Some(port),
        path: path.into(),
        ..NetworkInput::default()
    }
}

fn invocation(
    host: &str,
    method: &str,
    input: &NetworkInput,
    bounds: ResourceBounds,
) -> AuthorizedPrimitiveInvocation {
    AuthorizedPrimitiveInvocation {
        content_id: "content".into(),
        procedure_id: "procedure".into(),
        primitive: NativePrimitive::NetworkRequest,
        effect: Effect::Network,
        request: PrimitiveRequest::Network {
            host: host.into(),
            method: method.into(),
            body_bytes: input.body.len() as u64,
        },
        input: network_input(input).expect("input"),
        bounds,
    }
}

fn body_response() -> TransportResponse {
    TransportResponse::new(
        200,
        vec![
            ("content-length".into(), BODY.len().to_string()),
            ("content-type".into(), "text/plain".into()),
        ],
        BODY.to_vec(),
    )
}

fn redirect(location: &str) -> TransportResponse {
    TransportResponse::new(302, vec![("location".into(), location.into())], Vec::new())
}

fn net_adapter(hosts: Vec<HostRule>, transport: Box<dyn HttpTransport>) -> NetworkAdapter {
    NetworkAdapter::new(bounds(64 * 1024, 5_000), hosts, ["GET", "POST"], transport)
        .expect("adapter")
}

/// An adapter for the loopback listener: plaintext on the listener's port, and
/// an explicitly widened address policy so the test may reach 127.0.0.1.
fn loopback_adapter(port: u16, limits: ResourceBounds) -> NetworkAdapter {
    NetworkAdapter::new(
        limits,
        vec![
            HostRule::secure(LOOPBACK)
                .with_plaintext()
                .with_ports([port]),
        ],
        ["GET"],
        Box::new(ReqwestTransport),
    )
    .expect("adapter")
    .with_address_policy(AddressPolicy::LoopbackPermitted)
}

/// Serve exactly one connection on loopback and return the bound port.
fn serve<F>(handler: F) -> (u16, thread::JoinHandle<()>)
where
    F: FnOnce(TcpStream) + Send + 'static,
{
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
    let port = listener.local_addr().expect("local address").port();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        handler(stream);
    });
    (port, handle)
}

/// Drain the request head so the client's write completes before the handler
/// answers.
fn read_head(stream: &mut TcpStream) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while stream.read(&mut byte).unwrap_or(0) == 1 {
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            return;
        }
    }
}

#[test]
fn returns_status_headers_and_body_for_a_successful_get() {
    let transport = MemoryTransport::new()
        .with_addresses(HOST, vec![public()])
        .with_response("GET", "https://api.example.test:443/hello", body_response());
    let mut adapter = net_adapter(vec![HostRule::secure(HOST)], Box::new(transport));

    let execution = adapter
        .execute(&invocation(
            HOST,
            "GET",
            &get("/hello"),
            bounds(64 * 1024, 5_000),
        ))
        .expect("request");

    assert_eq!(execution.effect, Effect::Network);
    assert_eq!(execution.output["status"], 200);
    assert_eq!(execution.output["scheme"], "https");
    assert_eq!(
        execution.output["url"],
        "https://api.example.test:443/hello"
    );
    assert_eq!(execution.output["headers"]["content-type"], "text/plain");
    assert_eq!(execution.output["body"], "hello, spoon!");
    assert_eq!(execution.output["bodyBytes"], BODY.len());
    assert_eq!(execution.output["hops"], 0);
    assert_eq!(execution.output["addresses"][0], "93.184.216.34");
    assert_eq!(execution.usage.steps, 1);
    assert_eq!(execution.usage.bytes, BODY.len() as u64);
}

#[test]
fn refuses_a_host_outside_the_allowlist() {
    let transport = MemoryTransport::new().with_addresses("other.example.test", vec![public()]);
    let mut adapter = net_adapter(vec![HostRule::secure(HOST)], Box::new(transport));

    let error = adapter
        .execute(&invocation(
            "other.example.test",
            "GET",
            &get("/hello"),
            bounds(4_096, 5_000),
        ))
        .expect_err("refused");

    assert!(
        matches!(error, CapabilityError::PermissionRequired(_)),
        "{error}"
    );
}

#[test]
fn refuses_a_host_that_only_shares_a_suffix_with_an_allowed_host() {
    let transport = MemoryTransport::new().with_addresses("evil-api.example.test", vec![public()]);
    let mut adapter = net_adapter(vec![HostRule::secure("example.test")], Box::new(transport));

    let error = adapter
        .execute(&invocation(
            "evil-api.example.test",
            "GET",
            &get("/hello"),
            bounds(4_096, 5_000),
        ))
        .expect_err("refused");

    assert!(
        matches!(error, CapabilityError::PermissionRequired(_)),
        "{error}"
    );
}

#[test]
fn refuses_plaintext_http_unless_the_host_is_configured_for_it() {
    let canned = || {
        MemoryTransport::new()
            .with_addresses(HOST, vec![public()])
            .with_response("GET", "http://api.example.test:80/hello", body_response())
    };

    let mut refused = net_adapter(vec![HostRule::secure(HOST)], Box::new(canned()));
    let error = refused
        .execute(&invocation(
            HOST,
            "GET",
            &plaintext("/hello", 80),
            bounds(4_096, 5_000),
        ))
        .expect_err("refused");
    assert!(error.to_string().contains("plaintext http"), "{error}");

    let mut permitted = net_adapter(
        vec![HostRule::secure(HOST).with_plaintext()],
        Box::new(canned()),
    );
    let execution = permitted
        .execute(&invocation(
            HOST,
            "GET",
            &plaintext("/hello", 80),
            bounds(4_096, 5_000),
        ))
        .expect("request");
    assert_eq!(execution.output["scheme"], "http");
    assert_eq!(execution.output["status"], 200);
}

#[test]
fn refuses_a_port_outside_the_configured_set() {
    let transport = MemoryTransport::new().with_addresses(HOST, vec![public()]);
    let mut adapter = net_adapter(
        vec![HostRule::secure(HOST).with_plaintext().with_ports([8443])],
        Box::new(transport),
    );

    let error = adapter
        .execute(&invocation(
            HOST,
            "GET",
            &plaintext("/hello", 8080),
            bounds(4_096, 5_000),
        ))
        .expect_err("refused");

    assert!(error.to_string().contains("network port 8080"), "{error}");
}

#[test]
fn refuses_a_method_outside_the_allowlist() {
    let transport = MemoryTransport::new().with_addresses(HOST, vec![public()]);
    let mut adapter = NetworkAdapter::new(
        bounds(4_096, 5_000),
        vec![HostRule::secure(HOST)],
        ["GET"],
        Box::new(transport),
    )
    .expect("adapter");

    let error = adapter
        .execute(&invocation(
            HOST,
            "DELETE",
            &get("/hello"),
            bounds(4_096, 5_000),
        ))
        .expect_err("refused");

    assert!(error.to_string().contains("method is outside"), "{error}");
}

#[test]
fn refuses_every_non_public_address_class_by_default() {
    for address in [
        "127.0.0.1",
        "::1",
        "::ffff:127.0.0.1",
        "10.0.0.5",
        "172.16.4.4",
        "192.168.1.9",
        "169.254.13.7",
        "fe80::1",
        "fc00::1",
        "0.0.0.0",
        "::",
        "255.255.255.255",
        "224.0.0.1",
        "100.64.0.1",
        "198.18.0.1",
    ] {
        let parsed: IpAddr = address.parse().expect("address");
        let transport = MemoryTransport::new()
            .with_addresses(HOST, vec![parsed])
            .with_response("GET", "https://api.example.test:443/hello", body_response());
        let mut adapter = net_adapter(vec![HostRule::secure(HOST)], Box::new(transport));

        let error = adapter
            .execute(&invocation(
                HOST,
                "GET",
                &get("/hello"),
                bounds(4_096, 5_000),
            ))
            .expect_err("refused");

        assert!(
            matches!(error, CapabilityError::PermissionRequired(_)),
            "{address} was permitted: {error}"
        );
        assert!(error.to_string().contains("public-only"), "{error}");
    }
}

#[test]
fn refuses_a_resolution_that_mixes_a_public_and_a_private_address() {
    let transport = MemoryTransport::new()
        .with_addresses(HOST, vec![public(), IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))])
        .with_response("GET", "https://api.example.test:443/hello", body_response());
    let mut adapter = net_adapter(vec![HostRule::secure(HOST)], Box::new(transport));

    let error = adapter
        .execute(&invocation(
            HOST,
            "GET",
            &get("/hello"),
            bounds(4_096, 5_000),
        ))
        .expect_err("refused");

    assert!(
        matches!(error, CapabilityError::PermissionRequired(_)),
        "{error}"
    );
}

#[test]
fn permits_loopback_only_when_the_address_policy_is_widened() {
    let canned = || {
        MemoryTransport::new()
            .with_addresses(HOST, vec![IpAddr::V4(Ipv4Addr::LOCALHOST)])
            .with_response("GET", "https://api.example.test:443/hello", body_response())
    };

    let mut widened = net_adapter(vec![HostRule::secure(HOST)], Box::new(canned()))
        .with_address_policy(AddressPolicy::LoopbackPermitted);
    let execution = widened
        .execute(&invocation(
            HOST,
            "GET",
            &get("/hello"),
            bounds(4_096, 5_000),
        ))
        .expect("request");
    assert_eq!(execution.output["addressPolicy"], "loopback-permitted");

    // A widened loopback rule must not also widen the private ranges.
    let transport = canned().with_addresses(HOST, vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))]);
    let mut widened = net_adapter(vec![HostRule::secure(HOST)], Box::new(transport))
        .with_address_policy(AddressPolicy::LoopbackPermitted);
    let error = widened
        .execute(&invocation(
            HOST,
            "GET",
            &get("/hello"),
            bounds(4_096, 5_000),
        ))
        .expect_err("refused");
    assert!(
        matches!(error, CapabilityError::PermissionRequired(_)),
        "{error}"
    );
}

#[test]
fn refuses_loopback_over_a_real_socket_under_the_default_address_policy() {
    // No listener is started: the refusal must happen before anything is
    // dialed, which is the point of validating the resolved address first.
    let mut adapter = NetworkAdapter::new(
        bounds(4_096, 5_000),
        vec![
            HostRule::secure(LOOPBACK)
                .with_plaintext()
                .with_ports([8080]),
        ],
        ["GET"],
        Box::new(ReqwestTransport),
    )
    .expect("adapter");

    let error = adapter
        .execute(&invocation(
            LOOPBACK,
            "GET",
            &plaintext("/hello", 8080),
            bounds(4_096, 5_000),
        ))
        .expect_err("refused");

    assert!(
        matches!(error, CapabilityError::PermissionRequired(_)),
        "{error}"
    );
    assert!(error.to_string().contains("public-only"), "{error}");
}

#[test]
fn refuses_a_redirect_by_default() {
    let transport = MemoryTransport::new()
        .with_addresses(HOST, vec![public()])
        .with_response(
            "GET",
            "https://api.example.test:443/start",
            redirect("/next"),
        )
        .with_response("GET", "https://api.example.test:443/next", body_response());
    let mut adapter = net_adapter(vec![HostRule::secure(HOST)], Box::new(transport));

    let error = adapter
        .execute(&invocation(
            HOST,
            "GET",
            &get("/start"),
            bounds(4_096, 5_000),
        ))
        .expect_err("refused");

    assert!(error.to_string().contains("redirect"), "{error}");
}

#[test]
fn follows_a_same_origin_redirect_when_hops_are_permitted() {
    let transport = MemoryTransport::new()
        .with_addresses(HOST, vec![public()])
        .with_response(
            "GET",
            "https://api.example.test:443/start",
            redirect("/next"),
        )
        .with_response("GET", "https://api.example.test:443/next", body_response());
    let mut adapter = net_adapter(vec![HostRule::secure(HOST)], Box::new(transport))
        .with_redirect_policy(RedirectPolicy {
            max_hops: 1,
            cross_origin: false,
        })
        .expect("redirect policy");

    let execution = adapter
        .execute(&invocation(
            HOST,
            "GET",
            &get("/start"),
            bounds(4_096, 5_000),
        ))
        .expect("request");

    assert_eq!(execution.output["hops"], 1);
    assert_eq!(execution.output["url"], "https://api.example.test:443/next");
    assert_eq!(execution.usage.steps, 2);
}

#[test]
fn refuses_a_cross_origin_redirect_unless_it_is_explicitly_permitted() {
    let canned = || {
        MemoryTransport::new()
            .with_addresses(HOST, vec![public()])
            .with_addresses(CDN, vec![public()])
            .with_response(
                "GET",
                "https://api.example.test:443/start",
                redirect("https://cdn.example.test/asset"),
            )
            .with_response("GET", "https://cdn.example.test:443/asset", body_response())
    };
    let hosts = || vec![HostRule::secure(HOST), HostRule::secure(CDN)];

    let mut refused = net_adapter(hosts(), Box::new(canned()))
        .with_redirect_policy(RedirectPolicy {
            max_hops: 2,
            cross_origin: false,
        })
        .expect("redirect policy");
    let error = refused
        .execute(&invocation(
            HOST,
            "GET",
            &get("/start"),
            bounds(4_096, 5_000),
        ))
        .expect_err("refused");
    assert!(error.to_string().contains("cross-origin"), "{error}");

    let mut permitted = net_adapter(hosts(), Box::new(canned()))
        .with_redirect_policy(RedirectPolicy {
            max_hops: 2,
            cross_origin: true,
        })
        .expect("redirect policy");
    let execution = permitted
        .execute(&invocation(
            HOST,
            "GET",
            &get("/start"),
            bounds(4_096, 5_000),
        ))
        .expect("request");
    assert_eq!(execution.output["host"], CDN);
}

#[test]
fn refuses_a_redirect_to_a_host_outside_the_allowlist() {
    let transport = MemoryTransport::new()
        .with_addresses(HOST, vec![public()])
        .with_addresses("evil.example.test", vec![public()])
        .with_response(
            "GET",
            "https://api.example.test:443/start",
            redirect("https://evil.example.test/steal"),
        )
        .with_response(
            "GET",
            "https://evil.example.test:443/steal",
            body_response(),
        );
    let mut adapter = net_adapter(vec![HostRule::secure(HOST)], Box::new(transport))
        .with_redirect_policy(RedirectPolicy {
            max_hops: 2,
            cross_origin: true,
        })
        .expect("redirect policy");

    let error = adapter
        .execute(&invocation(
            HOST,
            "GET",
            &get("/start"),
            bounds(4_096, 5_000),
        ))
        .expect_err("refused");

    assert!(
        error.to_string().contains("network host evil.example.test"),
        "{error}"
    );
}

#[test]
fn refuses_a_protocol_relative_redirect() {
    let transport = MemoryTransport::new()
        .with_addresses(HOST, vec![public()])
        .with_response(
            "GET",
            "https://api.example.test:443/start",
            redirect("//evil.example.test/steal"),
        );
    let mut adapter = net_adapter(vec![HostRule::secure(HOST)], Box::new(transport))
        .with_redirect_policy(RedirectPolicy {
            max_hops: 2,
            cross_origin: true,
        })
        .expect("redirect policy");

    let error = adapter
        .execute(&invocation(
            HOST,
            "GET",
            &get("/start"),
            bounds(4_096, 5_000),
        ))
        .expect_err("refused");

    assert!(matches!(error, CapabilityError::Invalid(_)), "{error}");
}

#[test]
fn drops_credentials_across_a_permitted_cross_origin_redirect() {
    let transport = MemoryTransport::new()
        .with_addresses(HOST, vec![public()])
        .with_addresses(CDN, vec![public()])
        .with_response(
            "GET",
            "https://api.example.test:443/start",
            redirect("https://cdn.example.test/asset"),
        )
        .with_response("GET", "https://cdn.example.test:443/asset", body_response());
    let log = transport.log();
    let mut adapter = net_adapter(
        vec![HostRule::secure(HOST), HostRule::secure(CDN)],
        Box::new(transport),
    )
    .with_redirect_policy(RedirectPolicy {
        max_hops: 2,
        cross_origin: true,
    })
    .expect("redirect policy");

    adapter
        .execute(&invocation(
            HOST,
            "GET",
            &with_headers(
                "/start",
                &[
                    ("authorization", "Bearer origin-scoped"),
                    ("accept", "text/plain"),
                ],
            ),
            bounds(4_096, 5_000),
        ))
        .expect("request");

    let sent = log.lock().expect("log").clone();
    assert_eq!(sent.len(), 2);
    let names = |index: usize| -> Vec<String> {
        sent[index]
            .headers
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    };
    assert!(names(0).contains(&"authorization".to_owned()));
    assert!(!names(1).contains(&"authorization".to_owned()));
    assert!(names(1).contains(&"accept".to_owned()));
}

#[test]
fn secret_headers_never_reach_the_receipt_the_output_or_an_error() {
    const TOKEN: &str = "Bearer super-secret-token";
    const SESSION: &str = "session=another-secret";
    let input = with_headers(
        "/private",
        &[
            ("authorization", TOKEN),
            ("cookie", SESSION),
            ("accept", "text/plain"),
        ],
    );
    let response = TransportResponse::new(
        200,
        vec![
            ("set-cookie".into(), "session=server-secret".into()),
            ("x-api-key".into(), "server-key-secret".into()),
            ("content-type".into(), "text/plain".into()),
        ],
        b"ok".to_vec(),
    );
    let transport = MemoryTransport::new()
        .with_addresses(HOST, vec![public()])
        .with_response("GET", "https://api.example.test:443/private", response);
    let log = transport.log();
    let mut adapter = net_adapter(vec![HostRule::secure(HOST)], Box::new(transport));

    let call = invocation(HOST, "GET", &input, bounds(4_096, 5_000));
    let receipt = adapter.policy().authorize(&call.request).expect("receipt");
    let execution = adapter.execute(&call).expect("request");

    // The credential really did reach the wire, so redaction is about what is
    // reported rather than about dropping the header.
    let sent = log.lock().expect("log").clone();
    assert!(
        sent[0]
            .headers
            .contains(&("authorization".to_owned(), TOKEN.to_owned()))
    );

    let secrets = [TOKEN, SESSION, "session=server-secret", "server-key-secret"];
    let reported = format!(
        "{} {}",
        serde_json::to_string(&receipt).expect("receipt json"),
        serde_json::to_string(&execution.output).expect("output json")
    );
    for secret in secrets {
        assert!(!reported.contains(secret), "{secret} leaked into a report");
    }
    assert_eq!(execution.output["headers"]["set-cookie"], REDACTED);
    assert_eq!(execution.output["headers"]["x-api-key"], REDACTED);
    assert_eq!(execution.output["headers"]["content-type"], "text/plain");
    assert_eq!(execution.output["redactedHeaders"][0], "set-cookie");

    // A failure path must not become the leak either.
    let mut failing = net_adapter(
        vec![HostRule::secure(HOST)],
        Box::new(MemoryTransport::new().with_addresses(HOST, vec![public()])),
    );
    let error = failing
        .execute(&invocation(HOST, "GET", &input, bounds(4_096, 5_000)))
        .expect_err("no canned response");
    for secret in secrets {
        assert!(
            !error.to_string().contains(secret),
            "{secret} leaked into an error"
        );
    }
}

#[test]
fn refuses_a_caller_supplied_transport_owned_header() {
    let transport = MemoryTransport::new().with_addresses(HOST, vec![public()]);
    let mut adapter = net_adapter(vec![HostRule::secure(HOST)], Box::new(transport));

    let error = adapter
        .execute(&invocation(
            HOST,
            "GET",
            &with_headers("/hello", &[("host", "evil.example.test")]),
            bounds(4_096, 5_000),
        ))
        .expect_err("refused");

    assert!(matches!(error, CapabilityError::Invalid(_)), "{error}");
}

#[test]
fn refuses_a_path_that_is_not_a_single_slash_absolute_path() {
    let transport = MemoryTransport::new().with_addresses(HOST, vec![public()]);
    let mut adapter = net_adapter(vec![HostRule::secure(HOST)], Box::new(transport));

    for path in ["hello", "//evil.example.test/", "/hello?x=1", "/hel lo"] {
        let error = adapter
            .execute(&invocation(HOST, "GET", &get(path), bounds(4_096, 5_000)))
            .expect_err("refused");
        assert!(
            matches!(error, CapabilityError::Invalid(_)),
            "{path}: {error}"
        );
    }
}

#[test]
fn refuses_an_unsupported_primitive() {
    let transport = MemoryTransport::new().with_addresses(HOST, vec![public()]);
    let mut adapter = net_adapter(vec![HostRule::secure(HOST)], Box::new(transport));
    let mut call = invocation(HOST, "GET", &get("/hello"), bounds(4_096, 5_000));
    call.primitive = NativePrimitive::FileRead;

    let error = adapter.execute(&call).expect_err("refused");

    assert!(
        matches!(error, CapabilityError::AdapterViolation(_)),
        "{error}"
    );
    assert!(CapabilityInvocationAdapter::policy(&adapter, &NativePrimitive::FileRead).is_none());
}

#[test]
fn a_total_timeout_aborts_a_slow_response() {
    let (port, server) = serve(|mut stream| {
        read_head(&mut stream);
        thread::sleep(Duration::from_millis(1_500));
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nhi");
    });
    let limits = bounds(4_096, 300);
    let mut adapter = loopback_adapter(port, limits.clone());

    let error = adapter
        .execute(&invocation(
            LOOPBACK,
            "GET",
            &plaintext("/slow", port),
            limits,
        ))
        .expect_err("timed out");

    assert!(error.to_string().contains(TIMEOUT_MESSAGE), "{error}");
    server.join().expect("server");
}

#[test]
fn aborts_an_oversized_response_while_it_streams() {
    const DECLARED: usize = 32 * 1024 * 1024;
    const CHUNK: usize = 64 * 1024;
    let written = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&written);
    let (port, server) = serve(move |mut stream| {
        read_head(&mut stream);
        let head = format!("HTTP/1.1 200 OK\r\ncontent-length: {DECLARED}\r\n\r\n");
        if stream.write_all(head.as_bytes()).is_err() {
            return;
        }
        let chunk = vec![b'x'; CHUNK];
        let mut sent = 0;
        while sent < DECLARED && stream.write_all(&chunk).is_ok() {
            sent += CHUNK;
            counter.store(sent, Ordering::SeqCst);
        }
    });
    let limits = bounds(4_096, 10_000);
    let mut adapter = loopback_adapter(port, limits.clone());

    let error = adapter
        .execute(&invocation(
            LOOPBACK,
            "GET",
            &plaintext("/big", port),
            limits,
        ))
        .expect_err("refused");

    assert!(error.to_string().contains(OVERSIZED_MESSAGE), "{error}");
    server.join().expect("server");
    // The abort has to happen mid-download. Anything close to the declared
    // length would mean the body was buffered and only then rejected. What the
    // server gets to write before the client hangs up is whatever fits in the
    // socket buffers, measured at 448 KiB on this machine; the bound is loose
    // enough to absorb a larger buffer and still far below the declared size.
    let written = written.load(Ordering::SeqCst);
    assert!(
        written < 4 * 1024 * 1024,
        "server wrote {written} of {DECLARED} bytes, so the response was buffered rather than aborted"
    );
}

/// The real transport, resolving through a host-supplied pin instead of the
/// system resolver. This is the shape a deployment uses when the address set is
/// decided outside the adapter.
struct PinnedTransport {
    address: IpAddr,
    inner: ReqwestTransport,
}

impl HttpTransport for PinnedTransport {
    fn resolve(&mut self, _host: &str, _port: u16) -> Result<Vec<IpAddr>, CapabilityError> {
        Ok(vec![self.address])
    }

    fn send(&mut self, request: &TransportRequest) -> Result<TransportResponse, CapabilityError> {
        self.inner.send(request)
    }
}

/// Proves the validated address set really decides the destination rather than
/// merely being checked alongside it. The host is in the `.invalid` TLD, which
/// no resolver can ever answer, so the request can only succeed if `reqwest`
/// dialed the address the adapter validated and never resolved the name for
/// itself. That is the whole of the DNS-rebinding guarantee, and no packet
/// leaves the machine on the passing path.
#[test]
fn the_validated_address_decides_the_destination() {
    let (port, server) = serve(|mut stream| {
        read_head(&mut stream);
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\ncontent-length: 13\r\ncontent-type: text/plain\r\n\r\nhello, spoon!",
        );
    });
    let host = "pinned.invalid";
    let limits = bounds(64 * 1024, 5_000);
    let mut adapter = NetworkAdapter::new(
        limits.clone(),
        vec![HostRule::secure(host).with_plaintext().with_ports([port])],
        ["GET"],
        Box::new(PinnedTransport {
            address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            inner: ReqwestTransport,
        }),
    )
    .expect("adapter")
    .with_address_policy(AddressPolicy::LoopbackPermitted);

    let execution = adapter
        .execute(&invocation(host, "GET", &plaintext("/hello", port), limits))
        .expect("request");

    assert_eq!(execution.output["status"], 200);
    assert_eq!(execution.output["body"], "hello, spoon!");
    assert_eq!(execution.output["addresses"][0], "127.0.0.1");
    server.join().expect("server");
}

#[test]
fn the_offline_and_real_transports_produce_identical_output() {
    let (port, server) = serve(|mut stream| {
        read_head(&mut stream);
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\ncontent-length: 13\r\ncontent-type: text/plain\r\n\r\nhello, spoon!",
        );
    });
    let input = plaintext("/hello", port);
    let limits = bounds(64 * 1024, 5_000);

    let mut real = loopback_adapter(port, limits.clone());
    let real_output = real
        .execute(&invocation(LOOPBACK, "GET", &input, limits.clone()))
        .expect("real request")
        .output;
    server.join().expect("server");

    let transport = MemoryTransport::new()
        .with_addresses(LOOPBACK, vec![IpAddr::V4(Ipv4Addr::LOCALHOST)])
        .with_response(
            "GET",
            &format!("http://127.0.0.1:{port}/hello"),
            body_response(),
        );
    let mut offline = NetworkAdapter::new(
        limits.clone(),
        vec![
            HostRule::secure(LOOPBACK)
                .with_plaintext()
                .with_ports([port]),
        ],
        ["GET"],
        Box::new(transport),
    )
    .expect("adapter")
    .with_address_policy(AddressPolicy::LoopbackPermitted);
    let offline_output = offline
        .execute(&invocation(LOOPBACK, "GET", &input, limits))
        .expect("offline request")
        .output;

    assert_eq!(real_output, offline_output);
}

#[test]
fn refuses_a_body_larger_than_the_declared_bound() {
    let transport = MemoryTransport::new()
        .with_addresses(HOST, vec![public()])
        .with_response(
            "POST",
            "https://api.example.test:443/submit",
            body_response(),
        );
    let mut adapter = net_adapter(vec![HostRule::secure(HOST)], Box::new(transport));
    let input = NetworkInput {
        path: "/submit".into(),
        body: "0123456789".into(),
        ..NetworkInput::default()
    };
    let mut call = invocation(HOST, "POST", &input, bounds(4_096, 5_000));
    call.request = PrimitiveRequest::Network {
        host: HOST.into(),
        method: "POST".into(),
        body_bytes: 4,
    };

    let error = adapter.execute(&call).expect_err("refused");

    assert!(matches!(error, CapabilityError::Invalid(_)), "{error}");
}

#[test]
fn rejects_bounds_and_methods_outside_the_supported_envelope() {
    let build = |bounds: ResourceBounds, methods: Vec<&'static str>| {
        NetworkAdapter::new(
            bounds,
            vec![HostRule::secure(HOST)],
            methods,
            Box::new(MemoryTransport::new()),
        )
    };

    assert!(build(bounds(0, 5_000), vec!["GET"]).is_err());
    assert!(build(bounds(4_096, 0), vec!["GET"]).is_err());
    assert!(build(bounds(4_096, 5_000), vec!["BREW"]).is_err());
    assert!(build(bounds(4_096, 5_000), Vec::new()).is_err());
    assert!(build(bounds(4_096, 5_000), vec!["GET"]).is_ok());

    let adapter = build(bounds(4_096, 5_000), vec!["GET"]).expect("adapter");
    assert!(
        adapter
            .with_redirect_policy(RedirectPolicy {
                max_hops: 200,
                cross_origin: false,
            })
            .is_err()
    );
}

/// An unresolvable name must fail as a resolution error rather than reaching a
/// socket, and it must never fall back to an ambient default.
#[test]
fn a_name_with_no_address_fails_without_dialing() {
    let mut adapter = net_adapter(
        vec![HostRule::secure(HOST)],
        Box::new(MemoryTransport::new()),
    );

    let error = adapter
        .execute(&invocation(
            HOST,
            "GET",
            &get("/hello"),
            bounds(4_096, 5_000),
        ))
        .expect_err("refused");

    assert!(
        error.to_string().contains("host resolution failed"),
        "{error}"
    );
}

/// `HostRule` and `NetworkInput` are the configuration surface, so their serde
/// shape is part of the contract.
#[test]
fn host_rules_and_input_round_trip_through_serde() {
    let rule = HostRule::secure(HOST).with_plaintext().with_ports([8443]);
    let encoded = serde_json::to_value(&rule).expect("encode");
    assert_eq!(encoded["plaintext"], true);
    assert_eq!(
        serde_json::from_value::<HostRule>(encoded).expect("decode"),
        rule
    );

    let input = with_headers("/hello", &[("accept", "text/plain")]);
    let encoded = network_input(&input).expect("encode");
    assert_eq!(
        serde_json::from_value::<NetworkInput>(encoded).expect("decode"),
        input
    );

    let unknown = serde_json::json!({"path": "/hello", "proxy": "http://evil"});
    assert!(serde_json::from_value::<NetworkInput>(unknown).is_err());
}

/// The offline transport is not a shortcut around policy: a canned response
/// still has to survive every bound the production path applies.
#[test]
fn the_offline_transport_enforces_the_same_byte_cap() {
    let transport = MemoryTransport::new()
        .with_addresses(HOST, vec![public()])
        .with_response(
            "GET",
            "https://api.example.test:443/big",
            TransportResponse::new(200, Vec::new(), vec![b'x'; 8_192]),
        );
    let mut adapter = net_adapter(vec![HostRule::secure(HOST)], Box::new(transport));

    let error = adapter
        .execute(&invocation(HOST, "GET", &get("/big"), bounds(1_024, 5_000)))
        .expect_err("refused");

    assert!(error.to_string().contains(OVERSIZED_MESSAGE), "{error}");
}

#[test]
fn header_names_and_values_are_bounded() {
    let transport = MemoryTransport::new().with_addresses(HOST, vec![public()]);
    let mut adapter = net_adapter(vec![HostRule::secure(HOST)], Box::new(transport));
    let long = "v".repeat(2_048);
    let mut oversized = BTreeMap::new();
    oversized.insert("x-note".to_owned(), long);

    for input in [
        with_headers("/hello", &[("bad name", "value")]),
        with_headers("/hello", &[("x-note", "line\r\nsmuggled: yes")]),
        NetworkInput {
            path: "/hello".into(),
            headers: oversized,
            ..NetworkInput::default()
        },
    ] {
        let error = adapter
            .execute(&invocation(HOST, "GET", &input, bounds(4_096, 5_000)))
            .expect_err("refused");
        assert!(matches!(error, CapabilityError::Invalid(_)), "{error}");
    }
}
