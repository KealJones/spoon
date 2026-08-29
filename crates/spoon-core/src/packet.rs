//! The bounded, read-only context projection handed to a front language model.
//!
//! The interpreter should not work from the current sentence alone, but it also
//! must not become an unbounded database agent. This module is the compromise:
//! the Engine decides what is eligible, redacts it, bounds it, and projects it
//! into a packet addressed entirely by request-local aliases.
//!
//! Two properties matter more than convenience here. Nothing in a packet is a
//! durable identifier, so a model cannot learn or leak one. And nothing is
//! dropped silently: every bound that trims a group records a truncation flag,
//! because a quietly shortened context reads downstream as a complete one.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::Value;
use crate::language::{LanguageError, TokenRange, TokenStream};

pub const DEFAULT_MAX_PACKET_TURNS: usize = 8;
pub const DEFAULT_MAX_PACKET_CATALOG: usize = 32;
pub const DEFAULT_MAX_PATTERNS_PER_ENTRY: usize = 4;
pub const DEFAULT_MAX_PACKET_TERMINOLOGY: usize = 64;
pub const DEFAULT_MAX_PACKET_ENVIRONMENT: usize = 32;
pub const DEFAULT_MAX_SUMMARY_BYTES: usize = 512;
pub const DEFAULT_MAX_PACKET_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PacketLimits {
    pub max_turns: usize,
    pub max_catalog_entries: usize,
    pub max_patterns_per_entry: usize,
    pub max_terminology: usize,
    pub max_environment: usize,
    pub max_summary_bytes: usize,
    pub max_packet_bytes: usize,
}

impl Default for PacketLimits {
    fn default() -> Self {
        Self {
            max_turns: DEFAULT_MAX_PACKET_TURNS,
            max_catalog_entries: DEFAULT_MAX_PACKET_CATALOG,
            max_patterns_per_entry: DEFAULT_MAX_PATTERNS_PER_ENTRY,
            max_terminology: DEFAULT_MAX_PACKET_TERMINOLOGY,
            max_environment: DEFAULT_MAX_PACKET_ENVIRONMENT,
            max_summary_bytes: DEFAULT_MAX_SUMMARY_BYTES,
            max_packet_bytes: DEFAULT_MAX_PACKET_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnRole {
    User,
    Spoon,
}

/// A prior turn, as a summary rather than a raw trace. Raw traces carry paths,
/// receipts, and permission state that have no business in a model prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PacketTurn {
    pub alias: String,
    pub role: TurnRole,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<PacketFact>,
}

/// A previously observed fact, exposed by alias so a residual claim can cite it
/// without ever naming the underlying record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PacketFact {
    pub alias: String,
    pub predicate: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PacketSlot {
    pub name: String,
    pub required: bool,
    pub value_kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PacketCatalogEntry {
    pub alias: String,
    /// A stable semantic key such as `arithmetic.multiply`, never a UUID.
    pub key: String,
    pub slots: Vec<PacketSlot>,
    pub patterns: Vec<String>,
    /// Whether this key currently resolves to an executable procedure.
    pub bound: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PacketAlias {
    pub alias: String,
    pub surface: String,
    /// The catalog or concept alias this surface form refers to.
    pub refers_to: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PacketEnvFact {
    pub alias: String,
    pub predicate: String,
    pub value: Value,
}

/// What a bound removed. Present so a downstream reader can tell a complete
/// context from a trimmed one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TruncationFlag {
    pub group: String,
    pub dropped: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LanguageContextPacket {
    pub utterance: TokenStream,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<PacketTurn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub catalog: Vec<PacketCatalogEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terminology: Vec<PacketAlias>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<PacketEnvFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub truncation: Vec<TruncationFlag>,
}

impl LanguageContextPacket {
    pub fn new(utterance: TokenStream) -> Self {
        Self {
            utterance,
            turns: Vec::new(),
            catalog: Vec::new(),
            terminology: Vec::new(),
            environment: Vec::new(),
            truncation: Vec::new(),
        }
    }

    /// Every alias the packet exposes. Grounding rejects any model reference
    /// outside this set, so a model cannot resolve a handle it was never given.
    pub fn aliases(&self) -> BTreeSet<String> {
        let mut aliases = BTreeSet::new();
        for turn in &self.turns {
            aliases.insert(turn.alias.clone());
            for fact in &turn.facts {
                aliases.insert(fact.alias.clone());
            }
        }
        for entry in &self.catalog {
            aliases.insert(entry.alias.clone());
        }
        for alias in &self.terminology {
            aliases.insert(alias.alias.clone());
        }
        for fact in &self.environment {
            aliases.insert(fact.alias.clone());
        }
        aliases
    }

    /// Aliases that trace back to a previously observed fact. A residual claim
    /// citing context provenance must name one of these; citing a catalog entry
    /// or a bare turn would be citing a capability, not an observation.
    pub fn fact_aliases(&self) -> BTreeSet<String> {
        self.turns
            .iter()
            .flat_map(|turn| turn.facts.iter())
            .map(|fact| fact.alias.clone())
            .chain(self.environment.iter().map(|fact| fact.alias.clone()))
            .collect()
    }

    /// Trims every group to its bound, recording what was removed, then checks
    /// the serialized size. Groups are trimmed before the size check so the
    /// caller learns which group overflowed rather than just that the packet
    /// was too large.
    pub fn enforce(&mut self, limits: &PacketLimits) -> Result<(), LanguageError> {
        truncate(
            &mut self.turns,
            limits.max_turns,
            "turns",
            &mut self.truncation,
        );
        truncate(
            &mut self.catalog,
            limits.max_catalog_entries,
            "catalog",
            &mut self.truncation,
        );
        truncate(
            &mut self.terminology,
            limits.max_terminology,
            "terminology",
            &mut self.truncation,
        );
        truncate(
            &mut self.environment,
            limits.max_environment,
            "environment",
            &mut self.truncation,
        );

        let mut dropped_patterns = 0usize;
        for entry in &mut self.catalog {
            if entry.patterns.len() > limits.max_patterns_per_entry {
                dropped_patterns += entry.patterns.len() - limits.max_patterns_per_entry;
                entry.patterns.truncate(limits.max_patterns_per_entry);
            }
        }
        if dropped_patterns > 0 {
            self.truncation.push(TruncationFlag {
                group: "catalogPatterns".to_string(),
                dropped: dropped_patterns,
            });
        }

        let mut clipped_summaries = 0usize;
        for turn in &mut self.turns {
            if turn.summary.len() > limits.max_summary_bytes {
                // Clip on a character boundary so the summary stays valid UTF-8.
                let mut end = limits.max_summary_bytes;
                while end > 0 && !turn.summary.is_char_boundary(end) {
                    end -= 1;
                }
                turn.summary.truncate(end);
                clipped_summaries += 1;
            }
        }
        if clipped_summaries > 0 {
            self.truncation.push(TruncationFlag {
                group: "turnSummaries".to_string(),
                dropped: clipped_summaries,
            });
        }

        let encoded = serde_json::to_string(self).map_err(|error| {
            LanguageError::Invalid(format!("context packet could not be serialized: {error}"))
        })?;
        if encoded.len() > limits.max_packet_bytes {
            return Err(LanguageError::LimitExceeded {
                kind: "context packet bytes",
                limit: limits.max_packet_bytes,
            });
        }
        Ok(())
    }

    /// Rejects a packet that leaked a durable identifier or a secret-looking
    /// value. This runs on the Engine's own output, so a failure is an Engine
    /// bug rather than a hostile model, and it is worth catching before the
    /// packet reaches a provider.
    pub fn validate_redaction(&self) -> Result<(), LanguageError> {
        let encoded = serde_json::to_string(self).map_err(|error| {
            LanguageError::Invalid(format!("context packet could not be serialized: {error}"))
        })?;
        if crate::utterance::contains_durable_id(&encoded) {
            return Err(LanguageError::Invalid(
                "context packet contains a durable identifier".into(),
            ));
        }
        Ok(())
    }
}

fn truncate<T>(items: &mut Vec<T>, limit: usize, group: &str, flags: &mut Vec<TruncationFlag>) {
    if items.len() > limit {
        let dropped = items.len() - limit;
        items.truncate(limit);
        flags.push(TruncationFlag {
            group: group.to_string(),
            dropped,
        });
    }
}

// ---------------------------------------------------------------------------
// Supplemental context
// ---------------------------------------------------------------------------

/// The only follow-up an interpreter may make. Each variant names something the
/// packet already surfaced, so the model can ask for detail but cannot widen
/// its own reach. There is deliberately no free-form field, no path, no query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum SupplementalRequest {
    /// Full slot schema and every pattern for one catalog entry already listed.
    CatalogDetail { alias: String },
    /// Up to four older same-session turn summaries.
    TurnWindow { count: usize },
    /// Terminology for one grounded surface phrase in the current utterance.
    /// The variant-level rename is required: `rename_all` on an enum renames
    /// variants, not the fields inside them, so without it this one field
    /// would arrive snake_case while every sibling type is camelCase.
    #[serde(rename_all = "camelCase")]
    Terminology { source_tokens: TokenRange },
}

pub const MAX_TURN_WINDOW: usize = 4;

impl SupplementalRequest {
    /// Validates a request against the packet that produced it. A request
    /// naming an alias the packet never contained means the model invented a
    /// handle, which is refused rather than resolved.
    pub fn validate_for(&self, packet: &LanguageContextPacket) -> Result<(), LanguageError> {
        match self {
            Self::CatalogDetail { alias } => {
                if !packet.catalog.iter().any(|entry| &entry.alias == alias) {
                    return Err(LanguageError::Invalid(format!(
                        "supplemental request names catalog alias {alias:?}, which was not in the packet"
                    )));
                }
                Ok(())
            }
            Self::TurnWindow { count } => {
                if *count == 0 || *count > MAX_TURN_WINDOW {
                    return Err(LanguageError::Invalid(format!(
                        "turn window must be between 1 and {MAX_TURN_WINDOW}"
                    )));
                }
                Ok(())
            }
            Self::Terminology { source_tokens } => {
                source_tokens.ground(&packet.utterance).map(|_| ())
            }
        }
    }
}

/// Tracks the one round an interpreter is allowed. A second request is a
/// rejected analysis rather than a second answer, because "one round" has to be
/// enforced somewhere and the model is not the place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupplementalBudget {
    pub rounds_used: usize,
}

impl SupplementalBudget {
    pub const MAX_ROUNDS: usize = 1;

    pub fn consume(&mut self) -> Result<(), LanguageError> {
        if self.rounds_used >= Self::MAX_ROUNDS {
            return Err(LanguageError::Invalid(
                "the interpreter already used its supplemental-context round".into(),
            ));
        }
        self.rounds_used += 1;
        Ok(())
    }

    pub fn exhausted(&self) -> bool {
        self.rounds_used >= Self::MAX_ROUNDS
    }
}
