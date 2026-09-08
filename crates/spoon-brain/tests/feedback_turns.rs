//! Feedback must change the running brain, not just a helper's counters.
//! Seats are scripted so these tests isolate routing, execution and SQLite.

use async_trait::async_trait;
use spoon_brain::{Brain, BrainConfig, EarsPath, Seats};
use spoon_concept::{Concept, SymbolId};
use spoon_eval::Budget;
use spoon_seat::{
    Ears, Heard, LlmError, Mouth, MouthReply, SeatCounters, Taught, Teacher, TeacherAsk,
    TeacherReply, Turn,
};
use spoon_store::Store;
use std::sync::{Arc, Mutex};

struct Reading;
#[async_trait]
impl Ears for Reading {
    fn hear_native(&self, _: &str) -> Option<Heard> {
        None
    }
    async fn hear(
        &self,
        text: &str,
        _: &[String],
        _: &[Turn],
        _: &[String],
    ) -> Result<Heard, LlmError> {
        let n = text
            .split_whitespace()
            .last()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(7);
        let mut heard = Heard::native(
            vec![Concept::call(
                "ask",
                [Concept::call(
                    "math-add",
                    [Concept::int(n), Concept::int(n)],
                )],
            )],
            0.9,
        );
        heard.names.push(Arc::from("ask"));
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
        // Deliberately no "noted" prefix: presentation is not execution history.
        format!("{c:?}")
    }
}
struct FixReading(Arc<Mutex<Vec<TeacherAsk>>>);
#[async_trait]
impl Teacher for FixReading {
    async fn teach(&self, ask: &TeacherAsk, _: &[Arc<str>]) -> Result<Taught, LlmError> {
        self.0.lock().unwrap().push(ask.clone());
        let reply = match ask {
            TeacherAsk::Reading { utterance, .. } => {
                let n = utterance
                    .split_whitespace()
                    .last()
                    .unwrap()
                    .parse::<i64>()
                    .unwrap();
                TeacherReply::Reading {
                    steps: vec![Concept::call(
                        "ask",
                        [Concept::call(
                            "math-mul",
                            [Concept::int(n), Concept::int(3)],
                        )],
                    )],
                    lesson: None,
                }
            }
            _ => TeacherReply::Unknown {
                why: "no capability missing".into(),
            },
        };
        Ok(Taught {
            reply,
            exchange: None,
        })
    }
}
fn brain(store: Store, teacher: Option<Box<dyn Teacher>>, checks: u32) -> Brain {
    let registry = spoon_natives::bootstrap();
    spoon_natives::seed_bootstrap(&store, &registry).unwrap();
    spoon_infer::seed_meta_rules(&store).unwrap();
    let symbols = Arc::new(store.load_symbol_table().unwrap());
    Brain::new(
        store,
        registry,
        symbols,
        Seats {
            ears: Box::new(Reading),
            mouth: Box::new(PlainMouth),
            teacher,
            counters: Arc::new(SeatCounters::default()),
        },
        BrainConfig {
            check_clean_readings: checks,
            eval_budget: Budget::deterministic(),
            ..BrainConfig::default()
        },
    )
    .unwrap()
}
fn result(turn: &spoon_brain::TurnResult) -> Option<i64> {
    turn.episode.result.as_ref()?.as_ground()?.as_i64()
}

#[tokio::test]
async fn teacher_correction_replaces_a_computable_wrong_reading_immediately() {
    let asks = Arc::new(Mutex::new(Vec::new()));
    let mut b = brain(
        Store::open_in_memory().unwrap(),
        Some(Box::new(FixReading(asks.clone()))),
        1,
    );
    let first = b.turn("s", "shipping for 7").await.unwrap();
    assert_eq!(result(&first), Some(21));
    let next = b.turn("s", "shipping for 14").await.unwrap();
    assert_eq!(result(&next), Some(42));
    assert_eq!(next.episode.ears_path, EarsPath::Native);
    assert_eq!(
        asks.lock().unwrap().len(),
        1,
        "the corrected shape should be reusable immediately"
    );
}

#[tokio::test]
async fn negative_feedback_relearns_the_original_request_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain.db");
    let asks = Arc::new(Mutex::new(Vec::new()));
    {
        let mut b = brain(
            Store::open(&path).unwrap(),
            Some(Box::new(FixReading(asks.clone()))),
            0,
        );
        let wrong = b.turn("s", "shipping for 7").await.unwrap();
        assert_eq!(result(&wrong), Some(14));
        assert_eq!(
            b.store().symbol_name(SymbolId::of("ask")).unwrap().as_deref(),
            Some("ask"),
            "a learned phrasing must persist the names in its stored reading"
        );
        assert!(
            !wrong.episode.realizations.is_empty(),
            "a question must retain the computation's trace"
        );
        let fixed = b.turn("s", "that's wrong").await.unwrap();
        assert_eq!(
            result(&fixed),
            Some(21),
            "feedback must cause a new attempt at the original request"
        );
        let recorded = b.store().corrected_episodes(10).unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            serde_json::from_str::<spoon_brain::Episode>(&recorded[0])
                .unwrap()
                .id,
            wrong.episode.id
        );
        let pairs = b.store().all_pairs(20).unwrap();
        assert!(
            pairs
                .iter()
                .any(|p| p.utterance == "shipping for 7" && p.failures > 0),
            "the rejected reading must lose standing"
        );
        let calls = asks.lock().unwrap();
        assert!(
            matches!(&calls[0], TeacherAsk::Reading { utterance, trouble, .. }
            if &**utterance == "shipping for 7" && trouble.contains("that's wrong") && trouble.contains("14"))
        );
        assert_eq!(fixed.episode.metrics.interior_model_calls, 0);
    }
    let mut reopened = brain(Store::open(&path).unwrap(), None, 0);
    let reused = reopened.turn("s", "shipping for 9").await.unwrap();
    assert_eq!(result(&reused), Some(27));
    assert_eq!(reused.episode.ears_path, EarsPath::Native);
    assert_eq!(reused.episode.metrics.teacher_calls, 0);
}

#[tokio::test]
async fn newly_learned_pairs_keep_their_identity_before_restart() {
    let mut b = brain(Store::open_in_memory().unwrap(), None, 0);
    b.turn("s", "shipping for 7").await.unwrap();
    let again = b.turn("s", "shipping for 8").await.unwrap();
    assert_eq!(again.episode.ears_path, EarsPath::Native);
    b.turn("s", "that's wrong").await.unwrap();
    let pairs = b.store().all_pairs(20).unwrap();
    assert!(pairs.iter().any(|p| p.failures > 0));
    assert_eq!(b.store().corrected_episodes(10).unwrap().len(), 1);
}
