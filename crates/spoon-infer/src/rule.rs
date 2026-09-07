//! Rules, and finding the ones worth trying.

use std::sync::Arc;

use spoon_concept::{
    Activation, Concept, Realization, RealizationSpec, RuleDirection, SymbolId, Tier,
};
use spoon_store::Store;

use crate::error::{InferError, Result};

/// A rule pulled out of a stored realization, with its parts already separated.
///
/// The store holds rules as `RealizationSpec::Rule`. Re-matching that enum on
/// every derivation step would mean re-deciding the same thing thousands of
/// times, so a rule is unpacked once when the index is built.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredRule {
    /// Realization name, which is the rule's identity for evidence purposes.
    pub name: Arc<str>,
    /// The concept the realization is attached to.
    pub target: Concept,
    /// What must hold for the conclusion to follow.
    pub pattern: Concept,
    /// An extra concept that must be derivable, checked after the conclusion
    /// unifies. `Symmetric<FriendWith>` is a condition, not an antecedent.
    pub condition: Option<Concept>,
    /// What follows.
    pub produce: Concept,
    pub direction: RuleDirection,
    pub activation: Activation,
    pub tier: Tier,
}

impl StoredRule {
    /// Unpack a realization, or say why it is not a rule.
    pub fn from_realization(realization: &Realization) -> Option<StoredRule> {
        match &realization.spec {
            RealizationSpec::Rule {
                pattern,
                condition,
                produce,
                direction,
            } => Some(StoredRule {
                name: realization.name.clone(),
                target: realization.target.clone(),
                pattern: pattern.clone(),
                condition: condition.clone(),
                produce: produce.clone(),
                direction: *direction,
                activation: realization.activation.clone(),
                tier: realization.tier,
            }),
            _ => None,
        }
    }

    /// The head symbol of the conclusion, when it has one.
    ///
    /// `None` means the conclusion's head is itself a hole or a compound, as in
    /// `?0<?1, ?2>`, which quantifies over relations. Such a rule can conclude
    /// anything, so no head index can exclude it and it has to be tried
    /// against every goal.
    pub fn produce_head(&self) -> Option<SymbolId> {
        self.produce.head_symbol()
    }

    pub fn produce_arity(&self) -> usize {
        self.produce.arity()
    }

    /// Deprecated rules never fire. Tier is otherwise not a privilege: a
    /// learned rule competes with a bootstrap one on evidence.
    pub fn is_usable(&self) -> bool {
        self.tier != Tier::Deprecated && self.direction.allows_backward()
    }
}

/// Finding the rules whose conclusion could match a goal.
///
/// Abstracted so the strategy can change without the chaining engine noticing.
/// A linear scan is correct and is what [`ScanIndex`] does; a discrimination
/// tree answers the same question without touching every rule.
pub trait RuleIndex: Send + Sync {
    /// Rules whose conclusion could unify with `goal`.
    ///
    /// Over-approximating is allowed and expected: the index exists to avoid
    /// obviously hopeless unification attempts, and the caller unifies properly
    /// afterwards. Under-approximating is a correctness bug, because a rule
    /// that is never offered can never fire.
    fn candidates(&self, goal: &Concept) -> Vec<Arc<StoredRule>>;

    /// Every rule held, in a stable order.
    fn all(&self) -> Vec<Arc<StoredRule>>;

    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Every backward-usable rule in the store, in a stable order.
///
/// Sorted by name so that two brains holding the same rules try them in the
/// same order and produce the same derivation.
pub fn load_rules(store: &Store) -> Result<Vec<Arc<StoredRule>>> {
    let mut rules: Vec<Arc<StoredRule>> = store
        .all_realizations()
        .map_err(InferError::from)?
        .iter()
        .filter_map(StoredRule::from_realization)
        .filter(StoredRule::is_usable)
        .map(Arc::new)
        .collect();
    rules.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(rules)
}

/// The obvious index: keep every rule and offer them all.
///
/// Correct by construction and the reference the faster index is checked
/// against. Fine while a brain holds tens of rules; it is the wrong shape once
/// it holds thousands, because every derivation step walks the whole set.
#[derive(Debug, Clone, Default)]
pub struct ScanIndex {
    rules: Vec<Arc<StoredRule>>,
}

impl ScanIndex {
    pub fn new(rules: Vec<Arc<StoredRule>>) -> Self {
        ScanIndex { rules }
    }

    pub fn from_store(store: &Store) -> Result<Self> {
        Ok(ScanIndex::new(load_rules(store)?))
    }
}

impl RuleIndex for ScanIndex {
    fn candidates(&self, goal: &Concept) -> Vec<Arc<StoredRule>> {
        let goal_head = goal.head_symbol();
        let goal_arity = goal.arity();
        self.rules
            .iter()
            .filter(|rule| match (rule.produce_head(), goal_head) {
                // Both heads are known symbols: they have to agree, and so does
                // the arity.
                (Some(rule_head), Some(goal_head)) => {
                    rule_head == goal_head && rule.produce_arity() == goal_arity
                }
                // A rule concluding `?0<..>` quantifies over relations and can
                // conclude anything, so it stays a candidate.
                (None, _) => true,
                // A goal whose head is not a plain symbol could still unify
                // with a concrete conclusion.
                (Some(_), None) => true,
            })
            .cloned()
            .collect()
    }

    fn all(&self) -> Vec<Arc<StoredRule>> {
        self.rules.clone()
    }

    fn len(&self) -> usize {
        self.rules.len()
    }
}
