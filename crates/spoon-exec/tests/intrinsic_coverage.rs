//! Structural guards over the intrinsic surface.
//!
//! Three failures are easy to introduce and hard to notice by reading a diff:
//! an `IntrinsicOp` variant that is declared but never evaluated, one that is
//! evaluated but never tested, and one that is evaluated but absent from the
//! Teacher lesson grammar. The second is the most dangerous, because learned
//! procedures execute these operations and an untested operation is an
//! unproven claim about what a procedure will do. The third decides whether an
//! operation is reachable by a learned procedure at all, which is the
//! difference between an implemented row and a Rust-only one in
//! `PRIMITIVE-CAPABILITY-INVENTORY.md`.
//!
//! This file checks all three by reading the source tree, so the guarantee
//! survives anyone adding a variant without also adding coverage. It
//! deliberately reads files rather than using a macro, because the point is to
//! catch a variant whose author forgot every other step.

use spoon_core::IntrinsicOp;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The workspace `crates/` directory, resolved from this crate's manifest so
/// the test does not depend on the working directory it is invoked from.
fn crates_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("spoon-exec sits under crates/")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("read {path:?}: {error}"))
}

fn rust_sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // `target` can contain generated sources that would skew the
                // scan, and it is never part of the authored tree.
                if path.file_name().is_some_and(|name| name == "target") {
                    continue;
                }
                walk(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(&crates_root(), &mut found);
    found
}

/// Every `IntrinsicOp` variant name.
///
/// These come from the operation table rather than from parsing the source,
/// because the table generates the enum: a variant that exists is necessarily
/// in `ALL`.
fn declared_variants() -> BTreeSet<&'static str> {
    IntrinsicOp::ALL
        .iter()
        .map(|op| op.variant_name())
        .collect()
}

/// Text of everything that counts as a test: files under a `tests/` directory,
/// plus the `#[cfg(test)]` tail of any other source file.
fn test_sources() -> String {
    let mut blob = String::new();
    for path in rust_sources() {
        let text = read(&path);
        let is_integration_test = path
            .components()
            .any(|component| component.as_os_str() == "tests");
        if is_integration_test {
            blob.push_str(&text);
        } else if let Some(index) = text.find("#[cfg(test)]") {
            blob.push_str(&text[index..]);
        }
        blob.push('\n');
    }
    blob
}

#[test]
fn every_declared_intrinsic_is_evaluated() {
    let declared = declared_variants();
    assert!(
        declared.len() > 100,
        "parsed only {} variants, so the parser is wrong rather than the code",
        declared.len()
    );

    let evaluator = read(&crates_root().join("spoon-exec/src/eval.rs"));
    let unevaluated: Vec<&str> = declared
        .iter()
        .filter(|variant| !evaluator.contains(&format!("IntrinsicOp::{variant}")))
        .copied()
        .collect();

    assert!(
        unevaluated.is_empty(),
        "declared but never evaluated, so a procedure naming one would fail at \
         runtime rather than at review: {unevaluated:?}"
    );
}

/// The Teacher prompt may only advertise operations a lesson can actually use.
///
/// This crossed the language boundary and went unnoticed: the prompt offered 96
/// operations the engine's lesson compiler could not name, so the Teacher would
/// author a correct lesson, admission would reject the draft, and the cycle
/// would complete having learned nothing. Nothing failed loudly. The prompt is
/// TypeScript and the operation table is Rust, so the only place this agreement
/// can be checked is a test that reads both.
#[test]
fn every_operation_advertised_to_the_teacher_can_be_named_by_a_lesson() {
    let prompt = read(
        &crates_root()
            .parent()
            .expect("crates/ sits at the workspace root")
            .join("packages/teacher/src/prompt.ts"),
    );

    // The vocabulary lines are prose of the form `"Text: length, text_split,
    // ..."`, one family per line.
    let advertised: BTreeSet<String> = prompt
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            if !trimmed.starts_with('"') {
                return None;
            }
            let (family, names) = trimmed.trim_start_matches('"').split_once(": ")?;
            if !family.chars().all(|c| c.is_ascii_alphabetic()) {
                return None;
            }
            Some(names.to_string())
        })
        .flat_map(|names| {
            names
                .split(',')
                .map(|name| {
                    name.trim()
                        .trim_end_matches(['.', '"', ';'])
                        .trim()
                        .to_string()
                })
                .filter(|name| {
                    !name.is_empty()
                        && name
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                })
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        advertised.len() > 100,
        "parsed only {} advertised operations, so the parser is wrong rather \
         than the prompt",
        advertised.len()
    );

    let unnameable: Vec<&String> = advertised
        .iter()
        .filter(|name| IntrinsicOp::from_lesson_name(name).is_none())
        .collect();

    assert!(
        unnameable.is_empty(),
        "{} operation(s) are advertised to the Teacher but cannot be named by a \
         lesson, so authoring one silently learns nothing: {unnameable:?}",
        unnameable.len()
    );
}

/// The evaluator's argument counts, parsed from its arity table.
///
/// Arms group many operations onto one count, so variant names accumulate
/// until the count that closes the arm.
fn evaluator_arity() -> BTreeMap<String, usize> {
    let source = read(&crates_root().join("spoon-exec/src/eval.rs"));
    let start = source
        .find("fn intrinsic_arity(")
        .expect("the evaluator declares an arity table");
    let body = &source[start..start + source[start..].find("\n}").expect("it terminates")];

    let mut arity = BTreeMap::new();
    let mut pending: Vec<String> = Vec::new();
    for line in body.lines() {
        for fragment in line.split('|') {
            let fragment = fragment.trim();
            if let Some(rest) = fragment.strip_prefix("IntrinsicOp::") {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                if !name.is_empty() {
                    pending.push(name);
                }
            }
        }
        if let Some((_, count)) = line.rsplit_once("=> ")
            && let Ok(count) = count.trim().trim_end_matches(',').parse::<usize>() {
                for name in pending.drain(..) {
                    arity.insert(name, count);
                }
            }
    }
    arity
}

/// The inspector's operation reference must match what the evaluator accepts.
///
/// The inspector is where a person or a model looks up how to call an
/// operation, so a wrong argument count there produces calls that cannot work.
/// It listed `range` as taking 2 arguments when it takes 3, and `coalesce` as
/// taking 1 when it is variadic with a minimum of 2.
#[test]
fn the_inspector_operation_reference_matches_the_evaluator() {
    let inspector = read(
        &crates_root()
            .parent()
            .expect("crates/ sits at the workspace root")
            .join("packages/inspector/src/server.ts"),
    );

    let listed: BTreeMap<String, usize> = inspector
        .split("{name:'")
        .skip(1)
        .filter_map(|entry| {
            let (name, rest) = entry.split_once('\'')?;
            let arity = rest.strip_prefix(",arity:")?;
            let arity: String = arity.chars().take_while(|c| c.is_ascii_digit()).collect();
            Some((name.to_string(), arity.parse().ok()?))
        })
        .collect();

    assert!(
        listed.len() > 100,
        "parsed only {} inspector entries, so the parser is wrong rather than \
         the reference",
        listed.len()
    );

    let arity = evaluator_arity();
    assert_eq!(
        arity.len(),
        IntrinsicOp::ALL.len(),
        "the parsed arity table does not cover every operation"
    );

    let by_lesson_name: BTreeMap<&str, &str> = IntrinsicOp::ALL
        .iter()
        .map(|op| {
            let variant: &'static str = op.variant_name();
            (op.lesson_name(), variant)
        })
        .collect();

    let mut wrong = Vec::new();
    let mut unknown = Vec::new();
    for (name, listed_arity) in &listed {
        match by_lesson_name.get(name.as_str()) {
            None => unknown.push(name.clone()),
            Some(variant) => {
                let actual = arity[*variant];
                if actual != *listed_arity {
                    wrong.push(format!(
                        "{name}: evaluator {actual}, inspector {listed_arity}"
                    ));
                }
            }
        }
    }

    assert!(
        unknown.is_empty(),
        "the inspector documents operations that do not exist: {unknown:?}"
    );
    assert!(
        wrong.is_empty(),
        "the inspector documents argument counts the evaluator will refuse: {wrong:?}"
    );

    let undocumented: Vec<&str> = by_lesson_name
        .keys()
        .filter(|name| !listed.contains_key(**name))
        .copied()
        .collect();
    assert!(
        undocumented.is_empty(),
        "{} operation(s) exist but the inspector does not document them: \
         {undocumented:?}",
        undocumented.len()
    );
}

/// Naming is derived from one table, and this proves the derivation is total.
#[test]
fn every_declared_intrinsic_round_trips_through_its_lesson_name() {
    let names: BTreeSet<&str> = IntrinsicOp::ALL.iter().map(|op| op.lesson_name()).collect();
    assert_eq!(
        names.len(),
        IntrinsicOp::ALL.len(),
        "two operations share a lesson name, so one of them is unreachable"
    );

    for op in IntrinsicOp::ALL {
        let name = op.lesson_name();
        assert_eq!(
            IntrinsicOp::from_lesson_name(name),
            Some(*op),
            "{name} does not resolve back to the operation it names"
        );
    }
}

#[test]
fn every_declared_intrinsic_is_exercised_by_a_test() {
    let declared = declared_variants();
    assert!(
        declared.len() > 100,
        "found only {} operations, so this test would pass vacuously",
        declared.len()
    );
    let tests = test_sources();

    let untested: Vec<&str> = declared
        .iter()
        .filter(|variant| !tests.contains(&format!("IntrinsicOp::{variant}")))
        .copied()
        .collect();

    assert!(
        untested.is_empty(),
        "{} intrinsic(s) are evaluated but never tested. Learned procedures \
         execute these, so an untested operation is an unproven claim about \
         what a procedure does: {untested:?}",
        untested.len()
    );
}
