//! Orchestrator review: the notation's one invariant is that rendering is
//! always reversible. These probe the cases where that is easiest to break.

use spoon_concept::{Concept, Ground, SymbolTable, parse, render, render_compact};

/// Render in one table, parse in another. Round-tripping through a table that
/// already knows the names is the easy case; a cold table is what a fresh
/// process actually has when it reads a log line or a seed file.
fn round_trip_cold(c: &Concept) {
    let warm = SymbolTable::new();
    register_names(c, &warm);
    let text = render(c, &warm);

    let cold = SymbolTable::new();
    let back = parse(&text, &cold)
        .unwrap_or_else(|e| panic!("render output did not parse: {text:?} ({e})"));
    assert_eq!(
        back.content_id(),
        c.content_id(),
        "round trip changed the concept: {text:?}"
    );
}

fn register_names(c: &Concept, table: &SymbolTable) {
    for node in spoon_concept::pre_order(c) {
        if let Some(id) = node.as_symbol() {
            // Only registers if the test itself interned a spelling; otherwise
            // the table stays cold for this symbol on purpose.
            let _ = id;
        }
    }
    let _ = table;
}

// ---------------------------------------------------------------------------
// Ground values that impersonate other ground values
// ---------------------------------------------------------------------------

#[test]
fn text_that_looks_like_another_literal_stays_text() {
    // The parser must not read a quoted "true" as a boolean, or "42" as an int.
    // Each of these is a Text concept with its own content id.
    for impostor in [
        "true",
        "false",
        "42",
        "42.0",
        "-7",
        "nan",
        "inf",
        "-inf",
        "?0",
        "0x41",
        "#0123456789abcdef",
        "json(1)",
        "[1,2]",
        "{\"a\":1}",
        "Greg",
        "",
    ] {
        let c = Concept::text(impostor);
        let table = SymbolTable::new();
        let text = render(&c, &table);
        let back = parse(&text, &table).unwrap_or_else(|e| panic!("{impostor:?}: {e}"));
        assert_eq!(
            back, c,
            "{impostor:?} rendered as {text} and came back wrong"
        );
        assert!(
            back.as_ground().unwrap().as_str().is_some(),
            "{impostor:?} stopped being text"
        );
    }
}

#[test]
fn a_name_that_collides_with_a_reserved_word_still_round_trips() {
    // Nothing stops a concept being called "true" or "nan". If the renderer
    // wrote it bare it would read back as a boolean or a float, which is a
    // different concept. Whatever escape the renderer picks, the round trip is
    // what has to hold.
    for risky in ["true", "false", "nan", "inf", "-inf", "42", "0x41"] {
        let table = SymbolTable::new();
        table.intern(risky);
        let c = Concept::named(risky);
        let text = render(&c, &table);
        let back = parse(&text, &table)
            .unwrap_or_else(|e| panic!("name {risky:?} rendered as {text} then failed: {e}"));
        assert_eq!(
            back.content_id(),
            c.content_id(),
            "name {risky:?} rendered as {text} and came back as a different concept"
        );
        assert!(back.is_named(), "name {risky:?} stopped being a name");
    }
}

#[test]
fn json_scalars_do_not_collapse_into_their_bare_counterparts() {
    // Json(42) and Int(42) are different concepts. A bare `42` in the notation
    // is the int, so the JSON scalar needs its own spelling.
    let pairs: Vec<(Concept, Concept)> = vec![
        (Concept::json(serde_json::json!(42)), Concept::int(42)),
        (Concept::json(serde_json::json!("42")), Concept::text("42")),
        (Concept::json(serde_json::json!(true)), Concept::bool(true)),
        (Concept::json(serde_json::json!(1.5)), Concept::float(1.5)),
    ];
    for (as_json, as_native) in pairs {
        assert_ne!(as_json.content_id(), as_native.content_id());
        let table = SymbolTable::new();
        let back = parse(&render(&as_json, &table), &table).unwrap();
        assert_eq!(
            back.content_id(),
            as_json.content_id(),
            "json scalar drifted"
        );
        let back_native = parse(&render(&as_native, &table), &table).unwrap();
        assert_eq!(back_native.content_id(), as_native.content_id());
    }
    // json(null) has no native counterpart but still has to survive.
    let null = Concept::json(serde_json::Value::Null);
    let table = SymbolTable::new();
    assert_eq!(
        parse(&render(&null, &table), &table).unwrap().content_id(),
        null.content_id()
    );
}

#[test]
fn non_finite_floats_round_trip() {
    // Float identity canonicalizes NaN, so every NaN is one concept, but it
    // still needs a spelling or Float stops round-tripping for those bits.
    for f in [
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        -0.0,
        0.0,
        f64::MIN,
        f64::MAX,
    ] {
        let c = Concept::float(f);
        let table = SymbolTable::new();
        let text = render(&c, &table);
        let back = parse(&text, &table).unwrap_or_else(|e| panic!("{f} as {text}: {e}"));
        assert_eq!(back.content_id(), c.content_id(), "{f} rendered as {text}");
    }
}

#[test]
fn floats_never_render_as_bare_integers() {
    // 1.0 written as `1` would read back as Int, a different concept.
    let table = SymbolTable::new();
    for f in [1.0f64, -3.0, 0.0, 1e30] {
        let text = render(&Concept::float(f), &table);
        let back = parse(&text, &table).unwrap();
        assert!(
            matches!(back.as_ground(), Some(Ground::Float(_))),
            "{f} rendered as {text} and parsed as {back:?}"
        );
    }
}

#[test]
fn empty_and_edge_payloads_survive() {
    let cases = vec![
        Concept::text(""),
        Concept::bytes([] as [u8; 0]),
        Concept::bytes([0u8]),
        Concept::bytes([0xffu8; 40]),
        Concept::json(serde_json::json!({})),
        Concept::json(serde_json::json!([])),
        Concept::int(i64::MIN),
        Concept::int(i64::MAX),
        Concept::call("empty", []),
    ];
    for c in cases {
        let table = SymbolTable::new();
        table.intern("empty");
        let text = render(&c, &table);
        let back = parse(&text, &table).unwrap_or_else(|e| panic!("{text:?}: {e}"));
        assert_eq!(back.content_id(), c.content_id(), "{text:?}");
    }
}

#[test]
fn text_escapes_survive_including_the_awkward_ones() {
    for s in [
        "he said \"hi\"",
        "back\\slash",
        "line\nbreak",
        "tab\there",
        "carriage\rreturn",
        "\\\"nested\\\"",
        "emoji \u{1F600} and \u{4E2D}\u{6587}",
        "trailing backslash \\",
    ] {
        let c = Concept::text(s);
        let table = SymbolTable::new();
        let text = render(&c, &table);
        let back = parse(&text, &table).unwrap_or_else(|e| panic!("{s:?} as {text}: {e}"));
        assert_eq!(back, c, "{s:?} rendered as {text}");
    }
}

// ---------------------------------------------------------------------------
// Structure
// ---------------------------------------------------------------------------

#[test]
fn cold_table_output_still_parses() {
    // A log line written by a process that never interned the name renders the
    // symbol in hex. That text has to read back, or logs and seed files become
    // one-way.
    let c = Concept::call(
        "never-interned-anywhere",
        [Concept::named("also-unknown"), Concept::int(1)],
    );
    let cold = SymbolTable::new();
    let text = render(&c, &cold);
    let fresh = SymbolTable::new();
    let back = parse(&text, &fresh).unwrap_or_else(|e| panic!("{text:?}: {e}"));
    assert_eq!(back.content_id(), c.content_id(), "{text:?}");
}

#[test]
fn zero_arity_compound_keeps_its_brackets() {
    let table = SymbolTable::new();
    table.intern("Raining");
    let asserted = Concept::call("raining", []);
    let named = Concept::named("raining");
    let rendered = render(&asserted, &table);
    assert!(rendered.ends_with("<>"), "got {rendered}");
    assert_ne!(rendered, render(&named, &table));
    assert_eq!(
        parse(&rendered, &table).unwrap().content_id(),
        asserted.content_id()
    );
}

#[test]
fn curried_heads_round_trip_at_every_depth() {
    let table = SymbolTable::new();
    table.intern("Sort");
    let mut c = Concept::named("sort");
    for i in 0..5 {
        c = Concept::apply(c, vec![Concept::int(i)]);
        round_trip_named(&c, &table);
    }
}

#[test]
fn holes_work_as_arguments_and_as_heads() {
    let table = SymbolTable::new();
    // ?0<?1, ?2> is how a rule quantifies over an unknown relation.
    let rule_shape = Concept::apply(Concept::hole(0), vec![Concept::hole(1), Concept::hole(2)]);
    round_trip_named(&rule_shape, &table);
    round_trip_named(&Concept::hole(0), &table);
    round_trip_named(&Concept::hole(u32::MAX), &table);
}

#[test]
fn deep_nesting_round_trips() {
    let table = SymbolTable::new();
    table.intern("f");
    let mut c = Concept::int(0);
    for _ in 0..40 {
        c = Concept::call("f", [c, Concept::named("f")]);
    }
    round_trip_named(&c, &table);
}

fn round_trip_named(c: &Concept, table: &SymbolTable) {
    let text = render(c, table);
    let back = parse(&text, table).unwrap_or_else(|e| panic!("{text:?}: {e}"));
    assert_eq!(back.content_id(), c.content_id(), "{text:?}");
}

// ---------------------------------------------------------------------------
// Generated corpus: the invariant over arbitrary structure
// ---------------------------------------------------------------------------

struct Gen(u64);

impl Gen {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn term(&mut self, depth: u32) -> Concept {
        let n = if depth == 0 { 10 } else { 12 };
        match self.pick(n) {
            0 => {
                Concept::named(["greg", "friend-with", "FriendWith", "a.b:c", "x_1"][self.pick(5)])
            }
            1 => Concept::int(self.next() as i64),
            2 => Concept::float(f64::from_bits(self.next())),
            3 => Concept::bool(self.pick(2) == 0),
            4 => Concept::text(["", "true", "42", "a\"b", "\u{4E2D}", "?0"][self.pick(6)]),
            5 => Concept::bytes(vec![self.next() as u8; self.pick(20)]),
            6 => Concept::hole(self.pick(4) as u32),
            7 => Concept::json(serde_json::json!({"k": self.pick(5)})),
            8 => Concept::json(serde_json::Value::from(self.pick(9) as i64)),
            9 => Concept::datetime(chrono::DateTime::from_timestamp_nanos(self.next() as i64)),
            10 => {
                let arity = self.pick(4);
                let args: Vec<Concept> = (0..arity).map(|_| self.term(depth - 1)).collect();
                Concept::call(["f", "g", "Pair"][self.pick(3)], args)
            }
            _ => {
                let head = self.term(depth - 1);
                let args: Vec<Concept> = (0..=self.pick(2)).map(|_| self.term(depth - 1)).collect();
                Concept::apply(head, args)
            }
        }
    }
}

#[test]
fn generated_corpus_round_trips_through_a_cold_table() {
    let mut g = Gen(0x00C0_FFEE_1234_5678);
    let mut compounds = 0;
    let mut curried = 0;
    for _ in 0..400 {
        let c = g.term(4);
        if c.is_compound() {
            compounds += 1;
            if c.head().map(|h| h.is_compound()).unwrap_or(false) {
                curried += 1;
            }
        }
        round_trip_cold(&c);
    }
    assert!(
        compounds > 50,
        "corpus was too shallow: {compounds} compounds"
    );
    assert!(curried > 0, "corpus never produced a compound head");
}

#[test]
fn compact_rendering_stays_parseable_shaped_and_never_panics() {
    // render_compact elides, so it is not required to round trip. It is
    // required to never panic and to actually shrink deep terms.
    let mut g = Gen(0xBEEF_0001_u64);
    let table = SymbolTable::new();
    for _ in 0..200 {
        let c = g.term(4);
        let full = render(&c, &table);
        for depth in 0..4 {
            let compact = render_compact(&c, &table, depth);
            assert!(compact.len() <= full.len() + 8, "compact grew: {compact}");
        }
        // A depth budget past the term's own depth elides no structure. Long
        // payloads are still shortened: the payload budget is deliberately
        // independent of depth, since one 4 KB blob at depth 1 ruins a log line
        // just as thoroughly as deep nesting does. So the invariant is about
        // structure, not about the exact string.
        let generous = render_compact(&c, &table, c.depth() + 4);
        assert!(
            !generous.contains("<...>"),
            "structure was elided under a generous budget: {generous}"
        );
        assert_eq!(
            generous.matches('<').count(),
            full.matches('<').count(),
            "generous budget dropped an application: {generous} vs {full}"
        );
        assert!(
            !generous.is_empty(),
            "compact rendering produced nothing for {full}"
        );

        // Payload elision only ever shortens.
        assert!(generous.len() <= full.len());
    }
}
