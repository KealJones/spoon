use serde::{Deserialize, Serialize};
use spoon_core::{ProcedureId, Value};

/// A recorded execution trace, made of one step per procedure call.
///
/// This is deliberately coarse-grained: recording every sub-expression
/// evaluation would drown the signal that credit assignment needs. What
/// matters for replay is which procedures were invoked, with what inputs,
/// and what they returned.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExecTrace {
    pub steps: Vec<ExecStep>,
}

impl ExecTrace {
    pub fn new() -> Self {
        Self { steps: Vec::new() }
    }

    pub fn push(&mut self, step: ExecStep) {
        self.steps.push(step);
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }
}

/// A single recorded step in an execution trace: a procedure call along
/// with its input and output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecStep {
    pub expr_description: String,
    pub input: Option<Value>,
    pub output: Value,
    pub procedure_called: Option<ProcedureId>,
    /// The exact version that was executed. Older serialized traces may not
    /// carry this field and therefore cannot be replayed safely.
    #[serde(default)]
    pub procedure_version: Option<u32>,
    /// Results of every declared contract condition for this call. Conditions
    /// without executable checks remain visible as `NotExecutable` rather
    /// than disappearing from the trace.
    #[serde(default)]
    pub contract_checks: ContractChecks,
    /// Whether the call completed successfully. This is authoritative even
    /// when the output is Null, which can be either a legitimate procedure
    /// result or the placeholder for a call that produced no value.
    #[serde(default)]
    pub status: ExecStepStatus,
}

impl ExecStep {
    pub fn for_call(procedure: ProcedureId, name: &str, args: &[Value], output: Value) -> Self {
        Self::for_versioned_call(
            procedure,
            name,
            args,
            output,
            None,
            ContractChecks::default(),
        )
    }

    pub fn for_versioned_call(
        procedure: ProcedureId,
        name: &str,
        args: &[Value],
        output: Value,
        version: Option<u32>,
        contract_checks: ContractChecks,
    ) -> Self {
        let input = if args.is_empty() {
            None
        } else {
            Some(Value::List(args.to_vec()))
        };
        Self {
            expr_description: format!("call {name}"),
            input,
            output,
            procedure_called: Some(procedure),
            procedure_version: version,
            contract_checks,
            status: ExecStepStatus::Succeeded,
        }
    }
}

/// The terminal status of a traced procedure call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecStepStatus {
    #[default]
    Succeeded,
    Failed {
        error: String,
    },
}

/// Contract-check evidence captured for one procedure call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractChecks {
    pub requires: Vec<ConditionCheck>,
    pub promises: Vec<ConditionCheck>,
    pub fails_when: Vec<ConditionCheck>,
}

/// The result of checking one declared contract condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionCheck {
    pub description: String,
    pub status: ConditionCheckStatus,
}

/// Whether a condition upheld its part of the contract. For `fails_when`, a
/// false expression is `Passed` and a true expression is `Violated`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConditionCheckStatus {
    Passed,
    Violated,
    NotExecutable,
}
