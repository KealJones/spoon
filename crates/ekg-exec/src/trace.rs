use ekg_core::{ProcedureId, Value};
use serde::{Deserialize, Serialize};

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
}

impl ExecStep {
    pub fn for_call(procedure: ProcedureId, name: &str, args: &[Value], output: Value) -> Self {
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
        }
    }
}
