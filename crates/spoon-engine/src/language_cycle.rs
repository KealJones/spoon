//! Building the context packet and holding a multi-part cycle across suspends.
//!
//! Two things live here that the cycle cannot do inline. Projecting Engine
//! state into a bounded, alias-only packet is a redaction step that deserves to
//! be tested on its own. And a cycle whose parts finish at different times
//! needs somewhere to keep the work already done, which `CycleProgress` has no
//! room for.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use spoon_core::language::{LanguageError, TokenStream};
use spoon_core::packet::{
    LanguageContextPacket, PacketAlias, PacketCatalogEntry, PacketEnvFact, PacketFact,
    PacketLimits, PacketSlot, PacketTurn, TurnRole,
};
use spoon_core::utterance::PartId;
use spoon_core::{EpisodeId, Value};

use crate::intent_catalog::{IntentCatalogEntry, IntentCatalogPattern};
use crate::parts::PartsRun;

/// Already-fetched Engine state, ready to project. Taking data rather than a
/// store keeps the redaction and bounding logic testable without a database.
#[derive(Debug, Clone, Default)]
pub struct PacketSources {
    pub catalog: Vec<(IntentCatalogEntry, Vec<IntentCatalogPattern>)>,
    pub turns: Vec<TurnSource>,
    pub terminology: Vec<(String, String)>,
    pub environment: Vec<(String, Value)>,
}

#[derive(Debug, Clone)]
pub struct TurnSource {
    pub role: TurnRole,
    pub summary: String,
    /// Facts this turn established, which a later residual claim may cite.
    pub facts: Vec<(String, Value)>,
}

/// Projects Engine state into the packet handed to a front model.
///
/// Every entry is addressed by a request-local alias. Nothing durable crosses
/// this boundary, which is checked rather than assumed: the packet is validated
/// for leaked identifiers before it is returned.
pub fn build_packet(
    utterance: TokenStream,
    sources: &PacketSources,
    limits: &PacketLimits,
) -> Result<LanguageContextPacket, LanguageError> {
    let mut packet = LanguageContextPacket::new(utterance);

    let mut fact_index = 0usize;
    for (index, turn) in sources.turns.iter().enumerate() {
        let facts = turn
            .facts
            .iter()
            .map(|(predicate, value)| {
                let alias = format!("f{fact_index}");
                fact_index += 1;
                PacketFact {
                    alias,
                    predicate: predicate.clone(),
                    value: value.clone(),
                }
            })
            .collect();
        packet.turns.push(PacketTurn {
            alias: format!("t{index}"),
            role: turn.role,
            summary: turn.summary.clone(),
            facts,
        });
    }

    // Catalog aliases are indexed so terminology can point at them, which is
    // how a surface form reaches a semantic key without naming a row.
    let mut key_to_alias = BTreeMap::new();
    for (index, (entry, patterns)) in sources.catalog.iter().enumerate() {
        let alias = format!("c{index}");
        key_to_alias.insert(entry.key.clone(), alias.clone());
        packet.catalog.push(PacketCatalogEntry {
            alias,
            key: entry.key.clone(),
            slots: entry
                .slots
                .iter()
                .map(|slot| PacketSlot {
                    name: slot.name.clone(),
                    required: slot.required,
                    value_kind: slot.value_kind.clone(),
                })
                .collect(),
            // Highest support first, so a bound that trims patterns keeps the
            // ones that actually earned their place.
            patterns: {
                let mut ranked: Vec<&IntentCatalogPattern> = patterns.iter().collect();
                ranked.sort_by_key(|pattern| std::cmp::Reverse(pattern.support));
                ranked
                    .into_iter()
                    .map(|pattern| pattern.pattern.clone())
                    .collect()
            },
            bound: entry.procedure_id.is_some(),
        });
    }

    for (index, (surface, key)) in sources.terminology.iter().enumerate() {
        let Some(refers_to) = key_to_alias.get(key) else {
            // A term pointing at a key the packet does not carry would be an
            // alias the model cannot resolve, so it is dropped rather than
            // shipped dangling.
            continue;
        };
        packet.terminology.push(PacketAlias {
            alias: format!("a{index}"),
            surface: surface.clone(),
            refers_to: refers_to.clone(),
        });
    }

    for (index, (predicate, value)) in sources.environment.iter().enumerate() {
        packet.environment.push(PacketEnvFact {
            alias: format!("e{index}"),
            predicate: predicate.clone(),
            value: value.clone(),
        });
    }

    packet.enforce(limits)?;
    // The packet is Engine output, so a leaked identifier here is an Engine
    // bug. Catching it before it reaches a provider is cheap.
    packet.validate_redaction()?;
    Ok(packet)
}

/// Why a multi-part cycle is suspended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuspendReason {
    /// The blocked part needs a procedure taught.
    Teacher,
    /// The blocked part needs the user.
    Clarification,
}

/// A multi-part cycle waiting to continue.
///
/// The run inside carries the frozen analysis and the outcomes collected so
/// far. Resuming binds one part and continues; it never re-analyzes the
/// original utterance, because a model asked to segment the same text twice can
/// segment it differently and orphan everything already done.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPartsCycle {
    pub run: PartsRun,
    pub blocked_on: PartId,
    pub reason: SuspendReason,
    /// Turns already emitted, so a resumed cycle can label the next one.
    pub turns: usize,
}

impl PendingPartsCycle {
    pub fn new(run: PartsRun, blocked_on: PartId, reason: SuspendReason) -> Self {
        Self {
            run,
            blocked_on,
            reason,
            turns: 1,
        }
    }

    pub fn turn_label(&self) -> String {
        format!("turn-{}", self.turns)
    }

    pub fn next_turn(&mut self) -> String {
        self.turns += 1;
        self.turn_label()
    }

    pub fn episode(&self) -> &EpisodeId {
        &self.run.episode
    }

    /// A stable digest of the frozen analysis and order. Comparing this across
    /// a suspend proves the resume path did not re-derive either.
    pub fn frozen_digest(&self) -> String {
        let order: Vec<&str> = self.run.order.iter().map(PartId::as_str).collect();
        format!("{}|{}", self.run.analysis.parts.len(), order.join(","))
    }
}
