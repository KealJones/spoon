//! Source Concepts and their first executable research pipeline.
//!
//! A source is ordinary Spoon data. The source identity, its supported
//! operations, and the pipeline that uses it all live in the same Concept
//! store. Rust natives provide the effectful edges, while a composed
//! realization stacks those edges into a reusable path.

use std::sync::Arc;

use chrono::Utc;
use spoon_concept::{
    Activation, Concept, ConceptMeta, Effect, Ground, Provenance, Realization, RealizationSpec,
    SymbolId, Tier,
};
use spoon_eval::{Arity, Ctx, EvalResult, NativeRegistry, native_error};
use spoon_store::Store;

use crate::text::want_text;

const WIKIDATA: &str = "wikidata";
const RESEARCH_SEARCH: &str = "research-search";

/// Register the effectful and pure edges used by source compositions.
pub fn register(registry: &mut NativeRegistry) {
    registry.register(
        "source-query-url",
        query_url,
        Arity::Exact(2),
        spoon_eval::ArgStrategy::Eager,
        Effect::Read,
        "turn a source and text query into a source request URL",
    );
    registry.register(
        "source-register",
        register_source,
        Arity::Exact(3),
        spoon_eval::ArgStrategy::Eager,
        Effect::Write,
        "retain a named source from a URL template and credential reference",
    );
    registry.register(
        "source-evidence",
        evidence,
        Arity::Exact(3),
        spoon_eval::ArgStrategy::Eager,
        Effect::Write,
        "persist an external source payload as evidence and return a research result",
    );
}

/// Seed the source vocabulary, source relationships, and the composed research
/// pipeline. This is idempotent and preserves realization evidence on restart.
pub fn seed(store: &Store) -> spoon_store::Result<()> {
    let now = Utc::now();
    let wikidata = Concept::named(WIKIDATA);
    let source = Concept::call("source", [wikidata.clone()]);

    describe(
        store,
        wikidata.clone(),
        ["Wikidata"],
        "a public structured knowledge graph source",
    )?;
    for (concept, forms, note) in vec![
        (
            Concept::named(RESEARCH_SEARCH),
            vec!["research", "research search"],
            "search a retained source Concept",
        ),
        (
            Concept::named("source-query-url"),
            vec!["source query URL"],
            "construct a request URL for a source",
        ),
        (
            Concept::named("source-register"),
            vec!["register source"],
            "retain a user-provided source URL template",
        ),
        (
            Concept::named("source-evidence"),
            vec!["source evidence"],
            "persist a source response with provenance",
        ),
        (
            Concept::named("research-result"),
            vec!["research result"],
            "a normalized result from a source pipeline",
        ),
        (
            Concept::named("evidence"),
            vec!["evidence"],
            "a source payload retained for later inspection",
        ),
        (
            Concept::named("supports"),
            vec!["supports"],
            "a source or capability relationship",
        ),
        (
            Concept::named("credential-ref"),
            vec!["credential reference"],
            "a non-secret reference to runtime credentials, such as an environment variable",
        ),
    ] {
        describe(store, concept, forms, note)?;
    }
    for name in [
        WIKIDATA,
        "source",
        "source-endpoint",
        RESEARCH_SEARCH,
        "source-query-url",
        "source-register",
        "source-evidence",
        "research-result",
        "evidence",
        "supports",
        "credential-ref",
        "entity-search",
        "statement-fetch",
        "graph-query",
    ] {
        store.register_symbol(name)?;
    }

    assert_once(store, source, Provenance::Bootstrap)?;
    assert_once(
        store,
        Concept::call(
            "source-endpoint",
            [
                wikidata.clone(),
                Concept::text(
                    "https://www.wikidata.org/w/api.php?action=wbsearchentities&search={query}&language=en&format=json",
                ),
            ],
        ),
        Provenance::Bootstrap,
    )?;
    for capability in [
        "entity-search",
        "statement-fetch",
        "graph-query",
        RESEARCH_SEARCH,
    ] {
        let relation = Concept::call("supports", [wikidata.clone(), Concept::named(capability)]);
        assert_once(store, relation, Provenance::Bootstrap)?;
    }

    // The source is a normal composition. Its generic shape can support more
    // sources later by extending source-query-url, without changing the
    // research capability itself.
    let query = Concept::hole(1);
    let fetch = Concept::call(
        "io-fetch-json",
        [Concept::call(
            "source-query-url",
            [Concept::hole(0), query.clone()],
        )],
    );
    let body = Concept::call("source-evidence", [Concept::hole(0), query, fetch]);
    store.put_realization(&Realization {
        target: Concept::named(RESEARCH_SEARCH),
        name: "source-research/http-json".into(),
        spec: RealizationSpec::Composed { body },
        effect: Effect::Network,
        activation: Activation::new(now),
        provenance: Provenance::Bootstrap,
        tier: Tier::Kernel,
    })?;

    Ok(())
}

fn describe<I, S>(store: &Store, concept: Concept, forms: I, note: &str) -> spoon_store::Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    store.put_meta(
        &ConceptMeta::new(concept, Provenance::Bootstrap, Tier::Kernel, Utc::now())
            .with_surface_forms(forms)
            .with_note(note),
    )
}

fn assert_once(store: &Store, concept: Concept, provenance: Provenance) -> spoon_store::Result<()> {
    if !store.holds(&concept)? {
        store.assert_concept(&concept, provenance, None, None)?;
    }
    Ok(())
}

fn query_url(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let source = &args[0];
    let query = want_text("source-query-url", &args[1])?;
    let Some(endpoint) = source_fact(ctx.store(), "source-endpoint", source)? else {
        return Err(native_error(
            "source-query-url",
            format!("unsupported source {}", source.content_id().short()),
        ));
    };
    let template = endpoint
        .arg(1)
        .and_then(|value| value.as_ground())
        .and_then(Ground::as_str)
        .ok_or_else(|| native_error("source-query-url", "source endpoint is not text"))?;
    if !template.contains("{query}") {
        return Err(native_error(
            "source-query-url",
            "source endpoint template must contain {query}",
        ));
    }
    Ok(Concept::text(
        template.replace("{query}", &percent_encode(query)),
    ))
}

fn register_source(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let name = want_text("source-register", &args[0])?;
    if name.trim().is_empty() {
        return Err(native_error(
            "source-register",
            "source name cannot be empty",
        ));
    }
    let endpoint = want_text("source-register", &args[1])?;
    if !(endpoint.starts_with("https://") || endpoint.starts_with("http://")) {
        return Err(native_error(
            "source-register",
            "source endpoint must start with http:// or https://",
        ));
    }
    if !endpoint.contains("{query}") {
        return Err(native_error(
            "source-register",
            "source endpoint template must contain {query}",
        ));
    }
    let env_name = want_text("source-register", &args[2])?;
    let source = Concept::named(name);
    ctx.store().register_symbol(name)?;
    ctx.store().put_meta(
        &ConceptMeta::new(
            source.clone(),
            Provenance::User { episode: None },
            Tier::Provisional,
            Utc::now(),
        )
        .with_surface_forms([name])
        .with_note("a user-registered source URL template"),
    )?;
    ctx.store().assert_concept(
        &Concept::call("source", [source.clone()]),
        Provenance::User { episode: None },
        None,
        None,
    )?;
    ctx.store().assert_concept(
        &Concept::call("source-endpoint", [source.clone(), Concept::text(endpoint)]),
        Provenance::User { episode: None },
        None,
        None,
    )?;
    ctx.store().assert_concept(
        &Concept::call(
            "supports",
            [source.clone(), Concept::named(RESEARCH_SEARCH)],
        ),
        Provenance::User { episode: None },
        None,
        None,
    )?;
    if !env_name.is_empty() {
        ctx.store().assert_concept(
            &Concept::call("credential-ref", [source.clone(), Concept::text(env_name)]),
            Provenance::User { episode: None },
            None,
            None,
        )?;
    }
    Ok(source)
}

fn source_fact(
    store: &Store,
    head: &str,
    source: &Concept,
) -> Result<Option<Concept>, spoon_eval::EvalError> {
    Ok(store
        .concepts_by_head(SymbolId::of(head), 128)?
        .into_iter()
        .find(|concept| concept.arg(0) == Some(source)))
}

fn evidence(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let source = args[0].clone();
    let query = args[1].clone();
    let payload = args[2].clone();
    let evidence = Concept::call("evidence", [source.clone(), query.clone(), payload]);
    let source_name = if source.as_symbol() == Some(SymbolId::of(WIKIDATA)) {
        WIKIDATA.to_string()
    } else {
        format!("concept:{}", source.content_id().short())
    };
    ctx.store().assert_concept(
        &evidence,
        Provenance::External {
            source: Arc::from(source_name),
        },
        None,
        None,
    )?;
    Ok(Concept::call("research-result", [source, query, evidence]))
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(char::from(b"0123456789ABCDEF"[(byte >> 4) as usize]));
            out.push(char::from(b"0123456789ABCDEF"[(byte & 0x0f) as usize]));
        }
    }
    out
}
