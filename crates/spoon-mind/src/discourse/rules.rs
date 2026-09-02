//! Rules: storage and single-variable forward chaining.
//!
//! Rules are stored as JSON in the KV store under key "rules". The forward
//! chainer is intentionally simple: one universally-quantified variable per
//! rule, conjunctive antecedents, no negation-as-failure.

use serde::{Deserialize, Serialize};

use spoon_core::can::Can;
use spoon_core::store::Store;
use spoon_core::types::*;

// ---------- Rule struct ----------

/// A stored if-then rule derived from a universal assertion or an explicit
/// SCE "If ... then ..." sentence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub sce: String,
    /// Predicates that must all hold before the consequent fires.
    pub antecedent: Vec<Pred>,
    /// Predicates to assert when the antecedent matches.
    pub consequent: Vec<Pred>,
    /// All referents from the source clause (used to find the universal var).
    pub referents: Vec<Referent>,
}

// ---------- helpers ----------

pub(crate) fn capitalize_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn modal_str(m: &Modal) -> String {
    match m {
        Modal::Must => "must".into(),
        Modal::Should => "should".into(),
        Modal::May => "may".into(),
        Modal::Can => "can".into(),
    }
}

// ---------- persistence ----------

pub fn load_rules(store: &Store) -> anyhow::Result<Vec<Rule>> {
    match store.kv_get("rules")? {
        Some(v) => Ok(serde_json::from_value(v)?),
        None => Ok(vec![]),
    }
}

fn save_rules(store: &Store, rules: &[Rule]) -> anyhow::Result<()> {
    store.kv_set("rules", &serde_json::to_value(rules)?)
}

/// Store a rule derived from an explicit SCE Rule clause (Act::Rule).
pub fn add_rule(store: &Store, clause: &Clause) -> anyhow::Result<Rule> {
    let mut rules = load_rules(store).unwrap_or_default();
    // Dedup by SCE text.
    if let Some(existing) = rules.iter().find(|r| r.sce == clause.sce) {
        return Ok(existing.clone());
    }
    let all_refs: Vec<Referent> = clause
        .referents
        .iter()
        .chain(clause.then_referents.iter())
        .cloned()
        .collect();
    let rule = Rule {
        id: format!("rule_{}", rules.len()),
        sce: clause.sce.clone(),
        antecedent: clause.conditions.clone(),
        consequent: clause.then.clone(),
        referents: all_refs,
    };
    rules.push(rule.clone());
    save_rules(store, &rules)?;
    Ok(rule)
}

// ---------- universal_to_rule ----------

/// Ensure a concept for `noun` exists in `can` (and is persisted), returning
/// its ID. Creates a provisional Entity concept extending Thing if absent.
pub fn ensure_concept_in_can(can: &mut Can, store: &Store, noun: &str) -> ConceptId {
    let existing = can.concepts_for_noun(noun);
    if let Some(c) = existing.first() {
        return c.id.clone();
    }
    let id_str = capitalize_first(noun);
    let concept = Concept {
        id: ConceptId(id_str.clone()),
        kind: ConceptKind::Entity,
        extends: vec![ConceptId("Thing".into())],
        role_of: None,
        nouns: vec![noun.to_string()],
        description: String::new(),
        tier: Tier::Provisional,
        provenance: Provenance::User {
            utterance: format!("auto-created for noun '{}'", noun),
        },
    };
    can.add_concept(concept.clone());
    let _ = store.save_concept(&concept);
    ConceptId(id_str)
}

/// Build the consequent Pred list from clause conditions, transforming the
/// `be` copula into explicit `rel.is` / `rel.is_a` predicates.
fn transform_consequent(
    can: &mut Can,
    store: &Store,
    conditions: &[Pred],
    referents: &[Referent],
) -> Vec<Pred> {
    conditions
        .iter()
        .filter_map(|p| transform_pred(can, store, p, referents))
        .collect()
}

fn transform_pred(
    can: &mut Can,
    store: &Store,
    pred: &Pred,
    referents: &[Referent],
) -> Option<Pred> {
    if pred.pred == "be" {
        // Attribute form: be(x1) attr="brown" -> rel.is(x1, Text("brown"))
        if let Some(attr) = &pred.attr {
            let mut args: Vec<Term> = pred.args.clone();
            args.push(Term::Value {
                value: Value::Text(attr.clone()),
            });
            return Some(Pred {
                pred: "rel.is".to_string(),
                args,
                negated: pred.negated,
                modal: pred.modal.clone(),
                adjuncts: vec![],
                attr: None,
            });
        }
        // NP form: be(x1, x2) where x2 is Indef/noun -> rel.is_a(x1, Name(concept))
        if pred.args.len() >= 2 {
            if let Term::Var { var: x2_var } = &pred.args[1] {
                let ref2 = referents.iter().find(|r| &r.var == x2_var);
                if let Some(r) = ref2 {
                    if let Some(noun) = &r.noun {
                        let concept = ensure_concept_in_can(can, store, noun);
                        return Some(Pred {
                            pred: "rel.is_a".to_string(),
                            args: vec![
                                pred.args[0].clone(),
                                Term::Value {
                                    value: Value::Name(concept.0),
                                },
                            ],
                            negated: pred.negated,
                            modal: pred.modal.clone(),
                            adjuncts: vec![],
                            attr: None,
                        });
                    }
                }
            }
        }
        None
    } else {
        Some(pred.clone())
    }
}

/// Convert a universal assertion (Act::Assert with Quant::Every/No etc.) into
/// a stored Rule. The antecedent encodes the subject constraint (is_a); the
/// consequent encodes the clause conditions (transformed).
pub fn universal_to_rule(
    can: &mut Can,
    store: &Store,
    clause: &Clause,
) -> anyhow::Result<Rule> {
    let mut rules = load_rules(store).unwrap_or_default();
    // Dedup by SCE.
    if let Some(existing) = rules.iter().find(|r| r.sce == clause.sce) {
        return Ok(existing.clone());
    }

    // Find the universal referent.
    let u_ref = clause
        .referents
        .iter()
        .find(|r| {
            matches!(
                r.quant,
                Quant::Every | Quant::No | Quant::AtLeast(_) | Quant::AtMost(_) | Quant::Exactly(_)
            )
        })
        .ok_or_else(|| anyhow::anyhow!("No universal referent in clause"))?;

    let u_var = u_ref.var.clone();

    // Build antecedent from the universal noun constraint.
    let mut antecedent = Vec::new();
    if let Some(noun) = &u_ref.noun {
        let concept = ensure_concept_in_can(can, store, noun);
        antecedent.push(Pred {
            pred: "rel.is_a".to_string(),
            args: vec![
                Term::Var { var: u_var.clone() },
                Term::Value {
                    value: Value::Name(concept.0),
                },
            ],
            negated: false,
            modal: None,
            adjuncts: vec![],
            attr: None,
        });
    }

    // For Quant::No we also include the conditions (modifiers on the subject)
    // in the antecedent.
    let is_no = matches!(u_ref.quant, Quant::No);

    // Build consequent from conditions (transformed).
    let all_refs: Vec<Referent> = clause
        .referents
        .iter()
        .chain(clause.then_referents.iter())
        .cloned()
        .collect();

    let consequent = if is_no {
        // For No: conditions become consequent with negation added.
        transform_consequent(can, store, &clause.conditions, &all_refs)
            .into_iter()
            .map(|mut p| {
                p.negated = true;
                p
            })
            .collect()
    } else {
        transform_consequent(can, store, &clause.conditions, &all_refs)
    };

    let rule = Rule {
        id: format!("rule_{}", rules.len()),
        sce: clause.sce.clone(),
        antecedent,
        consequent,
        referents: all_refs,
    };

    rules.push(rule.clone());
    save_rules(store, &rules)?;
    Ok(rule)
}

// ---------- forward chaining ----------

/// Collect all entity values (Name variants) that satisfy every antecedent
/// predicate of the rule. Uses the `u_var` position as the binding variable.
fn find_universal_matches(
    store: &Store,
    antecedent: &[Pred],
    u_var: &str,
) -> Vec<Value> {
    use std::collections::HashSet;

    let mut candidates: Option<HashSet<String>> = None;

    for pred in antecedent {
        let action_id = ActionId(pred.pred.clone());

        // Find which arg position holds the universal variable.
        let u_pos = pred.args.iter().position(|t| {
            matches!(t, Term::Var { var } if var == u_var)
        });
        let Some(u_pos) = u_pos else { continue };

        // Build query pattern: None at u_pos, bound values elsewhere.
        let pattern: Vec<Option<Value>> = pred
            .args
            .iter()
            .map(|t| match t {
                Term::Var { var } if var == u_var => None,
                Term::Value { value } => Some(value.clone()),
                _ => None,
            })
            .collect();

        let facts = store.query_facts(&action_id, &pattern).unwrap_or_default();

        let new_candidates: HashSet<String> = facts
            .iter()
            .filter(|f| f.truth && f.invalidated_at.is_none())
            .filter_map(|f| f.args.get(u_pos))
            .filter_map(|v| {
                if let Value::Name(n) = v {
                    Some(n.clone())
                } else {
                    None
                }
            })
            .collect();

        candidates = Some(match candidates {
            None => new_candidates,
            Some(existing) => existing.intersection(&new_candidates).cloned().collect(),
        });
    }

    candidates
        .unwrap_or_default()
        .into_iter()
        .map(Value::Name)
        .collect()
}

/// Instantiate a consequent pred for a specific entity binding of `u_var`.
/// Returns None if any term cannot be resolved (e.g. unbound Indef in consequent).
fn instantiate_pred(pred: &Pred, u_var: &str, entity_val: &Value) -> Option<Fact> {
    let mut args: Vec<Value> = Vec::new();
    for term in &pred.args {
        match term {
            Term::Var { var } if var == u_var => args.push(entity_val.clone()),
            Term::Value { value } => args.push(value.clone()),
            // Unresolved var in consequent - skip this pred.
            _ => return None,
        }
    }
    // Attr form: add text arg.
    if let Some(attr) = &pred.attr {
        args.push(Value::Text(attr.clone()));
    }
    Some(Fact {
        id: 0,
        pred: ActionId(pred.pred.clone()),
        args,
        truth: !pred.negated,
        modal: pred.modal.as_ref().map(|m| modal_str(m)),
        asserted_at: now_ms(),
        invalidated_at: None,
        source: "derived".to_string(),
        episode_id: None,
    })
}

/// Single pass of forward chaining over all stored rules. Derives new facts,
/// inserts them into the store, and returns the inserted ones.
///
/// `_new_facts` is a hint about recently added facts (not used in this simple
/// implementation - we always scan everything to keep the logic sound).
pub fn forward_chain(
    _can: &Can,
    store: &Store,
    _new_facts: &[Fact],
    max_derivations: usize,
) -> Vec<Fact> {
    let rules = match load_rules(store) {
        Ok(r) => r,
        Err(_) => return vec![],
    };

    let mut derived: Vec<Fact> = Vec::new();

    'rules: for rule in &rules {
        // Find the universal (binding) variable.
        let u_ref = rule.referents.iter().find(|r| {
            matches!(
                r.quant,
                Quant::Every | Quant::No | Quant::AtLeast(_) | Quant::AtMost(_) | Quant::Exactly(_)
            )
        });
        let Some(u_ref) = u_ref else { continue };
        let u_var = &u_ref.var;

        let matches = find_universal_matches(store, &rule.antecedent, u_var);

        for entity_val in &matches {
            if derived.len() >= max_derivations {
                break 'rules;
            }
            for cons_pred in &rule.consequent {
                if let Some(fact) = instantiate_pred(cons_pred, u_var, entity_val) {
                    // Skip if this fact already exists.
                    let pattern: Vec<Option<Value>> =
                        fact.args.iter().map(|a| Some(a.clone())).collect();
                    let already_exists = store
                        .query_facts(&fact.pred, &pattern)
                        .map(|v| v.iter().any(|f| f.truth == fact.truth))
                        .unwrap_or(false);
                    if !already_exists {
                        if let Ok(id) = store.insert_fact(&fact) {
                            derived.push(Fact { id, ..fact });
                        }
                    }
                }
            }
        }
    }

    derived
}
