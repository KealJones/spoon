//! Hearing and vocabulary retrieval.

use super::*;

impl Brain {
    pub(super) async fn hear(
        &mut self,
        session: &str,
        text: &str,
        metrics: &mut TurnMetrics,
    ) -> (Heard, EarsPath) {
        // Native first, always. Every turn the model does not handle is the
        // weaning curve moving.
        self.last_pair = None;
        if let Some(heard) = self.ears.hear_native(text) {
            metrics.ears_native += 1;
            return (heard, EarsPath::Native);
        }
        // Then whatever this user has said before. A phrasing learned from an
        // earlier turn costs nothing and is the only part of the ears that gets
        // better with use.
        if let Some((steps, confidence, pair)) = self.phrasing.recognize_from(text) {
            metrics.ears_native += 1;
            let mut heard = Heard::native(steps, confidence);
            // A recognized reading can mint names the store has never seen, and
            // an id whose spelling was never recorded prints as hex.
            heard.names = name_shaped_words(text);
            self.last_pair = pair;
            return (heard, EarsPath::Native);
        }
        let vocabulary: Vec<String> = self
            .described_vocabulary()
            .iter()
            .map(Described::line)
            .collect();
        let recent = self.recent_turns(session);
        let rules = self.ears_rules();
        match self.ears.hear(text, &vocabulary, &recent, &rules).await {
            Ok(heard) if only_pleasantries(&heard.steps) && !NativeEars::is_social_only(text) => {
                // The model saw a greeting and dropped the request. "hey
                // reverse spoon lol" is a request with a polite opener, and
                // replying hello to it is worse than saying it was not
                // understood: a greeting looks like success, so nothing is
                // recorded, the Teacher is never asked, and the same sentence
                // fails the same way forever.
                //
                // Whether the sentence is only pleasantries is something the
                // native path already decides without a model, so there is
                // something solid to check the model against.
                metrics.ears_failed += 1;
                (Heard::native(Vec::new(), 0.0), EarsPath::Failed)
            }
            Ok(heard) => {
                metrics.ears_model += 1;
                (heard, EarsPath::Model)
            }
            Err(_) => {
                metrics.ears_failed += 1;
                (Heard::native(Vec::new(), 0.0), EarsPath::Failed)
            }
        }
    }

    /// Rules the Teacher has written about how to read things.
    ///
    /// Ordinary stored concepts, so they survive a restart, appear in the
    /// inspector, and can be retracted like anything else that turns out to be
    /// wrong. Capped because the prompt is working memory: past a point another
    /// rule costs more attention than it buys.
    pub(super) fn ears_rules(&self) -> Vec<String> {
        const MAX_RULES: usize = 12;
        let retired = self.retired_names();
        self.store
            .concepts_by_head(spoon_concept::SymbolId::of("ears-rule"), 64)
            .unwrap_or_default()
            .into_iter()
            .filter(|c| self.store.holds(c).unwrap_or(false))
            .filter_map(|c| {
                c.arg(0)
                    .and_then(Concept::as_ground)
                    .and_then(|g| g.as_str().map(str::to_string))
            })
            // Advice about a capability that no longer exists is worse than no
            // advice: it reads with all the authority of something Spoon
            // learned the hard way, and it steers the ears straight at a head
            // nothing can carry out. Dropping it at read time means rules
            // written before a rename die on their own, with no migration.
            .filter(|rule| !mentions_any(rule, &retired))
            .take(MAX_RULES)
            .collect()
    }

    /// Names that used to mean something and no longer do.
    pub(super) fn retired_names(&self) -> Vec<String> {
        self.store
            .concepts_by_head(spoon_concept::SymbolId::of("retired"), 128)
            .unwrap_or_default()
            .into_iter()
            .filter(|c| self.store.holds(c).unwrap_or(false))
            .filter_map(|c| c.arg(0)?.as_symbol())
            .filter_map(|id| self.symbols.resolve(id).map(|n| n.to_lowercase()))
            .collect()
    }

    /// The last few turns of this session, oldest first.
    ///
    /// Half of ordinary speech refers backwards. "that is called a palindrome"
    /// is not interpretable on its own, and reading each utterance in isolation
    /// is why it came back as a synonym pointing at an unbound hole: the ears
    /// correctly knew something was missing and had no way to find it.
    pub(super) fn recent_turns(&self, session: &str) -> Vec<Turn> {
        const WINDOW: usize = 4;
        let mut turns: Vec<Turn> = self
            .store
            .recent_episodes(WINDOW, Some(session))
            .unwrap_or_default()
            .iter()
            .filter_map(|json| serde_json::from_str::<Episode>(json).ok())
            .map(|e| Turn {
                said: Arc::from(e.user_text.as_str()),
                // What it was read as, not what was said back. The ears are
                // being reminded of their own prior output so a reference can
                // resolve to a concept rather than to prose.
                understood: Arc::from(
                    e.steps
                        .iter()
                        .map(|s| render(s, &self.symbols))
                        .collect::<Vec<_>>()
                        .join("\n")
                        .as_str(),
                ),
            })
            .collect();
        turns.reverse();
        turns
    }

    /// Concept names worth showing the ears, most useful first.
    ///
    /// Ranked by activation, so a brain used for one domain surfaces that
    /// domain's vocabulary and a rarely used concept stops costing prompt
    /// space without being forgotten.
    pub(super) fn vocabulary(&self) -> Vec<Arc<str>> {
        // Retired names keep their meta, which is what makes them still look
        // like vocabulary. Offering one to the ears or the Teacher is how a
        // dead capability gets written back into a durable rule two turns
        // after it was retired.
        let retired = self.retired_names();
        let live: Vec<Arc<str>> = self
            .store
            // Ask for more than will be shown, because the ranking that
            // matters here is not the one activation produces.
            .ranked_surface_forms(self.config.vocabulary_size * 4, Utc::now())
            .unwrap_or_default()
            .into_iter()
            .filter(|name| !retired.iter().any(|r| r == &name.to_lowercase()))
            .collect();

        // Capabilities first, entities with whatever room is left.
        //
        // The ears prompt is working memory and it fills up. After a training
        // run that mentioned a few dozen people, those names outranked the
        // verbs on recency and pushed them out, and the model started failing
        // sentences it had been reading correctly an hour earlier: "make
        // REALIZATION lowercase" came back as a shrug once `lower` was no
        // longer in front of it.
        //
        // The asymmetry is real rather than a heuristic. The ears' job is to
        // choose an operation, and the operations are a small closed set that
        // has to be visible. Entities arrive in the sentence itself and can be
        // read straight off it, so a name absent from the prompt costs much
        // less than a verb absent from it.
        // One query, not one per name. Asking the store about each candidate
        // separately meant several hundred round trips per turn and took the
        // bench from three seconds a case to forty.
        let realized: std::collections::HashSet<_> = self
            .store
            .all_realizations()
            .unwrap_or_default()
            .into_iter()
            .filter(|r| !matches!(r.spec, spoon_concept::RealizationSpec::Rule { .. }))
            .map(|r| r.target.content_id())
            .collect();
        let (capabilities, entities): (Vec<_>, Vec<_>) = live
            .into_iter()
            .partition(|name| realized.contains(&Concept::named(name).content_id()));
        capabilities
            .into_iter()
            .chain(entities)
            .take(self.config.vocabulary_size)
            .collect()
    }

    /// What already realizes this concept, and how each one just fared.
    ///
    /// Answering a gap well needs to know the concept is not missing, only
    /// incomplete. Sent as plain sentences because the Teacher is a language
    /// model and this is the context a person would give.
    pub(super) fn existing_forms(&self, gap: &Concept) -> Vec<Arc<str>> {
        let Some(head) = gap.head() else {
            return Vec::new();
        };
        let mut out: Vec<Arc<str>> = Vec::new();

        for realization in self.store.realizations_for(head).unwrap_or_default() {
            let shape = match &realization.spec {
                spoon_concept::RealizationSpec::Native { native } => self
                    .registry
                    .get(native)
                    .map(|e| format!("{} ({})", e.doc, e.arity.describe()))
                    .unwrap_or_else(|| "a native".to_string()),
                spoon_concept::RealizationSpec::Composed { body } => {
                    format!("built as {}", render(body, &self.symbols))
                }
                other => format!("a {} realization", other.kind().as_str()),
            };
            out.push(Arc::from(
                format!("{} already exists: {shape}", realization.name).as_str(),
            ));
        }

        // The arguments it was actually given, which is what the existing forms
        // could not handle and what a new one has to.
        let given: Vec<String> = gap
            .args()
            .iter()
            .map(|a| render(a, &self.symbols))
            .collect();
        if !given.is_empty() {
            out.push(Arc::from(
                format!(
                    "it was called with {}, which none of those accept",
                    given.join(", ")
                )
                .as_str(),
            ));
        }
        out
    }

    /// The same vocabulary, with each concept's shape and what it does.
    ///
    /// A bare list of names makes the ears guess at how a concept is called,
    /// and every wrong guess turns into a hand-written accommodation somewhere
    /// else: `list<[4, 9, 2, 7]>` where `list<4, 9, 2, 7>` was meant, `reverse`
    /// handed text when it takes a list. Those are one bug wearing different
    /// clothes, and patching each place it surfaces is how a system accumulates
    /// special cases instead of getting better.
    ///
    /// Arity comes from the registry for natives and from the hole count for
    /// learned bodies, since those are the only places that know it.
    pub(super) fn described_vocabulary(&self) -> Vec<Described> {
        self.vocabulary()
            .into_iter()
            .map(|name| {
                let target = Concept::named(&name);
                let native_arity = self
                    .store
                    .realizations_for(&target)
                    .ok()
                    .into_iter()
                    .flatten()
                    .find_map(|r| match &r.spec {
                        spoon_concept::RealizationSpec::Native { native } => {
                            self.registry.get(native).map(|e| e.arity.describe())
                        }
                        spoon_concept::RealizationSpec::Composed { body } => {
                            Some(format!("exactly {} argument(s)", holes(body).len()))
                        }
                        _ => None,
                    });
                let doc = self
                    .store
                    .get_meta(&target)
                    .ok()
                    .flatten()
                    .and_then(|m| m.note.map(|n| n.to_string()));
                Described {
                    name,
                    arity: native_arity,
                    doc,
                }
            })
            .collect()
    }

}
