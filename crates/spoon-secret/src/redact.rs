use std::fmt;

use serde_json::Value;

use crate::{ResolvedSecret, SecretRef, hex_encode, wipe};

/// The replacement written in place of a secret value. It names the reference,
/// which is safe and is what a reader of a receipt actually needs, and never
/// the value or its length.
pub fn redaction_marker(reference: &SecretRef) -> String {
    format!("[redacted:{reference}]")
}

/// Removes known secret values from text and JSON on the way out.
///
/// This is defense in depth, not the primary control. The primary control is
/// that a value never leaves the adapter that resolved it. The redactor exists
/// because that control can be broken by a mistake: an error message that
/// interpolates a request URL, a receipt that echoes a header, a log line that
/// dumps a body. It only knows values that were registered from a
/// [`ResolvedSecret`], so it cannot invent a match.
#[derive(Default)]
pub struct Redactor {
    entries: Vec<Entry>,
}

struct Entry {
    needle: String,
    marker: String,
}

impl Redactor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a resolved value in every form it plausibly reaches a string.
    ///
    /// The literal value covers a body, a header, or an error message. The
    /// percent-encoded form covers a URL query parameter, where a value
    /// containing reserved characters is escaped before it is logged. The hex
    /// form covers binary key material, which is how such a value is rendered
    /// when it is rendered at all.
    pub fn register(&mut self, secret: &ResolvedSecret) {
        let marker = redaction_marker(secret.reference());
        let mut needles = vec![hex_encode(secret.expose())];
        if let Ok(text) = std::str::from_utf8(secret.expose()) {
            needles.push(percent_encode(text));
            needles.push(text.to_owned());
        }
        for needle in needles {
            if needle.is_empty() || self.entries.iter().any(|entry| entry.needle == needle) {
                continue;
            }
            self.entries.push(Entry {
                needle,
                marker: marker.clone(),
            });
        }
        // Longest first, so a value that contains another registered value is
        // replaced whole instead of being broken up by the shorter match.
        self.entries
            .sort_by_key(|entry| std::cmp::Reverse(entry.needle.len()));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn redact_text(&self, text: &str) -> String {
        let mut redacted = text.to_owned();
        for entry in &self.entries {
            if redacted.contains(&entry.needle) {
                redacted = redacted.replace(&entry.needle, &entry.marker);
            }
        }
        redacted
    }

    /// Redacts values, object keys, and numbers. Keys are included because a
    /// map keyed by a token is a real shape, and numbers because a numeric
    /// secret such as a PIN would otherwise pass through untouched.
    pub fn redact_json(&self, value: &Value) -> Value {
        match value {
            Value::Null | Value::Bool(_) => value.clone(),
            Value::Number(number) => {
                let text = number.to_string();
                let redacted = self.redact_text(&text);
                if redacted == text {
                    value.clone()
                } else {
                    Value::String(redacted)
                }
            }
            Value::String(text) => Value::String(self.redact_text(text)),
            Value::Array(items) => {
                Value::Array(items.iter().map(|item| self.redact_json(item)).collect())
            }
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, field)| (self.redact_text(key), self.redact_json(field)))
                    .collect(),
            ),
        }
    }
}

impl fmt::Debug for Redactor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Redactor {{ entries: {} }}", self.entries.len())
    }
}

impl Drop for Redactor {
    fn drop(&mut self) {
        for entry in &mut self.entries {
            // Safety: writing zero bytes over a `String` keeps it valid UTF-8,
            // and the needle is never read after this point.
            wipe(unsafe { entry.needle.as_mut_vec() });
        }
    }
}

/// Percent-encoding of everything outside the RFC 3986 unreserved set, which
/// is what a query-string writer produces for a secret containing reserved
/// characters.
fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}
