//! Candidate Laboratory: engine-local composition of known procedures.
//!
//! When no single procedure matches a request but the request mentions
//! multiple known procedure names, this module attempts to compose them
//! into a sequential chain and execute it without teacher or interpreter
//! assistance. (section 17, escalation rung 4: Compose)

use spoon_core::concept::Lifecycle;
use spoon_core::contract::{Condition, Contract, CostEstimate};
use spoon_core::evidence::Confidence;
use spoon_core::expr::Expr;
use spoon_core::procedure::{Param, Procedure, ProcedureId};
use spoon_core::value::Value;

/// A candidate composition ready for quarantine execution.
#[derive(Debug, Clone)]
pub struct CompositionCandidate {
    pub procedure: Procedure,
    /// The procedures being composed, in execution order.
    pub chain: Vec<ChainedProcedure>,
    /// Literal inputs extracted from the request for the first procedure.
    pub inputs: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct ChainedProcedure {
    pub id: ProcedureId,
    pub name: String,
    pub version: u32,
}

/// Try to compose a sequential chain of known procedures from the request.
///
/// Returns `None` when fewer than two procedures are mentioned or when
/// the chain cannot be constructed (e.g. arity mismatch).
pub fn attempt_composition(
    situation: &str,
    procedures: &[Procedure],
    literals: &[Value],
) -> Option<CompositionCandidate> {
    let mentions = find_mentioned_procedures(situation, procedures);
    if mentions.len() < 2 {
        return None;
    }

    let ordered = sequence_mentions(situation, &mentions);
    build_chain(&ordered, literals)
}

/// A procedure name found in the request text, with its position.
#[derive(Debug, Clone)]
struct ProcedureMention<'a> {
    procedure: &'a Procedure,
    position: usize,
}

/// Find procedures whose names appear in the request text (case-insensitive).
fn find_mentioned_procedures<'a>(
    situation: &str,
    procedures: &'a [Procedure],
) -> Vec<ProcedureMention<'a>> {
    let lower = situation.to_lowercase();
    let mut mentions: Vec<ProcedureMention<'a>> = Vec::new();

    for procedure in procedures {
        if procedure.lifecycle != Lifecycle::Active
            && procedure.lifecycle != Lifecycle::Validated
            && procedure.lifecycle != Lifecycle::Provisional
        {
            continue;
        }
        let name_lower = procedure.name.to_lowercase();
        if name_lower.is_empty() {
            continue;
        }
        if let Some(pos) = lower.find(&name_lower) {
            let already = mentions.iter().any(|m| m.procedure.id == procedure.id);
            if !already {
                mentions.push(ProcedureMention {
                    procedure,
                    position: pos,
                });
            }
        }
    }

    mentions.sort_by_key(|m| m.position);
    mentions
}

/// Determine execution order from textual sequencing clues.
///
/// Recognized patterns:
/// - "X and then Y" / "X then Y" / "X followed by Y" - X before Y
/// - "Y after X" - X before Y
/// - Default: left-to-right order of first mention
fn sequence_mentions<'a>(situation: &str, mentions: &[ProcedureMention<'a>]) -> Vec<&'a Procedure> {
    if mentions.len() != 2 {
        return mentions.iter().map(|m| m.procedure).collect();
    }

    let lower = situation.to_lowercase();
    let first = &mentions[0];
    let second = &mentions[1];

    let first_name = first.procedure.name.to_lowercase();
    let second_name = second.procedure.name.to_lowercase();

    // Check "after" pattern: "Y after X" means X runs first
    if let Some(after_pos) = lower.find("after") {
        let after_end = after_pos + 5;
        if second.position < after_pos && after_end <= lower.len() {
            let after_rest = &lower[after_end..];
            if after_rest.trim_start().starts_with(&first_name) {
                return vec![first.procedure, second.procedure];
            }
        }
        if first.position < after_pos && after_end <= lower.len() {
            let after_rest = &lower[after_end..];
            if after_rest.trim_start().starts_with(&second_name) {
                return vec![second.procedure, first.procedure];
            }
        }
    }

    // Default: left-to-right mention order (already sorted by position)
    mentions.iter().map(|m| m.procedure).collect()
}

/// Build a composed procedure that chains procedures sequentially.
///
/// The output of each step feeds as the first argument to the next.
/// Remaining literal inputs are bound to the first procedure's parameters.
fn build_chain(procedures: &[&Procedure], literals: &[Value]) -> Option<CompositionCandidate> {
    if procedures.len() < 2 {
        return None;
    }

    let first = procedures[0];

    // The first procedure gets the literal inputs.
    // If it needs more args than we have literals, we can't compose.
    if !first.params.is_empty() && literals.is_empty() {
        return None;
    }

    // For each subsequent procedure, it must accept at least one argument
    // (the output of the previous step).
    for proc in &procedures[1..] {
        if proc.params.is_empty() {
            return None;
        }
    }

    let chain: Vec<ChainedProcedure> = procedures
        .iter()
        .map(|p| ChainedProcedure {
            id: p.id,
            name: p.name.clone(),
            version: p.version,
        })
        .collect();

    // Build composed params: same as first procedure's params
    let params: Vec<Param> = first.params.clone();

    // Build the body: Let-chain of CallExact nodes
    let first_args: Vec<Expr> = if first.params.is_empty() {
        literals
            .iter()
            .take(1)
            .map(|v| Expr::Literal(v.clone()))
            .collect()
    } else {
        first
            .params
            .iter()
            .enumerate()
            .map(|(i, param)| {
                if i < literals.len() {
                    Expr::Literal(literals[i].clone())
                } else {
                    Expr::Var(param.name.clone())
                }
            })
            .collect()
    };

    // Start building from the innermost (last) call working backwards
    let last_idx = procedures.len() - 1;
    let mut body = Expr::CallExact {
        procedure: procedures[last_idx].id,
        version: procedures[last_idx].version,
        args: vec![Expr::Var(format!("step_{}", last_idx - 1))],
    };

    // Wrap in Let bindings from second-to-last down to first
    for i in (1..last_idx).rev() {
        body = Expr::Let {
            name: format!("step_{i}"),
            value: Box::new(Expr::CallExact {
                procedure: procedures[i].id,
                version: procedures[i].version,
                args: vec![Expr::Var(format!("step_{}", i - 1))],
            }),
            body: Box::new(body),
        };
    }

    // Wrap the outermost Let with the first procedure call
    body = Expr::Let {
        name: "step_0".into(),
        value: Box::new(Expr::CallExact {
            procedure: first.id,
            version: first.version,
            args: first_args,
        }),
        body: Box::new(body),
    };

    // Build a composed name
    let composed_name = chain
        .iter()
        .map(|c| c.name.as_str())
        .collect::<Vec<_>>()
        .join(" then ");

    // Synthesize a minimal contract from the chain
    let contract = synthesize_contract(&chain, procedures);

    let mut composed = Procedure::new(composed_name, params, body);
    composed.contract = contract;
    composed.lifecycle = Lifecycle::Provisional;

    Some(CompositionCandidate {
        inputs: literals.to_vec(),
        procedure: composed,
        chain,
    })
}

/// Build a minimal contract for the composed procedure.
fn synthesize_contract(chain: &[ChainedProcedure], procedures: &[&Procedure]) -> Contract {
    let first = procedures[0];
    let last = procedures[procedures.len() - 1];

    let mut requires: Vec<Condition> = first.contract.requires.clone();
    let mut promises: Vec<Condition> = last.contract.promises.clone();

    if requires.is_empty() {
        requires.push(Condition::described(format!(
            "Inputs must be valid for {}",
            first.name,
        )));
    }
    if promises.is_empty() {
        promises.push(Condition::described(format!(
            "Output is the result of {} applied sequentially",
            chain
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(" then "),
        )));
    }

    let total_ops: u32 = procedures.iter().map(|p| p.contract.costs.operations).sum();

    Contract {
        requires,
        promises,
        fails_when: Vec::new(),
        costs: CostEstimate {
            operations: total_ops,
            description: format!("{}-step sequential composition", chain.len()),
        },
        confidence: Confidence::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spoon_core::contract::Contract;
    use spoon_core::procedure::{Param, ParamType};
    use spoon_core::value::Value;

    fn make_proc(name: &str, param_names: &[&str]) -> Procedure {
        let params: Vec<Param> = param_names
            .iter()
            .map(|n| Param::typed(*n, ParamType::Number))
            .collect();
        let body = if params.is_empty() {
            Expr::Literal(Value::Int(0))
        } else {
            Expr::Var(params[0].name.clone())
        };
        Procedure::new(name, params, body)
    }

    #[test]
    fn find_mentioned_procedures_matches_case_insensitively() {
        let glorp = make_proc("glorp", &["x"]);
        let snip = make_proc("snip", &["x"]);
        let procs = vec![glorp.clone(), snip.clone()];

        let mentions = find_mentioned_procedures("Glorp 7 and then snip it", &procs);
        assert_eq!(mentions.len(), 2);
        assert_eq!(mentions[0].procedure.name, "glorp");
        assert_eq!(mentions[1].procedure.name, "snip");
    }

    #[test]
    fn find_mentioned_procedures_returns_empty_for_no_match() {
        let glorp = make_proc("glorp", &["x"]);
        let procs = vec![glorp];

        let mentions = find_mentioned_procedures("do something unrelated", &procs);
        assert!(mentions.is_empty());
    }

    #[test]
    fn sequence_default_is_mention_order() {
        let glorp = make_proc("glorp", &["x"]);
        let snip = make_proc("snip", &["x"]);

        let mentions = vec![
            ProcedureMention {
                procedure: &glorp,
                position: 0,
            },
            ProcedureMention {
                procedure: &snip,
                position: 10,
            },
        ];

        let ordered = sequence_mentions("glorp 7 and snip it", &mentions);
        assert_eq!(ordered[0].name, "glorp");
        assert_eq!(ordered[1].name, "snip");
    }

    #[test]
    fn sequence_after_reverses_order() {
        let glorp = make_proc("glorp", &["x"]);
        let snip = make_proc("snip", &["x"]);

        let mentions = vec![
            ProcedureMention {
                procedure: &snip,
                position: 0,
            },
            ProcedureMention {
                procedure: &glorp,
                position: 15,
            },
        ];

        let ordered = sequence_mentions("snip the result after glorp on 7", &mentions);
        assert_eq!(ordered[0].name, "glorp");
        assert_eq!(ordered[1].name, "snip");
    }

    #[test]
    fn build_chain_produces_let_call_exact() {
        let glorp = make_proc("glorp", &["x"]);
        let snip = make_proc("snip", &["x"]);
        let procs: Vec<&Procedure> = vec![&glorp, &snip];
        let literals = vec![Value::Int(7)];

        let candidate = build_chain(&procs, &literals).unwrap();

        assert_eq!(candidate.procedure.name, "glorp then snip");
        assert_eq!(candidate.chain.len(), 2);
        assert_eq!(candidate.procedure.lifecycle, Lifecycle::Provisional);

        // Verify the body shape: Let { step_0 = CallExact(glorp, [7]), body = CallExact(snip, [step_0]) }
        match &candidate.procedure.body {
            Expr::Let { name, value, body } => {
                assert_eq!(name, "step_0");
                match value.as_ref() {
                    Expr::CallExact {
                        procedure,
                        version,
                        args,
                    } => {
                        assert_eq!(*procedure, glorp.id);
                        assert_eq!(*version, 1);
                        assert_eq!(args.len(), 1);
                        assert!(matches!(&args[0], Expr::Literal(Value::Int(7))));
                    }
                    other => panic!("expected CallExact, got {other:?}"),
                }
                match body.as_ref() {
                    Expr::CallExact {
                        procedure,
                        version,
                        args,
                    } => {
                        assert_eq!(*procedure, snip.id);
                        assert_eq!(*version, 1);
                        assert_eq!(args.len(), 1);
                        assert!(matches!(&args[0], Expr::Var(v) if v == "step_0"));
                    }
                    other => panic!("expected CallExact, got {other:?}"),
                }
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn attempt_composition_returns_none_for_single_match() {
        let glorp = make_proc("glorp", &["x"]);
        let procs = vec![glorp];
        let literals = vec![Value::Int(7)];

        assert!(attempt_composition("glorp 7", &procs, &literals).is_none());
    }

    #[test]
    fn attempt_composition_returns_none_when_no_literals() {
        let glorp = make_proc("glorp", &["x"]);
        let snip = make_proc("snip", &["x"]);
        let procs = vec![glorp, snip];

        assert!(attempt_composition("glorp and then snip", &procs, &[]).is_none());
    }

    #[test]
    fn attempt_composition_succeeds_for_glorp_then_snip() {
        let glorp = make_proc("glorp", &["x"]);
        let snip = make_proc("snip", &["x"]);
        let procs = vec![glorp.clone(), snip.clone()];
        let literals = vec![Value::Int(7)];

        let candidate = attempt_composition("Glorp 7 and then snip it", &procs, &literals);
        assert!(candidate.is_some());

        let candidate = candidate.unwrap();
        assert_eq!(candidate.chain[0].name, "glorp");
        assert_eq!(candidate.chain[1].name, "snip");
    }

    #[test]
    fn contract_inherits_from_chain_endpoints() {
        let mut glorp = make_proc("glorp", &["x"]);
        glorp.contract = Contract {
            requires: vec![Condition::described("x must be numeric")],
            ..Contract::default()
        };
        let mut snip = make_proc("snip", &["x"]);
        snip.contract = Contract {
            promises: vec![Condition::described("result is halved")],
            ..Contract::default()
        };

        let procs: Vec<&Procedure> = vec![&glorp, &snip];
        let chain: Vec<ChainedProcedure> = procs
            .iter()
            .map(|p| ChainedProcedure {
                id: p.id,
                name: p.name.clone(),
                version: p.version,
            })
            .collect();

        let contract = synthesize_contract(&chain, &procs);
        assert_eq!(contract.requires[0].description, "x must be numeric");
        assert_eq!(contract.promises[0].description, "result is halved");
    }

    #[test]
    fn three_procedure_chain_builds_nested_lets() {
        let a = make_proc("alpha", &["x"]);
        let b = make_proc("beta", &["x"]);
        let c = make_proc("gamma", &["x"]);
        let procs: Vec<&Procedure> = vec![&a, &b, &c];
        let literals = vec![Value::Int(1)];

        let candidate = build_chain(&procs, &literals).unwrap();
        assert_eq!(candidate.chain.len(), 3);
        assert_eq!(candidate.procedure.name, "alpha then beta then gamma");

        // Outer: Let step_0 = CallExact(alpha), body = Let step_1 = CallExact(beta, step_0), body = CallExact(gamma, step_1)
        match &candidate.procedure.body {
            Expr::Let { name, body, .. } => {
                assert_eq!(name, "step_0");
                match body.as_ref() {
                    Expr::Let { name, body, .. } => {
                        assert_eq!(name, "step_1");
                        assert!(matches!(body.as_ref(), Expr::CallExact { .. }));
                    }
                    other => panic!("expected inner Let, got {other:?}"),
                }
            }
            other => panic!("expected outer Let, got {other:?}"),
        }
    }

    #[test]
    fn retired_procedures_are_excluded() {
        let mut glorp = make_proc("glorp", &["x"]);
        glorp.lifecycle = Lifecycle::Retired;
        let snip = make_proc("snip", &["x"]);
        let procs = vec![glorp, snip];

        let result = attempt_composition("glorp then snip 5", &procs, &[Value::Int(5)]);
        assert!(result.is_none());
    }
}
