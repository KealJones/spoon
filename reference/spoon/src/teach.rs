//! `spoon teach`: a teacher curriculum against the persistent Brain. The
//! teacher seat writes specs, phrasings, stances and concept models; Spoon
//! code synthesizes the programs, stores pairs, facts and stances, and checks
//! each lesson through the interior. Finished lesson keys live in kv
//! `teach.done`, so a capped run can be resumed. Online only.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use serde::{Deserialize, Serialize};
use spoon_core::types::*;
use spoon_mind::brain::Brain;
use spoon_mind::teacher::{Lesson, Teacher};

/// kv key of the ledger: lesson key -> what happened.
pub const DONE_KEY: &str = "teach.done";

pub const DEFAULT_THEMES: &[&str] = &[
    "everyday text and number helpers",
    "facts about common animals and objects",
    "how people phrase requests casually",
    "human topics like friendship and work",
];

/// Messy phrasings asked for per capability or phrasings lesson.
const PHRASINGS_PER_LESSON: usize = 6;
/// Discourse session the lessons run in.
const SESSION: &str = "teach";
/// Facts the teacher states are exportable, unlike the user's.
const SOURCE: &str = "teacher";

pub struct TeachArgs {
    pub lessons: usize,
    pub themes: Vec<String>,
    pub max_minutes: u64,
    pub dry_run: bool,
}

/// One ledger line under `teach.done`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Done {
    pub ok: bool,
    pub at: i64,
    pub note: String,
}

#[derive(Default)]
struct Summary {
    attempted: BTreeMap<&'static str, usize>,
    succeeded: BTreeMap<&'static str, usize>,
    /// (verb, program) per synthesized capability.
    learned: Vec<(String, String)>,
    pairs: usize,
    stances: usize,
    facts: usize,
    skipped: usize,
    stopped_early: bool,
}

pub async fn run(brain: Arc<Brain>, args: TeachArgs) -> anyhow::Result<()> {
    let started = Instant::now();
    if brain.cfg.offline {
        bail!("spoon teach needs the teacher seat: drop --offline");
    }
    let teacher = brain
        .teacher()
        .ok_or_else(|| anyhow!("teacher model '{}' is not answering at Ollama; is it running?", brain.cfg.teacher_model))?;
    let themes: Vec<String> = if args.themes.is_empty() {
        DEFAULT_THEMES.iter().map(|t| t.to_string()).collect()
    } else {
        args.themes.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
    };

    let known_verbs = brain.known_verbs();
    println!("curriculum: {} lessons on {} ({} known verbs)", args.lessons, themes.join("; "), known_verbs.len());
    let lessons = teacher.curriculum(args.lessons, &known_verbs, &themes).await?;
    for (i, lesson) in lessons.iter().enumerate() {
        println!("  {:>2}. {}", i + 1, describe(lesson));
    }
    if args.dry_run {
        println!("dry run: nothing taught");
        return Ok(());
    }

    let mut done = load_done(&brain)?;
    let deadline = started + Duration::from_secs(args.max_minutes * 60);
    let mut summary = Summary::default();
    for (i, lesson) in lessons.iter().enumerate() {
        if Instant::now() >= deadline {
            summary.stopped_early = true;
            println!("time cap of {} min reached", args.max_minutes);
            break;
        }
        let key = lesson.key();
        if done.contains_key(&key) {
            summary.skipped += 1;
            println!("[{}/{}] done before, skipping: {}", i + 1, lessons.len(), describe(lesson));
            continue;
        }
        println!("[{}/{}] {}", i + 1, lessons.len(), describe(lesson));
        *summary.attempted.entry(lesson.kind()).or_default() += 1;
        let (ok, note) = match teach_lesson(&brain, teacher, lesson, &mut summary).await {
            Ok(note) => {
                *summary.succeeded.entry(lesson.kind()).or_default() += 1;
                (true, note)
            }
            Err(e) => (false, e.to_string()),
        };
        println!("      {}: {note}", if ok { "ok" } else { "failed" });
        done.insert(key, Done { ok, at: now_ms(), note });
        save_done(&brain, &done)?;
    }

    print_summary(&brain, &summary, started.elapsed());
    Ok(())
}

// ---------------------------------------------------------------------------
// Lessons
// ---------------------------------------------------------------------------

async fn teach_lesson(brain: &Brain, teacher: &Teacher, lesson: &Lesson, s: &mut Summary) -> anyhow::Result<String> {
    match lesson {
        Lesson::Capability { description, signature_hint } => capability(brain, teacher, description, signature_hint, s).await,
        Lesson::Facts { sce } => facts(brain, sce, s).await,
        Lesson::Phrasings { sce, verb } => phrasings(brain, teacher, sce, verb, s).await,
        Lesson::Opinion { topic } => opinion(brain, teacher, topic, s).await,
        Lesson::Concept { noun } => concept(brain, teacher, noun, s).await,
    }
}

/// The teacher writes the spec (typed examples); the synthesizer writes the
/// program; the teacher then supplies messy phrasings for the new command.
/// Nothing the teacher emits is ever stored as program text.
async fn capability(brain: &Brain, teacher: &Teacher, description: &str, hint: &str, s: &mut Summary) -> anyhow::Result<String> {
    let known = brain.known_verbs();
    let hinted = hint.split('(').next().and_then(as_verb);
    let capability = hinted.clone().unwrap_or_else(|| description.to_string());
    let context = if hint.trim().is_empty() {
        description.to_string()
    } else {
        format!("{description}. Signature hint: {hint}")
    };
    let mut spec = teacher.spec_for(&capability, &context, &brain.known_types()).await?;
    println!(
        "      spec: {}({}) -> {}   verbs: {}",
        spec.name_hint,
        spec.params.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", "),
        spec.ret,
        spec.verbs.join(", ")
    );
    for ex in &spec.examples {
        println!("        {} -> {}", ex.inputs.iter().map(literal).collect::<Vec<_>>().join(", "), literal(&ex.output));
    }

    // The verb that routes to it: the curriculum's own name from the hint
    // (`word_count` -> `word-count`), else the teacher's canonical verbs,
    // else the spec's name hint. Never one Spoon already answers to.
    let usable = |v: &String| as_verb(v).filter(|v| !known.contains(v));
    let verb = hinted
        .iter()
        .chain(spec.verbs.iter())
        .chain(std::iter::once(&spec.name_hint))
        .find_map(usable)
        .ok_or_else(|| anyhow!("no usable verb: candidates {:?} are known or malformed", spec.verbs))?;
    let mut verbs: Vec<String> = spec.verbs.iter().filter_map(usable).collect();
    verbs.dedup();
    spec.verbs = verbs;

    let outcome = brain.learn_from_spec(&verb, spec.clone())?;
    s.learned.push((verb.clone(), outcome.description.clone()));
    println!("      learned: {verb} = {}", outcome.description);

    // The meaning handed to the phrasings teacher carries an example so a
    // short command like `square 3` is not misread as a place.
    let meaning = match spec.examples.first() {
        Some(ex) => format!(
            "{}; for example {} gives {}",
            spec.description,
            ex.inputs.iter().map(literal).collect::<Vec<_>>().join(", "),
            literal(&ex.output)
        ),
        None => spec.description.clone(),
    };
    let phrasing_note = match canonical_command(&verb, &spec) {
        Some(sce) => match teacher.phrasings_for(&sce, &verb, &meaning, PHRASINGS_PER_LESSON).await {
            Ok(pairs) => {
                let n = brain.add_pairs(&pairs).await?;
                s.pairs += n;
                format!("{n} phrasings for '{sce}'")
            }
            Err(e) => format!("phrasings skipped: {e}"),
        },
        None => "phrasings skipped: first parameter has no SCE literal form".into(),
    };
    Ok(format!("{verb} = {}; {phrasing_note}", outcome.description))
}

/// SCE statements go straight into memory as teacher facts.
async fn facts(brain: &Brain, sce: &[String], s: &mut Summary) -> anyhow::Result<String> {
    let mut asserted = 0;
    let mut failed = Vec::new();
    for sentence in sce {
        match brain.assert_sce(SESSION, sentence, SOURCE).await {
            Ok(n) if n > 0 => asserted += 1,
            Ok(_) => failed.push(format!("'{sentence}' contradicts memory")),
            Err(e) => failed.push(e.to_string()),
        }
    }
    s.facts += asserted;
    let note = format!("{asserted}/{} asserted{}", sce.len(), suffix("; failed: ", &failed));
    if asserted == 0 {
        bail!("{note}");
    }
    Ok(note)
}

/// Messy ways to say an SCE sentence Spoon already understands.
async fn phrasings(brain: &Brain, teacher: &Teacher, sce: &str, verb: &str, s: &mut Summary) -> anyhow::Result<String> {
    let sce = &canonical_sce(brain, sce).ok_or_else(|| anyhow!("'{sce}' is not SCE Spoon can parse"))?;
    // Pairs must point at something Spoon can do, or the ears learn to hear
    // a command nobody answers.
    if !brain.known_verbs().iter().any(|k| k == verb) {
        bail!("verb '{verb}' is not something Spoon can do yet");
    }
    let pairs = teacher.phrasings_for(sce, verb, "", PHRASINGS_PER_LESSON).await?;
    for p in &pairs {
        println!("        {:?}", p.utterance);
    }
    let n = brain.add_pairs(&pairs).await?;
    s.pairs += n;
    if n == 0 {
        bail!("{} phrasings, none new", pairs.len());
    }
    Ok(format!("{n} new phrasings for '{sce}'"))
}

/// A stance the interior can answer with; verified through a real turn.
async fn opinion(brain: &Brain, teacher: &Teacher, topic: &str, s: &mut Summary) -> anyhow::Result<String> {
    let taught = teacher.stance_for(topic, "").await?;
    let stance = brain.add_stance(&taught)?;
    s.stances += 1;
    println!("      stance: {} ({:.2}; {} reasons)", stance.stance, stance.confidence, stance.reasons.len());

    let question = format!("What does Assistant think about {}?", topic_keyword(topic));
    let r = brain.turn(SESSION, &question).await?;
    let has_view = r.episode.response.moves.iter().any(|m| matches!(m, Move::Opinion { .. }));
    if !has_view {
        bail!("stance stored, but '{question}' still answers: {}", clip(&r.text, 100));
    }
    Ok(format!("'{question}' -> {}", clip(&r.text, 100)))
}

/// The teacher's concept model becomes SCE the interior owns: parents as
/// universals, properties as has-facts, alternate nouns as synonyms.
async fn concept(brain: &Brain, teacher: &Teacher, lesson_noun: &str, s: &mut Summary) -> anyhow::Result<String> {
    let model = teacher.concept_for(lesson_noun.trim(), "", &brain.known_types()).await?;
    println!("      model: {} ({:?}) extends {:?} nouns {:?}", model.id.0, kind_label(&model.kind), model.extends, model.nouns);

    // The head noun must be a word SCE can carry; a curriculum "noun" that is
    // really a phrase falls back to the model's own nouns, then its id.
    let noun = as_noun(lesson_noun)
        .or_else(|| model.nouns.iter().find_map(|n| as_noun(n)))
        .unwrap_or_else(|| pascal_to_noun(&model.id.0));

    let mut sentences: Vec<String> = model
        .extends
        .iter()
        .map(|parent| pascal_to_noun(&parent.0))
        .filter(|p| p != &noun)
        .map(|p| format!("Every {noun} is {} {p}.", article(&p)))
        .collect();
    if let ConceptKind::Structure { properties } = &model.kind {
        for p in properties {
            let prop = p.name.trim().to_lowercase().replace(['_', ' '], "-");
            sentences.push(format!("{} {noun} has {} {prop}.", capitalize(article(&noun)), article(&prop)));
        }
    }
    let mut asserted = 0;
    let mut failed = Vec::new();
    for sce in &sentences {
        match brain.assert_sce(SESSION, sce, SOURCE).await {
            Ok(n) if n > 0 => asserted += 1,
            Ok(_) => failed.push(format!("'{sce}' contradicts memory")),
            Err(e) => failed.push(e.to_string()),
        }
    }
    s.facts += asserted;

    let mut synonyms = 0;
    let mut alts: Vec<String> = model.nouns.iter().filter_map(|n| as_noun(n)).filter(|n| *n != noun).collect();
    alts.dedup();
    for alt in alts {
        let r = brain.turn(SESSION, &format!("\"{alt}\" means \"{noun}\".")).await?;
        if r.episode.response.moves.iter().any(|m| matches!(m, Move::Learned { .. })) {
            synonyms += 1;
        }
    }

    let note = format!("{asserted}/{} facts, {synonyms} synonyms{}", sentences.len(), suffix("; failed: ", &failed));
    if asserted + synonyms == 0 {
        bail!("nothing usable in the model: {note}");
    }
    Ok(note)
}

// ---------------------------------------------------------------------------
// Ledger and summary
// ---------------------------------------------------------------------------

fn load_done(brain: &Brain) -> anyhow::Result<BTreeMap<String, Done>> {
    Ok(brain
        .store
        .lock()
        .kv_get(DONE_KEY)?
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

fn save_done(brain: &Brain, done: &BTreeMap<String, Done>) -> anyhow::Result<()> {
    brain.store.lock().kv_set(DONE_KEY, &serde_json::to_value(done)?)
}

fn print_summary(brain: &Brain, s: &Summary, elapsed: Duration) {
    let m = brain.metrics();
    println!();
    println!("teach summary ({:.0}s{})", elapsed.as_secs_f64(), if s.stopped_early { ", time cap" } else { "" });
    for (kind, attempted) in &s.attempted {
        println!("  {kind:<11} {}/{attempted}", s.succeeded.get(kind).copied().unwrap_or(0));
    }
    if s.skipped > 0 {
        println!("  skipped     {} (already in {DONE_KEY})", s.skipped);
    }
    println!("  learned actions: {}", s.learned.len());
    for (verb, program) in &s.learned {
        println!("    {verb} = {program}");
    }
    println!("  pairs added:     {}", s.pairs);
    println!("  stances added:   {}", s.stances);
    println!("  facts asserted:  {}", s.facts);
    println!("  teacher LLM calls: {}   ears: {}   mouth: {}   interior: {}", m.teacher_llm_calls, m.ears_llm_calls, m.mouth_llm_calls, m.interior_llm_calls);
    println!("  store: {} actions, {} pairs, {} facts, {} stances", m.store.actions, m.store.pairs, m.store.facts, m.store.stances);
    println!("export with: spoon export --out seed.json");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn describe(lesson: &Lesson) -> String {
    match lesson {
        Lesson::Capability { description, signature_hint } if signature_hint.is_empty() => format!("capability: {description}"),
        Lesson::Capability { description, signature_hint } => format!("capability: {description} ({signature_hint})"),
        Lesson::Facts { sce } => format!("facts: {}", sce.join(" ")),
        Lesson::Phrasings { sce, verb } => format!("phrasings: {sce} ({verb})"),
        Lesson::Opinion { topic } => format!("opinion: {topic}"),
        Lesson::Concept { noun } => format!("concept: {noun}"),
    }
}

/// A usable SCE verb lemma: lowercase letters, hyphens between words
/// (`word count` and `word_count` become `word-count`).
pub fn as_verb(raw: &str) -> Option<String> {
    let v = raw.trim().to_lowercase().replace(['_', ' '], "-");
    let ok = !v.is_empty()
        && v.len() <= 30
        && v.split('-').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase()))
        && !matches!(v.as_str(), "assistant" | "user" | "is" | "be" | "do" | "does");
    ok.then_some(v)
}

/// A noun SCE can carry: one or two lowercase words (joined by a hyphen),
/// leading article dropped. Phrases are rejected.
pub fn as_noun(raw: &str) -> Option<String> {
    let lower = raw.trim().to_lowercase();
    let rest = ["a ", "an ", "the "].iter().find_map(|art| lower.strip_prefix(art)).unwrap_or(&lower);
    let words: Vec<&str> = rest.split([' ', '_', '-']).filter(|w| !w.is_empty()).collect();
    let ok = (1..=2).contains(&words.len())
        && words.iter().all(|w| w.chars().all(|c| c.is_ascii_lowercase()))
        && rest.len() <= 25;
    ok.then(|| words.join("-"))
}

/// The teacher's canonical sentence as SCE the gate accepts, or None. Small
/// models drop the quotes around a text object (`Assistant, reverse hello!`),
/// which is the one repair tried: quote a bare lowercase object and keep it
/// only if that parses.
fn canonical_sce(brain: &Brain, sce: &str) -> Option<String> {
    let sce = sce.trim();
    if brain.parses(sce) {
        return Some(sce.to_string());
    }
    let body = sce.strip_prefix("Assistant, ")?.strip_suffix('!')?;
    let (verb, object) = body.split_once(' ')?;
    let bare = !object.contains('"') && object.chars().all(|c| c.is_ascii_lowercase());
    let quoted = format!("Assistant, {verb} \"{object}\"!");
    (bare && brain.parses(&quoted)).then_some(quoted)
}

/// `Assistant, <verb> <first input of the first example>!`, the command the
/// phrasings map to. Commands take one object; the rest is elicited.
fn canonical_command(verb: &str, spec: &Spec) -> Option<String> {
    let first = spec.examples.first()?.inputs.first()?;
    let lit = match first {
        Value::Int(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Text(t) if !t.contains('"') && !t.trim().is_empty() => format!("\"{t}\""),
        Value::Name(n) if n.chars().all(|c| c.is_alphanumeric()) => n.clone(),
        _ => return None,
    };
    Some(format!("Assistant, {verb} {lit}!"))
}

fn literal(v: &Value) -> String {
    match v {
        Value::Text(t) => format!("{t:?}"),
        other => other.to_string(),
    }
}

const STOPWORDS: &[&str] = &[
    "a", "an", "the", "of", "to", "in", "on", "at", "for", "and", "or", "it", "is", "are", "be", "about", "should", "vs",
    "versus", "with", "from", "do", "does", "you", "your", "we", "our", "i", "my", "how", "what", "why", "when", "than",
    "good", "bad", "ok", "better", "worse", "really", "very", "people", "whether", "if", "ever", "not", "no", "yes",
];

/// The word of a topic the opinion question asks about; `store.stances`
/// matches it as a substring of the stored topic.
pub fn topic_keyword(topic: &str) -> String {
    let words: Vec<String> = topic
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    words
        .iter()
        .filter(|w| !STOPWORDS.contains(&w.as_str()))
        .max_by_key(|w| w.len())
        .or_else(|| words.first())
        .cloned()
        .unwrap_or_else(|| topic.trim().to_lowercase())
}

/// `EmailAddress` -> `email-address`.
fn pascal_to_noun(id: &str) -> String {
    let mut out = String::with_capacity(id.len() + 4);
    for (i, c) in id.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('-');
        }
        out.extend(c.to_lowercase());
    }
    out.replace(['_', ' '], "-")
}

fn article(noun: &str) -> &'static str {
    if noun.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" }
}

fn capitalize(s: &str) -> String {
    let mut cs = s.chars();
    match cs.next() {
        Some(f) => f.to_uppercase().chain(cs).collect(),
        None => String::new(),
    }
}

fn kind_label(kind: &ConceptKind) -> &'static str {
    match kind {
        ConceptKind::Primitive { .. } => "primitive",
        ConceptKind::Structure { .. } => "structure",
        ConceptKind::Enum { .. } => "enum",
        ConceptKind::Entity => "entity",
    }
}

fn suffix(label: &str, items: &[String]) -> String {
    if items.is_empty() { String::new() } else { format!("{label}{}", items.join("; ")) }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{head}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbs_are_lowercase_hyphenated_words() {
        assert_eq!(as_verb("Double"), Some("double".into()));
        assert_eq!(as_verb("word_count"), Some("word-count".into()));
        assert_eq!(as_verb("count words "), Some("count-words".into()));
        assert_eq!(as_verb("word_count(Text) -> Int".split('(').next().unwrap()), Some("word-count".into()));
        assert_eq!(as_verb(""), None);
        assert_eq!(as_verb("do"), None);
        assert_eq!(as_verb("3x"), None);
        assert_eq!(as_verb("fn => x"), None);
    }

    #[test]
    fn canonical_command_uses_the_first_input() {
        let mut spec = Spec {
            id: "t".into(),
            name_hint: "shout".into(),
            verbs: vec![],
            phrasings: vec![],
            params: vec![Type::Text, Type::Int],
            param_names: vec![],
            ret: Type::Text,
            examples: vec![Example { inputs: vec![Value::text("hi"), Value::Int(2)], output: Value::text("HIHI") }],
            description: String::new(),
            source: "teacher".into(),
        };
        assert_eq!(canonical_command("shout", &spec).as_deref(), Some("Assistant, shout \"hi\"!"));
        spec.examples[0].inputs[0] = Value::List(vec![]);
        assert_eq!(canonical_command("shout", &spec), None);
    }

    #[test]
    fn nouns_are_one_or_two_words() {
        assert_eq!(as_noun("recipe"), Some("recipe".into()));
        assert_eq!(as_noun("a Recipe"), Some("recipe".into()));
        assert_eq!(as_noun("email address"), Some("email-address".into()));
        assert_eq!(as_noun("a friendship bond that requires mutual trust"), None);
        assert_eq!(as_noun("3d"), None);
        assert_eq!(as_noun(""), None);
    }

    #[test]
    fn topic_keyword_skips_stopwords() {
        assert_eq!(topic_keyword("is coffee healthy"), "healthy");
        assert_eq!(topic_keyword("friendship"), "friendship");
        assert_eq!(topic_keyword("working from home"), "working");
        assert_eq!(topic_keyword("the"), "the");
    }

    #[test]
    fn concept_sentences_read_as_sce() {
        assert_eq!(pascal_to_noun("EmailAddress"), "email-address");
        assert_eq!(pascal_to_noun("Animal"), "animal");
        assert_eq!(format!("Every recipe is {} {}.", article("document"), "document"), "Every recipe is a document.");
        assert_eq!(format!("{} apple has {} core.", capitalize(article("apple")), article("core")), "An apple has a core.");
    }
}
