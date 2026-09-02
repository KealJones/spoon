//! Spec, pair, and concept parsing for the teacher seat.

use spoon_core::{
    Cardinality, Concept, ConceptId, ConceptKind, Example, Pair, Property, Provenance,
    Spec, Tier, Type, Value,
};

// ---------------------------------------------------------------------------
// Type parsing
// ---------------------------------------------------------------------------

/// Parse a type-name string into a `Type`.
///
/// Supported: Int, Float, Text, Bool, Name, Path, Url, DateTime, Duration,
/// Json, Null, List<T> (nestable), and PascalCase identifiers as
/// `Type::Concept`.
pub fn parse_type(s: &str) -> Result<Type, String> {
    let s = s.trim();
    match s {
        "Null" => Ok(Type::Null),
        "Bool" => Ok(Type::Bool),
        "Int" => Ok(Type::Int),
        "Float" => Ok(Type::Float),
        "Text" => Ok(Type::Text),
        "Name" => Ok(Type::Name),
        "DateTime" => Ok(Type::DateTime),
        "Duration" => Ok(Type::Duration),
        "Path" => Ok(Type::Path),
        "Url" => Ok(Type::Url),
        "Json" => Ok(Type::Json),
        _ if s.starts_with("List<") && s.ends_with('>') => {
            let inner = &s[5..s.len() - 1];
            parse_type(inner).map(|t| Type::List(Box::new(t)))
        }
        _ => {
            let first = s
                .chars()
                .next()
                .ok_or_else(|| "empty type string".to_string())?;
            if first.is_ascii_uppercase() && s.chars().all(|c| c.is_alphanumeric()) {
                Ok(Type::Concept(ConceptId(s.to_string())))
            } else {
                Err(format!("unknown or malformed type: {s}"))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Spec parsing
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct WireSpec {
    name_hint: String,
    #[serde(default)]
    description: String,
    params: Vec<String>,
    #[serde(default)]
    param_names: Vec<String>,
    ret: String,
    #[serde(default)]
    verbs: Vec<String>,
    #[serde(default)]
    phrasings: Vec<String>,
    examples: Vec<WireExample>,
}

#[derive(serde::Deserialize)]
struct WireExample {
    inputs: Vec<serde_json::Value>,
    output: serde_json::Value,
}

/// Parse a teacher-produced spec JSON string into a validated `Spec`.
///
/// `spec_id` is the stable id assigned by the caller (uuid or lesson id).
pub fn parse_spec_json(json: &str, spec_id: &str) -> Result<Spec, String> {
    let json = super::strip_fences(json);
    let raw: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("spec JSON parse error: {e}"))?;
    super::check_no_code(&raw)?;

    let wire: WireSpec = serde_json::from_value(raw.clone())
        .map_err(|e| format!("spec schema error: {e}"))?;

    if wire.params.is_empty() || wire.params.len() > 4 {
        return Err(format!(
            "spec must have 1-4 params, got {}",
            wire.params.len()
        ));
    }
    if wire.examples.len() < 4 || wire.examples.len() > 8 {
        return Err(format!(
            "spec must have 4-8 examples, got {}",
            wire.examples.len()
        ));
    }
    if !is_snake_case(&wire.name_hint) {
        return Err(format!(
            "name_hint '{}' is not snake_case ASCII",
            wire.name_hint
        ));
    }

    let params: Vec<Type> = wire
        .params
        .iter()
        .map(|s| parse_type(s))
        .collect::<Result<_, _>>()?;
    let ret = parse_type(&wire.ret)?;

    let examples: Vec<Example> = wire
        .examples
        .iter()
        .enumerate()
        .map(|(i, ex)| {
            if ex.inputs.len() != params.len() {
                return Err(format!(
                    "example {i}: expected {} inputs, got {}",
                    params.len(),
                    ex.inputs.len()
                ));
            }
            let inputs: Vec<Value> = ex
                .inputs
                .iter()
                .zip(&params)
                .enumerate()
                .map(|(j, (v, t))| {
                    coerce_value(v, t)
                        .map_err(|e| format!("example {i} input {j}: {e}"))
                })
                .collect::<Result<_, _>>()?;
            let output = coerce_value(&ex.output, &ret)
                .map_err(|e| format!("example {i} output: {e}"))?;
            Ok(Example { inputs, output })
        })
        .collect::<Result<_, _>>()?;

    let spec = Spec {
        id: spec_id.to_string(),
        name_hint: wire.name_hint,
        verbs: wire.verbs,
        phrasings: wire.phrasings,
        params,
        param_names: wire.param_names,
        ret,
        examples,
        description: wire.description,
        source: "teacher".to_string(),
    };

    spec.validate().map_err(|e| format!("spec validation failed: {e}"))?;
    Ok(spec)
}

// ---------------------------------------------------------------------------
// Pairs parsing
// ---------------------------------------------------------------------------

/// Parse a teacher-produced phrasings JSON into `Pair` values.
///
/// Expected shape: `{"phrasings": ["...", "..."]}`
/// `sce` is the canonical SCE all phrasings map to.
pub fn parse_pairs_json(json: &str, sce: &str) -> Result<Vec<Pair>, String> {
    let json = super::strip_fences(json);
    let raw: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("pairs JSON parse error: {e}"))?;
    super::check_no_code(&raw)?;

    let arr = raw
        .get("phrasings")
        .and_then(|p| p.as_array())
        .ok_or_else(|| "pairs JSON must have a 'phrasings' array".to_string())?;

    arr.iter()
        .map(|p| {
            let utterance = p
                .as_str()
                .ok_or_else(|| format!("phrasing must be a string, got: {p}"))?
                .to_string();
            Ok(Pair {
                id: 0,
                utterance,
                sce: sce.to_string(),
                source: "teacher".to_string(),
                at: 0,
                credit: 0,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Concept parsing
// ---------------------------------------------------------------------------

/// Parse a teacher-produced concept JSON into a `Concept`.
///
/// Expected shape: `{"id": "Dog", "kind": "entity", "extends": [...], ...}`
pub fn parse_concept_json(json: &str) -> Result<Concept, String> {
    let json = super::strip_fences(json);
    let raw: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("concept JSON parse error: {e}"))?;
    super::check_no_code(&raw)?;

    let id_str = raw
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "concept missing 'id'".to_string())?;
    if !is_pascal_case(id_str) {
        return Err(format!("concept id '{id_str}' must be PascalCase"));
    }
    let id = ConceptId(id_str.to_string());

    let kind_str = raw
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "concept missing 'kind'".to_string())?;

    let kind = match kind_str {
        "entity" => ConceptKind::Entity,
        "structure" => {
            let props = raw
                .get("properties")
                .and_then(|p| p.as_array())
                .ok_or_else(|| "structure concept missing 'properties' array".to_string())?;
            if props.is_empty() || props.len() > 6 {
                return Err(format!(
                    "structure must have 1-6 properties, got {}",
                    props.len()
                ));
            }
            let properties: Vec<Property> = props
                .iter()
                .map(|p| {
                    let name = p
                        .get("name")
                        .and_then(|n| n.as_str())
                        .ok_or_else(|| "property missing 'name'".to_string())?
                        .to_string();
                    let ty_str = p
                        .get("type")
                        .and_then(|t| t.as_str())
                        .ok_or_else(|| format!("property '{name}' missing 'type'"))?;
                    let ty = parse_type(ty_str)?;
                    let required = p
                        .get("required")
                        .and_then(|r| r.as_bool())
                        .unwrap_or(true);
                    Ok(Property {
                        name,
                        ty,
                        required,
                        max: Cardinality::One,
                        projectable: true,
                    })
                })
                .collect::<Result<_, String>>()?;
            ConceptKind::Structure { properties }
        }
        other => {
            return Err(format!(
                "unsupported concept kind '{other}'; use 'entity' or 'structure'"
            ))
        }
    };

    let extends: Vec<ConceptId> = match raw.get("extends").and_then(|e| e.as_array()) {
        Some(arr) => arr
            .iter()
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| "extends entry must be a string".to_string())
                    .map(|s| ConceptId(s.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?,
        None => vec![],
    };

    let nouns: Vec<String> = match raw.get("nouns").and_then(|n| n.as_array()) {
        Some(arr) => arr
            .iter()
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| "noun must be a string".to_string())
                    .map(str::to_string)
            })
            .collect::<Result<Vec<_>, _>>()?,
        None => vec![],
    };

    let description = raw
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();

    Ok(Concept {
        id,
        kind,
        extends,
        role_of: None,
        nouns,
        description,
        tier: Tier::Provisional,
        provenance: Provenance::Teacher { lesson_id: String::new() },
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Coerce a raw JSON value into a `Value` of the declared type.
fn coerce_value(raw: &serde_json::Value, ty: &Type) -> Result<Value, String> {
    match ty {
        Type::Int => raw
            .as_i64()
            .map(Value::Int)
            .ok_or_else(|| format!("expected Int, got {raw}")),
        Type::Float => raw
            .as_f64()
            .map(Value::Float)
            .ok_or_else(|| format!("expected Float, got {raw}")),
        Type::Text => raw
            .as_str()
            .map(|s| Value::Text(s.to_string()))
            .ok_or_else(|| format!("expected Text, got {raw}")),
        Type::Bool => raw
            .as_bool()
            .map(Value::Bool)
            .ok_or_else(|| format!("expected Bool, got {raw}")),
        Type::Name => raw
            .as_str()
            .map(|s| Value::Name(s.to_string()))
            .ok_or_else(|| format!("expected Name, got {raw}")),
        Type::Path => raw
            .as_str()
            .map(|s| Value::Path(s.to_string()))
            .ok_or_else(|| format!("expected Path, got {raw}")),
        Type::Url => raw
            .as_str()
            .map(|s| Value::Url(s.to_string()))
            .ok_or_else(|| format!("expected Url, got {raw}")),
        Type::DateTime => raw
            .as_i64()
            .map(Value::DateTime)
            .ok_or_else(|| format!("expected DateTime (unix ms int), got {raw}")),
        Type::Duration => raw
            .as_f64()
            .map(Value::Duration)
            .ok_or_else(|| format!("expected Duration (seconds float), got {raw}")),
        Type::Json => Ok(Value::Json(raw.clone())),
        Type::Null => {
            if raw.is_null() {
                Ok(Value::Null)
            } else {
                Err(format!("expected null, got {raw}"))
            }
        }
        Type::List(inner) => {
            let arr = raw
                .as_array()
                .ok_or_else(|| format!("expected List, got {raw}"))?;
            arr.iter()
                .map(|v| coerce_value(v, inner))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::List)
        }
        Type::Concept(_) => raw
            .as_str()
            .map(|s| Value::Name(s.to_string()))
            .ok_or_else(|| format!("expected concept (string name), got {raw}")),
        Type::Any => Ok(Value::Json(raw.clone())),
        Type::Func(_, _) => Err("Func type not allowed in spec examples".to_string()),
    }
}

fn is_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn is_pascal_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false)
        && s.chars().all(|c| c.is_alphanumeric())
}
