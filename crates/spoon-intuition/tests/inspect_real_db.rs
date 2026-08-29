use spoon_intuition::{IntuitionStore, RecallKind};

#[test]
#[ignore = "inspection helper: point SPOON_INSPECT_DB at a real database"]
fn print_ranked_procedures() {
    let path = std::env::var("SPOON_INSPECT_DB").expect("set SPOON_INSPECT_DB");
    let query = std::env::var("SPOON_INSPECT_QUERY").expect("set SPOON_INSPECT_QUERY");
    let store = IntuitionStore::open(&path).unwrap();

    let kind = match std::env::var("SPOON_INSPECT_KIND").as_deref() {
        Ok("concept") => RecallKind::Concept,
        Ok("episode") => RecallKind::Episode,
        _ => RecallKind::Procedure,
    };
    let ranked = store.rank_of_kind(&query, kind, 123).unwrap();
    println!("query: {query}");
    println!("returned {} procedures", ranked.len());
    for (position, candidate) in ranked.iter().enumerate() {
        println!(
            "{:>3}. activation={:>6.4} sim={:>6.3} terms={:<2} {}",
            position + 1,
            candidate.activation,
            candidate.similarity,
            candidate.terms_matched,
            candidate.text.chars().take(52).collect::<String>(),
        );
    }
}
