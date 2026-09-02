//! Integration tests for the SQLite store.

use spoon_core::{
    store::{EpisodeQuery, Seed, Stance, Store},
    types::*,
};

fn make_episode(session_id: &str, user_text: &str, keywords: Vec<String>) -> Episode {
    Episode {
        id: 0,
        session_id: session_id.to_string(),
        at: now_ms(),
        user_text: user_text.to_string(),
        sce: format!("assistant, note(\"{}\")", user_text),
        clauses: vec![],
        plans: vec![],
        response: spoon_core::types::ResponsePlan::default(),
        reply_text: format!("noted: {}", user_text),
        metrics: TurnMetrics::default(),
        credit: 0,
        keywords,
    }
}

fn make_fact(pred: &str, args: Vec<Value>, source: &str) -> Fact {
    Fact {
        id: 0,
        pred: ActionId(pred.to_string()),
        args,
        truth: true,
        modal: None,
        asserted_at: now_ms(),
        invalidated_at: None,
        source: source.to_string(),
        episode_id: None,
    }
}

fn make_concept(id: &str, noun: &str) -> Concept {
    Concept {
        id: ConceptId(id.to_string()),
        kind: ConceptKind::Entity,
        extends: vec![],
        role_of: None,
        nouns: vec![noun.to_string()],
        description: format!("test concept {id}"),
        tier: Tier::Provisional,
        provenance: Provenance::Kernel,
    }
}

fn make_action_program(id: &str) -> Action {
    let program = Program::new(
        vec![Type::Int],
        Type::Int,
        Expr::Param { index: 0 },
    );
    Action {
        id: ActionId(id.to_string()),
        inputs: vec![Input::required("x", Type::Int)],
        output: Type::Int,
        effect: Effect::Pure,
        imp: Impl::Program { program },
        role: Role::Command,
        verbs: vec![id.to_string()],
        phrasings: vec![],
        description: format!("test action {id}"),
        tier: Tier::Provisional,
        provenance: Provenance::Kernel,
        stats: Stats::default(),
    }
}

// ---- tests ------------------------------------------------------------------

#[test]
fn open_memory() {
    let store = Store::open_memory().unwrap();
    let counts = store.counts().unwrap();
    assert_eq!(counts.concepts, 0);
    assert_eq!(counts.actions, 0);
}

#[test]
fn save_load_concept_action_can() {
    let store = Store::open_memory().unwrap();

    let concept = make_concept("Animal", "animal");
    store.save_concept(&concept).unwrap();

    let action = make_action_program("identity");
    store.save_action(&action).unwrap();

    let can = store.load_can().unwrap();
    let (nc, na) = can.len();
    assert_eq!(nc, 1);
    assert_eq!(na, 1);

    let loaded_c = can.concept(&ConceptId("Animal".into())).unwrap();
    assert_eq!(loaded_c.nouns, concept.nouns);
    assert_eq!(loaded_c.kind, concept.kind);

    let loaded_a = can.action(&ActionId("identity".into())).unwrap();
    assert_eq!(loaded_a.verbs, action.verbs);
    assert_eq!(loaded_a.output, action.output);
    // Verify Impl::Program round-trips
    assert!(matches!(loaded_a.imp, Impl::Program { .. }));
}

#[test]
fn insert_query_facts() {
    let store = Store::open_memory().unwrap();

    // own(Name("John"), Name("Dog"))
    let f1 = make_fact("own", vec![Value::Name("John".into()), Value::Name("Dog".into())], "user");
    // own(Name("John"), Name("Cat"))
    let f2 = make_fact("own", vec![Value::Name("John".into()), Value::Name("Cat".into())], "user");
    // own(Name("Mary"), Name("Dog"))
    let f3 = make_fact("own", vec![Value::Name("Mary".into()), Value::Name("Dog".into())], "user");

    store.insert_fact(&f1).unwrap();
    store.insert_fact(&f2).unwrap();
    store.insert_fact(&f3).unwrap();

    let pred = ActionId("own".into());

    // Pattern: [Some(John), None] -> John's pets (2 results)
    let pat = vec![Some(Value::Name("John".into())), None];
    let results = store.query_facts(&pred, &pat).unwrap();
    assert_eq!(results.len(), 2, "John owns 2 things");
    for r in &results {
        assert_eq!(r.pred.0, "own");
        assert_eq!(r.args[0], Value::Name("John".into()));
    }

    // Pattern: [None, Some(Dog)] -> Dog owners (2 results)
    let pat2 = vec![None, Some(Value::Name("Dog".into()))];
    let results2 = store.query_facts(&pred, &pat2).unwrap();
    assert_eq!(results2.len(), 2, "Dog has 2 owners");

    // facts_about(Name("Dog")) -> 2 facts mention Dog
    let about_dog = store.facts_about(&Value::Name("Dog".into())).unwrap();
    assert_eq!(about_dog.len(), 2);
}

#[test]
fn invalidate_excludes_from_query() {
    let store = Store::open_memory().unwrap();

    let f = make_fact("has", vec![Value::Name("Alice".into()), Value::Text("wings".into())], "user");
    let id = store.insert_fact(&f).unwrap();

    let pred = ActionId("has".into());
    let before = store.query_facts(&pred, &[]).unwrap();
    assert_eq!(before.len(), 1);

    store.invalidate_fact(id, now_ms()).unwrap();

    let after = store.query_facts(&pred, &[]).unwrap();
    assert_eq!(after.len(), 0, "invalidated fact must not appear");

    let about = store.facts_about(&Value::Name("Alice".into())).unwrap();
    assert_eq!(about.len(), 0, "facts_about also excludes invalidated");
}

#[test]
fn episodes_fts_and_last_episode() {
    let store = Store::open_memory().unwrap();

    let e1 = make_episode("sess-A", "John went to the park", vec!["john".into(), "park".into()]);
    let e2 = make_episode("sess-A", "Mary baked a cake", vec!["mary".into(), "cake".into()]);
    let e3 = make_episode("sess-B", "John bought a cake", vec!["john".into(), "cake".into()]);

    store.insert_episode(&e1).unwrap();
    // small sleep to distinguish at timestamps
    std::thread::sleep(std::time::Duration::from_millis(2));
    store.insert_episode(&e2).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    store.insert_episode(&e3).unwrap();

    // FTS search for "john" should return e1 and e3
    let q = EpisodeQuery {
        session_id: None,
        keywords: &["john".to_string()],
        since_ms: None,
        limit: 10,
    };
    let results = store.episodes(&q).unwrap();
    assert_eq!(results.len(), 2);
    let texts: Vec<&str> = results.iter().map(|e| e.user_text.as_str()).collect();
    assert!(texts.contains(&"John went to the park"));
    assert!(texts.contains(&"John bought a cake"));

    // FTS search scoped to sess-A
    let q2 = EpisodeQuery {
        session_id: Some("sess-A"),
        keywords: &["john".to_string()],
        since_ms: None,
        limit: 10,
    };
    let results2 = store.episodes(&q2).unwrap();
    assert_eq!(results2.len(), 1);
    assert_eq!(results2[0].user_text, "John went to the park");

    // last_episode for sess-A is e2
    let last_a = store.last_episode("sess-A").unwrap().unwrap();
    assert_eq!(last_a.user_text, "Mary baked a cake");

    // last_episode for sess-B is e3
    let last_b = store.last_episode("sess-B").unwrap().unwrap();
    assert_eq!(last_b.user_text, "John bought a cake");

    // No episodes for unknown session
    let none = store.last_episode("sess-Z").unwrap();
    assert!(none.is_none());
}

#[test]
fn episodes_plain_filter() {
    let store = Store::open_memory().unwrap();

    let mut e = make_episode("sess-1", "hello world", vec![]);
    store.insert_episode(&e).unwrap();
    e.user_text = "another turn".to_string();
    store.insert_episode(&e).unwrap();

    let q = EpisodeQuery { session_id: Some("sess-1"), keywords: &[], since_ms: None, limit: 1 };
    let r = store.episodes(&q).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].user_text, "another turn"); // newest first
}

#[test]
fn pairs_credit_filter() {
    let store = Store::open_memory().unwrap();

    let p_good = Pair { id: 0, utterance: "hello world".into(), sce: "assistant, greet()".into(), source: "seed".into(), at: now_ms(), credit: 1 };
    let p_bad  = Pair { id: 0, utterance: "goodbye world".into(), sce: "assistant, farewell()".into(), source: "seed".into(), at: now_ms(), credit: -1 };
    let p_zero = Pair { id: 0, utterance: "ok world".into(), sce: "assistant, ok()".into(), source: "llm".into(), at: now_ms(), credit: 0 };

    store.insert_pair(&p_good).unwrap();
    store.insert_pair(&p_bad).unwrap();
    store.insert_pair(&p_zero).unwrap();

    let good = store.pairs(0).unwrap(); // credit >= 0
    assert_eq!(good.len(), 2, "credit 0 and 1 pass, -1 fails");

    let only_pos = store.pairs(1).unwrap();
    assert_eq!(only_pos.len(), 1);
    assert_eq!(only_pos[0].utterance, "hello world");

    let all_incl_bad = store.pairs(-1).unwrap();
    assert_eq!(all_incl_bad.len(), 3);
}

#[test]
fn pairs_dedup() {
    let store = Store::open_memory().unwrap();
    let p = Pair { id: 0, utterance: "hello".into(), sce: "greet()".into(), source: "llm".into(), at: now_ms(), credit: 0 };
    let id1 = store.insert_pair(&p).unwrap();
    let id2 = store.insert_pair(&p).unwrap();
    assert_eq!(id1, id2, "duplicate pair returns same id");

    let all = store.pairs(-127).unwrap();
    assert_eq!(all.len(), 1);
}

#[test]
fn stance_upsert_idempotent() {
    let store = Store::open_memory().unwrap();

    let s = Stance {
        id: 0,
        topic: "climate".into(),
        stance: "serious".into(),
        reasons: vec!["science".into()],
        confidence: 0.9,
        source: "teacher".into(),
        at: now_ms(),
    };
    let id1 = store.upsert_stance(&s).unwrap();

    // Same topic+stance, updated confidence
    let s2 = Stance { confidence: 0.95, reasons: vec!["more data".into()], ..s.clone() };
    let id2 = store.upsert_stance(&s2).unwrap();

    assert_eq!(id1, id2, "upsert returns same row id");

    let all = store.stances(&[]).unwrap();
    assert_eq!(all.len(), 1);
    assert!((all[0].confidence - 0.95_f32).abs() < 0.001);

    // Keyword filter
    let by_kw = store.stances(&["clima".to_string()]).unwrap();
    assert_eq!(by_kw.len(), 1);

    let none = store.stances(&["unrelated".to_string()]).unwrap();
    assert!(none.is_empty());
}

#[test]
fn kv_roundtrip() {
    let store = Store::open_memory().unwrap();

    let val = serde_json::json!({"answer": 42, "ok": true});
    store.kv_set("my_key", &val).unwrap();

    let loaded = store.kv_get("my_key").unwrap().unwrap();
    assert_eq!(loaded, val);

    let missing = store.kv_get("no_such_key").unwrap();
    assert!(missing.is_none());

    // Overwrite
    store.kv_set("my_key", &serde_json::json!(99)).unwrap();
    let updated = store.kv_get("my_key").unwrap().unwrap();
    assert_eq!(updated, serde_json::json!(99));
}

#[test]
fn export_import_seed() {
    let src = Store::open_memory().unwrap();

    // Populate source store
    let c = make_concept("Widget", "widget");
    src.save_concept(&c).unwrap();

    let a = make_action_program("double");
    src.save_action(&a).unwrap();

    let f = make_fact(
        "owns",
        vec![Value::Name("Alice".into()), Value::Name("Widget".into())],
        "teacher",
    );
    src.insert_fact(&f).unwrap();

    let p = Pair { id: 0, utterance: "two of it".into(), sce: "double(x)".into(), source: "seed".into(), at: now_ms(), credit: 0 };
    src.insert_pair(&p).unwrap();

    let s = Stance { id: 0, topic: "widgets".into(), stance: "useful".into(), reasons: vec![], confidence: 0.8, source: "teacher".into(), at: now_ms() };
    src.upsert_stance(&s).unwrap();

    src.kv_set("seed.lang", &serde_json::json!("en")).unwrap();
    src.kv_set("internal", &serde_json::json!("skip")).unwrap();

    let seed = src.export_seed("test-seed").unwrap();
    assert_eq!(seed.name, "test-seed");
    // Provisional tier concepts/actions are exported
    assert_eq!(seed.concepts.len(), 1);
    assert_eq!(seed.actions.len(), 1);
    assert_eq!(seed.facts.len(), 1);
    assert_eq!(seed.pairs.len(), 1);
    assert_eq!(seed.stances.len(), 1);
    assert_eq!(seed.kv.len(), 1, "only 'seed.' kv exported");
    assert_eq!(seed.kv[0].0, "seed.lang");

    // Import into a fresh store
    let dst = Store::open_memory().unwrap();
    let written = dst.import_seed(&seed).unwrap();
    // 1 concept + 1 action + 1 pair + 1 fact + 1 stance + 1 kv = 6
    assert_eq!(written, 6);

    let counts = dst.counts().unwrap();
    assert_eq!(counts.concepts, 1);
    assert_eq!(counts.actions, 1);
    assert_eq!(counts.facts, 1);
    assert_eq!(counts.pairs, 1);
    assert_eq!(counts.stances, 1);
}

#[test]
fn export_excludes_kernel_actions() {
    let store = Store::open_memory().unwrap();

    let mut kernel_action = make_action_program("kernel-op");
    kernel_action.tier = Tier::Kernel;
    store.save_action(&kernel_action).unwrap();

    let prov_action = make_action_program("prov-op");
    store.save_action(&prov_action).unwrap();

    let seed = store.export_seed("test").unwrap();
    assert_eq!(seed.actions.len(), 1, "kernel action excluded from export");
    assert_eq!(seed.actions[0].id.0, "prov-op");
}

#[test]
fn import_does_not_overwrite_kernel() {
    let store = Store::open_memory().unwrap();

    let mut kernel_action = make_action_program("op");
    kernel_action.tier = Tier::Kernel;
    kernel_action.description = "original kernel".into();
    store.save_action(&kernel_action).unwrap();

    // Seed contains same id as Provisional
    let mut seed_action = make_action_program("op");
    seed_action.description = "seed version".into();
    let seed = Seed {
        version: 1,
        name: "test".into(),
        created_at: now_ms(),
        concepts: vec![],
        actions: vec![seed_action],
        pairs: vec![],
        facts: vec![],
        stances: vec![],
        kv: vec![],
    };

    store.import_seed(&seed).unwrap();

    let can = store.load_can().unwrap();
    let loaded = can.action(&ActionId("op".into())).unwrap();
    assert_eq!(loaded.description, "original kernel", "kernel action must not be overwritten");
}

#[test]
fn file_persistence() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("spoon-test-{}.sqlite3", uuid::Uuid::new_v4()));

    // Open, write, close
    {
        let store = Store::open(&path).unwrap();
        let c = make_concept("Persist", "persist");
        store.save_concept(&c).unwrap();

        let pair = Pair { id: 0, utterance: "persist".into(), sce: "persist()".into(), source: "seed".into(), at: now_ms(), credit: 0 };
        store.insert_pair(&pair).unwrap();
    }

    // Reopen and verify
    {
        let store = Store::open(&path).unwrap();
        let counts = store.counts().unwrap();
        assert_eq!(counts.concepts, 1, "concept survived restart");
        assert_eq!(counts.pairs, 1, "pair survived restart");

        let can = store.load_can().unwrap();
        assert!(can.concept(&ConceptId("Persist".into())).is_some());
    }

    // Cleanup
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
}

#[test]
fn episode_credit_update() {
    let store = Store::open_memory().unwrap();
    let e = make_episode("sess-1", "was this correct?", vec![]);
    let id = store.insert_episode(&e).unwrap();

    store.update_episode_credit(id, 1).unwrap();

    let q = EpisodeQuery { session_id: Some("sess-1"), keywords: &[], since_ms: None, limit: 10 };
    let eps = store.episodes(&q).unwrap();
    assert_eq!(eps[0].credit, 1);
}
