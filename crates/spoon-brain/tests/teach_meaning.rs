//! Teacher may mint several concepts and compose them without examples.
//! The synthesizer is the layer that needs examples. Teaching is meaning.

use async_trait::async_trait;
use spoon_brain::{Brain, BrainConfig, Seats};
use spoon_concept::{Concept, RealizationSpec};
use spoon_eval::Budget;
use spoon_seat::{
    Ears, Heard, LlmError, Mouth, MouthReply, SeatCounters, Taught, Teacher, TeacherAsk,
    TeacherReply, Turn,
};
use spoon_store::Store;
use std::sync::Arc;

struct GapEars;
#[async_trait]
impl Ears for GapEars {
    fn hear_native(&self, _: &str) -> Option<Heard> {
        None
    }
    async fn hear(
        &self,
        _: &str,
        _: &[String],
        _: &[Turn],
        _: &[String],
    ) -> Result<Heard, LlmError> {
        let mut heard = Heard::native(
            vec![Concept::call(
                "do",
                [Concept::call(
                    "most-adorable",
                    [Concept::call(
                        "list-of",
                        [Concept::int(1), Concept::int(3), Concept::int(2)],
                    )],
                )],
            )],
            0.9,
        );
        heard.names.extend(
            ["do", "most-adorable", "list-of"]
                .into_iter()
                .map(Arc::from),
        );
        heard.used_model = true;
        Ok(heard)
    }
}

struct PlainMouth;
#[async_trait]
impl Mouth for PlainMouth {
    async fn say(&self, c: &Concept, _: &[Concept]) -> Result<MouthReply, LlmError> {
        Ok(MouthReply::native(self.say_native(c, &[])))
    }
    fn say_native(&self, c: &Concept, _: &[Concept]) -> String {
        format!("{c:?}")
    }
}

struct LessonTeacher;
#[async_trait]
impl Teacher for LessonTeacher {
    async fn teach(&self, ask: &TeacherAsk, _: &[Arc<str>]) -> Result<Taught, LlmError> {
        let replies = match ask {
            TeacherAsk::Capability { .. } => vec![
                TeacherReply::NewConcept {
                    concept: Concept::named("adorableness-score"),
                    relations: Vec::new(),
                    surface_forms: vec!["cute".into(), "adorable".into()],
                },
                TeacherReply::Composition {
                    target: Concept::named("most-adorable"),
                    body: Concept::call("list-max-of", [Concept::hole(0)]),
                },
            ],
            _ => vec![TeacherReply::Unknown {
                why: "reading is fine".into(),
            }],
        };
        Ok(Taught {
            replies,
            exchange: None,
        })
    }
}

fn brain(store: Store) -> Brain {
    let registry = spoon_natives::bootstrap();
    spoon_natives::seed_bootstrap(&store, &registry).unwrap();
    spoon_infer::seed_meta_rules(&store).unwrap();
    let symbols = Arc::new(store.load_symbol_table().unwrap());
    Brain::new(
        store,
        registry,
        symbols,
        Seats {
            ears: Box::new(GapEars),
            mouth: Box::new(PlainMouth),
            teacher: Some(Box::new(LessonTeacher)),
            counters: Arc::new(SeatCounters::default()),
        },
        BrainConfig {
            check_clean_readings: 0,
            eval_budget: Budget::deterministic(),
            ..BrainConfig::default()
        },
    )
    .unwrap()
}

#[tokio::test]
async fn a_lesson_without_examples_is_kept_and_used() {
    let mut b = brain(Store::open_in_memory().unwrap());
    let turn = b.turn("s", "pick the cutest").await.unwrap();
    let score = b
        .store()
        .get_meta(&Concept::named("adorableness-score"))
        .unwrap();
    assert!(
        score.is_some(),
        "Teacher must be able to mint a helper concept"
    );
    let realized = b
        .store()
        .realizations_for(&Concept::named("most-adorable"))
        .unwrap();
    assert!(
        realized.iter().any(|r| matches!(
            r.spec,
            RealizationSpec::Composed { ref body }
                if body.head_symbol() == Some(spoon_concept::SymbolId::of("list-max-of"))
        )),
        "composition must land without examples; notes were {:?}",
        turn.episode.learning
    );
    let value = turn
        .episode
        .result
        .as_ref()
        .and_then(|c| c.as_ground())
        .and_then(|g| g.as_i64());
    assert_eq!(
        value,
        Some(3),
        "retry should use the taught composition, got {:?}",
        turn.episode.result
    );
}
