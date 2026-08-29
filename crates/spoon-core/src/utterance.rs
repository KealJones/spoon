//! Utterance-level analysis: segmentation into speech-act parts.
//!
//! One utterance is frequently several speech acts. `"hey whats 2+2 and then
//! double that"` is a greeting, a question, and a second question that consumes
//! the first answer. Treating it as one intent drops most of it.
//!
//! This module adds the grain above `IntentFrameSet`: an `UtteranceAnalysis`
//! wrapping per-part frame sets, with the dependency structure between parts
//! derived by the Engine rather than asserted by the model.
//!
//! The original `TokenStream` remains the only source of provenance. Cleaned
//! text is a derived, aligned document; it is never treated as source.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::Value;
use crate::language::{
    DialogueAct, IntentDisposition, IntentFrameSet, InterpretationProposal, LanguageError,
    LanguageLimits, TextSpan, TokenKind, TokenRange, TokenStream,
};

pub const DEFAULT_MAX_PARTS: usize = 8;
pub const DEFAULT_MAX_MENTIONS_PER_PART: usize = 16;
pub const DEFAULT_MAX_RESIDUALS_PER_PART: usize = 8;
pub const DEFAULT_MAX_RESIDUALS_PER_UTTERANCE: usize = 32;
pub const DEFAULT_MAX_RESIDUAL_SCOPE: usize = 8;
pub const DEFAULT_MAX_LANGUAGE_WRITES: usize = 16;
pub const DEFAULT_MAX_TEMPLATE_BYTES: usize = 1_024;

/// Bounds for one utterance analysis. The language limits govern the underlying
/// token stream and frame sets; the rest bound the new per-part structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UtteranceLimits {
    pub language: LanguageLimits,
    pub max_parts: usize,
    pub max_mentions_per_part: usize,
    pub max_residuals_per_part: usize,
    pub max_residuals_per_utterance: usize,
    pub max_residual_scope: usize,
    pub max_language_writes: usize,
    pub max_template_bytes: usize,
}

impl Default for UtteranceLimits {
    fn default() -> Self {
        Self {
            language: LanguageLimits::default(),
            max_parts: DEFAULT_MAX_PARTS,
            max_mentions_per_part: DEFAULT_MAX_MENTIONS_PER_PART,
            max_residuals_per_part: DEFAULT_MAX_RESIDUALS_PER_PART,
            max_residuals_per_utterance: DEFAULT_MAX_RESIDUALS_PER_UTTERANCE,
            max_residual_scope: DEFAULT_MAX_RESIDUAL_SCOPE,
            max_language_writes: DEFAULT_MAX_LANGUAGE_WRITES,
            max_template_bytes: DEFAULT_MAX_TEMPLATE_BYTES,
        }
    }
}

/// A part identifier of the form `p0`. Parts are addressed by this rather than
/// by index so a stored analysis stays readable and a dangling reference is a
/// detectable error rather than an off-by-one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PartId(String);

impl PartId {
    pub fn new(index: usize) -> Self {
        Self(format!("p{index}"))
    }

    pub fn parse(raw: &str) -> Result<Self, LanguageError> {
        let Some(digits) = raw.strip_prefix('p') else {
            return Err(LanguageError::Invalid(format!(
                "part id {raw:?} must look like p0"
            )));
        };
        if digits.is_empty()
            || digits.len() > 3
            || !digits.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(LanguageError::Invalid(format!(
                "part id {raw:?} must look like p0"
            )));
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PartId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MentionKind {
    Entity,
    Value,
    Expression,
    Result,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartRefRole {
    Mention,
    Result,
}

/// How a mention was resolved. `PartRef` is the only variant that survives into
/// rendered text as a placeholder, because it names something not computed yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MentionResolution {
    Literal {
        value: Value,
    },
    PartRef {
        part: PartId,
        role: PartRefRole,
    },
    /// An alias that was already present in the supplied context packet. Never
    /// a durable database identifier.
    ContextRef {
        alias: String,
    },
    Unresolved {
        ambiguity: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mention {
    pub key: String,
    pub kind: MentionKind,
    /// Byte spans in the original stream. Empty only when `inferred` is true.
    pub surface: Vec<TextSpan>,
    pub inferred: bool,
    pub resolved: MentionResolution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResidualPolarity {
    Assert,
    Deny,
}

/// Where a proposed fact came from. There is no retrieval in this design, so
/// the only sources that exist are the utterance itself and the context packet.
/// A fact with neither is model-weight recall presented as knowledge, which is
/// exactly what must not be admitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResidualProvenance {
    /// The user said it. The span covers complete tokens in the original stream.
    Utterance { span: TextSpan },
    /// A packet alias that itself traces to a previously observed fact.
    Context { alias: String },
}

/// A fact the utterance asserted that is not executable this turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResidualClaim {
    pub id: String,
    pub predicate: String,
    pub value: Value,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scope: BTreeMap<String, Value>,
    pub polarity: ResidualPolarity,
    pub provenance: ResidualProvenance,
}

/// The closed set of language relationships the front model may propose. Core
/// stores `Relationship.kind` as a free string, so this set is what the Engine
/// admits, and anything outside it is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LanguageWriteKind {
    AliasOf,
    Termed,
    IntentOf,
}

impl LanguageWriteKind {
    pub fn as_relationship_kind(self) -> &'static str {
        match self {
            Self::AliasOf => "alias-of",
            Self::Termed => "termed",
            Self::IntentOf => "intent-of",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LanguageWrite {
    pub kind: LanguageWriteKind,
    /// The new surface form, phrase, or semantic key being proposed.
    pub surface: String,
    /// Request-local alias of the existing concept this attaches to.
    pub target_alias: String,
    /// Where in the original utterance the surface form appeared.
    pub source_spans: Vec<TextSpan>,
}

/// One contiguous correspondence between derived text and original source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Alignment {
    pub cleaned: TextSpan,
    pub original: TextSpan,
}

/// Cleaned text plus its correspondence back to the original.
///
/// Cleaned text may introduce wording that has no original counterpart, because
/// materializing `"it"` into `"the file from p0"` adds words the user never
/// said. Those regions are deliberately left unaligned: `original_span_for`
/// returns `None` for them, so no caller can mistake introduced text for
/// something the user actually uttered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlignedDocument {
    pub text: String,
    pub alignment: Vec<Alignment>,
}

impl AlignedDocument {
    pub fn validate_for(&self, original: &TokenStream) -> Result<(), LanguageError> {
        let mut previous_end = 0usize;
        for entry in &self.alignment {
            entry.cleaned.validate_for(&self.text)?;
            entry.original.validate_for(&original.document.text)?;
            validate_token_span(&entry.original, original)?;
            if entry.cleaned.start_byte < previous_end {
                return Err(LanguageError::Invalid(
                    "alignment entries must be ordered and non-overlapping in cleaned text".into(),
                ));
            }
            previous_end = entry.cleaned.end_byte;
        }
        Ok(())
    }

    /// The original span a cleaned range derives from, or `None` when the range
    /// is introduced text with no counterpart in what the user said.
    pub fn original_span_for(&self, cleaned: TextSpan) -> Option<TextSpan> {
        self.alignment
            .iter()
            .find(|entry| {
                entry.cleaned.start_byte <= cleaned.start_byte
                    && entry.cleaned.end_byte >= cleaned.end_byte
            })
            .map(|entry| entry.original)
    }
}

/// One speech act within an utterance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Part {
    pub id: PartId,
    /// Byte spans in the original stream covering this part's source tokens.
    pub spans: Vec<TextSpan>,
    /// Surface template with `{key}` placeholders keyed to mentions.
    pub template: String,
    pub mentions: Vec<Mention>,
    /// Mentions resolved from the context packet rather than from the surface.
    pub context_bindings: Vec<Mention>,
    pub intent: IntentFrameSet,
    pub act: DialogueAct,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub residual: Vec<ResidualClaim>,
}

impl Part {
    /// Parts this one consumes a not-yet-computed value from.
    pub fn depends_on(&self) -> BTreeSet<PartId> {
        self.mentions
            .iter()
            .chain(self.context_bindings.iter())
            .filter_map(|mention| match &mention.resolved {
                MentionResolution::PartRef { part, .. } => Some(part.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn is_executable(&self) -> bool {
        matches!(self.intent.disposition, IntentDisposition::Execute)
    }
}

/// A grounded, validated analysis of one utterance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UtteranceAnalysis {
    pub original: TokenStream,
    pub cleaned: AlignedDocument,
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub language_writes: Vec<LanguageWrite>,
}

impl UtteranceAnalysis {
    pub fn part(&self, id: &PartId) -> Option<&Part> {
        self.parts.iter().find(|part| &part.id == id)
    }

    /// The full dependency map, derived from `part_ref` mentions. The model
    /// never states dependencies directly; stating them would let it assert an
    /// execution order it has no authority over.
    pub fn depends_on(&self) -> BTreeMap<PartId, BTreeSet<PartId>> {
        self.parts
            .iter()
            .map(|part| (part.id.clone(), part.depends_on()))
            .collect()
    }

    /// Dispatch order: dependencies first, ties broken by source order so the
    /// same analysis always dispatches identically.
    pub fn dispatch_order(&self) -> Result<Vec<PartId>, LanguageError> {
        let dependencies = self.depends_on();
        let mut remaining: Vec<PartId> = self.parts.iter().map(|part| part.id.clone()).collect();
        let mut ordered: Vec<PartId> = Vec::with_capacity(remaining.len());
        let mut placed: BTreeSet<PartId> = BTreeSet::new();

        while !remaining.is_empty() {
            let ready = remaining.iter().position(|id| {
                dependencies
                    .get(id)
                    .is_none_or(|needs| needs.iter().all(|need| placed.contains(need)))
            });
            let Some(index) = ready else {
                return Err(LanguageError::Invalid(
                    "part dependencies contain a cycle".into(),
                ));
            };
            let id = remaining.remove(index);
            placed.insert(id.clone());
            ordered.push(id);
        }
        Ok(ordered)
    }

    /// Source order, which is always how the reply concatenates regardless of
    /// the order parts executed in.
    pub fn source_order(&self) -> Vec<PartId> {
        self.parts.iter().map(|part| part.id.clone()).collect()
    }
}

// ---------------------------------------------------------------------------
// Untrusted provider boundary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlignmentProposal {
    pub cleaned_start: usize,
    pub cleaned_end: usize,
    pub source_tokens: TokenRange,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MentionResolutionProposal {
    Literal { value: Value },
    PartRef { part: String, role: PartRefRole },
    ContextRef { alias: String },
    Unresolved { ambiguity: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MentionProposal {
    pub key: String,
    pub kind: MentionKind,
    #[serde(default)]
    pub source_tokens: Vec<TokenRange>,
    #[serde(default)]
    pub inferred: bool,
    pub resolved: MentionResolutionProposal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResidualProvenanceProposal {
    /// Token range in the original stream: the user said it.
    UtteranceTokens(TokenRange),
    /// Alias already present in the supplied packet.
    ContextAlias(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResidualProposal {
    pub id: String,
    pub predicate: String,
    pub value: Value,
    #[serde(default)]
    pub scope: BTreeMap<String, Value>,
    pub polarity: ResidualPolarity,
    pub provenance: ResidualProvenanceProposal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LanguageWriteProposal {
    pub kind: LanguageWriteKind,
    pub surface: String,
    pub target_alias: String,
    #[serde(default)]
    pub source_tokens: Vec<TokenRange>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartProposal {
    pub id: String,
    pub source_tokens: Vec<TokenRange>,
    pub template: String,
    pub act: DialogueAct,
    #[serde(default)]
    pub mentions: Vec<MentionProposal>,
    #[serde(default)]
    pub context_bindings: Vec<MentionProposal>,
    pub intent: InterpretationProposal,
    #[serde(default)]
    pub residual: Vec<ResidualProposal>,
}

/// Untrusted provider output. `ground_for` is the trust boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UtteranceAnalysisProposal {
    pub cleaned: String,
    #[serde(default)]
    pub alignment: Vec<AlignmentProposal>,
    pub parts: Vec<PartProposal>,
    #[serde(default)]
    pub language_writes: Vec<LanguageWriteProposal>,
}

impl UtteranceAnalysisProposal {
    /// Converts an untrusted proposal into a validated analysis.
    ///
    /// `packet_aliases` is the exact set of aliases the Engine put in the
    /// context packet. A reference to anything else means the model invented a
    /// handle, which is rejected rather than resolved.
    pub fn ground_for(
        &self,
        stream: &TokenStream,
        packet_aliases: &BTreeSet<String>,
        limits: &UtteranceLimits,
    ) -> Result<UtteranceAnalysis, LanguageError> {
        stream.validate(&limits.language)?;

        if self.parts.is_empty() {
            return Err(LanguageError::Invalid(
                "an utterance analysis must contain at least one part".into(),
            ));
        }
        if self.parts.len() > limits.max_parts {
            return Err(LanguageError::LimitExceeded {
                kind: "utterance parts",
                limit: limits.max_parts,
            });
        }
        if self.language_writes.len() > limits.max_language_writes {
            return Err(LanguageError::LimitExceeded {
                kind: "language writes",
                limit: limits.max_language_writes,
            });
        }
        if self.cleaned.len() > limits.language.max_input_bytes {
            return Err(LanguageError::LimitExceeded {
                kind: "cleaned bytes",
                limit: limits.language.max_input_bytes,
            });
        }

        reject_durable_ids(self)?;

        let ids = self.validate_part_ids()?;
        self.validate_coverage(stream, limits)?;

        let mut parts = Vec::with_capacity(self.parts.len());
        let mut residual_total = 0usize;
        for proposal in &self.parts {
            let part = proposal.ground_for(stream, packet_aliases, &ids, limits)?;
            residual_total = residual_total.saturating_add(part.residual.len());
            parts.push(part);
        }
        if residual_total > limits.max_residuals_per_utterance {
            return Err(LanguageError::LimitExceeded {
                kind: "utterance residual claims",
                limit: limits.max_residuals_per_utterance,
            });
        }

        let cleaned = AlignedDocument {
            text: self.cleaned.clone(),
            alignment: self
                .alignment
                .iter()
                .map(|entry| {
                    Ok(Alignment {
                        cleaned: TextSpan::new(entry.cleaned_start, entry.cleaned_end),
                        original: entry.source_tokens.ground(stream)?,
                    })
                })
                .collect::<Result<Vec<_>, LanguageError>>()?,
        };
        cleaned.validate_for(stream)?;

        let mut language_writes = Vec::with_capacity(self.language_writes.len());
        for write in &self.language_writes {
            validate_bounded_name("language write surface", &write.surface)?;
            validate_bounded_name("language write target alias", &write.target_alias)?;
            if !packet_aliases.contains(&write.target_alias) {
                return Err(LanguageError::Invalid(format!(
                    "language write targets alias {:?}, which was not supplied in the context packet",
                    write.target_alias
                )));
            }
            language_writes.push(LanguageWrite {
                kind: write.kind,
                surface: write.surface.clone(),
                target_alias: write.target_alias.clone(),
                source_spans: write
                    .source_tokens
                    .iter()
                    .map(|range| range.ground(stream))
                    .collect::<Result<Vec<_>, LanguageError>>()?,
            });
        }

        let analysis = UtteranceAnalysis {
            original: stream.clone(),
            cleaned,
            parts,
            language_writes,
        };
        // Rejecting a cycle here rather than at dispatch keeps an unrunnable
        // analysis from ever being stored as if it were valid.
        analysis.dispatch_order()?;
        Ok(analysis)
    }

    fn validate_part_ids(&self) -> Result<BTreeSet<PartId>, LanguageError> {
        let mut ids = BTreeSet::new();
        for part in &self.parts {
            let id = PartId::parse(&part.id)?;
            if !ids.insert(id.clone()) {
                return Err(LanguageError::Invalid(format!("duplicate part id {id}")));
            }
        }
        Ok(ids)
    }

    /// Part spans must not overlap, and together they must cover every
    /// non-whitespace token. Silently dropping half an utterance is the exact
    /// failure this analysis exists to prevent, so partial coverage is an
    /// error rather than a truncation.
    fn validate_coverage(
        &self,
        stream: &TokenStream,
        limits: &UtteranceLimits,
    ) -> Result<(), LanguageError> {
        let mut owner: Vec<Option<usize>> = vec![None; stream.tokens.len()];
        for (index, part) in self.parts.iter().enumerate() {
            if part.source_tokens.is_empty() {
                return Err(LanguageError::Invalid(format!(
                    "part {:?} has no source tokens",
                    part.id
                )));
            }
            if part.template.len() > limits.max_template_bytes {
                return Err(LanguageError::LimitExceeded {
                    kind: "part template bytes",
                    limit: limits.max_template_bytes,
                });
            }
            for range in &part.source_tokens {
                if range.start_token >= range.end_token || range.end_token > stream.tokens.len() {
                    return Err(LanguageError::Invalid(
                        "part token range must be non-empty and inside the current token stream"
                            .into(),
                    ));
                }
                for slot in owner
                    .iter_mut()
                    .take(range.end_token)
                    .skip(range.start_token)
                {
                    if slot.is_some() {
                        return Err(LanguageError::Invalid(
                            "part source spans must not overlap".into(),
                        ));
                    }
                    *slot = Some(index);
                }
            }
        }

        for (index, token) in stream.tokens.iter().enumerate() {
            if token.kind != TokenKind::Whitespace && owner[index].is_none() {
                return Err(LanguageError::Invalid(
                    "part source spans must cover every non-whitespace token in the utterance"
                        .into(),
                ));
            }
        }
        Ok(())
    }
}

impl PartProposal {
    fn ground_for(
        &self,
        stream: &TokenStream,
        packet_aliases: &BTreeSet<String>,
        part_ids: &BTreeSet<PartId>,
        limits: &UtteranceLimits,
    ) -> Result<Part, LanguageError> {
        let id = PartId::parse(&self.id)?;
        if self.mentions.len() > limits.max_mentions_per_part
            || self.context_bindings.len() > limits.max_mentions_per_part
        {
            return Err(LanguageError::LimitExceeded {
                kind: "part mentions",
                limit: limits.max_mentions_per_part,
            });
        }
        if self.residual.len() > limits.max_residuals_per_part {
            return Err(LanguageError::LimitExceeded {
                kind: "part residual claims",
                limit: limits.max_residuals_per_part,
            });
        }

        let spans = self
            .source_tokens
            .iter()
            .map(|range| range.ground(stream))
            .collect::<Result<Vec<_>, LanguageError>>()?;

        let mut intent = self.intent.ground_for(stream, &limits.language)?;

        let mut mentions = Vec::with_capacity(self.mentions.len());
        for mention in &self.mentions {
            mentions.push(mention.ground_for(stream, packet_aliases, part_ids, &id)?);
        }
        let mut context_bindings = Vec::with_capacity(self.context_bindings.len());
        for mention in &self.context_bindings {
            let grounded = mention.ground_for(stream, packet_aliases, part_ids, &id)?;
            if !grounded.inferred {
                return Err(LanguageError::Invalid(format!(
                    "context binding {:?} must be marked inferred",
                    grounded.key
                )));
            }
            context_bindings.push(grounded);
        }

        // An Execute part that still holds an unresolved mention cannot run, so
        // it is coerced to Clarify rather than dispatched on a guess.
        let unresolved = mentions
            .iter()
            .chain(context_bindings.iter())
            .any(|mention| matches!(mention.resolved, MentionResolution::Unresolved { .. }));
        if unresolved && intent.disposition == IntentDisposition::Execute {
            intent.disposition = IntentDisposition::Clarify;
            intent.selected = None;
        }

        let mut residual = Vec::with_capacity(self.residual.len());
        for claim in &self.residual {
            residual.push(claim.ground_for(stream, packet_aliases, limits)?);
        }

        Ok(Part {
            id,
            spans,
            template: self.template.clone(),
            mentions,
            context_bindings,
            intent,
            act: self.act,
            residual,
        })
    }
}

impl MentionProposal {
    fn ground_for(
        &self,
        stream: &TokenStream,
        packet_aliases: &BTreeSet<String>,
        part_ids: &BTreeSet<PartId>,
        owner: &PartId,
    ) -> Result<Mention, LanguageError> {
        validate_mention_key(&self.key)?;
        let surface = self
            .source_tokens
            .iter()
            .map(|range| range.ground(stream))
            .collect::<Result<Vec<_>, LanguageError>>()?;

        if !self.inferred && surface.is_empty() {
            return Err(LanguageError::Invalid(format!(
                "mention {:?} is not inferred, so it must carry a source span",
                self.key
            )));
        }

        let resolved = match &self.resolved {
            MentionResolutionProposal::Literal { value } => MentionResolution::Literal {
                value: value.clone(),
            },
            MentionResolutionProposal::PartRef { part, role } => {
                let target = PartId::parse(part)?;
                if &target == owner {
                    return Err(LanguageError::Invalid(format!(
                        "part {owner} refers to itself"
                    )));
                }
                if !part_ids.contains(&target) {
                    return Err(LanguageError::Invalid(format!(
                        "mention {:?} references unknown part {target}",
                        self.key
                    )));
                }
                MentionResolution::PartRef {
                    part: target,
                    role: *role,
                }
            }
            MentionResolutionProposal::ContextRef { alias } => {
                validate_bounded_name("context alias", alias)?;
                if !packet_aliases.contains(alias) {
                    return Err(LanguageError::Invalid(format!(
                        "mention {:?} references alias {alias:?}, which was not supplied in the context packet",
                        self.key
                    )));
                }
                MentionResolution::ContextRef {
                    alias: alias.clone(),
                }
            }
            MentionResolutionProposal::Unresolved { ambiguity } => {
                validate_bounded_name("mention ambiguity", ambiguity)?;
                MentionResolution::Unresolved {
                    ambiguity: ambiguity.clone(),
                }
            }
        };

        Ok(Mention {
            key: self.key.clone(),
            kind: self.kind,
            surface,
            inferred: self.inferred,
            resolved,
        })
    }
}

impl ResidualProposal {
    fn ground_for(
        &self,
        stream: &TokenStream,
        packet_aliases: &BTreeSet<String>,
        limits: &UtteranceLimits,
    ) -> Result<ResidualClaim, LanguageError> {
        validate_bounded_name("residual id", &self.id)?;
        validate_bounded_name("residual predicate", &self.predicate)?;
        if self.scope.len() > limits.max_residual_scope {
            return Err(LanguageError::LimitExceeded {
                kind: "residual scope entries",
                limit: limits.max_residual_scope,
            });
        }
        for key in self.scope.keys() {
            validate_bounded_name("residual scope key", key)?;
        }

        let provenance = match &self.provenance {
            ResidualProvenanceProposal::UtteranceTokens(range) => ResidualProvenance::Utterance {
                span: range.ground(stream)?,
            },
            ResidualProvenanceProposal::ContextAlias(alias) => {
                validate_bounded_name("residual context alias", alias)?;
                if !packet_aliases.contains(alias) {
                    return Err(LanguageError::Invalid(format!(
                        "residual {:?} cites alias {alias:?}, which was not supplied in the context packet",
                        self.id
                    )));
                }
                ResidualProvenance::Context {
                    alias: alias.clone(),
                }
            }
        };

        Ok(ResidualClaim {
            id: self.id.clone(),
            predicate: self.predicate.clone(),
            value: self.value.clone(),
            scope: self.scope.clone(),
            polarity: self.polarity,
            provenance,
        })
    }
}

// ---------------------------------------------------------------------------
// Shared validation
// ---------------------------------------------------------------------------

fn validate_bounded_name(kind: &str, value: &str) -> Result<(), LanguageError> {
    if value.is_empty() || value.len() > 256 {
        return Err(LanguageError::Invalid(format!(
            "{kind} must be between 1 and 256 bytes"
        )));
    }
    Ok(())
}

fn validate_mention_key(key: &str) -> Result<(), LanguageError> {
    let mut bytes = key.bytes();
    let prefix = bytes.next();
    let valid_prefix = matches!(prefix, Some(b'e') | Some(b'v') | Some(b'x'));
    let digits: Vec<u8> = bytes.collect();
    if !valid_prefix
        || digits.is_empty()
        || digits.len() > 3
        || !digits.iter().all(u8::is_ascii_digit)
    {
        return Err(LanguageError::Invalid(format!(
            "mention key {key:?} must look like e0, v0, or x0"
        )));
    }
    Ok(())
}

fn validate_token_span(span: &TextSpan, stream: &TokenStream) -> Result<(), LanguageError> {
    let starts = stream
        .tokens
        .iter()
        .any(|token| token.span.start_byte == span.start_byte);
    let ends = stream
        .tokens
        .iter()
        .any(|token| token.span.end_byte == span.end_byte);
    if !starts || !ends {
        return Err(LanguageError::Invalid(
            "span must cover complete tokens in the original stream".into(),
        ));
    }
    Ok(())
}

/// A durable identifier anywhere in a proposal means the model was handed, or
/// invented, a database handle. Either way the Engine mints IDs, so the whole
/// analysis is refused rather than partially sanitized.
fn reject_durable_ids(proposal: &UtteranceAnalysisProposal) -> Result<(), LanguageError> {
    let encoded = serde_json::to_string(proposal).map_err(|error| {
        LanguageError::Invalid(format!("proposal could not be inspected: {error}"))
    })?;
    if contains_uuid(&encoded) {
        return Err(LanguageError::Invalid(
            "proposal contains a durable identifier; the Engine mints identifiers".into(),
        ));
    }
    Ok(())
}

fn contains_uuid(text: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let bytes = text.as_bytes();
    let total: usize = GROUPS.iter().sum::<usize>() + GROUPS.len() - 1;
    if bytes.len() < total {
        return false;
    }
    (0..=bytes.len() - total).any(|start| {
        let mut cursor = start;
        for (index, width) in GROUPS.iter().enumerate() {
            if index > 0 {
                if bytes[cursor] != b'-' {
                    return false;
                }
                cursor += 1;
            }
            if !bytes[cursor..cursor + width]
                .iter()
                .all(u8::is_ascii_hexdigit)
            {
                return false;
            }
            cursor += width;
        }
        // Reject only a complete identifier, so ordinary hex-looking prose does
        // not trip the check.
        bytes
            .get(cursor)
            .is_none_or(|byte| !byte.is_ascii_hexdigit())
    })
}
