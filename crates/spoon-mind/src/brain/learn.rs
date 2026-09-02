//! Learning: synthesize actions from specs (user examples or the teacher),
//! word synonyms, and referent corrections. Everything persists in the store.

use std::collections::HashMap;

use anyhow::{anyhow, bail};

use spoon_core::store::Store;
use spoon_core::types::*;

use crate::discourse::{self, ground_all, Entity};
use crate::dispatch::ask_examples_move;
use crate::grow::{self, SynthBudget, SynthOutcome};

use spoon_lang::ears::Gate;

use super::respond::describe_program;
use super::session::Pending;
use super::Brain;

/// kv key under which word synonyms persist. The `seed.` prefix makes them
/// part of `spoon export`.
pub const SYNONYMS_KEY: &str = "seed.synonyms";

/// What `Brain::learn_from_spec` produced.
#[derive(Debug, Clone)]
pub struct LearnOutcome {
    pub action: Action,
    /// Readable program, e.g. `add(x, x)`.
    pub description: String,
}

/// Not enough usable `rel.V` facts to build a spec.
#[derive(Debug)]
pub struct ExamplesShort {
    pub have: usize,
    pub why: String,
}

/// Build a Spec for verb `v` from stored property facts `rel.V(x, y)`: the
/// leading args are the inputs, the last arg is the output.
pub fn spec_from_facts(verb: &str, store: &Store) -> Result<Spec, ExamplesShort> {
    let rel_id = ActionId(format!("rel.{verb}"));
    let facts = store
        .query_facts(&rel_id, &[])
        .map_err(|e| ExamplesShort { have: 0, why: format!("store error: {e}") })?;
    let positive: Vec<&Fact> = facts.iter().filter(|f| f.truth && f.args.len() >= 2).collect();

    let mut examples = Vec::new();
    let mut param_types: Option<Vec<Type>> = None;
    let mut ret_type: Option<Type> = None;
    for fact in &positive {
        let inputs: Vec<Value> = fact.args[..fact.args.len() - 1].to_vec();
        let output = fact.args.last().unwrap().clone();
        let input_tys: Vec<Type> = inputs.iter().map(Value::type_of).collect();
        match &param_types {
            None => {
                param_types = Some(input_tys);
                ret_type = Some(output.type_of());
            }
            Some(existing) if existing.len() != input_tys.len() => continue,
            Some(_) => {}
        }
        examples.push(Example { inputs, output });
    }

    if examples.len() < 2 {
        return Err(ExamplesShort {
            have: examples.len(),
            why: format!("need at least 2 consistent examples for '{verb}', have {}", examples.len()),
        });
    }

    Ok(Spec {
        id: format!("user:{}:{}", verb, now_ms()),
        name_hint: verb.to_string(),
        verbs: vec![verb.to_string()],
        phrasings: vec![],
        params: param_types.unwrap(),
        param_names: vec![],
        ret: ret_type.unwrap(),
        examples,
        description: String::new(),
        source: "user".to_string(),
    })
}

/// Replace whole words (outside double quotes) by their canonical form,
/// keeping a leading capital: `John owns a pup.` -> `John owns a dog.`
pub fn apply_synonyms(text: &str, map: &HashMap<String, String>) -> String {
    if map.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let mut in_quotes = false;
    let flush = |word: &mut String, out: &mut String, in_quotes: bool| {
        if word.is_empty() {
            return;
        }
        match (in_quotes, map.get(&word.to_lowercase())) {
            (false, Some(canon)) => {
                if word.chars().next().is_some_and(char::is_uppercase) {
                    let mut cs = canon.chars();
                    if let Some(f) = cs.next() {
                        out.extend(f.to_uppercase());
                        out.push_str(cs.as_str());
                    }
                } else {
                    out.push_str(canon);
                }
            }
            _ => out.push_str(word),
        }
        word.clear();
    };
    for c in text.chars() {
        if c.is_alphanumeric() || c == '\'' {
            word.push(c);
        } else {
            flush(&mut word, &mut out, in_quotes);
            if c == '"' {
                in_quotes = !in_quotes;
            }
            out.push(c);
        }
    }
    flush(&mut word, &mut out, in_quotes);
    out
}

fn replace_word(text: &str, from: &str, to: &str) -> String {
    let mut map = HashMap::new();
    map.insert(from.to_lowercase(), to.to_string());
    apply_synonyms(text, &map)
}

impl Brain {
    /// Synthesize `verb` from a validated spec, register the action (CAN,
    /// store, parser lexicon) and describe it. Used by the example path and
    /// the teacher path; the LLM call lives in the caller.
    pub fn learn_from_spec(&self, verb: &str, mut spec: Spec) -> anyhow::Result<LearnOutcome> {
        spec.validate().map_err(|e| anyhow!("spec for '{verb}' is invalid: {e}"))?;
        if !spec.verbs.iter().any(|v| v == verb) {
            spec.verbs.insert(0, verb.to_string());
        }
        if spec.name_hint.is_empty() {
            spec.name_hint = verb.to_string();
        }
        self.counters.bump(&self.counters.synth_attempted);
        let outcome = {
            let can = self.can.lock();
            grow::synthesize(&spec, &can, &self.kernel, &SynthBudget::default())
        };
        match outcome {
            SynthOutcome::Found { program, .. } => {
                let action = grow::action_from_program(&spec, &program);
                self.register_learned_action(&action)?;
                self.counters.bump(&self.counters.synth_succeeded);
                let description = describe_program(&program, &self.can.lock());
                Ok(LearnOutcome { action, description })
            }
            SynthOutcome::Exhausted { tried, millis } => bail!(
                "no program fits the {} examples for '{verb}' (tried {tried} in {millis}ms)",
                spec.examples.len()
            ),
            SynthOutcome::Budget { reason, .. } => bail!("synthesis budget for '{verb}': {reason}"),
        }
    }

    /// Learn `verb` from the property facts the user has stated about it.
    pub(super) fn learn_from_examples(&self, verb: &str) -> Result<LearnOutcome, ExamplesShort> {
        let spec = spec_from_facts(verb, &self.store.lock())?;
        let have = spec.examples.len();
        self.learn_from_spec(verb, spec).map_err(|e| ExamplesShort { have, why: e.to_string() })
    }

    /// Run the command that triggered learning and report what was learned.
    pub(super) fn run_learned(
        &self,
        outcome: &LearnOutcome,
        verb: &str,
        signals: &[Signal],
        sce: &str,
        session_id: &str,
        from_teacher: bool,
        trace: &mut Vec<String>,
    ) -> anyhow::Result<ResponsePlan> {
        let intent = Intent {
            goal: Goal::Action { action: outcome.action.id.clone() },
            signals: signals.to_vec(),
            routes: vec![outcome.action.id.clone()],
            sce: sce.to_string(),
        };
        let mut plan = self.handle_plan(&intent, vec![], session_id, trace)?;
        let source = if from_teacher { " (from the teacher)" } else { "" };
        plan.push(Move::Learned { what: format!("{verb} = {}{source}", outcome.description) });
        Ok(plan)
    }

    /// Remember that we are waiting for examples of `verb` and ask for them.
    pub(super) fn ask_for_examples(
        &self,
        session_id: &str,
        verb: &str,
        sce: &str,
        signals: &[Signal],
        have: usize,
        mut moves: Vec<Move>,
    ) -> ResponsePlan {
        self.with_session(session_id, |s| {
            s.pending = Some(Pending::UnknownVerb {
                verb: verb.to_string(),
                sce: sce.to_string(),
                signals: signals.to_vec(),
            });
        });
        // "The double of 3 is 6." needs the verb as a noun.
        self.gate.lock().lex.add_noun(verb);
        moves.retain(|m| !matches!(m, Move::Clarify { .. }));
        moves.push(ask_examples_move(verb, signals, have));
        ResponsePlan::new(moves)
    }

    /// The user answered a request for examples. Store any facts they stated,
    /// then try to learn again. `None` means the text was not an answer.
    pub(super) fn handle_unknown_verb_followup(
        &self,
        session_id: &str,
        text: &str,
        verb: &str,
        sce: &str,
        signals: &[Signal],
        trace: &mut Vec<String>,
    ) -> Option<ResponsePlan> {
        let clauses = {
            let gate = self.gate.lock();
            match gate.parse(text) {
                Ok(c) if !c.is_empty() => c,
                _ => {
                    let ears = self.ears.lock();
                    match ears.hear_native(text, &*gate) {
                        Some(r) if !r.clauses.is_empty() => r.clauses,
                        _ => return None,
                    }
                }
            }
        };
        if !clauses.iter().any(|c| matches!(c.act, Act::Assert)) {
            return None;
        }

        {
            let mut sessions = self.sessions.lock();
            let session = sessions.entry(session_id.to_string()).or_insert_with(super::Session::new);
            let mut can = self.can.lock();
            let store = self.store.lock();
            let gs = ground_all(&mut session.discourse, &clauses, &can);
            let mut fw = discourse::FactWriter { can: &mut can, store: &store };
            for g in &gs {
                let _ = discourse::assert_grounded(&mut fw, g, "user", None);
            }
        }

        match self.learn_from_examples(verb) {
            Ok(outcome) => {
                if self.cfg.debug {
                    trace.push(format!("learned '{verb}': {}", outcome.description));
                }
                self.run_learned(&outcome, verb, signals, sce, session_id, false, trace).ok()
            }
            Err(short) => {
                if self.cfg.debug {
                    trace.push(format!("learn failed: {}", short.why));
                }
                Some(self.ask_for_examples(session_id, verb, sce, signals, short.have, vec![]))
            }
        }
    }

    pub(super) fn register_learned_action(&self, action: &Action) -> anyhow::Result<()> {
        let mut can = self.can.lock();
        let store = self.store.lock();
        for v in &action.verbs {
            let rel_id = ActionId(format!("rel.{}", v));
            if can.action(&rel_id).is_some_and(|a| a.tier == Tier::Provisional) {
                can.remove_action(&rel_id);
                let _ = store.delete_action(&rel_id);
            }
        }
        can.add_action(action.clone());
        store.save_action(action)?;

        let mut gate = self.gate.lock();
        for v in &action.verbs {
            gate.lex.add_verb(v);
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Corrections
    // -----------------------------------------------------------------------

    /// `"pup" means "dog".`: remember the synonym (store + ears + turn input).
    pub(super) fn learn_synonym(&self, word: &str, means: &str) -> anyhow::Result<Move> {
        let word_l = word.to_lowercase();
        let means_l = means.to_lowercase();
        let json = {
            let mut syn = self.synonyms.lock();
            syn.insert(word_l.clone(), means_l.clone());
            serde_json::to_value(&*syn)?
        };
        self.store.lock().kv_set(SYNONYMS_KEY, &json)?;
        self.ears.lock().learn_word(&word_l, &means_l);
        self.gate.lock().lex.add_noun(&means_l);
        Ok(Move::Learned { what: format!("\"{word_l}\" means \"{means_l}\"") })
    }

    /// `User means Mary.`: the last stored facts were about the wrong entity.
    /// Retract them and re-store them with `new_name` in that role.
    pub(super) fn correct_referent(&self, session_id: &str, new_name: &str) -> anyhow::Result<ResponsePlan> {
        let last = self.with_session(session_id, |s| s.last_assert.clone());
        let Some(last) = last else {
            return Ok(ResponsePlan::single(Move::Clarify {
                question: format!("what should I change to {new_name}?"),
                options: vec![],
                slot_type: None,
            }));
        };

        let old = Value::Name(last.target.clone());
        let new = Value::Name(new_name.to_string());
        let corrected = {
            let store = self.store.lock();
            let facts: Vec<Fact> = store
                .facts_about(&old)?
                .into_iter()
                .filter(|f| f.id > last.facts_before && f.asserted_at >= last.at)
                .collect();
            let now = now_ms();
            for f in &facts {
                let mut nf = f.clone();
                nf.id = 0;
                nf.asserted_at = now;
                nf.args = f.args.iter().map(|a| if *a == old { new.clone() } else { a.clone() }).collect();
                discourse::supersede(&store, f.id, &nf)?;
            }
            // The ears mapping that produced the wrong fact is suspect.
            if matches!(last.ears_path, EarsPath::Llm | EarsPath::Phrasing | EarsPath::Retrieval) {
                for p in store.pairs(i8::MIN).unwrap_or_default() {
                    if p.sce == last.sce {
                        let _ = store.update_pair_credit(p.id, -1);
                    }
                }
            }
            facts.len()
        };
        if corrected == 0 {
            return Ok(ResponsePlan::single(Move::Clarify {
                question: format!("what should I change to {new_name}?"),
                options: vec![],
                slot_type: None,
            }));
        }

        self.with_session(session_id, |s| {
            let turn = s.discourse.turn;
            let concept = s
                .discourse
                .entities
                .iter()
                .find(|e| e.id == old)
                .and_then(|e| e.concept.clone());
            if !s.discourse.entities.iter().any(|e| e.id == new) {
                s.discourse.entities.push(Entity {
                    id: new.clone(),
                    concept,
                    noun: None,
                    mods: vec![],
                    last_mentioned: turn,
                    mentions: 1,
                });
            }
            if let Some(la) = &mut s.last_assert {
                la.target = new_name.to_string();
            }
        });

        let summary = replace_word(&last.sce, &last.target, new_name);
        let summary = summary.trim().trim_end_matches('.').to_string();
        Ok(ResponsePlan::single(Move::Ack { summary }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synonyms_replace_whole_words_outside_quotes() {
        let mut map = HashMap::new();
        map.insert("pup".to_string(), "dog".to_string());
        assert_eq!(apply_synonyms("John owns a pup.", &map), "John owns a dog.");
        assert_eq!(apply_synonyms("Pup is a puppet.", &map), "Dog is a puppet.");
        assert_eq!(apply_synonyms("\"pup\" means \"dog\".", &map), "\"pup\" means \"dog\".");
        assert_eq!(apply_synonyms("no pups here", &map), "no pups here");
    }
}
