//! Bounded, serializable language structures.
//!
//! This module is deliberately a substrate, not a language model. It preserves
//! source spans, exposes intent and dialogue values, and renders only claims
//! already supplied with evidence. It does not infer facts, invent sources, or
//! execute actions.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{EpisodeId, SourceKind, Value};

pub const DEFAULT_MAX_INPUT_BYTES: usize = 64 * 1024;
pub const DEFAULT_MAX_TOKENS: usize = 4_096;
pub const DEFAULT_MAX_CLAIMS: usize = 64;
pub const DEFAULT_MAX_CLAIM_BYTES: usize = 8 * 1024;
pub const DEFAULT_MAX_PLAN_BYTES: usize = 64 * 1024;
pub const DEFAULT_MAX_SLOTS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageLimits {
    pub max_input_bytes: usize,
    pub max_tokens: usize,
    pub max_claims: usize,
    pub max_claim_bytes: usize,
    pub max_plan_bytes: usize,
    pub max_slots: usize,
}

impl Default for LanguageLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
            max_tokens: DEFAULT_MAX_TOKENS,
            max_claims: DEFAULT_MAX_CLAIMS,
            max_claim_bytes: DEFAULT_MAX_CLAIM_BYTES,
            max_plan_bytes: DEFAULT_MAX_PLAN_BYTES,
            max_slots: DEFAULT_MAX_SLOTS,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LanguageError {
    #[error("language {kind} exceeded its limit of {limit}")]
    LimitExceeded { kind: &'static str, limit: usize },
    #[error("invalid language structure: {0}")]
    Invalid(String),
}

/// Byte offsets into the exact UTF-8 source text. They are never character or
/// grapheme indexes, so a span can be used directly for source provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextSpan {
    pub start_byte: usize,
    pub end_byte: usize,
}

impl TextSpan {
    pub const fn new(start_byte: usize, end_byte: usize) -> Self {
        Self {
            start_byte,
            end_byte,
        }
    }

    pub fn validate_for(&self, source: &str) -> Result<(), LanguageError> {
        if self.start_byte >= self.end_byte {
            return Err(LanguageError::Invalid(
                "a token span must have a non-empty range".into(),
            ));
        }
        if self.end_byte > source.len()
            || !source.is_char_boundary(self.start_byte)
            || !source.is_char_boundary(self.end_byte)
        {
            return Err(LanguageError::Invalid(
                "span is not on a UTF-8 character boundary within the source".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NormalizationForm {
    /// The substrate records text faithfully; normalization is an explicit
    /// transform, not an implicit rewrite that would invalidate source spans.
    Unchanged,
    Nfc,
    Nfd,
    Nfkc,
    Nfkd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDocument {
    pub text: String,
    pub normalization: NormalizationForm,
}

impl TextDocument {
    pub fn unchanged(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            normalization: NormalizationForm::Unchanged,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenKind {
    Word,
    Number,
    Whitespace,
    Punctuation,
    Symbol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    pub kind: TokenKind,
    pub span: TextSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenStream {
    pub document: TextDocument,
    pub tokens: Vec<Token>,
}

impl TokenStream {
    pub fn slice(&self, span: &TextSpan) -> Option<&str> {
        self.document.text.get(span.start_byte..span.end_byte)
    }

    pub fn validate(&self, limits: &LanguageLimits) -> Result<(), LanguageError> {
        let source = &self.document.text;
        if source.len() > limits.max_input_bytes {
            return Err(LanguageError::LimitExceeded {
                kind: "input bytes",
                limit: limits.max_input_bytes,
            });
        }
        if self.tokens.len() > limits.max_tokens {
            return Err(LanguageError::LimitExceeded {
                kind: "tokens",
                limit: limits.max_tokens,
            });
        }

        let mut previous_end = 0;
        for token in &self.tokens {
            token.span.validate_for(source)?;
            if token.span.start_byte != previous_end {
                return Err(LanguageError::Invalid(
                    "token spans must be ordered and cover source without gaps".into(),
                ));
            }
            previous_end = token.span.end_byte;
        }
        if previous_end != source.len() {
            return Err(LanguageError::Invalid(
                "token spans must cover the complete source".into(),
            ));
        }
        Ok(())
    }
}

/// Tokenizes source deterministically without normalizing or changing it. The
/// resulting spans are byte offsets into `document.text`.
pub fn tokenize(input: &str) -> Result<TokenStream, LanguageError> {
    tokenize_with_limits(input, &LanguageLimits::default())
}

pub fn tokenize_with_limits(
    input: &str,
    limits: &LanguageLimits,
) -> Result<TokenStream, LanguageError> {
    if input.len() > limits.max_input_bytes {
        return Err(LanguageError::LimitExceeded {
            kind: "input bytes",
            limit: limits.max_input_bytes,
        });
    }

    let mut tokens = Vec::new();
    let mut current_start: Option<usize> = None;
    let mut current_kind: Option<TokenKind> = None;

    for (offset, character) in input.char_indices() {
        let kind = classify_token_character(character);
        match (current_start, current_kind) {
            (Some(_start), Some(active)) if active == kind && is_mergeable(active) => {}
            (Some(start), Some(active)) => {
                tokens.push(Token {
                    kind: active,
                    span: TextSpan::new(start, offset),
                });
                current_start = Some(offset);
                current_kind = Some(kind);
            }
            _ => {
                current_start = Some(offset);
                current_kind = Some(kind);
            }
        }

        if tokens.len() >= limits.max_tokens {
            return Err(LanguageError::LimitExceeded {
                kind: "tokens",
                limit: limits.max_tokens,
            });
        }
    }

    if let (Some(start), Some(kind)) = (current_start, current_kind) {
        tokens.push(Token {
            kind,
            span: TextSpan::new(start, input.len()),
        });
    }
    if tokens.len() > limits.max_tokens {
        return Err(LanguageError::LimitExceeded {
            kind: "tokens",
            limit: limits.max_tokens,
        });
    }

    let stream = TokenStream {
        document: TextDocument::unchanged(input),
        tokens,
    };
    stream.validate(limits)?;
    Ok(stream)
}

fn classify_token_character(character: char) -> TokenKind {
    if character.is_whitespace() {
        TokenKind::Whitespace
    } else if character.is_numeric() {
        TokenKind::Number
    } else if character.is_alphabetic() || character == '_' || is_combining_mark(character) {
        TokenKind::Word
    } else if character.is_ascii_punctuation() {
        TokenKind::Punctuation
    } else {
        TokenKind::Symbol
    }
}

fn is_mergeable(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Word | TokenKind::Number | TokenKind::Whitespace
    )
}

fn is_combining_mark(character: char) -> bool {
    matches!(
        character as u32,
        0x0300..=0x036f | 0x1ab0..=0x1aff | 0x1dc0..=0x1dff | 0x20d0..=0x20ff | 0xfe20..=0xfe2f
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntentScope {
    CurrentTurn,
    Conversation,
    Workspace,
    External,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentSlot {
    pub name: String,
    pub value: Value,
    pub source_spans: Vec<TextSpan>,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentFrame {
    pub name: String,
    pub confidence: f32,
    pub scope: IntentScope,
    pub source_spans: Vec<TextSpan>,
    pub slots: Vec<IntentSlot>,
    /// Competing interpretations are retained rather than silently discarded.
    pub ambiguities: Vec<String>,
}

impl IntentFrame {
    pub fn validate(&self, limits: &LanguageLimits) -> Result<(), LanguageError> {
        validate_name("intent name", &self.name)?;
        validate_probability("intent confidence", self.confidence)?;
        if self.slots.len() > limits.max_slots {
            return Err(LanguageError::LimitExceeded {
                kind: "intent slots",
                limit: limits.max_slots,
            });
        }
        for slot in &self.slots {
            validate_name("slot name", &slot.name)?;
            validate_probability("slot confidence", slot.confidence)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialogueAct {
    Inform,
    Ask,
    Clarify,
    Confirm,
    Correct,
    Acknowledge,
    Refuse,
    Abstain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DialogueMove {
    pub act: DialogueAct,
    pub relates_to_turn: Option<String>,
}

impl DialogueMove {
    pub fn new(act: DialogueAct) -> Self {
        Self {
            act,
            relates_to_turn: None,
        }
    }
}

/// A durable pointer to evidence, not evidence text. Rendering a plan cannot
/// turn an ID into a new claim or disclose a source that the plan did not hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceReference {
    pub id: String,
    pub source_kind: SourceKind,
    pub linked_episode: Option<EpisodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroundedClaim {
    pub id: String,
    /// The exact already-grounded wording. The renderer preserves it verbatim.
    pub text: String,
    pub evidence: Vec<EvidenceReference>,
    /// Procedure/observation/knowledge references that produced the claim.
    pub provenance: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlannedClaim {
    Grounded(GroundedClaim),
    /// Retained in the plan for auditability, but never rendered as a fact.
    Unsupported {
        id: String,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UncertaintyLevel {
    Certain,
    Qualified,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Uncertainty {
    pub level: UncertaintyLevel,
    /// This is a supplied disclosure, never generated by the renderer.
    pub disclosure: Option<String>,
}

impl Uncertainty {
    pub fn certain() -> Self {
        Self {
            level: UncertaintyLevel::Certain,
            disclosure: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResponseTone {
    Neutral,
    Direct,
    Warm,
    Formal,
}

/// Formatting variation is intentionally content-free. It can make the same
/// response plan fit a surface style without authoring a new sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderVariant {
    Plain,
    Bulleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponsePlan {
    pub dialogue_move: DialogueMove,
    pub claims: Vec<PlannedClaim>,
    pub uncertainty: Uncertainty,
    pub tone: ResponseTone,
    pub variant: RenderVariant,
}

impl ResponsePlan {
    pub fn validate(&self, limits: &LanguageLimits) -> Result<(), LanguageError> {
        if self.claims.len() > limits.max_claims {
            return Err(LanguageError::LimitExceeded {
                kind: "response claims",
                limit: limits.max_claims,
            });
        }
        let mut total_bytes = 0usize;
        let mut ids = std::collections::BTreeSet::new();
        for planned in &self.claims {
            let (id, text) = match planned {
                PlannedClaim::Grounded(claim) => {
                    if claim.evidence.is_empty() {
                        return Err(LanguageError::Invalid(format!(
                            "grounded claim {:?} has no evidence reference",
                            claim.id
                        )));
                    }
                    for evidence in &claim.evidence {
                        validate_name("evidence reference", &evidence.id)?;
                    }
                    for provenance in &claim.provenance {
                        validate_name("claim provenance", provenance)?;
                    }
                    (&claim.id, &claim.text)
                }
                PlannedClaim::Unsupported { id, reason } => (id, reason),
            };
            validate_name("claim id", id)?;
            if !ids.insert(id) {
                return Err(LanguageError::Invalid(format!(
                    "response plan contains duplicate claim id {id:?}"
                )));
            }
            if text.is_empty() {
                return Err(LanguageError::Invalid(format!(
                    "claim {id:?} has empty text"
                )));
            }
            if text.len() > limits.max_claim_bytes {
                return Err(LanguageError::LimitExceeded {
                    kind: "claim bytes",
                    limit: limits.max_claim_bytes,
                });
            }
            total_bytes = total_bytes.saturating_add(text.len());
        }
        if let Some(disclosure) = &self.uncertainty.disclosure {
            total_bytes = total_bytes.saturating_add(disclosure.len());
        }
        if total_bytes > limits.max_plan_bytes {
            return Err(LanguageError::LimitExceeded {
                kind: "response plan bytes",
                limit: limits.max_plan_bytes,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenderedResponse {
    pub text: String,
    pub included_claim_ids: Vec<String>,
    pub omitted_claim_ids: Vec<String>,
    pub uncertainty: Uncertainty,
    pub tone: ResponseTone,
}

/// A deliberately narrow no-model renderer. It selects only evidence-backed
/// claims already present in the response plan, preserves their text verbatim,
/// and varies only their formatting. It never synthesizes authority, facts, or
/// an effect request.
#[derive(Debug, Default, Clone, Copy)]
pub struct ResponseRenderer;

impl ResponseRenderer {
    pub fn render(&self, plan: &ResponsePlan) -> Result<RenderedResponse, LanguageError> {
        plan.validate(&LanguageLimits::default())?;

        let mut included_claim_ids = Vec::new();
        let mut omitted_claim_ids = Vec::new();
        let mut texts = Vec::new();
        for planned in &plan.claims {
            match planned {
                PlannedClaim::Grounded(claim) => {
                    included_claim_ids.push(claim.id.clone());
                    texts.push(claim.text.as_str());
                }
                PlannedClaim::Unsupported { id, .. } => omitted_claim_ids.push(id.clone()),
            }
        }

        let text = match plan.variant {
            RenderVariant::Plain => texts.join("\n"),
            RenderVariant::Bulleted => texts
                .into_iter()
                .map(|claim| format!("- {claim}"))
                .collect::<Vec<_>>()
                .join("\n"),
        };

        Ok(RenderedResponse {
            text,
            included_claim_ids,
            omitted_claim_ids,
            uncertainty: plan.uncertainty.clone(),
            tone: plan.tone,
        })
    }
}

fn validate_name(kind: &str, value: &str) -> Result<(), LanguageError> {
    if value.is_empty() || value.len() > 256 {
        return Err(LanguageError::Invalid(format!(
            "{kind} must be between 1 and 256 bytes"
        )));
    }
    Ok(())
}

fn validate_probability(kind: &str, value: f32) -> Result<(), LanguageError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(LanguageError::Invalid(format!(
            "{kind} must be a finite value from 0 through 1"
        )));
    }
    Ok(())
}
