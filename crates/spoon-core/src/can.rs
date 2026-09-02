//! In-memory index over the Concept Action Network.
//!
//! The store persists concepts and actions; this struct is the fast working
//! copy the planner, lexicon, and synthesizer query. Mutations go through
//! `Can` and are mirrored to the store by the owner (`Brain`).

use std::collections::{HashMap, HashSet};

use crate::types::*;

#[derive(Debug, Default, Clone)]
pub struct Can {
    concepts: HashMap<ConceptId, Concept>,
    actions: HashMap<ActionId, Action>,
    /// verb lemma -> actions using it
    by_verb: HashMap<String, Vec<ActionId>>,
    /// noun lemma -> concepts using it
    by_noun: HashMap<String, Vec<ConceptId>>,
    /// output type -> actions producing it (exact type key)
    by_output: HashMap<String, Vec<ActionId>>,
}

fn type_key(t: &Type) -> String {
    t.to_string()
}

impl Can {
    pub fn new() -> Can {
        Can::default()
    }

    // ---- concepts ----
    pub fn add_concept(&mut self, c: Concept) {
        for n in &c.nouns {
            self.by_noun.entry(n.to_lowercase()).or_default().push(c.id.clone());
        }
        self.concepts.insert(c.id.clone(), c);
    }
    pub fn concept(&self, id: &ConceptId) -> Option<&Concept> {
        self.concepts.get(id)
    }
    pub fn concepts(&self) -> impl Iterator<Item = &Concept> {
        self.concepts.values()
    }
    pub fn concepts_for_noun(&self, noun: &str) -> Vec<&Concept> {
        self.by_noun
            .get(&noun.to_lowercase())
            .map(|ids| ids.iter().filter_map(|i| self.concepts.get(i)).collect())
            .unwrap_or_default()
    }
    pub fn nouns(&self) -> impl Iterator<Item = &String> {
        self.by_noun.keys()
    }
    /// Transitive is-a closure including the concept itself.
    pub fn ancestors(&self, id: &ConceptId) -> Vec<ConceptId> {
        let mut seen = HashSet::new();
        let mut stack = vec![id.clone()];
        let mut out = vec![];
        while let Some(c) = stack.pop() {
            if !seen.insert(c.clone()) {
                continue;
            }
            out.push(c.clone());
            if let Some(con) = self.concepts.get(&c) {
                stack.extend(con.extends.iter().cloned());
                if let Some(r) = &con.role_of {
                    stack.push(r.clone());
                }
            }
        }
        out
    }
    pub fn is_a(&self, sub: &ConceptId, sup: &ConceptId) -> bool {
        self.ancestors(sub).contains(sup)
    }

    /// Type assignability with CAN knowledge: structural rules plus is-a.
    pub fn assignable(&self, target: &Type, source: &Type) -> bool {
        if target.accepts(source) {
            return true;
        }
        match (target, source) {
            (Type::Concept(t), Type::Concept(s)) => self.is_a(s, t),
            // A named primitive concept (e.g. Temperature over Float) accepts its base.
            (Type::Concept(t), s) => self
                .concepts
                .get(t)
                .map(|c| matches!(&c.kind, ConceptKind::Primitive { ty } if ty.accepts(s)))
                .unwrap_or(false),
            (t, Type::Concept(s)) => self
                .concepts
                .get(s)
                .map(|c| matches!(&c.kind, ConceptKind::Primitive { ty } if t.accepts(ty)))
                .unwrap_or(false),
            (Type::List(t), Type::List(s)) => self.assignable(t, s),
            _ => false,
        }
    }

    // ---- actions ----
    pub fn add_action(&mut self, a: Action) {
        self.remove_action_index(&a.id);
        for v in &a.verbs {
            self.by_verb.entry(v.to_lowercase()).or_default().push(a.id.clone());
        }
        self.by_output.entry(type_key(&a.output)).or_default().push(a.id.clone());
        self.actions.insert(a.id.clone(), a);
    }
    fn remove_action_index(&mut self, id: &ActionId) {
        if let Some(old) = self.actions.get(id) {
            for v in &old.verbs {
                if let Some(list) = self.by_verb.get_mut(&v.to_lowercase()) {
                    list.retain(|x| x != id);
                }
            }
            if let Some(list) = self.by_output.get_mut(&type_key(&old.output)) {
                list.retain(|x| x != id);
            }
        }
    }
    pub fn remove_action(&mut self, id: &ActionId) -> Option<Action> {
        self.remove_action_index(id);
        self.actions.remove(id)
    }
    pub fn action(&self, id: &ActionId) -> Option<&Action> {
        self.actions.get(id)
    }
    pub fn action_mut(&mut self, id: &ActionId) -> Option<&mut Action> {
        self.actions.get_mut(id)
    }
    pub fn actions(&self) -> impl Iterator<Item = &Action> {
        self.actions.values()
    }
    pub fn actions_for_verb(&self, verb: &str) -> Vec<&Action> {
        self.by_verb
            .get(&verb.to_lowercase())
            .map(|ids| ids.iter().filter_map(|i| self.actions.get(i)).collect())
            .unwrap_or_default()
    }
    pub fn verbs(&self) -> impl Iterator<Item = &String> {
        self.by_verb.keys()
    }
    /// Actions whose output is assignable to `ty`. Excludes Deprecated.
    pub fn producers_of(&self, ty: &Type) -> Vec<&Action> {
        self.actions
            .values()
            .filter(|a| a.tier != Tier::Deprecated && self.assignable(ty, &a.output))
            .collect()
    }
    pub fn pure_actions(&self) -> Vec<&Action> {
        self.actions
            .values()
            .filter(|a| a.effect == Effect::Pure && a.tier != Tier::Deprecated)
            .collect()
    }
    pub fn len(&self) -> (usize, usize) {
        (self.concepts.len(), self.actions.len())
    }

    /// Record a use for activation statistics.
    pub fn touch(&mut self, id: &ActionId, success: bool) {
        if let Some(a) = self.actions.get_mut(id) {
            let now = now_ms();
            a.stats.uses += 1;
            if success {
                a.stats.successes += 1;
            } else {
                a.stats.failures += 1;
            }
            a.stats.last_used = Some(now);
            a.stats.history.push(now);
            if a.stats.history.len() > 64 {
                a.stats.history.remove(0);
            }
        }
    }

    /// ACT-R base-level activation: ln(sum (age_seconds)^-d), d = 0.5.
    /// Kernel actions get a floor so they are never starved.
    pub fn activation(&self, id: &ActionId) -> f64 {
        let Some(a) = self.actions.get(id) else { return f64::NEG_INFINITY };
        let now = now_ms();
        let sum: f64 = a
            .stats
            .history
            .iter()
            .map(|t| {
                let age = ((now - t).max(1000) as f64) / 1000.0;
                age.powf(-0.5)
            })
            .sum();
        let base = if sum > 0.0 { sum.ln() } else { -10.0 };
        match a.tier {
            Tier::Kernel => base.max(-2.0),
            Tier::Consolidated => base + 0.5,
            _ => base,
        }
    }
}
