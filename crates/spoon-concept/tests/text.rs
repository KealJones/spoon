//! The angle-bracket notation, in both directions.
//!
//! The load-bearing property is that [`parse`] undoes [`render`] exactly, for
//! every shape a concept can take. Everything else here is a specific case
//! that property has to cover.

use chrono::{DateTime, Utc};
use serde_json::json;
use spoon_concept::{
    Concept, ConceptDisplay, ParseError, SymbolId, SymbolTable, display, parse, render,
    render_compact,
};

fn table_with(names: &[&str]) -> SymbolTable {
    let table = SymbolTable::new();
    for name in names {
        table.intern(name);
    }
    table
}

/// Render, parse, and demand the concept came back unchanged.
fn roundtrip(concept: &Concept, table: &SymbolTable) -> String {
    let text = render(concept, table);
    let back = match parse(&text, table) {
        Ok(back) => back,
        Err(err) => panic!("parsing rendered {text:?} failed: {err}"),
    };
    assert_eq!(&back, concept, "round trip changed the concept: {text}");
    assert_eq!(
        back.content_id(),
        concept.content_id(),
        "round trip changed the content id: {text}"
    );
    text
}

fn nullary(head: Concept) -> Concept {
    Concept::apply(head, Vec::<Concept>::new())
}

fn timestamp(secs: i64, nanos: u32) -> DateTime<Utc> {
    DateTime::from_timestamp(secs, nanos).expect("timestamp in range")
}

// ---- rendering every ground variant ----

#[test]
fn renders_each_ground_variant_exactly() {
    let table = SymbolTable::new();
    assert_eq!(render(&Concept::bool(true), &table), "true");
    assert_eq!(render(&Concept::bool(false), &table), "false");
    assert_eq!(render(&Concept::int(42), &table), "42");
    assert_eq!(render(&Concept::int(-7), &table), "-7");
    assert_eq!(render(&Concept::float(1.0), &table), "1.0");
    assert_eq!(render(&Concept::float(2.5), &table), "2.5");
    assert_eq!(render(&Concept::float(-0.25), &table), "-0.25");
    assert_eq!(render(&Concept::text("hello"), &table), "\"hello\"");
    assert_eq!(
        render(&Concept::bytes([0xde, 0xad, 0xbe, 0xef]), &table),
        "0xdeadbeef"
    );
    assert_eq!(render(&Concept::bytes([] as [u8; 0]), &table), "0x");
    assert_eq!(
        render(&Concept::datetime(timestamp(1_700_000_000, 0)), &table),
        "2023-11-14T22:13:20Z"
    );
    assert_eq!(
        render(&Concept::json(json!({"a": 1, "b": [true, null]})), &table),
        "{\"a\":1,\"b\":[true,null]}"
    );
    assert_eq!(render(&Concept::json(json!([1, 2, 3])), &table), "[1,2,3]");
    assert_eq!(render(&Concept::json(json!(42)), &table), "json(42)");
    assert_eq!(render(&Concept::json(json!(null)), &table), "json(null)");
    assert_eq!(render(&Concept::json(json!("42")), &table), "json(\"42\")");
}

#[test]
fn parses_each_ground_variant_back() {
    let table = SymbolTable::new();
    let cases = [
        Concept::bool(true),
        Concept::bool(false),
        Concept::int(42),
        Concept::int(-7),
        Concept::int(i64::MIN),
        Concept::int(i64::MAX),
        Concept::float(1.0),
        Concept::float(2.5),
        Concept::float(-0.25),
        Concept::float(1e300),
        Concept::text("hello"),
        Concept::text(""),
        Concept::bytes([0xde, 0xad, 0xbe, 0xef]),
        Concept::bytes([] as [u8; 0]),
        Concept::datetime(timestamp(1_700_000_000, 0)),
        Concept::datetime(timestamp(0, 123_456_789)),
        Concept::datetime(timestamp(-86_400, 1_000)),
        Concept::json(json!({"a": 1})),
        Concept::json(json!([1, "two", false])),
        Concept::json(json!(42)),
        Concept::json(json!(null)),
        Concept::json(json!("nested \"quotes\"")),
    ];
    for case in &cases {
        roundtrip(case, &table);
    }
}

#[test]
fn non_finite_floats_round_trip() {
    let table = SymbolTable::new();
    assert_eq!(render(&Concept::float(f64::INFINITY), &table), "inf");
    assert_eq!(render(&Concept::float(f64::NEG_INFINITY), &table), "-inf");
    assert_eq!(render(&Concept::float(f64::NAN), &table), "nan");
    roundtrip(&Concept::float(f64::INFINITY), &table);
    roundtrip(&Concept::float(f64::NEG_INFINITY), &table);
    // Ground collapses every NaN bit pattern to one value, so this is a real
    // equality, not an approximation.
    roundtrip(&Concept::float(f64::NAN), &table);
    roundtrip(&Concept::float(-0.0), &table);
}

#[test]
fn int_float_and_text_are_three_concepts() {
    let table = SymbolTable::new();
    let int = Concept::int(42);
    let float = Concept::float(42.0);
    let text = Concept::text("42");

    assert_eq!(render(&int, &table), "42");
    assert_eq!(render(&float, &table), "42.0");
    assert_eq!(render(&text, &table), "\"42\"");

    assert_ne!(int.content_id(), float.content_id());
    assert_ne!(int.content_id(), text.content_id());
    assert_ne!(float.content_id(), text.content_id());

    assert_eq!(parse("42", &table).unwrap(), int);
    assert_eq!(parse("42.0", &table).unwrap(), float);
    assert_eq!(parse("\"42\"", &table).unwrap(), text);
}

#[test]
fn json_scalar_is_not_the_matching_ground_concept() {
    let table = SymbolTable::new();
    let json_int = Concept::json(json!(42));
    let plain_int = Concept::int(42);
    assert_ne!(json_int.content_id(), plain_int.content_id());
    assert_eq!(parse("json(42)", &table).unwrap(), json_int);
    assert_eq!(parse("42", &table).unwrap(), plain_int);
}

// ---- names and structure ----

#[test]
fn renders_names_with_the_casing_they_were_interned_with() {
    let table = table_with(&["FriendWith", "Greg", "Keal"]);
    let concept = Concept::call(
        "FriendWith",
        [Concept::named("Greg"), Concept::named("Keal")],
    );
    assert_eq!(render(&concept, &table), "FriendWith<Greg, Keal>");
    roundtrip(&concept, &table);
}

#[test]
fn parse_interns_names_so_they_print_back_as_written() {
    let table = SymbolTable::new();
    let concept = parse("Employment<Greg, Workiva>", &table).unwrap();
    assert_eq!(render(&concept, &table), "Employment<Greg, Workiva>");
    assert_eq!(concept.head_symbol(), Some(SymbolId::of("employment")));
}

#[test]
fn names_are_case_insensitive_for_identity() {
    let table = SymbolTable::new();
    let first = parse("FriendWith<Greg>", &table).unwrap();
    let second = parse("friendwith<greg>", &table).unwrap();
    assert_eq!(first, second);
    // First spelling wins for display.
    assert_eq!(render(&second, &table), "FriendWith<Greg>");
}

#[test]
fn separators_are_ordinary_name_characters() {
    let table = SymbolTable::new();
    let dashed = parse("friend-with<co-op, a_b.c:d>", &table).unwrap();
    assert_eq!(render(&dashed, &table), "friend-with<co-op, a_b.c:d>");
    // Separators are not collapsed: these are different concepts.
    assert_ne!(
        parse("co-op", &table).unwrap().content_id(),
        parse("coop", &table).unwrap().content_id()
    );
}

#[test]
fn unregistered_symbols_render_as_hex_and_read_back() {
    let table = SymbolTable::new();
    let ghost = Concept::named("Ghost");
    let expected = format!("#{:016x}", SymbolId::of("Ghost").as_u64());
    assert_eq!(render(&ghost, &table), expected);
    let back = parse(&expected, &table).unwrap();
    assert_eq!(back, ghost);
    // Reading a hex symbol teaches the table nothing, so it stays hex.
    assert_eq!(render(&back, &table), expected);
}

#[test]
fn reserved_words_cannot_be_written_as_names() {
    let table = table_with(&["True"]);
    let named_true = Concept::named("True");
    let rendered = render(&named_true, &table);
    assert!(rendered.starts_with('#'), "got {rendered}");
    assert_eq!(parse(&rendered, &table).unwrap(), named_true);
    assert_ne!(parse("true", &table).unwrap(), named_true);
}

#[test]
fn deep_nesting_round_trips() {
    let table = table_with(&[
        "Sum", "Map", "Friends", "Height", "Stated", "Keal", "IsSad", "Greg",
    ]);
    let sum = Concept::call(
        "Sum",
        [Concept::call(
            "Map",
            [Concept::named("Friends"), Concept::named("Height")],
        )],
    );
    assert_eq!(roundtrip(&sum, &table), "Sum<Map<Friends, Height>>");

    let stated = Concept::call(
        "Stated",
        [
            Concept::named("Keal"),
            Concept::call("IsSad", [Concept::named("Greg")]),
        ],
    );
    assert_eq!(roundtrip(&stated, &table), "Stated<Keal, IsSad<Greg>>");

    // Twelve levels, to make sure nothing in the recursion is depth-limited.
    let mut deep = Concept::int(0);
    for _ in 0..12 {
        deep = Concept::call("Sum", [deep]);
    }
    assert_eq!(deep.depth(), 13);
    roundtrip(&deep, &table);
}

#[test]
fn zero_arg_compound_is_not_the_atomic_concept() {
    let table = table_with(&["Raining"]);
    let atomic = Concept::named("Raining");
    let applied = nullary(atomic.clone());

    assert_eq!(render(&atomic, &table), "Raining");
    assert_eq!(render(&applied, &table), "Raining<>");
    assert_ne!(atomic, applied);
    assert_ne!(atomic.content_id(), applied.content_id());

    roundtrip(&atomic, &table);
    roundtrip(&applied, &table);
    assert_eq!(parse("Raining<>", &table).unwrap(), applied);
    assert_eq!(parse("Raining", &table).unwrap(), atomic);
}

#[test]
fn compound_heads_are_parenthesized() {
    let table = table_with(&["Sort", "Descending", "Friends", "By"]);
    let concept = Concept::apply(
        Concept::call("Sort", [Concept::named("Descending")]),
        vec![Concept::named("Friends")],
    );
    assert_eq!(roundtrip(&concept, &table), "(Sort<Descending>)<Friends>");

    // Two levels of application, which is where an unparenthesized spelling
    // would become ambiguous.
    let curried = Concept::apply(concept.clone(), vec![Concept::named("By")]);
    assert_eq!(
        roundtrip(&curried, &table),
        "((Sort<Descending>)<Friends>)<By>"
    );

    // A ground head is legal too and needs no parentheses.
    let ground_head = Concept::apply(Concept::int(42), vec![Concept::int(1)]);
    assert_eq!(roundtrip(&ground_head, &table), "42<1>");
}

#[test]
fn holes_round_trip() {
    let table = table_with(&["Symmetric"]);
    assert_eq!(roundtrip(&Concept::hole(0), &table), "?0");
    assert_eq!(
        roundtrip(&Concept::call("Symmetric", [Concept::hole(0)]), &table),
        "Symmetric<?0>"
    );

    // A hole can be the head: this is how a rule quantifies over relations.
    let pattern = Concept::apply(Concept::hole(0), vec![Concept::hole(1), Concept::hole(2)]);
    assert_eq!(roundtrip(&pattern, &table), "?0<?1, ?2>");
    roundtrip(&Concept::hole(u32::MAX), &table);
}

#[test]
fn whitespace_and_newlines_are_insignificant() {
    let table = SymbolTable::new();
    let tight = parse("Stated<Keal,IsSad<Greg>>", &table).unwrap();
    let loose = parse("  Stated <\n  Keal ,\n  IsSad< Greg >\n>  ", &table).unwrap();
    assert_eq!(tight, loose);
}

// ---- text values ----

#[test]
fn text_escapes_round_trip() {
    let table = SymbolTable::new();
    let value = "quote:\" backslash:\\ newline:\n carriage:\r tab:\t done";
    let concept = Concept::text(value);
    let rendered = roundtrip(&concept, &table);
    assert_eq!(
        rendered,
        "\"quote:\\\" backslash:\\\\ newline:\\n carriage:\\r tab:\\t done\""
    );
    assert_eq!(
        parse(&rendered, &table)
            .unwrap()
            .as_ground()
            .unwrap()
            .as_str(),
        Some(value)
    );

    // Notation characters inside a text value stay text.
    let quoted = Concept::text("FriendWith<Greg, Keal>");
    assert_eq!(roundtrip(&quoted, &table), "\"FriendWith<Greg, Keal>\"");
}

#[test]
fn unicode_survives_in_text_and_names() {
    let table = table_with(&["Café", "Ünïcode", "日本語"]);
    roundtrip(&Concept::text("héllo 👋 日本語 \u{1f600}"), &table);
    roundtrip(&Concept::named("Café"), &table);
    let concept = Concept::call(
        "Ünïcode",
        [Concept::named("日本語"), Concept::text("naïve, not naive")],
    );
    assert_eq!(
        roundtrip(&concept, &table),
        "Ünïcode<日本語, \"naïve, not naive\">"
    );
}

// ---- display wrapper ----

#[test]
fn display_wrapper_formats_without_allocating_a_string_first() {
    let table = table_with(&["Add"]);
    let concept = Concept::call("Add", [Concept::int(42), Concept::int(1)]);
    assert_eq!(format!("{}", display(&concept, &table)), "Add<42, 1>");
    assert_eq!(
        format!("{}", ConceptDisplay::new(&concept, &table)),
        "Add<42, 1>"
    );
    assert_eq!(
        format!("{}", display(&concept, &table).truncated(1)),
        "Add<...>"
    );
    assert_eq!(
        display(&concept, &table).to_string(),
        render(&concept, &table)
    );
}

// ---- compact rendering ----

#[test]
fn compact_elides_below_the_requested_depth() {
    let table = table_with(&["Sum", "Map", "Friends", "Height"]);
    let concept = Concept::call(
        "Sum",
        [Concept::call(
            "Map",
            [Concept::named("Friends"), Concept::named("Height")],
        )],
    );
    assert_eq!(render_compact(&concept, &table, 0), "...");
    assert_eq!(render_compact(&concept, &table, 1), "Sum<...>");
    assert_eq!(render_compact(&concept, &table, 2), "Sum<Map<...>>");
    assert_eq!(
        render_compact(&concept, &table, 3),
        "Sum<Map<Friends, Height>>"
    );
    assert_eq!(
        render_compact(&concept, &table, 99),
        render(&concept, &table)
    );
}

#[test]
fn compact_keeps_zero_arg_brackets_and_parenthesized_heads() {
    let table = table_with(&["Raining", "Sort", "Descending", "Friends"]);
    assert_eq!(
        render_compact(&nullary(Concept::named("Raining")), &table, 1),
        "Raining<>"
    );
    let concept = Concept::apply(
        Concept::call("Sort", [Concept::named("Descending")]),
        vec![Concept::named("Friends")],
    );
    // The head sits one level in, exactly as Concept::depth() counts it, so
    // this term needs depth 3 to print in full.
    assert_eq!(concept.depth(), 3);
    assert_eq!(render_compact(&concept, &table, 1), "(Sort<...>)<...>");
    assert_eq!(render_compact(&concept, &table, 2), "(Sort<...>)<Friends>");
    assert_eq!(
        render_compact(&concept, &table, 3),
        "(Sort<Descending>)<Friends>"
    );
}

#[test]
fn compact_truncates_long_bytes_and_json() {
    let table = SymbolTable::new();
    let long: Vec<u8> = (0u8..24).collect();
    let concept = Concept::bytes(&long);
    let full = render(&concept, &table);
    assert_eq!(full.len(), 2 + 48);
    let compact = render_compact(&concept, &table, 4);
    assert_eq!(compact, "0x000102030405060708090a0b0c0d0e0f...");

    let payload = json!({"a": "x".repeat(200)});
    let json_concept = Concept::json(payload);
    let compact_json = render_compact(&json_concept, &table, 4);
    assert!(compact_json.ends_with("..."), "got {compact_json}");
    assert!(compact_json.len() < 80, "got {compact_json}");
    // Short ground values are untouched by compact mode.
    assert_eq!(render_compact(&Concept::int(42), &table, 1), "42");
}

// ---- errors ----

fn error(input: &str) -> ParseError {
    let table = SymbolTable::new();
    parse(input, &table).expect_err("expected a parse error")
}

#[test]
fn empty_input_says_so() {
    assert_eq!(error(""), ParseError::Empty);
    assert_eq!(error("   \n\t "), ParseError::Empty);
    assert_eq!(error("").to_string(), "empty input: expected a concept");
}

#[test]
fn unclosed_argument_list_points_at_the_end() {
    let input = "FriendWith<Greg, Keal";
    let err = error(input);
    assert_eq!(err.offset(), input.len());
    assert_eq!(
        err.to_string(),
        "expected ',' or '>' at offset 21, found end of input"
    );
}

#[test]
fn missing_comma_names_the_offending_token() {
    let input = "FriendWi<Greg Keal>";
    let err = error(input);
    assert_eq!(err.offset(), input.find("Keal").unwrap());
    assert_eq!(
        err.to_string(),
        "expected ',' or '>' at offset 14, found \"Keal\""
    );
}

#[test]
fn unclosed_text_points_at_the_opening_quote() {
    let input = "Says<Greg, \"hello>";
    let err = error(input);
    assert_eq!(err.offset(), 11);
    assert!(
        err.to_string().contains("unterminated text literal"),
        "got {err}"
    );
}

#[test]
fn stray_and_trailing_commas_are_rejected() {
    let leading = error("Friend<,Greg>");
    assert_eq!(leading.offset(), 7);
    assert_eq!(
        leading.to_string(),
        "expected a concept at offset 7, found \",\""
    );

    let trailing = error("Friend<Greg,>");
    assert_eq!(trailing.offset(), 12);
    assert_eq!(
        trailing.to_string(),
        "expected a concept after ',' at offset 12, found \">\""
    );

    let doubled = error("Friend<Greg,,Keal>");
    assert_eq!(doubled.offset(), 12);
}

#[test]
fn trailing_garbage_after_a_complete_term_is_rejected() {
    let err = error("Greg Keal");
    assert_eq!(err.offset(), 5);
    assert_eq!(
        err.to_string(),
        "trailing input at offset 5: found \"Keal\" after a complete concept"
    );

    // Currying has exactly one spelling, and it is not this one.
    let curried = error("A<1><2>");
    assert_eq!(curried.offset(), 4);
}

#[test]
fn parenthesized_head_must_be_applied() {
    let err = error("(Greg)");
    assert_eq!(err.offset(), 6);
    assert_eq!(
        err.to_string(),
        "expected '<' after a parenthesized head at offset 6, found end of input"
    );
    let unclosed = error("(Sort<Descending><Friends>");
    assert!(unclosed.to_string().contains("')'"), "got {unclosed}");
}

#[test]
fn malformed_literals_explain_themselves() {
    let bad_escape = error("\"oops \\q\"");
    assert_eq!(bad_escape.offset(), 6);
    assert!(
        bad_escape.to_string().contains("unknown escape"),
        "got {bad_escape}"
    );

    let odd_hex = error("0xabc");
    assert_eq!(odd_hex.offset(), 0);
    assert!(
        odd_hex.to_string().contains("even number of hex digits"),
        "got {odd_hex}"
    );

    let bad_hex = error("0xzz");
    assert!(bad_hex.to_string().contains("hex digits"), "got {bad_hex}");

    let bad_number = error("1.2.3");
    assert_eq!(bad_number.offset(), 0);
    assert!(
        bad_number.to_string().contains("invalid number literal"),
        "got {bad_number}"
    );

    let bad_hole = error("Symmetric<?x>");
    assert_eq!(bad_hole.offset(), 10);
    assert!(bad_hole.to_string().contains("after '?'"), "got {bad_hole}");

    let bad_symbol = error("#abc");
    assert_eq!(bad_symbol.offset(), 0);
    assert!(
        bad_symbol.to_string().contains("16 hex digits"),
        "got {bad_symbol}"
    );

    let bad_char = error("Friend<@>");
    assert_eq!(bad_char.offset(), 7);
    assert_eq!(bad_char.to_string(), "unexpected character '@' at offset 7");
}

#[test]
fn malformed_json_explains_itself() {
    let unclosed = error("{\"a\":1");
    assert_eq!(unclosed.offset(), 0);
    assert!(
        unclosed.to_string().contains("unterminated json object"),
        "got {unclosed}"
    );

    let unclosed_array = error("Payload<[1, 2>");
    assert!(
        unclosed_array
            .to_string()
            .contains("unterminated json array"),
        "got {unclosed_array}"
    );

    let bad = error("{a}");
    assert_eq!(bad.offset(), 0);
    assert!(bad.to_string().contains("invalid json"), "got {bad}");

    let unclosed_scalar = error("json(42");
    assert!(
        unclosed_scalar.to_string().contains("unterminated json"),
        "got {unclosed_scalar}"
    );
}

#[test]
fn offsets_are_byte_offsets_even_after_multibyte_names() {
    let input = "Ünïcode<Greg Keal>";
    let err = error(input);
    assert_eq!(err.offset(), input.find("Keal").unwrap());
}

// ---- generated corpus ----

/// xorshift64. Deterministic, seeded, no dependency, and good enough to walk a
/// generator through structurally varied shapes.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

const NAMES: [&str; 12] = [
    "Greg",
    "Keal",
    "FriendWith",
    "friend-with",
    "Sum",
    "Map",
    "Sort",
    "Descending",
    "a_b.c:d",
    "Café",
    "日本語",
    "Employment",
];

const TEXTS: [&str; 8] = [
    "",
    "hello",
    "with \"quotes\" and \\slashes\\",
    "line\nbreak\ttab\rreturn",
    "héllo 👋",
    "<not, a, compound>",
    "42",
    "json(1)",
];

fn ground_leaf(rng: &mut Rng) -> Concept {
    match rng.below(12) {
        0 => Concept::bool(rng.below(2) == 0),
        1 => Concept::int(rng.next_u64() as i64),
        2 => Concept::int(i64::MIN),
        3 => Concept::float(rng.next_u64() as i64 as f64 / 7.0),
        // Any bit pattern at all: subnormals, infinities and NaN included,
        // since all three have spellings that have to read back exactly.
        4 => Concept::float(f64::from_bits(rng.next_u64())),
        5 => Concept::text(TEXTS[rng.below(TEXTS.len() as u64) as usize]),
        6 => {
            let len = rng.below(24) as usize;
            let bytes: Vec<u8> = (0..len).map(|_| rng.below(256) as u8).collect();
            Concept::bytes(bytes)
        }
        7 => Concept::datetime(timestamp(
            rng.below(4_000_000_000) as i64 - 2_000_000_000,
            rng.below(1_000_000_000) as u32,
        )),
        8 => Concept::json(
            json!({"k": rng.below(1000), "s": TEXTS[rng.below(TEXTS.len() as u64) as usize]}),
        ),
        9 => Concept::json(json!([rng.below(10), null, true])),
        10 => Concept::json(json!(rng.below(10))),
        _ => Concept::json(json!(null)),
    }
}

fn leaf(rng: &mut Rng) -> Concept {
    match rng.below(6) {
        0 => Concept::hole(rng.below(4) as u32),
        1 | 2 => Concept::named(NAMES[rng.below(NAMES.len() as u64) as usize]),
        _ => ground_leaf(rng),
    }
}

fn generate(rng: &mut Rng, budget: usize) -> Concept {
    if budget == 0 || rng.below(3) == 0 {
        return leaf(rng);
    }
    let head = match rng.below(8) {
        0 => generate(rng, budget - 1),
        1 => Concept::hole(rng.below(4) as u32),
        _ => Concept::named(NAMES[rng.below(NAMES.len() as u64) as usize]),
    };
    let arity = rng.below(4) as usize;
    let args: Vec<Concept> = (0..arity).map(|_| generate(rng, budget - 1)).collect();
    Concept::apply(head, args)
}

#[test]
fn generated_corpus_round_trips() {
    let table = table_with(&NAMES);
    let mut rng = Rng(0x5060_7080_9010_1112);
    let mut seen_compound_head = 0;
    let mut seen_zero_arity = 0;
    let mut seen_holes = 0;
    let mut max_depth = 0;

    for index in 0..250 {
        let concept = generate(&mut rng, 5);
        max_depth = max_depth.max(concept.depth());
        if let Some(head) = concept.head() {
            if head.is_compound() {
                seen_compound_head += 1;
            }
            if concept.arity() == 0 {
                seen_zero_arity += 1;
            }
        }
        if concept.is_hole() {
            seen_holes += 1;
        }
        let text = render(&concept, &table);
        let back = match parse(&text, &table) {
            Ok(back) => back,
            Err(err) => panic!("corpus item {index} failed: {err}\n  text: {text}"),
        };
        assert_eq!(back, concept, "corpus item {index} changed: {text}");
        assert_eq!(back.content_id(), concept.content_id());
        // Rendering is a function of the concept, not of parse history.
        assert_eq!(render(&back, &table), text);
    }

    // The corpus is only worth something if it actually covers the hard shapes.
    assert!(
        seen_compound_head > 0,
        "corpus never generated a compound head"
    );
    assert!(
        seen_zero_arity > 0,
        "corpus never generated a zero-arg compound"
    );
    assert!(seen_holes > 0, "corpus never generated a bare hole");
    assert!(
        max_depth >= 4,
        "corpus stayed shallow: max depth {max_depth}"
    );
}

#[test]
fn generated_corpus_round_trips_through_a_cold_table() {
    // Same corpus, but nothing is interned up front: every name renders as
    // #hex and has to read back through the symbol literal path.
    let table = SymbolTable::new();
    let mut rng = Rng(0x0bad_c0de_dead_beef);
    for index in 0..250 {
        let concept = generate(&mut rng, 4);
        let text = render(&concept, &table);
        let back = match parse(&text, &table) {
            Ok(back) => back,
            Err(err) => panic!("cold corpus item {index} failed: {err}\n  text: {text}"),
        };
        assert_eq!(back, concept, "cold corpus item {index} changed: {text}");
    }
}

#[test]
fn compact_rendering_of_the_corpus_stays_bounded() {
    let table = table_with(&NAMES);
    let mut rng = Rng(0x1234_5678_9abc_def0);
    for _ in 0..200 {
        let concept = generate(&mut rng, 6);
        // Compact mode has two independent budgets: depth, and the size of a
        // single byte or json payload. Once the depth budget covers the whole
        // term, only the payload budget can still elide anything, so a term
        // short enough to have no oversized payload must render identically.
        let full = render(&concept, &table);
        if full.len() <= 34 {
            assert_eq!(render_compact(&concept, &table, concept.depth()), full);
        }
        let compact = render_compact(&concept, &table, 2);
        assert!(!compact.is_empty());
        if concept.depth() > 3 {
            assert!(
                compact.contains("..."),
                "expected elision in {compact} for depth {}",
                concept.depth()
            );
        }
    }
}
