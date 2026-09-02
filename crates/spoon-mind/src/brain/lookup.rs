//! The lookup chain: what Spoon does when a turn names something it cannot
//! place.
//!
//! Three sources, cheapest first, and no LLM in the interior:
//!
//!   1. WordNet hypernyms shipped in `data/seed/lexicon.json`. Offline,
//!      instant, and the reason `Is a dog an animal?` is answerable at all.
//!   2. Wikidata through the `know.wikidata_describe` kernel primitive.
//!      Network, so it needs `--offline` off and a permission mode that
//!      allows a Network effect.
//!   3. The Teacher seat (`concept_for`). This is a model writing a concept
//!      model, never a program, which is the one thing the Teacher is for.
//!
//! Steps 1 and 2 run inside `turn_sync` before dispatch, so an answer that
//! needed a lookup still comes out of memory on the same turn. Step 3 is
//! async and runs after `turn_sync`, and only for a term the turn is really
//! asking about.
//!
//! Everything learned is stored as a concept-level `rel.is_a` fact, which is
//! the same shape the discourse layer reads back. No parallel structure.

use std::collections::HashMap;
use std::path::Path;

use spoon_core::kernel::Ctx;
use spoon_core::types::*;

use spoon_lang::sce::singularize_noun;

use crate::discourse::{self, class_parents, concept_of_noun, FactWriter};
use crate::dispatch::{self, DispatchCtx, Dispatched};

use super::session::Session;
use super::{Brain, BrainHost};

/// How many terms one turn may look up. A sentence full of new words is a
/// sentence to ask about, not a research project.
const MAX_TERMS_PER_TURN: usize = 2;

/// How far up the hypernym chain one lookup walks.
const MAX_HYPERNYM_HOPS: usize = 3;

// ---------------------------------------------------------------------------
// WordNet hypernyms
// ---------------------------------------------------------------------------

/// noun -> hypernym, read once from the seed lexicon.
#[derive(Default)]
pub struct Hypernyms {
    parents: HashMap<String, String>,
}

impl Hypernyms {
    pub fn load(data_dir: &Path) -> Hypernyms {
        #[derive(serde::Deserialize)]
        struct Entry {
            lemma: String,
            plural: Option<String>,
            hypernym: Option<String>,
        }
        #[derive(serde::Deserialize)]
        struct Seed {
            nouns: Vec<Entry>,
        }
        let Ok(text) = std::fs::read_to_string(data_dir.join("seed/lexicon.json")) else {
            return Hypernyms::default();
        };
        let Ok(seed) = serde_json::from_str::<Seed>(&text) else {
            return Hypernyms::default();
        };
        let mut parents = HashMap::new();
        for e in seed.nouns {
            let Some(parent) = e.hypernym.filter(|p| !p.is_empty() && *p != e.lemma) else { continue };
            if let Some(plural) = &e.plural {
                parents.insert(plural.to_lowercase(), parent.clone());
            }
            parents.insert(e.lemma.to_lowercase(), parent);
        }
        Hypernyms { parents }
    }

    /// `dog` -> `[animal, entity]`.
    fn chain(&self, noun: &str) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        let mut current = noun.to_lowercase();
        while out.len() < MAX_HYPERNYM_HOPS {
            let Some(parent) = self.parents.get(&current) else { break };
            if out.contains(parent) || parent == &noun.to_lowercase() {
                break;
            }
            out.push(parent.clone());
            current = parent.clone();
        }
        out
    }
}

// ---------------------------------------------------------------------------
// What a turn is about
// ---------------------------------------------------------------------------

/// A term the turn asks or asserts the identity of. Anything else in the
/// sentence is a participant: nobody needs to look up `Sandra` to store that
/// Sandra moved to the garden.
#[derive(Debug, Clone, PartialEq)]
pub struct Topic {
    pub term: String,
    /// The turn asked what this is, rather than telling us something about it.
    pub asked: bool,
}

/// Subjects and complements of a copula, plus the noun a wh-question is about.
pub fn topics(clauses: &[Clause]) -> Vec<Topic> {
    let mut out: Vec<Topic> = vec![];
    for clause in clauses {
        let asked = matches!(clause.act, Act::Question { .. });
        let refs: Vec<&Referent> = clause.referents.iter().chain(clause.then_referents.iter()).collect();
        for pred in clause.conditions.iter().filter(|p| p.pred == "be") {
            for arg in &pred.args {
                let Term::Var { var } = arg else { continue };
                let Some(r) = refs.iter().find(|r| &r.var == var) else { continue };
                let term = match (&r.quant, &r.noun) {
                    (Quant::Named(name), _) => name.clone(),
                    (_, Some(noun)) => singularize_noun(noun),
                    _ => continue,
                };
                if term.is_empty() || out.iter().any(|t| t.term.eq_ignore_ascii_case(&term)) {
                    continue;
                }
                out.push(Topic { term, asked });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The chain
// ---------------------------------------------------------------------------

/// What one lookup stored, in English, for the trace and the reply.
pub struct Found {
    pub source: &'static str,
    pub sentences: Vec<String>,
}

impl Brain {
    /// Steps 1 and 2. Runs the offline source for every topic, then the
    /// network source for the ones that still have nowhere to sit and that
    /// the turn is really about. Returns the topics nobody could place.
    pub(super) fn lookup_topics(
        &self,
        clauses: &[Clause],
        unknown_words: &[String],
        trace: &mut Vec<String>,
    ) -> Vec<Topic> {
        let mut unplaced: Vec<Topic> = vec![];
        for topic in topics(clauses).into_iter().take(MAX_TERMS_PER_TURN) {
            if self.knows_class(&topic.term) {
                continue;
            }
            if let Some(found) = self.from_wordnet(&topic.term) {
                trace.push(format!("lookup {}: {} ({})", topic.term, found.sentences.join(" "), found.source));
                continue;
            }
            let novel = topic.asked || unknown_words.iter().any(|w| w.eq_ignore_ascii_case(&topic.term));
            if !novel {
                continue;
            }
            if let Some(found) = self.from_wikidata(&topic.term) {
                trace.push(format!("lookup {}: {} ({})", topic.term, found.sentences.join(" "), found.source));
                continue;
            }
            unplaced.push(topic);
        }
        unplaced
    }

    /// True when memory already says what this term is a kind of.
    fn knows_class(&self, term: &str) -> bool {
        let can = self.can.lock();
        let store = self.store.lock();
        let concept = concept_of_noun(&can, term);
        !class_parents(&can, &store, &concept).is_empty()
    }

    /// Step 1: the hypernym chain in the seed lexicon.
    fn from_wordnet(&self, term: &str) -> Option<Found> {
        let chain = self.hypernyms.chain(&singularize_noun(term));
        if chain.is_empty() {
            return None;
        }
        let mut child = singularize_noun(term);
        let mut sentences = vec![];
        for parent in chain {
            if let Some(s) = self.store_class(&child, &parent, "wordnet") {
                sentences.push(s);
            }
            child = parent;
        }
        (!sentences.is_empty()).then_some(Found { source: "wordnet", sentences })
    }

    /// Step 2: the top Wikidata hit, reduced to the one thing SCE can carry.
    fn from_wikidata(&self, term: &str) -> Option<Found> {
        if self.cfg.offline {
            return None;
        }
        let id = ActionId("know.wikidata_describe".into());
        // A lookup nobody asked for must not stop to ask permission.
        if self.cfg.permission_mode.needs_confirmation(Effect::Network) {
            return None;
        }
        let described = {
            let can = self.can.lock();
            let host = BrainHost::memoryless(self.cfg.permission_mode);
            let mut ctx = Ctx::new(&can, &self.kernel, &host);
            self.kernel.call(&mut ctx, &id, &[Value::Name(term.to_string())]).ok()?
        };
        let parent = head_noun(described.as_str()?)?;
        let sentence = self.store_class(&singularize_noun(term), &parent, "wikidata")?;
        Some(Found { source: "wikidata", sentences: vec![sentence] })
    }

    /// Step 3: the Teacher seat. Only reached when the first two found
    /// nothing and the turn really was about the term.
    pub(super) async fn lookup_with_teacher(&self, topic: &Topic, context: &str) -> Option<Found> {
        let teacher = self.teacher.as_ref()?;
        let known: Vec<String> = self.can.lock().concepts().map(|c| c.id.0.clone()).take(30).collect();
        let model = teacher.concept_for(&topic.term, context, &known).await.ok()?;
        let child = singularize_noun(&topic.term);
        let sentences: Vec<String> = model
            .extends
            .iter()
            .map(|p| pascal_to_noun(&p.0))
            .filter(|p| p != &child && p != "thing")
            .filter_map(|parent| self.store_class(&child, &parent, "teacher"))
            .collect();
        (!sentences.is_empty()).then_some(Found { source: "teacher", sentences })
    }

    /// Ask the question again now that memory knows more. `None` means the
    /// answer did not change, so the original reply stands.
    pub(super) fn answer_again(&self, session_id: &str, clauses: &[Clause]) -> Option<ResponsePlan> {
        let questions: Vec<Clause> =
            clauses.iter().filter(|c| matches!(c.act, Act::Question { .. })).cloned().collect();
        if questions.is_empty() {
            return None;
        }
        let dispatched = {
            let mut sessions = self.sessions.lock();
            let session = sessions.entry(session_id.to_string()).or_insert_with(Session::new);
            let mut can = self.can.lock();
            let store = self.store.lock();
            let host = BrainHost::memoryless(self.cfg.permission_mode);
            let grounded = discourse::ground_all(&mut session.discourse, &questions, &can);
            let mut dctx = DispatchCtx {
                can: &mut can,
                store: &store,
                kernel: &self.kernel,
                host: &host,
                session_id,
                episode_id: None,
                returning_user: true,
            };
            dispatch::dispatch_turn(&mut dctx, &grounded, &session.discourse).ok()?
        };
        match dispatched {
            Dispatched::Moves(moves)
                if moves.iter().any(|m| matches!(m, Move::Answer { .. } | Move::YesNo { .. })) =>
            {
                Some(ResponsePlan::new(moves))
            }
            _ => None,
        }
    }

    /// Write one concept-level `is_a` and return it as English, or `None` if
    /// memory already had it.
    fn store_class(&self, child: &str, parent: &str, source: &str) -> Option<String> {
        let mut can = self.can.lock();
        let store = self.store.lock();
        let mut fw = FactWriter { can: &mut can, store: &store };
        discourse::assert_class_is_a(&mut fw, child, parent, source).ok()??;
        Some(format!("Every {child} is {} {parent}.", article(parent)))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The one noun in a Wikidata description SCE can carry.
///
/// Descriptions are noun phrases with a tail: "2012 film by Joss Whedon",
/// "species of mammal", "superhero team appearing in American comic books".
/// The head is the last plain noun before the first preposition or comma.
fn head_noun(description: &str) -> Option<String> {
    if description.trim().is_empty() || description.trim() == "no results" {
        return None;
    }
    let body = description.split_once(':').map(|(_, rest)| rest).unwrap_or(description);
    let mut head: Option<String> = None;
    for raw in body.split(|c: char| c == ',' || c == '(' || c.is_whitespace()) {
        let word = raw.trim().to_lowercase();
        if word.is_empty() {
            continue;
        }
        if matches!(
            word.as_str(),
            "of" | "by" | "in" | "from" | "with" | "for" | "at" | "on" | "to" | "and" | "or" | "that" | "which"
        ) {
            break;
        }
        if word.ends_with("ing") || !word.chars().all(|c| c.is_ascii_alphabetic()) {
            continue;
        }
        head = Some(word);
    }
    head.filter(|h| h.len() > 2)
}

fn article(noun: &str) -> &'static str {
    if noun.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    }
}

/// `SuperHero` -> `super-hero`, the shape SCE can carry.
fn pascal_to_noun(id: &str) -> String {
    let mut out = String::new();
    for (i, c) in id.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('-');
        }
        out.extend(c.to_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hypernym_chain_walks_up() {
        let h = Hypernyms::load(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().join("data").as_path());
        let chain = h.chain("dog");
        assert_eq!(chain.first().map(String::as_str), Some("animal"), "chain {chain:?}");
        assert!(chain.contains(&"entity".to_string()), "chain {chain:?}");
        assert!(h.chain("zxqv").is_empty());
        // Plurals reach the same parent.
        assert_eq!(h.chain("dogs").first().map(String::as_str), Some("animal"));
    }

    #[test]
    fn wikidata_descriptions_reduce_to_one_noun() {
        assert_eq!(head_noun("The Avengers: 2012 film by Joss Whedon").as_deref(), Some("film"));
        assert_eq!(head_noun("Avengers: superhero team appearing in American comic books").as_deref(), Some("team"));
        assert_eq!(head_noun("dog: species of mammal").as_deref(), Some("species"));
        assert_eq!(head_noun("no results"), None);
    }

    #[test]
    fn pascal_ids_become_nouns() {
        assert_eq!(pascal_to_noun("SuperHero"), "super-hero");
        assert_eq!(pascal_to_noun("Movie"), "movie");
    }
}
