//! A capability spec: what the synthesizer is asked to build. Written by the
//! teacher, by the user ("here are examples"), or by the brain from a failed
//! plan. Never contains executable bodies.

use serde::{Deserialize, Serialize};

use super::value::{Type, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Example {
    pub inputs: Vec<Value>,
    pub output: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spec {
    /// Stable id (uuid or teacher lesson id) used in `Provenance::Synthesized`.
    pub id: String,
    /// Suggested action name, snake_case, e.g. "double".
    pub name_hint: String,
    /// Verb lemmas for SCE ("double", "twice"). First is canonical.
    #[serde(default)]
    pub verbs: Vec<String>,
    /// Messy phrasings that should route here ("4 twice", "double it").
    #[serde(default)]
    pub phrasings: Vec<String>,
    pub params: Vec<Type>,
    #[serde(default)]
    pub param_names: Vec<String>,
    pub ret: Type,
    pub examples: Vec<Example>,
    #[serde(default)]
    pub description: String,
    /// "teacher" | "user" | "brain"
    #[serde(default)]
    pub source: String,
}

impl Spec {
    /// Every example must have `params.len()` inputs whose types are accepted
    /// by the declared params, and an output accepted by `ret`.
    pub fn validate(&self) -> Result<(), String> {
        if self.examples.is_empty() {
            return Err("spec has no examples".into());
        }
        for (i, ex) in self.examples.iter().enumerate() {
            if ex.inputs.len() != self.params.len() {
                return Err(format!(
                    "example {i}: {} inputs, spec declares {} params",
                    ex.inputs.len(),
                    self.params.len()
                ));
            }
            for (j, (v, t)) in ex.inputs.iter().zip(&self.params).enumerate() {
                if !t.accepts_value(v) {
                    return Err(format!("example {i} input {j}: {} is not a {t}", v.type_of()));
                }
            }
            if !self.ret.accepts_value(&ex.output) {
                return Err(format!("example {i} output: {} is not a {}", ex.output.type_of(), self.ret));
            }
        }
        Ok(())
    }
}
