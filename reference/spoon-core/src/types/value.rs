//! Runtime values and their static types.
//!
//! `Type` is the currency of the whole system: the planner connects actions by
//! type, the synthesizer enumerates by type, the lexicon attaches nouns to
//! concept types. Keep it small.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

use super::can::ConceptId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "t", content = "c", rename_all = "snake_case")]
pub enum Type {
    Null,
    Bool,
    Int,
    Float,
    /// Free text. Not NL-visible as a vocabulary item.
    Text,
    /// A proper name / identifier (Viv `name`). NL-visible.
    Name,
    /// Absolute instant, UTC.
    DateTime,
    /// Length of time in seconds.
    Duration,
    /// Filesystem path.
    Path,
    Url,
    /// Arbitrary JSON.
    Json,
    List(Box<Type>),
    /// A structured or enum concept from the CAN. Primitive concepts map to the
    /// primitive variants above instead.
    Concept(ConceptId),
    /// Function type, used for lambdas passed to higher-order primitives.
    Func(Vec<Type>, Box<Type>),
    /// Top type. Only for a few reflective primitives; the synthesizer never
    /// enumerates into `Any`.
    Any,
}

impl Type {
    pub fn list(inner: Type) -> Type {
        Type::List(Box::new(inner))
    }
    pub fn func(params: Vec<Type>, ret: Type) -> Type {
        Type::Func(params, Box::new(ret))
    }
    pub fn is_numeric(&self) -> bool {
        matches!(self, Type::Int | Type::Float)
    }
    /// Structural assignability without CAN knowledge. `is_a` edges between
    /// concepts are resolved by the CAN, not here.
    pub fn accepts(&self, other: &Type) -> bool {
        match (self, other) {
            (Type::Any, _) => true,
            (Type::Float, Type::Int) => true,
            (Type::List(a), Type::List(b)) => a.accepts(b),
            (Type::Func(pa, ra), Type::Func(pb, rb)) => {
                pa.len() == pb.len()
                    && pa.iter().zip(pb).all(|(x, y)| y.accepts(x))
                    && ra.accepts(rb)
            }
            (a, b) => a == b,
        }
    }
    /// Runtime check of a concrete value against this type. Unlike `accepts`
    /// it looks inside lists, so an empty list conforms to any list type and
    /// a `Name` conforms to any entity concept (is-a is the CAN's business).
    pub fn accepts_value(&self, v: &Value) -> bool {
        match (self, v) {
            (Type::Any, _) => true,
            (Type::Float, Value::Int(_)) => true,
            (Type::List(t), Value::List(items)) => items.iter().all(|i| t.accepts_value(i)),
            (Type::Concept(_), Value::Name(_)) => true,
            (Type::Concept(c), Value::Struct { concept, .. }) => c == concept,
            (Type::Func(ps, r), Value::Lambda(l)) => {
                ps.len() == l.params.len() && r.accepts(&l.ret)
            }
            (t, v) => *t == v.type_of(),
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Null => write!(f, "null"),
            Type::Bool => write!(f, "bool"),
            Type::Int => write!(f, "int"),
            Type::Float => write!(f, "float"),
            Type::Text => write!(f, "text"),
            Type::Name => write!(f, "name"),
            Type::DateTime => write!(f, "datetime"),
            Type::Duration => write!(f, "duration"),
            Type::Path => write!(f, "path"),
            Type::Url => write!(f, "url"),
            Type::Json => write!(f, "json"),
            Type::List(t) => write!(f, "[{t}]"),
            Type::Concept(c) => write!(f, "{}", c.0),
            Type::Func(ps, r) => {
                write!(f, "(")?;
                for (i, p) in ps.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ") -> {r}")
            }
            Type::Any => write!(f, "any"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Name(String),
    /// Unix milliseconds UTC.
    DateTime(i64),
    /// Seconds.
    Duration(f64),
    Path(String),
    Url(String),
    Json(serde_json::Value),
    List(Vec<Value>),
    Struct {
        concept: ConceptId,
        fields: BTreeMap<String, Value>,
    },
    /// A closure: a program body expecting `params`. Only produced by the IR,
    /// consumed by higher-order primitives.
    Lambda(Box<super::ir::Lambda>),
}

impl Value {
    pub fn type_of(&self) -> Type {
        match self {
            Value::Null => Type::Null,
            Value::Bool(_) => Type::Bool,
            Value::Int(_) => Type::Int,
            Value::Float(_) => Type::Float,
            Value::Text(_) => Type::Text,
            Value::Name(_) => Type::Name,
            Value::DateTime(_) => Type::DateTime,
            Value::Duration(_) => Type::Duration,
            Value::Path(_) => Type::Path,
            Value::Url(_) => Type::Url,
            Value::Json(_) => Type::Json,
            Value::List(items) => Type::List(Box::new(
                items.first().map(|v| v.type_of()).unwrap_or(Type::Any),
            )),
            Value::Struct { concept, .. } => Type::Concept(concept.clone()),
            Value::Lambda(l) => Type::Func(l.params.clone(), Box::new(l.ret.clone())),
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) if f.fract() == 0.0 => Some(*f as i64),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            Value::Duration(d) => Some(*d),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) | Value::Name(s) | Value::Path(s) | Value::Url(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(v) => Some(v),
            _ => None,
        }
    }
    pub fn text(s: impl Into<String>) -> Value {
        Value::Text(s.into())
    }
    pub fn name(s: impl Into<String>) -> Value {
        Value::Name(s.into())
    }
    /// Human-facing rendering used by the template realizer and the
    /// faithfulness check in the mouth.
    pub fn render(&self) -> String {
        match self {
            Value::Null => "nothing".into(),
            Value::Bool(b) => if *b { "yes" } else { "no" }.into(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => {
                if f.fract() == 0.0 && f.abs() < 1e15 {
                    format!("{}", *f as i64)
                } else {
                    let s = format!("{f:.4}");
                    s.trim_end_matches('0').trim_end_matches('.').to_string()
                }
            }
            Value::Text(s) | Value::Name(s) | Value::Path(s) | Value::Url(s) => s.clone(),
            Value::DateTime(ms) => chrono::DateTime::<chrono::Utc>::from_timestamp_millis(*ms)
                .map(|d| d.to_rfc3339())
                .unwrap_or_else(|| ms.to_string()),
            Value::Duration(s) => format!("{} seconds", Value::Float(*s).render()),
            Value::Json(j) => j.to_string(),
            Value::List(items) => {
                let parts: Vec<String> = items.iter().map(|v| v.render()).collect();
                parts.join(", ")
            }
            Value::Struct { concept, fields } => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", v.render()))
                    .collect();
                format!("{} ({})", concept.0, parts.join(", "))
            }
            Value::Lambda(_) => "<function>".into(),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.render())
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Text(v.to_string())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Text(v)
    }
}
impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::List(v)
    }
}
