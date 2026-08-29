use spoon_secret::{
    ResolvedSecret, SecretError, SecretRef, Signature, SignatureAlgorithm, SigningIdentity,
    hmac_sha256,
};

const SIGNED_AT: i64 = 1_700_000_000;
const IDENTITY: &str = "spoon:local:forge";
/// The shape a seed forge would sign: the content address of a bundle.
const PAYLOAD: &[u8] = b"sha256:9f2c1b7a4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8";

fn key(seed: u8) -> Vec<u8> {
    vec![seed; 32]
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// Known-answer tests from RFC 4231 section 4. Cases 1 through 4, 6, and 7;
/// case 5 is a truncation test and this implementation does not truncate.
#[test]
fn hmac_matches_the_rfc4231_test_vectors() {
    let cases: [(Vec<u8>, Vec<u8>, &str); 6] = [
        (
            vec![0x0b; 20],
            b"Hi There".to_vec(),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
        ),
        (
            b"Jefe".to_vec(),
            b"what do ya want for nothing?".to_vec(),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
        ),
        (
            vec![0xaa; 20],
            vec![0xdd; 50],
            "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe",
        ),
        (
            (0x01u8..=0x19).collect(),
            vec![0xcd; 50],
            "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b",
        ),
        (
            vec![0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First".to_vec(),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54",
        ),
        (
            vec![0xaa; 131],
            b"This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm.".to_vec(),
            "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2",
        ),
    ];
    for (key, message, expected) in cases {
        assert_eq!(hex(&hmac_sha256(&key, &message)), expected);
    }
}

#[test]
fn signing_then_verifying_round_trips() {
    let identity = SigningIdentity::new(IDENTITY, &key(0x11)).expect("identity");
    let signature = identity.sign(PAYLOAD, SIGNED_AT);

    assert_eq!(signature.identity, IDENTITY);
    assert_eq!(signature.algorithm, SignatureAlgorithm::HmacSha256);
    assert_eq!(signature.signed_at, SIGNED_AT);
    assert_eq!(signature.mac.len(), 64);
    identity.verify(PAYLOAD, &signature).expect("verify");

    // Deterministic, so a publication signature can be recomputed and compared
    // rather than only checked.
    assert_eq!(identity.sign(PAYLOAD, SIGNED_AT), signature);

    let text = serde_json::to_string(&signature).expect("serialize");
    assert!(!text.contains(&hex(&key(0x11))));
    let parsed: Signature = serde_json::from_str(&text).expect("deserialize");
    identity.verify(PAYLOAD, &parsed).expect("verify parsed");
}

#[test]
fn a_tampered_payload_or_receipt_fails() {
    let identity = SigningIdentity::new(IDENTITY, &key(0x11)).expect("identity");
    let signature = identity.sign(PAYLOAD, SIGNED_AT);

    assert!(matches!(
        identity.verify(
            b"sha256:0000000000000000000000000000000000000000000000000000000000000000",
            &signature
        ),
        Err(SecretError::SignatureMismatch { .. })
    ));

    let mut truncated_payload = PAYLOAD.to_vec();
    truncated_payload.pop();
    assert!(matches!(
        identity.verify(&truncated_payload, &signature),
        Err(SecretError::SignatureMismatch { .. })
    ));

    let mut moved = signature.clone();
    moved.signed_at += 1;
    assert!(matches!(
        identity.verify(PAYLOAD, &moved),
        Err(SecretError::SignatureMismatch { .. })
    ));

    let mut flipped = signature.clone();
    let last = signature.mac.chars().last().expect("mac");
    flipped.mac = format!(
        "{}{}",
        &signature.mac[..63],
        if last == '0' { '1' } else { '0' }
    );
    assert!(matches!(
        identity.verify(PAYLOAD, &flipped),
        Err(SecretError::SignatureMismatch { .. })
    ));

    let mut malformed = signature.clone();
    malformed.mac = "not-hex".to_owned();
    assert!(matches!(
        identity.verify(PAYLOAD, &malformed),
        Err(SecretError::SignatureMismatch { .. })
    ));
}

#[test]
fn a_wrong_identity_fails() {
    let signer = SigningIdentity::new(IDENTITY, &key(0x11)).expect("identity");
    let signature = signer.sign(PAYLOAD, SIGNED_AT);

    // Same name, different key: the tag does not verify.
    let impostor = SigningIdentity::new(IDENTITY, &key(0x22)).expect("identity");
    assert!(matches!(
        impostor.verify(PAYLOAD, &signature),
        Err(SecretError::SignatureMismatch { .. })
    ));

    // Different name: refused before any tag is compared, because the name is
    // part of what was signed.
    let other = SigningIdentity::new("spoon:local:server", &key(0x11)).expect("identity");
    assert!(matches!(
        other.verify(PAYLOAD, &signature),
        Err(SecretError::IdentityMismatch { expected, found })
            if expected == "spoon:local:server" && found == IDENTITY
    ));
    assert_ne!(other.sign(PAYLOAD, SIGNED_AT).mac, signature.mac);
}

#[test]
fn an_identity_hides_its_key_and_refuses_a_weak_one() {
    let identity = SigningIdentity::new(IDENTITY, &key(0xab)).expect("identity");
    let debug = format!("{identity:?}");
    assert_eq!(
        debug,
        "SigningIdentity { id: \"spoon:local:forge\", key: [redacted] }"
    );
    assert!(!debug.contains(&hex(&key(0xab))));
    assert_eq!(identity.id(), IDENTITY);

    assert!(matches!(
        SigningIdentity::new(IDENTITY, &[0xab; 31]),
        Err(SecretError::Invalid(_))
    ));
    assert!(matches!(
        SigningIdentity::new("", &key(0xab)),
        Err(SecretError::Invalid(_))
    ));
    assert!(matches!(
        SigningIdentity::new("spoon local forge", &key(0xab)),
        Err(SecretError::Invalid(_))
    ));
}

#[test]
fn an_identity_key_can_come_from_a_just_in_time_resolution() {
    let reference = SecretRef::new("forge", "publish-key", 1).expect("reference");
    let secret = ResolvedSecret::new(reference, key(0x5a)).expect("secret");
    let identity = SigningIdentity::from_secret(IDENTITY, &secret).expect("identity");
    let signature = identity.sign(PAYLOAD, SIGNED_AT);
    identity.verify(PAYLOAD, &signature).expect("verify");
    assert!(!format!("{signature:?}").contains(&hex(&key(0x5a))));
}
