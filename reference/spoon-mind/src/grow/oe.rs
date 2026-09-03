//! Observational-equivalence table for synthesis.
//!
//! Key = type tag + "|" + canonical output vector. The first (smallest, in
//! bottom-up enumeration) program per equivalence class is kept; later
//! programs with the same outputs are pruned.

use std::collections::HashMap;

use spoon_core::types::{Type, Value};

// ---- insertion result --------------------------------------------------

pub enum OeInsert {
    /// New class - caller should add the expression to the bank.
    New,
    /// Same output vector already present - discard the candidate.
    Dominated,
    /// Table is at capacity.
    TableFull,
}

// ---- table -------------------------------------------------------------

pub struct OeTable {
    inner: HashMap<String, ()>,
    max_entries: usize,
}

impl OeTable {
    pub fn new(max_entries: usize) -> Self {
        OeTable { inner: HashMap::new(), max_entries }
    }

    pub fn try_insert(&mut self, key: &str) -> OeInsert {
        if self.inner.contains_key(key) {
            return OeInsert::Dominated;
        }
        if self.inner.len() >= self.max_entries {
            return OeInsert::TableFull;
        }
        self.inner.insert(key.to_string(), ());
        OeInsert::New
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }
}

// ---- key construction --------------------------------------------------

/// Canonical key for an output vector, prefixed with the (inferred) output type.
pub fn oe_key(output_ty: &Type, outputs: &[Value]) -> String {
    let mut key = output_ty.to_string();
    key.push('|');
    for (i, v) in outputs.iter().enumerate() {
        if i > 0 {
            key.push('|');
        }
        key.push_str(&value_key(v));
    }
    key
}

fn value_key(v: &Value) -> String {
    match v {
        // Normalise floats to 9 dp to absorb floating-point rounding.
        Value::Float(f) => format!("F{:.9}", f),
        Value::List(items) => {
            let inner: Vec<String> = items.iter().map(value_key).collect();
            format!("L[{}]", inner.join(","))
        }
        // JSON, Struct, etc. fall through to serde.
        _ => serde_json::to_string(v).unwrap_or_else(|_| format!("{v:?}")),
    }
}

// ---- comparison --------------------------------------------------------

/// True when actual and expected vectors match element-wise with 1e-9 float
/// tolerance. Lists are compared recursively.
pub fn outputs_match(actual: &[Value], expected: &[Value]) -> bool {
    actual.len() == expected.len()
        && actual.iter().zip(expected.iter()).all(|(a, e)| values_eq(a, e))
}

pub fn values_eq(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < 1e-9,
        _ => match (a, b) {
            (Value::List(la), Value::List(lb)) => {
                la.len() == lb.len()
                    && la.iter().zip(lb.iter()).all(|(x, y)| values_eq(x, y))
            }
            _ => a == b,
        },
    }
}
