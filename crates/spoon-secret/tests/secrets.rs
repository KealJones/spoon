use serde_json::{Value, json};
use spoon_capability::{CapabilityError, NativePrimitive};
use spoon_secret::{
    EnvResolver, GrantLedger, GrantStatus, InMemoryResolver, Redactor, ResolvedSecret,
    SecretBroker, SecretError, SecretGrant, SecretRef, SecretResolver, SecretScope, SecretUse,
};

const NOW: i64 = 1_700_000_000;
const TOKEN: &str = "hunter2-must-never-be-logged";
const MARKER: &str = "[redacted:secret://api/publish-token@v1]";

fn reference() -> SecretRef {
    SecretRef::new("api", "publish-token", 1).expect("reference")
}

fn scope() -> SecretScope {
    SecretScope::new(
        NativePrimitive::NetworkRequest,
        "api.example.com",
        "publish",
    )
    .expect("scope")
}

fn use_site() -> SecretUse {
    SecretUse::new(
        NativePrimitive::NetworkRequest,
        "api.example.com",
        "publish",
    )
    .expect("use")
}

fn broker(value: &[u8], not_after: i64) -> SecretBroker<InMemoryResolver> {
    let mut resolver = InMemoryResolver::new();
    resolver.insert(reference(), value).expect("insert");
    let mut ledger = GrantLedger::new();
    ledger
        .issue(SecretGrant::new(reference(), vec![scope()], NOW, not_after).expect("grant"))
        .expect("issue");
    SecretBroker::new(ledger, resolver)
}

#[test]
fn a_reference_never_carries_a_value() {
    let broker = broker(TOKEN.as_bytes(), NOW + 60);
    let secret = broker
        .resolve(&reference(), &use_site(), NOW)
        .expect("resolve");
    let reference = secret.reference();

    let display = reference.to_string();
    let debug = format!("{reference:?}");
    let serialized = serde_json::to_string(reference).expect("serialize");
    assert_eq!(display, "secret://api/publish-token@v1");
    assert_eq!(debug, "SecretRef(secret://api/publish-token@v1)");
    assert_eq!(serialized, "\"secret://api/publish-token@v1\"");
    for rendered in [display, debug, serialized.clone()] {
        assert!(!rendered.contains(TOKEN), "{rendered} leaked the value");
    }

    let parsed: SecretRef = serde_json::from_str(&serialized).expect("deserialize");
    assert_eq!(&parsed, reference);
    assert_eq!(parsed.version(), 1);
    assert_eq!(parsed.namespace(), "api");
    assert_eq!(parsed.name(), "publish-token");
}

#[test]
fn a_malformed_reference_is_refused() {
    for text in [
        "api/publish-token@v1",
        "secret://api@v1",
        "secret://api/publish-token",
        "secret://api/publish-token@vzero",
        "secret://api/publish-token@v0",
        "secret://api/pub lish@v1",
        "secret:///publish-token@v1",
    ] {
        assert!(
            matches!(SecretRef::parse(text), Err(SecretError::Invalid(_))),
            "{text} parsed"
        );
    }
}

#[test]
fn a_resolved_secret_prints_redacted() {
    let broker = broker(TOKEN.as_bytes(), NOW + 60);
    let secret = broker
        .resolve(&reference(), &use_site(), NOW)
        .expect("resolve");

    let debug = format!("{secret:?}");
    assert!(!debug.contains(TOKEN));
    assert!(debug.contains(MARKER), "{debug} lacks a redaction marker");
    assert_eq!(secret.expose(), TOKEN.as_bytes());
}

#[test]
fn the_environment_resolver_honors_its_allowlist() {
    let allowed = SecretRef::new("env", "allowed", 1).expect("reference");
    let refused = SecretRef::new("env", "refused", 1).expect("reference");
    let absent = SecretRef::new("env", "absent", 2).expect("reference");
    assert_eq!(
        EnvResolver::env_key(&allowed),
        "SPOON_SECRET_ENV_ALLOWED_V1"
    );
    assert_eq!(EnvResolver::env_key(&absent), "SPOON_SECRET_ENV_ABSENT_V2");

    // Safety: this is the only test in the crate that touches the process
    // environment, and the names it writes are read by nothing else.
    unsafe {
        std::env::set_var("SPOON_SECRET_ENV_ALLOWED_V1", TOKEN);
        std::env::set_var("SPOON_SECRET_ENV_REFUSED_V1", TOKEN);
    }

    let resolver = EnvResolver::new([
        "SPOON_SECRET_ENV_ALLOWED_V1".to_owned(),
        "SPOON_SECRET_ENV_ABSENT_V2".to_owned(),
    ])
    .expect("resolver");

    assert_eq!(
        resolver.resolve(&allowed).expect("resolve").expose(),
        TOKEN.as_bytes()
    );
    // Present in the environment, absent from the allowlist: still refused.
    assert!(matches!(
        resolver.resolve(&refused),
        Err(SecretError::EnvNotAllowed { key }) if key == "SPOON_SECRET_ENV_REFUSED_V1"
    ));
    assert!(matches!(
        resolver.resolve(&absent),
        Err(SecretError::EnvUnavailable { key }) if key == "SPOON_SECRET_ENV_ABSENT_V2"
    ));
    assert!(matches!(
        EnvResolver::new(["BAD=NAME".to_owned()]),
        Err(SecretError::Invalid(_))
    ));
}

#[test]
fn an_unknown_reference_fails_cleanly() {
    let missing = SecretRef::new("api", "absent", 1).expect("reference");
    let ungranted = broker(TOKEN.as_bytes(), NOW + 60);
    assert!(matches!(
        ungranted.resolve(&missing, &use_site(), NOW),
        Err(SecretError::Unknown(reference)) if reference == missing
    ));

    let mut ledger = GrantLedger::new();
    ledger
        .issue(SecretGrant::new(missing.clone(), vec![scope()], NOW, NOW + 60).expect("grant"))
        .expect("issue");
    let starved = SecretBroker::new(ledger, InMemoryResolver::new());
    let error = starved
        .resolve(&missing, &use_site(), NOW)
        .expect_err("no material");
    assert_eq!(
        error.to_string(),
        "no secret grant or material is registered for secret://api/absent@v1"
    );
    assert!(matches!(
        CapabilityError::from(error),
        CapabilityError::PermissionRequired(_)
    ));
}

#[test]
fn use_outside_the_declared_scope_fails() {
    let broker = broker(TOKEN.as_bytes(), NOW + 60);
    let wrong = [
        SecretUse::new(NativePrimitive::FileWrite, "api.example.com", "publish"),
        SecretUse::new(
            NativePrimitive::NetworkRequest,
            "evil.example.com",
            "publish",
        ),
        SecretUse::new(
            NativePrimitive::NetworkRequest,
            "api.example.com",
            "authenticate",
        ),
    ];
    for use_site in wrong {
        let use_site = use_site.expect("use");
        let error = broker
            .resolve(&reference(), &use_site, NOW)
            .expect_err("out of scope");
        assert!(matches!(error, SecretError::OutOfScope { .. }), "{error}");
        assert!(!error.to_string().contains(TOKEN));
    }
    assert!(broker.resolve(&reference(), &use_site(), NOW).is_ok());
}

#[test]
fn a_file_scope_matches_on_a_path_boundary() {
    let reference = SecretRef::new("disk", "archive-key", 1).expect("reference");
    let scope = SecretScope::new(NativePrimitive::FileRead, "/var/data", "decrypt").expect("scope");
    let mut resolver = InMemoryResolver::new();
    resolver
        .insert(reference.clone(), b"disk-key")
        .expect("insert");
    let mut ledger = GrantLedger::new();
    ledger
        .issue(SecretGrant::new(reference.clone(), vec![scope], NOW, NOW + 60).expect("grant"))
        .expect("issue");
    let broker = SecretBroker::new(ledger, resolver);

    for permitted in ["/var/data", "/var/data/records.db", "/var/data/a/b"] {
        let use_site =
            SecretUse::new(NativePrimitive::FileRead, permitted, "decrypt").expect("use");
        assert!(
            broker.resolve(&reference, &use_site, NOW).is_ok(),
            "{permitted} was refused"
        );
    }
    for refused in ["/var/database/records.db", "/var", "/var/datax"] {
        let use_site = SecretUse::new(NativePrimitive::FileRead, refused, "decrypt").expect("use");
        assert!(
            matches!(
                broker.resolve(&reference, &use_site, NOW),
                Err(SecretError::OutOfScope { .. })
            ),
            "{refused} was permitted"
        );
    }
}

#[test]
fn an_expired_grant_fails() {
    let broker = broker(TOKEN.as_bytes(), NOW + 60);
    assert!(broker.resolve(&reference(), &use_site(), NOW + 60).is_ok());
    assert!(matches!(
        broker.resolve(&reference(), &use_site(), NOW + 61),
        Err(SecretError::Expired { not_after, now, .. }) if not_after == NOW + 60 && now == NOW + 61
    ));
    assert!(matches!(
        SecretGrant::new(reference(), vec![scope()], NOW, NOW),
        Err(SecretError::Invalid(_))
    ));
    assert!(matches!(
        SecretGrant::new(reference(), Vec::new(), NOW, NOW + 60),
        Err(SecretError::Invalid(_))
    ));
}

#[test]
fn rotation_supersedes_a_version_and_keeps_the_audit_trail() {
    let first = reference();
    let second = first.at_version(2).expect("reference");
    let mut broker = broker(TOKEN.as_bytes(), NOW + 60);

    let rotated =
        SecretGrant::new(second.clone(), vec![scope()], NOW + 10, NOW + 600).expect("grant");
    let superseded = broker
        .ledger_mut()
        .rotate(rotated, NOW + 10)
        .expect("rotate");
    assert_eq!(superseded, first);
    assert!(broker.resolver_mut().remove(&first));
    broker
        .resolver_mut()
        .insert(second.clone(), b"rotated-value")
        .expect("insert");

    assert!(matches!(
        broker.resolve(&first, &use_site(), NOW + 11),
        Err(SecretError::Superseded { current: 2, .. })
    ));
    assert_eq!(
        broker
            .resolve(&second, &use_site(), NOW + 11)
            .expect("resolve")
            .expose(),
        b"rotated-value"
    );

    let history = broker.ledger().history("api", "publish-token");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].grant.reference, first);
    assert_eq!(
        history[0].status,
        GrantStatus::Superseded {
            by: 2,
            at: NOW + 10
        }
    );
    assert_eq!(history[1].grant.reference, second);
    assert_eq!(history[1].status, GrantStatus::Active);

    // Rotation only moves forward, and a second grant cannot be issued behind
    // the ledger's back.
    let backwards = SecretGrant::new(first.clone(), vec![scope()], NOW, NOW + 600).expect("grant");
    assert!(matches!(
        broker.ledger_mut().rotate(backwards, NOW + 20),
        Err(SecretError::Invalid(_))
    ));
    let duplicate = SecretGrant::new(
        first.at_version(3).expect("reference"),
        vec![scope()],
        NOW,
        NOW + 600,
    )
    .expect("grant");
    assert!(matches!(
        broker.ledger_mut().issue(duplicate),
        Err(SecretError::Invalid(_))
    ));
}

#[test]
fn revocation_stops_resolution_and_is_recorded() {
    let mut broker = broker(TOKEN.as_bytes(), NOW + 600);
    broker
        .ledger_mut()
        .revoke(&reference(), NOW + 5)
        .expect("revoke");
    assert!(matches!(
        broker.resolve(&reference(), &use_site(), NOW + 6),
        Err(SecretError::Revoked { at, .. }) if at == NOW + 5
    ));
    assert_eq!(
        broker.ledger().record(&reference()).expect("record").status,
        GrantStatus::Revoked { at: NOW + 5 }
    );
    assert!(matches!(
        broker.ledger_mut().revoke(&reference(), NOW + 7),
        Err(SecretError::Invalid(_))
    ));
}

#[test]
fn the_ledger_round_trips_through_serde() {
    let broker = broker(TOKEN.as_bytes(), NOW + 60);
    let text = serde_json::to_string(broker.ledger()).expect("serialize");
    assert!(text.contains("secret://api/publish-token@v1"));
    assert!(!text.contains(TOKEN));
    let parsed: GrantLedger = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(&parsed, broker.ledger());
}

#[test]
fn redaction_covers_nested_json_url_queries_and_errors() {
    let value = "tok/en+with=chars";
    let mut resolver = InMemoryResolver::new();
    resolver
        .insert(reference(), value.as_bytes())
        .expect("insert");
    let mut ledger = GrantLedger::new();
    ledger
        .issue(SecretGrant::new(reference(), vec![scope()], NOW, NOW + 60).expect("grant"))
        .expect("issue");
    let broker = SecretBroker::new(ledger, resolver);

    let mut redactor = Redactor::new();
    let secret = broker
        .resolve_redacted(&reference(), &use_site(), NOW, &mut redactor)
        .expect("resolve");
    assert!(!redactor.is_empty());

    let receipt = json!({
        "request": {
            "headers": { "authorization": format!("Bearer {value}") },
            "retries": 2,
        },
        "trace": [{ "note": format!("sent {value} upstream") }],
    });
    let redacted = redactor.redact_json(&receipt);
    let rendered = serde_json::to_string(&redacted).expect("serialize");
    assert!(!rendered.contains(value), "{rendered} leaked the value");
    assert_eq!(
        redacted["request"]["headers"]["authorization"],
        json!(format!("Bearer {MARKER}"))
    );
    assert_eq!(redacted["request"]["retries"], json!(2));
    assert_eq!(
        redacted["trace"][0]["note"],
        json!(format!("sent {MARKER} upstream"))
    );

    let mut keyed = serde_json::Map::new();
    keyed.insert(value.to_owned(), json!("secret used as a field name"));
    assert!(
        redactor
            .redact_json(&Value::Object(keyed))
            .get(MARKER)
            .is_some()
    );

    let raw_query = format!("https://api.example.com/v1/publish?token={value}&page=2");
    assert_eq!(
        redactor.redact_text(&raw_query),
        format!("https://api.example.com/v1/publish?token={MARKER}&page=2")
    );
    let encoded_query = "https://api.example.com/v1/publish?token=tok%2Fen%2Bwith%3Dchars&page=2";
    assert_eq!(
        redactor.redact_text(encoded_query),
        format!("https://api.example.com/v1/publish?token={MARKER}&page=2")
    );

    let error = format!("upstream rejected credential {value} for api.example.com");
    let redacted_error = redactor.redact_text(&error);
    assert!(!redacted_error.contains(value));
    assert_eq!(
        redacted_error,
        format!("upstream rejected credential {MARKER} for api.example.com")
    );

    let pin = ResolvedSecret::new(
        SecretRef::new("api", "pin", 1).expect("reference"),
        b"918273645".to_vec(),
    )
    .expect("secret");
    redactor.register(&pin);
    assert_eq!(
        redactor.redact_json(&json!({ "pin": 918_273_645 })),
        json!({ "pin": "[redacted:secret://api/pin@v1]" })
    );

    assert_eq!(secret.expose(), value.as_bytes());
}

#[test]
fn a_binary_secret_is_redacted_in_its_hex_rendering() {
    let reference = SecretRef::new("disk", "master-key", 1).expect("reference");
    let secret = ResolvedSecret::new(reference, vec![0xde, 0xad, 0xbe, 0xef]).expect("secret");
    let mut redactor = Redactor::new();
    redactor.register(&secret);
    assert_eq!(
        redactor.redact_text("wrapped key deadbeef written to disk"),
        "wrapped key [redacted:secret://disk/master-key@v1] written to disk"
    );
}

#[test]
fn an_empty_value_is_refused() {
    let mut resolver = InMemoryResolver::new();
    assert!(matches!(
        resolver.insert(reference(), b""),
        Err(SecretError::Invalid(_))
    ));
    assert!(matches!(
        ResolvedSecret::new(reference(), Vec::new()),
        Err(SecretError::Invalid(_))
    ));
}
