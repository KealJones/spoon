//! Teacher consultation and capability acquisition.

use std::collections::HashSet;

use super::*;

#[derive(Default)]
pub(super) struct Teaching {
    pub notes: Vec<String>,
    pub exchanges: Vec<spoon_seat::Exchange>,
    pub reading: Option<(Vec<Concept>, i64)>,
}

impl Brain {
    pub(super) async fn consult_teacher(
        &mut self,
        text: &str,
        heard: &Heard,
        gaps: &[Concept],
        metrics: &mut TurnMetrics,
        feedback: Option<&str>,
    ) -> spoon_store::Result<Teaching> {
        let mut learning = Vec::new();
        let mut teacher_exchanges = Vec::new();
        let Some(teacher) = &self.teacher else {
            return Ok(Teaching::default());
        };
        // The same ranked vocabulary the ears get. A Teacher that does not know
        // what Spoon already has will refuse work it could have done: asked to
        // build string reversal without being told `chars` exists, it correctly
        // reports that nothing turns a string into a list.
        let vocabulary = self.vocabulary();
        let mut asks = Vec::new();
        // Ask about the reading first. When a turn goes wrong the cause is
        // often how it was heard rather than a capability that is missing, and
        // fixing a capability to serve a misreading builds the wrong thing
        // carefully. It is also the only answer that compounds: a corrected
        // reading becomes a phrasing the native path reuses for free.
        if !heard.steps.is_empty() || feedback.is_some() {
            asks.push(TeacherAsk::Reading {
                utterance: Arc::from(text),
                heard: Arc::from(
                    heard
                        .steps
                        .iter()
                        .map(|s| pycall::render_pycall(s, &self.symbols))
                        .collect::<Vec<_>>()
                        .join("\n")
                        .as_str(),
                ),
                trouble: Arc::from(
                    if let Some(feedback) = feedback {
                        feedback.to_string()
                    } else if gaps.is_empty() {
                        "nothing could be worked out from it".to_string()
                    } else {
                        format!(
                            "nothing realizes {}",
                            gaps.iter()
                                .map(|g| pycall::render_pycall(g, &self.symbols))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                    .as_str(),
                ),
            });
        }
        if gaps.is_empty() {
            for word in &heard.unknown {
                asks.push(TeacherAsk::Vocabulary {
                    word: word.clone(),
                    utterance: Arc::from(text),
                    position: Arc::from("unknown"),
                });
            }
        }
        for gap in gaps.iter().take(2) {
            asks.push(TeacherAsk::Capability {
                concept: gap.clone(),
                attempted: self.existing_forms(gap),
                utterance: Arc::from(text),
                unknown: heard.unknown.clone(),
            });
        }

        // Reading first, then one teaching ask per gap. The Teacher may mint
        // several concepts and compose them in that one lesson. Examples, if
        // it offers them, are handed to the synthesizer. They are not a gate
        // on keeping the lesson.
        const MAX_ASKS: usize = 6;
        for ask in asks.into_iter().take(MAX_ASKS) {
            let Ok(taught) = teacher.teach(&ask, &vocabulary).await else {
                continue;
            };
            metrics.teacher_calls += 1;
            if let Some(ex) = taught.exchange {
                teacher_exchanges.push(ex);
            }
            if std::env::var("SPOON_DEBUG").is_ok() {
                eprintln!("[teacher] {ask:?}\n      -> {:?}", taught.replies);
            }
            let subject = subject_of(&ask);
            let mut lesson = Vec::new();
            for reply in taught.replies {
                match reply {
                    TeacherReply::Reading {
                        steps,
                        lesson: rule,
                    } => {
                        if steps.is_empty() || steps == heard.steps {
                            continue;
                        }
                        let original = if let TeacherAsk::Reading { heard, .. } = &ask {
                            heard.to_string()
                        } else {
                            "?".to_string()
                        };
                        let corrected: Vec<String> =
                            steps.iter().map(|s| render(s, &self.symbols)).collect();
                        learning.push(format!(
                            "corrected reading: {} -> {}",
                            original,
                            corrected.join("; ")
                        ));
                        let pair = self.remember_pair(text, &steps, PairSource::Confirmed)?;
                        if let Some(rule) = rule {
                            learning.push(format!("new rule for reading: {rule}"));
                            let stored = Concept::call("ears-rule", [Concept::text(&*rule)]);
                            let _ = self.store.assert_concept(
                                &stored,
                                Provenance::Teacher { episode: None },
                                None,
                                None,
                            );
                        }
                        return Ok(Teaching {
                            notes: learning,
                            exchanges: teacher_exchanges,
                            reading: Some((steps, pair)),
                        });
                    }
                    other => lesson.push(other),
                }
            }
            learning.extend(self.absorb_lesson(lesson, subject.as_ref()));
        }

        Ok(Teaching {
            notes: learning,
            exchanges: teacher_exchanges,
            reading: None,
        })
    }

    /// Land a whole lesson: new concepts first, then compositions, then any
    /// examples for the synthesizer.
    fn absorb_lesson(&self, lesson: Vec<TeacherReply>, subject: Option<&Concept>) -> Vec<String> {
        let mut notes = Vec::new();
        let mut minted = HashSet::new();
        let mut compositions = Vec::new();
        let mut specs = Vec::new();
        for reply in lesson {
            match reply {
                TeacherReply::NewConcept { ref concept, .. } => {
                    if let Some(sym) = concept.as_symbol() {
                        minted.insert(sym);
                    }
                    notes.push(format!("taught concept {}", render(concept, &self.symbols)));
                    self.absorb(reply, None);
                }
                TeacherReply::Synonym { ref word, .. } => {
                    notes.push(format!("taught synonym {word}"));
                    self.absorb(reply, None);
                }
                TeacherReply::Composition { target, body } => {
                    compositions.push((target, body));
                }
                TeacherReply::Spec(spec) => specs.push(spec),
                TeacherReply::Unknown { why } => {
                    notes.push(format!("teacher had nothing: {why}"));
                }
                TeacherReply::Reading { .. } => {}
            }
        }

        let mut bound_subject = false;
        for (proposed, body) in compositions {
            let target = bind_compose_target(proposed, subject, &minted, &mut bound_subject);
            notes.push(format!(
                "taught {} = {}",
                render(&target, &self.symbols),
                render(&body, &self.symbols)
            ));
            let matched = specs.iter().find(|s| {
                s.target.as_symbol() == target.as_symbol()
                    || s.target.as_symbol() == subject.and_then(Concept::as_symbol)
            });
            if let Some(spec) = matched
                && !self.verify(&body, spec)
            {
                notes.push(
                    "that composition missed its own examples; kept it anyway as meaning"
                        .to_string(),
                );
            }
            self.absorb(TeacherReply::Composition { target, body }, None);
        }

        for spec in specs {
            let spec = Spec {
                target: bind_compose_target(spec.target, subject, &minted, &mut bound_subject),
                ..spec
            };
            if let Some(found) = self.learn_from_spec(&spec) {
                notes.push(found);
            }
        }
        notes
    }

    /// Take what the Teacher said and make it part of Spoon.
    ///
    /// Everything lands as an ordinary stored concept, at provisional tier: the
    /// Teacher proposes, experience decides. Nothing it says is trusted enough
    /// to arrive as kernel.
    ///
    /// `subject` is the concept the question was about, when there was one. The
    /// Teacher names its own answers, and asked how to reverse a string it will
    /// propose `reverse-text` while the concept that actually failed was
    /// `reverse`. Storing it under the invented name produces a correct
    /// realization that nothing ever calls, because no utterance will ever
    /// mention it. Binding the answer to the concept that failed is what closes
    /// the loop.
    pub(super) fn absorb(&self, reply: TeacherReply, subject: Option<Concept>) {
        let now = Utc::now();
        match reply {
            TeacherReply::Synonym { word, concept, .. } => {
                let claim = Concept::call("synonym", [Concept::text(&*word), concept]);
                let _ = self.store.assert_concept(
                    &claim,
                    Provenance::Teacher { episode: None },
                    None,
                    None,
                );
            }
            TeacherReply::NewConcept {
                concept,
                relations,
                surface_forms,
            } => {
                let meta = ConceptMeta::new(
                    concept.clone(),
                    Provenance::Teacher { episode: None },
                    Tier::Provisional,
                    now,
                )
                .with_surface_forms(surface_forms.iter().map(|s| s.as_ref()));
                let _ = self.store.put_meta(&meta);
                for relation in relations {
                    let _ = self.store.assert_concept(
                        &relation,
                        Provenance::Teacher { episode: None },
                        None,
                        None,
                    );
                }
            }
            TeacherReply::Composition { target, body } => {
                let target = retarget(target, subject.as_ref());
                // The Teacher proposes; something has to verify. A composition
                // arrives unchecked, and one that calls its own target without
                // a base case recurses until a budget stops it, turning a
                // single request into hundreds of failed steps. The Teacher
                // offered exactly that here: `reverse` defined as
                // `reverse<chars<?0>>`.
                //
                // A body may legitimately use other realizations of its own
                // target, which is how reversing text builds on reversing a
                // list, but only with something between the two calls. A body
                // whose outermost application is its own target has nothing in
                // between and cannot terminate.
                if body.head_symbol() == target.as_symbol() {
                    return;
                }
                let realization = spoon_concept::Realization {
                    target,
                    name: format!("taught-{}", now.timestamp_millis()).into(),
                    spec: spoon_concept::RealizationSpec::Composed { body },
                    effect: spoon_concept::Effect::Pure,
                    activation: spoon_concept::Activation::new(now),
                    provenance: Provenance::Teacher { episode: None },
                    tier: Tier::Provisional,
                };
                let _ = self.store.put_realization(&realization);
            }
            // A spec is examples, and examples are searchable. This is the
            // path that turns "I cannot do that" into something Spoon can do.
            TeacherReply::Spec(spec) => {
                let spec = Spec {
                    target: retarget(spec.target, subject.as_ref()),
                    ..spec
                };
                self.learn_from_spec(&spec);
            }
            // Handled where the reply arrives, since it is kept as a phrasing
            // rather than absorbed as knowledge.
            TeacherReply::Reading { .. } => {}
            // An admitted blank is a real answer. Nothing to store yet, but it
            // is worth not treating as a failure.
            TeacherReply::Unknown { .. } => {}
        }
    }

    /// Does this body actually do what the examples say?
    ///
    /// The same check synthesis applies to every candidate it considers, run
    /// once against the Teacher's guess. Verification is cheap where search is
    /// not, which is what makes accepting a body too large to find safe.
    pub(super) fn verify(&self, body: &Concept, spec: &Spec) -> bool {
        if spec.examples.len() < 3 {
            return false;
        }
        spec.examples.iter().all(|(inputs, expected)| {
            let term = spoon_concept::substitute_positional(body, inputs);
            let mut evaluator = Evaluator::new(&self.store, &self.registry)
                .with_budget(self.config.eval_budget)
                .with_permission(PermissionMode::AlwaysAsk);
            matches!(evaluator.evaluate(&term), Outcome::Value(v) if v == *expected)
        })
    }

    /// Search for a body satisfying the Teacher's examples, and keep it if one
    /// exists.
    ///
    /// The Teacher proposes; the synthesizer verifies. That separation is why a
    /// model is allowed near this at all: nothing it says is trusted, only its
    /// examples are, and a body that fails one of them is discarded.
    pub(super) fn learn_from_spec(&self, spec: &Spec) -> Option<String> {
        if spec.examples.is_empty() {
            return None;
        }
        // Larger than the default size cap, because the bodies worth learning
        // from a conversation are a little bigger than the ones worth testing.
        // Reversing text is `join<reverse<chars<?0>>, "">`, which is eight
        // nodes, and a cap of seven puts it permanently out of reach while
        // reporting an honest "searched everything, found nothing". The time
        // budget is what actually bounds the cost.
        let budget = SynthBudget {
            max_size: 9,
            max_millis: 8_000,
            ..SynthBudget::default()
        };
        let outcome = synthesize(spec, &self.store, &self.registry, budget);
        let SynthOutcome::Found { body, size, .. } = outcome else {
            return None;
        };
        let now = Utc::now();
        let realization = spoon_concept::Realization {
            target: spec.target.clone(),
            name: format!("synth-{}", now.timestamp_millis()).into(),
            spec: spoon_concept::RealizationSpec::Composed { body },
            effect: spoon_concept::Effect::Pure,
            activation: spoon_concept::Activation::new(now),
            // Provisional: it fits the examples it was shown, which is evidence
            // rather than proof. Use decides the rest.
            provenance: Provenance::Synthesized { episode: None },
            tier: Tier::Provisional,
        };
        let _ = self.store.put_realization(&realization);
        Some(format!(
            "the synthesizer found {} in {size} nodes, verified on {} examples",
            render(&spec.target, &self.symbols),
            spec.examples.len()
        ))
    }
}

/// Bind a taught composition onto the concept that actually failed, unless
/// this line is introducing a helper the Teacher just minted.
fn bind_compose_target(
    proposed: Concept,
    subject: Option<&Concept>,
    minted: &HashSet<spoon_concept::SymbolId>,
    bound_subject: &mut bool,
) -> Concept {
    if proposed.as_symbol().is_some_and(|s| minted.contains(&s)) {
        return proposed;
    }
    let Some(actual) = subject else {
        return proposed;
    };
    if !actual.is_named() {
        return proposed;
    }
    if proposed.as_symbol() == actual.as_symbol() {
        *bound_subject = true;
        return proposed;
    }
    if !*bound_subject {
        *bound_subject = true;
        return actual.clone();
    }
    proposed
}
