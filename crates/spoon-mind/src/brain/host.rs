//! Host implementation for the kernel: what `mem.*` primitives see.
//!
//! The host locks the store itself, so the brain must not hold the store lock
//! while the executor runs (see `exec.rs`). Where the brain does hold it
//! (dispatch, which only evaluates arithmetic), it builds a `memoryless` host.

use parking_lot::Mutex;

use spoon_core::kernel::{Host, PermissionMode};
use spoon_core::store::{EpisodeQuery, Store};
use spoon_core::types::*;

pub struct BrainHost<'a> {
    pub permission_mode: PermissionMode,
    store: Option<&'a Mutex<Store>>,
}

impl<'a> BrainHost<'a> {
    pub fn with_store(permission_mode: PermissionMode, store: &'a Mutex<Store>) -> Self {
        BrainHost { permission_mode, store: Some(store) }
    }

    /// For callers that already hold the store lock. `facts`/`recall` are empty.
    pub fn memoryless(permission_mode: PermissionMode) -> Self {
        BrainHost { permission_mode, store: None }
    }
}

fn query_words(query: &str) -> Vec<String> {
    const STOP: &[&str] = &["the", "and", "about", "facts", "what", "who", "does", "did"];
    query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.len() > 2 && !STOP.contains(&w.to_lowercase().as_str()))
        .map(str::to_string)
        .collect()
}

fn capitalize_first(s: &str) -> String {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) => c.to_uppercase().collect::<String>() + cs.as_str(),
        None => String::new(),
    }
}

fn render_fact(f: &Fact) -> String {
    let pred = f.pred.0.rsplit('.').next().unwrap_or(&f.pred.0);
    let args: Vec<String> = f.args.iter().map(Value::render).collect();
    let neg = if f.truth { "" } else { "not " };
    format!("{neg}{pred}({})", args.join(", "))
}

impl Host for BrainHost<'_> {
    fn facts(&self, pred: &ActionId, pattern: &[Option<Value>]) -> Vec<Fact> {
        match self.store {
            Some(store) => store.lock().query_facts(pred, pattern).unwrap_or_default(),
            None => vec![],
        }
    }

    fn recall(&self, query: &str, limit: usize) -> Vec<String> {
        let Some(store) = self.store else { return vec![] };
        let store = store.lock();
        let words = query_words(query);
        let is_a = ActionId("rel.is_a".into());
        let mut out: Vec<String> = Vec::new();
        for w in &words {
            // The word as a name (John), then every entity of that concept (dog -> dog_1).
            let mut entities = vec![Value::Name(w.clone())];
            let concept = Value::Name(capitalize_first(w));
            for f in store.query_facts(&is_a, &[None, Some(concept)]).unwrap_or_default() {
                if let Some(e) = f.args.first() {
                    entities.push(e.clone());
                }
            }
            for e in &entities {
                for f in store.facts_about(e).unwrap_or_default() {
                    out.push(render_fact(&f));
                }
            }
        }
        let q = EpisodeQuery { session_id: None, keywords: &words, since_ms: None, limit };
        for ep in store.episodes(&q).unwrap_or_default() {
            out.push(format!("you said \"{}\", i said \"{}\"", ep.user_text, ep.reply_text));
        }
        let mut seen = std::collections::HashSet::new();
        out.retain(|s| seen.insert(s.clone()));
        out.truncate(limit);
        out
    }

    fn now_ms(&self) -> i64 {
        now_ms()
    }

    fn permission_mode(&self) -> PermissionMode {
        self.permission_mode
    }
}
