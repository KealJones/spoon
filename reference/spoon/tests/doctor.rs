//! Tests for `spoon doctor` and `/debug/health`.

use std::sync::Arc;

use spoon::doctor::{
    analyse, analyse_with_context, AnalyseContext, DoctorArgs,
};
use spoon::doctor::detect::{
    detect_discourse_lead_in, detect_normalizer_inversion, detect_typo_repair_damage,
};
use spoon::doctor::report::input_shape;
use spoon::server::router;
use spoon_core::types::{EarsPath, Episode, Move, ResponsePlan, TurnMetrics};
use spoon_mind::brain::{Brain, BrainConfig};

// ---- episode builder -------------------------------------------------------

fn ep(
    user_text: &str,
    sce: &str,
    ears_path: Option<EarsPath>,
    moves: Vec<Move>,
    reply_text: &str,
    synthesis_attempted: bool,
    synthesis_succeeded: bool,
) -> Episode {
    Episode {
        id: 0,
        session_id: "test".into(),
        at: 0,
        user_text: user_text.into(),
        sce: sce.into(),
        clauses: vec![],
        plans: vec![],
        response: ResponsePlan::new(moves),
        reply_text: reply_text.into(),
        metrics: TurnMetrics {
            ears_path,
            interior_llm_calls: 0,
            ears_llm_calls: 0,
            mouth_llm_calls: 0,
            teacher_llm_calls: 0,
            plan_steps: 0,
            synthesis_attempted,
            synthesis_succeeded,
            teacher_fallback: false,
            reused_learned_action: false,
            ms_ears: 0,
            ms_interior: 0,
            ms_mouth: 0,
        },
        credit: 0,
        keywords: vec![],
        build_id: String::new(),
    }
}

fn ep_with_build(base: Episode, build: &str) -> Episode {
    Episode { build_id: build.into(), ..base }
}

fn success_ep(text: &str) -> Episode {
    ep(text, "User greets Assistant.", Some(EarsPath::Direct), vec![], "Hello!", false, false)
}

fn honest_unknown_ep(text: &str) -> Episode {
    ep(
        text,
        "User asks what Blorp is.",
        Some(EarsPath::Direct),
        vec![],
        "I don't know the answer to that.",
        false,
        false,
    )
}

fn failed_with_sce(user: &str, sce: &str) -> Episode {
    ep(user, sce, Some(EarsPath::Failed), vec![], "Hmm.", false, false)
}

fn failed_no_sce(user: &str) -> Episode {
    ep(user, "", Some(EarsPath::Failed), vec![], "Didn't get that.", false, false)
}

fn clarify_ep(text: &str) -> Episode {
    ep(
        text,
        "User rephrases.",
        Some(EarsPath::Llm),
        vec![Move::Clarify { question: "Which?".into(), options: vec![], slot_type: None }],
        "Which do you mean?",
        false,
        false,
    )
}

fn synth_failed_ep(text: &str) -> Episode {
    ep(text, "User commands.", Some(EarsPath::Direct), vec![], "Tried and failed.", true, false)
}

// ============================================================================
// 1. Detector unit tests
// ============================================================================

#[test]
fn detector_normalizer_inversion_imperative() {
    let sce = "Assistant, get User all the titles from https://example.com!";
    assert!(detect_normalizer_inversion(sce).is_some(), "should detect inversion");
}

#[test]
fn detector_normalizer_inversion_for_user() {
    let sce = "Not much, could Assistant summarize /path for User?";
    assert!(detect_normalizer_inversion(sce).is_some(), "should detect 'for user' inversion");
}

#[test]
fn detector_normalizer_inversion_user_subject_is_not_inversion() {
    let sce = "User goes to see the avengers move tomorrow.";
    assert!(
        detect_normalizer_inversion(sce).is_none(),
        "User as subject must NOT trigger inversion"
    );
}

#[test]
fn detector_normalizer_inversion_no_assistant_no_inversion() {
    let sce = "The avengers are fictional super heres.";
    assert!(detect_normalizer_inversion(sce).is_none());
}

#[test]
fn detector_typo_repair_damage_movie_to_move() {
    let user = "im going to see the avengers movie tomorrow";
    let sce = "User goes to see the avengers move tomorrow.";
    let r = detect_typo_repair_damage(user, sce);
    assert!(r.is_some(), "should detect movie -> move");
    let detail = r.unwrap();
    assert!(detail.contains("movie") && detail.contains("move"), "detail: {detail}");
}

#[test]
fn detector_typo_repair_damage_heros_to_heres() {
    let user = "The Avengers are fictional super heros";
    let sce = "The avengers are fictional super heres.";
    let r = detect_typo_repair_damage(user, sce);
    assert!(r.is_some(), "should detect heros -> heres");
}

#[test]
fn detector_typo_repair_damage_no_false_positive_normal_rewrite() {
    let user = "im going somewhere";
    let sce = "User goes somewhere.";
    let r = detect_typo_repair_damage(user, sce);
    assert!(r.is_none(), "normal conjugation rewrite must not fire: {:?}", r);
}

#[test]
fn detector_discourse_lead_in_not_much() {
    assert!(detect_discourse_lead_in("Not much, could Assistant summarize something?"));
}

#[test]
fn detector_discourse_lead_in_well() {
    assert!(detect_discourse_lead_in("Well, I think the answer is yes."));
}

#[test]
fn detector_discourse_lead_in_clean_sce_is_not_flagged() {
    assert!(!detect_discourse_lead_in("User asks what the weather is."));
    assert!(!detect_discourse_lead_in("Assistant, please close the door."));
}

// ============================================================================
// 2. Clustering: same symptom, different wording -> one cluster
// ============================================================================

#[test]
fn two_different_wording_same_symptom_land_in_one_cluster() {
    let episodes = vec![
        failed_with_sce(
            "can you get me all the titles from https://example.com",
            "Assistant, get User all the titles from https://example.com!",
        ),
        failed_with_sce(
            "please fetch the data from https://api.test.com/items",
            "Assistant, fetch User the data from https://api.test.com/items!",
        ),
    ];
    let r = analyse(&episodes);
    assert_eq!(r.failures, 2);
    let bucket = r.buckets.iter().find(|b| b.bucket == "ears_didnt_reparse").expect("bucket");
    assert_eq!(bucket.clusters.len(), 1, "both should collapse into one cluster: {:?}", bucket.clusters);
    assert_eq!(bucket.clusters[0].symptom, "normalizer_inversion");
    assert_eq!(bucket.clusters[0].count, 2);
}

#[test]
fn real_db_pattern_maps_to_two_clusters() {
    let episodes = vec![
        failed_with_sce(
            "im going to see the avengers movie tomorrow",
            "User goes to see the avengers move tomorrow.",
        ),
        failed_with_sce(
            "not much, could you summarize /Users/foo/README.md for me?",
            "Not much, could Assistant summarize /Users/foo/README.md for User?",
        ),
        failed_with_sce(
            "can you get me all the titles from https://jsonplaceholder.typicode.com/todos",
            "Assistant, get User all the titles from https://jsonplaceholder.typicode.com/todos!",
        ),
        failed_with_sce(
            "can you get me all of the \"title\" properties from https://jsonplaceholder.typicode.com/todos",
            "Assistant, get User all of the \"title\" properties from https://jsonplaceholder.typicode.com/todos!",
        ),
        failed_with_sce(
            "The Avengers are fictional super heros",
            "The avengers are fictional super heres.",
        ),
    ];
    let r = analyse(&episodes);
    assert_eq!(r.failures, 5);
    let bucket = r.buckets.iter().find(|b| b.bucket == "ears_didnt_reparse").expect("bucket");

    assert_eq!(
        bucket.clusters.len(),
        2,
        "5 failures must collapse to 2 clusters, got: {:?}",
        bucket.clusters.iter().map(|c| (&c.symptom, c.count)).collect::<Vec<_>>()
    );

    let inv = bucket.clusters.iter().find(|c| c.symptom == "normalizer_inversion").expect("inversion cluster");
    assert_eq!(inv.count, 3, "3 normalizer_inversion cases");

    let typo = bucket.clusters.iter().find(|c| c.symptom == "typo_repair_damage").expect("typo cluster");
    assert_eq!(typo.count, 2, "2 typo_repair_damage cases");

    assert_eq!(bucket.clusters[0].symptom, "normalizer_inversion");
}

// ============================================================================
// 3. Honest unknown is not a failure
// ============================================================================

#[test]
fn honest_unknown_excluded_from_failures() {
    let episodes = vec![honest_unknown_ep("what is Blorp?"), success_ep("hi")];
    let r = analyse(&episodes);
    assert_eq!(r.failures, 0);
    assert_eq!(r.honest_unknowns, 1);
    assert_eq!(r.failure_rate_pct, 0.0);
}

// ============================================================================
// 4. Failure rate denominator
// ============================================================================

#[test]
fn failure_rate_computed_over_total() {
    let episodes = vec![
        success_ep("hi"),
        honest_unknown_ep("what is x?"),
        failed_no_sce("zzz zzz"),
        failed_no_sce("zzz yyy"),
    ];
    let r = analyse(&episodes);
    assert_eq!(r.total, 4);
    assert_eq!(r.failures, 2);
    assert_eq!(r.honest_unknowns, 1);
    let expected = 2.0 / 4.0 * 100.0;
    assert!((r.failure_rate_pct - expected).abs() < 0.01);
}

// ============================================================================
// 5. All non-SCE bucket types
// ============================================================================

#[test]
fn all_bucket_types_classified() {
    let episodes = vec![
        success_ep("hi"),
        honest_unknown_ep("what is foo?"),
        failed_no_sce("zzqxzzqx"),
        failed_with_sce(
            "can you help",
            "Assistant, help User do something for User.",
        ),
        clarify_ep("do the thing"),
        synth_failed_ep("compute blorp"),
    ];
    let r = analyse(&episodes);
    assert_eq!(r.honest_unknowns, 1);
    assert_eq!(r.failures, 4);
    let names: Vec<&str> = r.buckets.iter().map(|b| b.bucket.as_str()).collect();
    assert!(names.contains(&"ears_no_parse"), "{names:?}");
    assert!(names.contains(&"ears_didnt_reparse"), "{names:?}");
    assert!(names.contains(&"asked_to_rephrase"), "{names:?}");
    assert!(names.contains(&"synthesis_failed"), "{names:?}");
}

// ============================================================================
// 6. Interior LLM violation
// ============================================================================

#[test]
fn interior_llm_violation_detected() {
    let mut bad = success_ep("hello");
    bad.metrics.interior_llm_calls = 1;
    let r = analyse(&[bad]);
    assert!(r.interior_llm_violation);
    assert_eq!(r.weaning.interior_llm_calls, 1);
}

#[test]
fn no_violation_when_clean() {
    let r = analyse(&[success_ep("hello"), success_ep("bye")]);
    assert!(!r.interior_llm_violation);
}

// ============================================================================
// 7. JSON roundtrip
// ============================================================================

#[test]
fn json_roundtrip() {
    let r = analyse(&[success_ep("hi"), failed_no_sce("zzqzxq")]);
    let s = serde_json::to_string_pretty(&r).expect("serialize");
    let v: serde_json::Value = serde_json::from_str(&s).expect("parse");
    assert!(v["total"].as_u64().is_some());
    assert!(v["failures"].as_u64().is_some());
    assert!(v["failure_rate_pct"].as_f64().is_some());
    assert!(v["weaning"].is_object());
    assert!(v["buckets"].is_array());
}

// ============================================================================
// 8. Input shape helpers (regression)
// ============================================================================

#[test]
fn input_shape_url_clusters_same_prefix() {
    let s1 = input_shape("can you get me all the titles from https://example.com/todos");
    let s2 = input_shape("can you fetch all the items from https://api.example.com/list");
    assert_eq!(s1, s2, "URL inputs with same prefix must share shape");
}

#[test]
fn input_shape_retains_markers() {
    let s = input_shape("please read /tmp/file.txt now");
    assert!(s.contains("<path>"), "shape should contain <path>: {s}");
}

// ============================================================================
// 9. /debug/health returns same counts as analyse()
// ============================================================================

#[tokio::test]
async fn server_health_matches_analyse() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("doctor_test.db");

    let brain = Brain::open(BrainConfig {
        db_path: Some(db_path),
        offline: true,
        debug: false,
        ..Default::default()
    })
    .await
    .expect("Brain::open");

    brain.turn("s1", "hello").await.unwrap();
    brain.turn("s1", "what is 2 plus 2?").await.unwrap();
    brain.turn("s2", "zxqzxq zxqzxq zxqzxq").await.unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(Arc::clone(&brain));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let client = reqwest::Client::new();
    let server_report: serde_json::Value = client
        .get(format!("http://{addr}/debug/health"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // Server filters to current build; all test episodes carry the same build_id.
    let episodes = brain.store.lock().episodes_window(None, 10_000).unwrap();
    let current_build = spoon_core::BUILD_ID;
    let current: Vec<_> = episodes
        .iter()
        .filter(|e| e.build_id == current_build)
        .cloned()
        .collect();
    let direct = analyse(&current);

    assert_eq!(server_report["total"].as_u64().unwrap() as usize, direct.total);
    assert_eq!(server_report["failures"].as_u64().unwrap() as usize, direct.failures);
    assert_eq!(server_report["honest_unknowns"].as_u64().unwrap() as usize, direct.honest_unknowns);
    assert_eq!(
        server_report["interior_llm_violation"].as_bool().unwrap(),
        direct.interior_llm_violation
    );
}

// ============================================================================
// 10. doctor run() errors on --ephemeral
// ============================================================================

#[tokio::test]
async fn doctor_errors_on_ephemeral() {
    let brain = Brain::open(BrainConfig { db_path: None, offline: true, ..Default::default() })
        .await
        .unwrap();
    let err = spoon::doctor::run(
        brain,
        &DoctorArgs { since_ms: None, limit: 100, json: false, all: false, build: None },
    )
    .await
    .err()
    .expect("should fail on ephemeral");
    assert!(
        err.to_string().contains("ephemeral") || err.to_string().contains("persistent"),
        "error must mention ephemeral/persistent: {err}"
    );
}

// ============================================================================
// 11. Build stamping and filtering
// ============================================================================

#[test]
fn episode_builder_can_carry_build_id() {
    let ep = ep_with_build(success_ep("hi"), "abc123");
    assert_eq!(ep.build_id, "abc123");
}

#[test]
fn default_view_excludes_unstamped_episodes() {
    // Unstamped episodes (build_id = "") should not appear in default view.
    let ctx = AnalyseContext {
        current_build: "build-x".into(),
        report_build: "build-x".into(),
        excluded_count: 0,
        all_mode: false,
    };
    // Pass only unstamped (build_id = "") - simulates what run() would do
    // after pre-filtering removes them.
    let episodes: Vec<Episode> = vec![];
    let r = analyse_with_context(&episodes, &ctx);
    assert_eq!(r.total, 0, "no current-build episodes means zero total");
    assert!(!r.all_mode);
}

#[test]
fn all_mode_includes_unstamped() {
    let ctx = AnalyseContext {
        current_build: "build-x".into(),
        report_build: String::new(),
        excluded_count: 0,
        all_mode: true,
    };
    // Pass episodes with and without build stamps.
    let eps = vec![
        ep_with_build(success_ep("hi"), ""),            // unstamped
        ep_with_build(success_ep("hey"), "build-old"),  // older build
        ep_with_build(success_ep("yo"), "build-x"),     // current build
    ];
    let r = analyse_with_context(&eps, &ctx);
    assert!(r.all_mode);
    assert_eq!(r.total, 3);
    assert_eq!(r.unstamped_count, 1, "one episode has no stamp");
}

#[test]
fn empty_current_build_has_zero_total() {
    let ctx = AnalyseContext {
        current_build: "brand-new-build".into(),
        report_build: "brand-new-build".into(),
        excluded_count: 5,   // 5 older episodes excluded
        all_mode: false,
    };
    let r = analyse_with_context(&[], &ctx);
    assert_eq!(r.total, 0);
    assert_eq!(r.failures, 0);
    assert_eq!(r.excluded_count, 5);
    // Failure rate must not produce NaN or divide-by-zero.
    assert_eq!(r.failure_rate_pct, 0.0);
}

#[test]
fn cluster_spanning_builds_reports_current_count() {
    // In --all mode: 2 from current build, 1 from older.
    let ctx = AnalyseContext {
        current_build: "build-new".into(),
        report_build: String::new(),
        excluded_count: 0,
        all_mode: true,
    };
    let mut e1 = failed_with_sce(
        "can you get the data from https://api.test.com",
        "Assistant, get User the data from https://api.test.com!",
    );
    e1.build_id = "build-new".into();

    let mut e2 = failed_with_sce(
        "please fetch stuff from https://other.com",
        "Assistant, fetch User stuff from https://other.com!",
    );
    e2.build_id = "build-new".into();

    let mut e3 = failed_with_sce(
        "get info from https://legacy.com",
        "Assistant, get User info from https://legacy.com!",
    );
    e3.build_id = "build-old".into();

    let r = analyse_with_context(&[e1, e2, e3], &ctx);
    let bucket = r.buckets.iter().find(|b| b.bucket == "ears_didnt_reparse").expect("bucket");
    let cluster = bucket.clusters.iter().find(|c| c.symptom == "normalizer_inversion").expect("cluster");

    assert_eq!(cluster.count, 3, "total members");
    assert_eq!(cluster.current_build_count, 2, "2 from current build");
}
