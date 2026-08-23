use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

use spoon_core::{
    BinOp, Condition, Expr, IntrinsicOp, LanguageError, LanguageLimits, Procedure, ProcedureId,
    SpoonError, TokenKind, UnOp, Value, tokenize_with_limits,
};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

use crate::trace::{ConditionCheck, ConditionCheckStatus, ContractChecks, ExecStep, ExecTrace};

/// A lexically-scoped variable environment.
///
/// Each procedure call, `let`, and collection-iteration expression pushes
/// its own scope so that shadowing works and bindings disappear once their
/// owning expression finishes evaluating.
#[derive(Debug, Clone, Default)]
pub struct Env {
    scopes: Vec<HashMap<String, Value>>,
}

impl Env {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    /// Look up a variable, searching from the innermost scope outward.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    /// Bind a variable in the current (innermost) scope.
    pub fn set(&mut self, name: impl Into<String>, value: Value) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.into(), value);
        }
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Pop the innermost scope. The outermost (global) scope is never
    /// popped, so this is safe to call even if unbalanced.
    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }
}

/// A step counter that caps how much work a single evaluation may do,
/// guarding against runaway recursion or unbounded loops in a procedure
/// body.
#[derive(Debug, Clone, Copy)]
pub struct ExecutionBudget {
    pub max_steps: u32,
    pub steps_used: u32,
}

impl Default for ExecutionBudget {
    fn default() -> Self {
        Self {
            max_steps: 1_000_000,
            steps_used: 0,
        }
    }
}

/// The result of executing a procedure: its return value plus the trace of
/// procedure calls made while producing it.
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub value: Value,
    pub trace: ExecTrace,
}

/// The outcome of an execution whose trace is retained whether it succeeds or
/// fails.
#[derive(Debug)]
pub struct ExecutionAttempt {
    pub result: Result<Value, SpoonError>,
    pub trace: ExecTrace,
}

impl ExecutionAttempt {
    /// Convert a captured attempt to the established success-only API shape.
    pub fn into_result(self) -> Result<ExecResult, SpoonError> {
        let Self { result, trace } = self;
        result.map(|value| ExecResult { value, trace })
    }
}

/// Evaluates expression trees against a table of registered procedures,
/// tracking a step budget and recording a call trace as it goes.
pub struct Evaluator {
    procedures: HashMap<ProcedureId, Procedure>,
    budget: ExecutionBudget,
    trace: ExecTrace,
}

impl Default for Evaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl Evaluator {
    pub fn new() -> Self {
        Self {
            procedures: HashMap::new(),
            budget: ExecutionBudget::default(),
            trace: ExecTrace::new(),
        }
    }

    /// Set the step budget available to this evaluator. Consuming builder
    /// so it composes with `Evaluator::new()`.
    pub fn with_budget(mut self, max_steps: u32) -> Self {
        self.budget = ExecutionBudget {
            max_steps,
            steps_used: 0,
        };
        self
    }

    pub fn register_procedure(&mut self, proc: Procedure) {
        self.procedures.insert(proc.id, proc);
    }

    pub fn budget(&self) -> ExecutionBudget {
        self.budget
    }

    /// Evaluate an expression in the given environment.
    ///
    /// Every call consumes one unit of the execution budget; once the
    /// budget is exhausted, evaluation fails with `SpoonError::BudgetExceeded`
    /// rather than continuing indefinitely.
    pub fn eval(&mut self, expr: &Expr, env: &mut Env) -> Result<Value, SpoonError> {
        self.check_budget()?;

        match expr {
            Expr::Literal(v) => Ok(v.clone()),

            Expr::Var(name) => env
                .get(name)
                .cloned()
                .ok_or_else(|| SpoonError::UndefinedVar(name.clone())),

            Expr::BinOp { op, left, right } => self.eval_binop(*op, left, right, env),

            Expr::UnOp { op, operand } => self.eval_unop(*op, operand, env),

            Expr::Call { procedure, args } => {
                let mut arg_values = Vec::with_capacity(args.len());
                for arg in args {
                    arg_values.push(self.eval(arg, env)?);
                }
                self.call_procedure(procedure, arg_values)
            }

            Expr::CallExact {
                procedure,
                version,
                args,
            } => {
                let registered = self
                    .procedures
                    .get(procedure)
                    .ok_or_else(|| SpoonError::UndefinedProcedure(procedure.to_string()))?;
                if registered.version != *version {
                    return Err(SpoonError::Other(format!(
                        "exact call requires procedure {procedure} version {version}, but registered version is {}",
                        registered.version
                    )));
                }
                let mut arg_values = Vec::with_capacity(args.len());
                for arg in args {
                    arg_values.push(self.eval(arg, env)?);
                }
                self.call_procedure(procedure, arg_values)
            }

            Expr::If { cond, then, else_ } => {
                let c = self.eval(cond, env)?;
                if c.truthy() {
                    self.eval(then, env)
                } else {
                    self.eval(else_, env)
                }
            }

            Expr::Let { name, value, body } => {
                let v = self.eval(value, env)?;
                env.push_scope();
                env.set(name.clone(), v);
                let result = self.eval(body, env);
                env.pop_scope();
                result
            }

            Expr::Block(exprs) => {
                let mut result = Value::Null;
                for e in exprs {
                    result = self.eval(e, env)?;
                }
                Ok(result)
            }

            Expr::ListExpr(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(self.eval(item, env)?);
                }
                Ok(Value::List(values))
            }

            Expr::Index { collection, index } => self.eval_index(collection, index, env),

            Expr::FieldAccess { object, field } => {
                let o = self.eval(object, env)?;
                match &o {
                    Value::Map(map) => map
                        .get(field)
                        .cloned()
                        .ok_or_else(|| SpoonError::FieldNotFound(field.clone())),
                    other => Err(SpoonError::type_error("map", other)),
                }
            }

            Expr::Map {
                collection,
                var,
                body,
            } => {
                let items = self.eval_as_list(collection, env)?;
                let mut result = Vec::with_capacity(items.len());
                env.push_scope();
                for item in items {
                    env.set(var.clone(), item);
                    match self.eval(body, env) {
                        Ok(v) => result.push(v),
                        Err(e) => {
                            env.pop_scope();
                            return Err(e);
                        }
                    }
                }
                env.pop_scope();
                Ok(Value::List(result))
            }

            Expr::Filter {
                collection,
                var,
                predicate,
            } => {
                let items = self.eval_as_list(collection, env)?;
                let mut result = Vec::with_capacity(items.len());
                env.push_scope();
                for item in items {
                    env.set(var.clone(), item.clone());
                    match self.eval(predicate, env) {
                        Ok(v) => {
                            if v.truthy() {
                                result.push(item);
                            }
                        }
                        Err(e) => {
                            env.pop_scope();
                            return Err(e);
                        }
                    }
                }
                env.pop_scope();
                Ok(Value::List(result))
            }

            Expr::Reduce {
                collection,
                init,
                acc,
                var,
                body,
            } => {
                let items = self.eval_as_list(collection, env)?;
                let mut accumulator = self.eval(init, env)?;
                env.push_scope();
                for item in items {
                    env.set(acc.clone(), accumulator);
                    env.set(var.clone(), item);
                    match self.eval(body, env) {
                        Ok(v) => accumulator = v,
                        Err(e) => {
                            env.pop_scope();
                            return Err(e);
                        }
                    }
                }
                env.pop_scope();
                Ok(accumulator)
            }

            Expr::Intrinsic { version, op, args } => self.eval_intrinsic(*version, *op, args, env),
        }
    }

    fn eval_intrinsic(
        &mut self,
        version: u16,
        op: IntrinsicOp,
        args: &[Expr],
        env: &mut Env,
    ) -> Result<Value, SpoonError> {
        if version != 1 {
            return Err(SpoonError::UnsupportedIntrinsicVersion(version));
        }

        let expected = intrinsic_arity(op);
        let arity_ok = if op == IntrinsicOp::Coalesce {
            args.len() >= expected
        } else {
            args.len() == expected
        };
        if !arity_ok {
            return Err(SpoonError::ArityMismatch {
                name: intrinsic_name(op).to_string(),
                expected,
                got: args.len(),
            });
        }

        let mut values = Vec::with_capacity(args.len());
        for argument in args {
            values.push(self.eval(argument, env)?);
        }
        let value = self.apply_intrinsic(op, values)?;
        self.ensure_intrinsic_output(intrinsic_name(op), &value)?;
        Ok(value)
    }

    /// Execute a registered procedure by id with the given arguments,
    /// returning both the result and the trace of procedure calls made
    /// while computing it.
    pub fn exec_procedure(
        &mut self,
        id: &ProcedureId,
        args: Vec<Value>,
    ) -> Result<ExecResult, SpoonError> {
        self.exec_procedure_captured(id, args).into_result()
    }

    /// Execute a registered procedure while retaining its trace on both
    /// success and failure.
    pub fn exec_procedure_captured(
        &mut self,
        id: &ProcedureId,
        args: Vec<Value>,
    ) -> ExecutionAttempt {
        self.trace = ExecTrace::new();
        self.budget.steps_used = 0;
        let result = self.call_procedure(id, args);
        let trace = std::mem::take(&mut self.trace);
        ExecutionAttempt { result, trace }
    }

    /// Replay a captured execution using replacement arguments for its
    /// top-level procedure call.
    ///
    /// Every procedure call in the source trace must still be registered at
    /// exactly the recorded version. Validation happens before execution so a
    /// stale trace can never produce a result using different knowledge.
    pub fn replay(
        &mut self,
        trace: &ExecTrace,
        top_level_args: Vec<Value>,
    ) -> Result<ExecResult, SpoonError> {
        for step in &trace.steps {
            let Some(procedure_id) = step.procedure_called else {
                continue;
            };
            let recorded_version = step.procedure_version.ok_or_else(|| {
                SpoonError::Other(format!(
                    "cannot replay procedure {procedure_id}: trace has no procedure version"
                ))
            })?;
            let registered = self.procedures.get(&procedure_id).ok_or_else(|| {
                SpoonError::Other(format!(
                    "cannot replay procedure {procedure_id}: it is not registered"
                ))
            })?;
            if registered.version != recorded_version {
                return Err(SpoonError::Other(format!(
                    "cannot replay procedure {procedure_id}: recorded version {recorded_version}, registered version {}",
                    registered.version
                )));
            }
        }

        let top_level_id = trace
            .steps
            .iter()
            .rev()
            .find_map(|step| step.procedure_called)
            .ok_or_else(|| {
                SpoonError::Other("cannot replay an empty execution trace".to_string())
            })?;

        self.exec_procedure(&top_level_id, top_level_args)
    }

    fn call_procedure(&mut self, id: &ProcedureId, args: Vec<Value>) -> Result<Value, SpoonError> {
        let proc = self
            .procedures
            .get(id)
            .cloned()
            .ok_or_else(|| SpoonError::UndefinedProcedure(id.to_string()))?;

        let mut contract_checks = ContractChecks::default();
        let mut output = Value::Null;

        let result = (|| {
            self.check_budget()?;

            if args.len() != proc.params.len() {
                return Err(SpoonError::ArityMismatch {
                    name: proc.name.clone(),
                    expected: proc.params.len(),
                    got: args.len(),
                });
            }

            let mut call_env = Env::new();
            for (param, arg) in proc.params.iter().zip(args.iter()) {
                call_env.set(param.name.clone(), arg.clone());
            }

            self.check_contract_conditions(
                &proc.contract.requires,
                &mut call_env,
                ContractConditionKind::Requires,
                &mut contract_checks.requires,
            )?;
            self.check_contract_conditions(
                &proc.contract.fails_when,
                &mut call_env,
                ContractConditionKind::FailsWhen,
                &mut contract_checks.fails_when,
            )?;

            output = self.eval(&proc.body, &mut call_env)?;

            call_env.set("result", output.clone());
            self.check_contract_conditions(
                &proc.contract.promises,
                &mut call_env,
                ContractConditionKind::Promises,
                &mut contract_checks.promises,
            )?;

            Ok(output.clone())
        })();

        let mut step = ExecStep::for_versioned_call(
            *id,
            &proc.name,
            &args,
            output,
            Some(proc.version),
            contract_checks,
        );
        if let Err(error) = &result {
            step.status = crate::trace::ExecStepStatus::Failed {
                error: error.to_string(),
            };
        }
        self.trace.push(step);

        result
    }

    fn check_contract_conditions(
        &mut self,
        conditions: &[Condition],
        env: &mut Env,
        kind: ContractConditionKind,
        checks: &mut Vec<ConditionCheck>,
    ) -> Result<(), SpoonError> {
        checks.reserve(conditions.len());
        for condition in conditions {
            let Some(expr) = &condition.check else {
                checks.push(ConditionCheck {
                    description: condition.description.clone(),
                    status: ConditionCheckStatus::NotExecutable,
                });
                continue;
            };

            let value = self.eval(expr, env)?;
            let evaluated = value
                .as_bool()
                .ok_or_else(|| SpoonError::type_error("bool", &value))?;
            let passed = match kind {
                ContractConditionKind::Requires | ContractConditionKind::Promises => evaluated,
                ContractConditionKind::FailsWhen => !evaluated,
            };
            let status = if passed {
                ConditionCheckStatus::Passed
            } else {
                ConditionCheckStatus::Violated
            };
            checks.push(ConditionCheck {
                description: condition.description.clone(),
                status,
            });

            if !passed {
                return Err(SpoonError::ContractViolation(format!(
                    "{} condition violated: {}",
                    kind.label(),
                    condition.description
                )));
            }
        }
        Ok(())
    }

    fn check_budget(&mut self) -> Result<(), SpoonError> {
        if self.budget.steps_used >= self.budget.max_steps {
            return Err(SpoonError::BudgetExceeded);
        }
        self.budget.steps_used += 1;
        Ok(())
    }

    /// Charge intrinsic work in bounded chunks, so one expression cannot hide
    /// linear or structural work behind its single outer AST evaluation step.
    fn charge_intrinsic_work(&mut self, amount: usize) -> Result<(), SpoonError> {
        let units = amount.div_ceil(INTRINSIC_WORK_CHUNK).max(1);
        for _ in 0..units {
            self.check_budget()?;
        }
        Ok(())
    }

    fn eval_as_list(&mut self, expr: &Expr, env: &mut Env) -> Result<Vec<Value>, SpoonError> {
        let v = self.eval(expr, env)?;
        match v {
            Value::List(items) => Ok(items),
            other => Err(SpoonError::type_error("list", &other)),
        }
    }

    fn eval_index(
        &mut self,
        collection: &Expr,
        index: &Expr,
        env: &mut Env,
    ) -> Result<Value, SpoonError> {
        let c = self.eval(collection, env)?;
        let i = self.eval(index, env)?;
        match &c {
            Value::List(items) => {
                let idx = i
                    .as_int()
                    .ok_or_else(|| SpoonError::type_error("int", &i))?;
                let len = items.len();
                let real_idx = if idx < 0 { idx + len as i64 } else { idx };
                if real_idx < 0 || real_idx as usize >= len {
                    return Err(SpoonError::IndexOutOfBounds {
                        index: idx,
                        length: len,
                    });
                }
                Ok(items[real_idx as usize].clone())
            }
            Value::Map(map) => {
                let key = i
                    .as_text()
                    .ok_or_else(|| SpoonError::type_error("text", &i))?;
                map.get(key)
                    .cloned()
                    .ok_or_else(|| SpoonError::FieldNotFound(key.to_string()))
            }
            other => Err(SpoonError::type_error("list or map", other)),
        }
    }

    fn eval_binop(
        &mut self,
        op: BinOp,
        left: &Expr,
        right: &Expr,
        env: &mut Env,
    ) -> Result<Value, SpoonError> {
        match op {
            BinOp::And => {
                let l = self.eval(left, env)?;
                let lb = l
                    .as_bool()
                    .ok_or_else(|| SpoonError::type_error("bool", &l))?;
                if !lb {
                    return Ok(Value::Bool(false));
                }
                let r = self.eval(right, env)?;
                let rb = r
                    .as_bool()
                    .ok_or_else(|| SpoonError::type_error("bool", &r))?;
                Ok(Value::Bool(rb))
            }
            BinOp::Or => {
                let l = self.eval(left, env)?;
                let lb = l
                    .as_bool()
                    .ok_or_else(|| SpoonError::type_error("bool", &l))?;
                if lb {
                    return Ok(Value::Bool(true));
                }
                let r = self.eval(right, env)?;
                let rb = r
                    .as_bool()
                    .ok_or_else(|| SpoonError::type_error("bool", &r))?;
                Ok(Value::Bool(rb))
            }
            _ => {
                let l = self.eval(left, env)?;
                let r = self.eval(right, env)?;
                apply_binop(op, l, r)
            }
        }
    }

    fn eval_unop(&mut self, op: UnOp, operand: &Expr, env: &mut Env) -> Result<Value, SpoonError> {
        let v = self.eval(operand, env)?;
        match (op, &v) {
            (UnOp::Neg, Value::Int(n)) => {
                n.checked_neg()
                    .map(Value::Int)
                    .ok_or_else(|| SpoonError::ArithmeticOverflow {
                        operation: "integer negation".into(),
                    })
            }
            (UnOp::Neg, Value::Float(f)) => Ok(Value::Float(-f)),
            (UnOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
            (UnOp::Neg, other) => Err(SpoonError::type_error("numeric", other)),
            (UnOp::Not, other) => Err(SpoonError::type_error("bool", other)),
        }
    }
}

const MAX_JSON_BYTES: usize = 1_048_576;
const MAX_JSON_DEPTH: usize = 128;
const MAX_PATH_BYTES: usize = 16_384;
const MAX_PATH_SEGMENTS: usize = 256;
const MAX_INTRINSIC_TEXT_BYTES: usize = 1_048_576;
const MAX_INTRINSIC_ITEMS: usize = 100_000;
const MAX_TOKENIZE_INPUT_BYTES: usize = 64 * 1024;
const MAX_TOKENIZE_ITEMS: usize = 4_096;
const INTRINSIC_WORK_CHUNK: usize = 64;

fn intrinsic_name(op: IntrinsicOp) -> &'static str {
    match op {
        IntrinsicOp::Length => "length",
        IntrinsicOp::TextByteLength => "text_byte_length",
        IntrinsicOp::TextScalarLength => "text_scalar_length",
        IntrinsicOp::TextGraphemeLength => "text_grapheme_length",
        IntrinsicOp::TextTokenize => "text_tokenize",
        IntrinsicOp::TextSplit => "text_split",
        IntrinsicOp::TextJoin => "text_join",
        IntrinsicOp::TextTrim => "text_trim",
        IntrinsicOp::TextLowercase => "text_lowercase",
        IntrinsicOp::TextUppercase => "text_uppercase",
        IntrinsicOp::TextContains => "text_contains",
        IntrinsicOp::TextStartsWith => "text_starts_with",
        IntrinsicOp::TextEndsWith => "text_ends_with",
        IntrinsicOp::TextReplace => "text_replace",
        IntrinsicOp::CollectionContains => "collection_contains",
        IntrinsicOp::CollectionFindIndex => "collection_find_index",
        IntrinsicOp::CountEqual => "count_equal",
        IntrinsicOp::MapKeys => "map_keys",
        IntrinsicOp::MapValues => "map_values",
        IntrinsicOp::JsonParse => "json_parse",
        IntrinsicOp::JsonStringify => "json_stringify",
        IntrinsicOp::PathGet => "path_get",
        IntrinsicOp::PathGetOptional => "path_get_optional",
        IntrinsicOp::JsonPointerGet => "json_pointer_get",
        IntrinsicOp::JsonPointerGetOptional => "json_pointer_get_optional",
        IntrinsicOp::JsonPointerSet => "json_pointer_set",
        IntrinsicOp::JsonPointerDelete => "json_pointer_delete",
        IntrinsicOp::Coalesce => "coalesce",
        IntrinsicOp::TextNormalizeNfc => "text_normalize_nfc",
        IntrinsicOp::TextNormalizeNfd => "text_normalize_nfd",
        IntrinsicOp::TextNormalizeNfkc => "text_normalize_nfkc",
        IntrinsicOp::TextNormalizeNfkd => "text_normalize_nfkd",
        IntrinsicOp::TextTrimStart => "text_trim_start",
        IntrinsicOp::TextTrimEnd => "text_trim_end",
        IntrinsicOp::TextGraphemeSubstring => "text_grapheme_substring",
        IntrinsicOp::TextIndexOf => "text_index_of",
        IntrinsicOp::TextCount => "text_count",
        IntrinsicOp::TextRepeat => "text_repeat",
        IntrinsicOp::TextConcatMany => "text_concat_many",
        IntrinsicOp::MapEntries => "map_entries",
        IntrinsicOp::MapFromEntries => "map_from_entries",
        IntrinsicOp::MapSet => "map_set",
        IntrinsicOp::MapDelete => "map_delete",
        IntrinsicOp::MapMerge => "map_merge",
        IntrinsicOp::CollectionSlice => "collection_slice",
        IntrinsicOp::CollectionReverse => "collection_reverse",
        IntrinsicOp::CollectionSort => "collection_sort",
        IntrinsicOp::CollectionUnique => "collection_unique",
        IntrinsicOp::CollectionFlatten => "collection_flatten",
        IntrinsicOp::CollectionZip => "collection_zip",
        IntrinsicOp::Range => "range",
        IntrinsicOp::TypeName => "type_name",
        IntrinsicOp::ParseInt => "parse_int",
        IntrinsicOp::ParseFloat => "parse_float",
        IntrinsicOp::ParseBool => "parse_bool",
        IntrinsicOp::ToText => "to_text",
        IntrinsicOp::NumericAbs => "numeric_abs",
        IntrinsicOp::NumericSign => "numeric_sign",
        IntrinsicOp::NumericMin => "numeric_min",
        IntrinsicOp::NumericMax => "numeric_max",
        IntrinsicOp::NumericClamp => "numeric_clamp",
        IntrinsicOp::NumericFloor => "numeric_floor",
        IntrinsicOp::NumericCeil => "numeric_ceil",
        IntrinsicOp::NumericRound => "numeric_round",
        IntrinsicOp::NumericTruncate => "numeric_truncate",
        IntrinsicOp::NumericPowInt => "numeric_pow_int",
        IntrinsicOp::NumericPowFloat => "numeric_pow_float",
        IntrinsicOp::IntegerQuotient => "integer_quotient",
        IntrinsicOp::IntegerRemainder => "integer_remainder",
    }
}

fn intrinsic_arity(op: IntrinsicOp) -> usize {
    match op {
        IntrinsicOp::TextSplit
        | IntrinsicOp::TextJoin
        | IntrinsicOp::TextContains
        | IntrinsicOp::TextStartsWith
        | IntrinsicOp::TextEndsWith
        | IntrinsicOp::CollectionContains
        | IntrinsicOp::CollectionFindIndex
        | IntrinsicOp::CountEqual
        | IntrinsicOp::PathGet
        | IntrinsicOp::PathGetOptional
        | IntrinsicOp::JsonPointerGet
        | IntrinsicOp::JsonPointerGetOptional
        | IntrinsicOp::JsonPointerDelete
        | IntrinsicOp::Coalesce
        | IntrinsicOp::TextIndexOf
        | IntrinsicOp::TextCount
        | IntrinsicOp::TextRepeat
        | IntrinsicOp::MapDelete
        | IntrinsicOp::MapMerge
        | IntrinsicOp::CollectionZip => 2,
        IntrinsicOp::TextReplace
        | IntrinsicOp::TextGraphemeSubstring
        | IntrinsicOp::MapSet
        | IntrinsicOp::CollectionSlice
        | IntrinsicOp::Range
        | IntrinsicOp::JsonPointerSet
        | IntrinsicOp::NumericClamp => 3,
        IntrinsicOp::Length
        | IntrinsicOp::TextByteLength
        | IntrinsicOp::TextScalarLength
        | IntrinsicOp::TextGraphemeLength
        | IntrinsicOp::TextTokenize
        | IntrinsicOp::TextTrim
        | IntrinsicOp::TextLowercase
        | IntrinsicOp::TextUppercase
        | IntrinsicOp::MapKeys
        | IntrinsicOp::MapValues
        | IntrinsicOp::JsonParse
        | IntrinsicOp::JsonStringify
        | IntrinsicOp::MapFromEntries
        | IntrinsicOp::TextNormalizeNfc
        | IntrinsicOp::TextNormalizeNfd
        | IntrinsicOp::TextNormalizeNfkc
        | IntrinsicOp::TextNormalizeNfkd
        | IntrinsicOp::TextTrimStart
        | IntrinsicOp::TextTrimEnd
        | IntrinsicOp::TextConcatMany
        | IntrinsicOp::MapEntries
        | IntrinsicOp::CollectionReverse
        | IntrinsicOp::CollectionSort
        | IntrinsicOp::CollectionUnique
        | IntrinsicOp::CollectionFlatten
        | IntrinsicOp::TypeName
        | IntrinsicOp::ParseInt
        | IntrinsicOp::ParseFloat
        | IntrinsicOp::ParseBool
        | IntrinsicOp::ToText
        | IntrinsicOp::NumericAbs
        | IntrinsicOp::NumericSign
        | IntrinsicOp::NumericFloor
        | IntrinsicOp::NumericCeil
        | IntrinsicOp::NumericRound
        | IntrinsicOp::NumericTruncate => 1,
        IntrinsicOp::NumericMin
        | IntrinsicOp::NumericMax
        | IntrinsicOp::NumericPowInt
        | IntrinsicOp::NumericPowFloat
        | IntrinsicOp::IntegerQuotient
        | IntrinsicOp::IntegerRemainder => 2,
    }
}

impl Evaluator {
    fn apply_intrinsic(&mut self, op: IntrinsicOp, args: Vec<Value>) -> Result<Value, SpoonError> {
        match op {
            IntrinsicOp::Length => match only_arg(args) {
                Value::Text(text) => {
                    self.charge_text(&text)?;
                    Ok(Value::Int(text.graphemes(true).count() as i64))
                }
                Value::List(items) => {
                    self.charge_items(items.len())?;
                    Ok(Value::Int(items.len() as i64))
                }
                Value::Map(entries) => {
                    self.charge_items(entries.len())?;
                    Ok(Value::Int(entries.len() as i64))
                }
                other => Err(SpoonError::type_error("text, list, or map", &other)),
            },
            IntrinsicOp::TextByteLength => {
                let text = text_arg(only_arg(args))?;
                self.charge_text(&text)?;
                Ok(Value::Int(text.len() as i64))
            }
            IntrinsicOp::TextScalarLength => {
                let text = text_arg(only_arg(args))?;
                self.charge_text(&text)?;
                Ok(Value::Int(text.chars().count() as i64))
            }
            IntrinsicOp::TextGraphemeLength => {
                let text = text_arg(only_arg(args))?;
                self.charge_text(&text)?;
                Ok(Value::Int(text.graphemes(true).count() as i64))
            }
            IntrinsicOp::TextTokenize => self.text_tokenize(text_arg(only_arg(args))?),
            IntrinsicOp::TextSplit => {
                let [text, delimiter] = two_args(args);
                let text = text_arg(text)?;
                let delimiter = text_arg(delimiter)?;
                self.charge_text(&text)?;
                self.charge_text(&delimiter)?;
                let items: Vec<Value> = if delimiter.is_empty() {
                    text.graphemes(true)
                        .map(|s| Value::Text(s.to_owned()))
                        .collect()
                } else {
                    text.split(&delimiter)
                        .map(|s| Value::Text(s.to_owned()))
                        .collect()
                };
                self.ensure_items("text_split output items", items.len())?;
                Ok(Value::List(items))
            }
            IntrinsicOp::TextJoin | IntrinsicOp::TextConcatMany => {
                let (items, delimiter) = if op == IntrinsicOp::TextJoin {
                    let [items, delimiter] = two_args(args);
                    (list_arg(items)?, text_arg(delimiter)?)
                } else {
                    (list_arg(only_arg(args))?, String::new())
                };
                self.charge_items(items.len())?;
                self.charge_text(&delimiter)?;
                let mut output = String::new();
                for (index, item) in items.into_iter().enumerate() {
                    let text = text_arg(item)?;
                    self.charge_text(&text)?;
                    if index > 0 {
                        append_text(&mut output, &delimiter, intrinsic_name(op))?;
                    }
                    append_text(&mut output, &text, intrinsic_name(op))?;
                }
                Ok(Value::Text(output))
            }
            IntrinsicOp::TextTrim | IntrinsicOp::TextTrimStart | IntrinsicOp::TextTrimEnd => {
                let text = text_arg(only_arg(args))?;
                self.charge_text(&text)?;
                let output = match op {
                    IntrinsicOp::TextTrim => text.trim(),
                    IntrinsicOp::TextTrimStart => text.trim_start(),
                    _ => text.trim_end(),
                };
                self.ensure_text("text trim output bytes", output.len())?;
                Ok(Value::Text(output.to_owned()))
            }
            IntrinsicOp::TextLowercase
            | IntrinsicOp::TextUppercase
            | IntrinsicOp::TextNormalizeNfc
            | IntrinsicOp::TextNormalizeNfd
            | IntrinsicOp::TextNormalizeNfkc
            | IntrinsicOp::TextNormalizeNfkd => {
                let text = text_arg(only_arg(args))?;
                self.charge_text(&text)?;
                let output = match op {
                    IntrinsicOp::TextLowercase => text.to_lowercase(),
                    IntrinsicOp::TextUppercase => text.to_uppercase(),
                    IntrinsicOp::TextNormalizeNfc => text.nfc().collect(),
                    IntrinsicOp::TextNormalizeNfd => text.nfd().collect(),
                    IntrinsicOp::TextNormalizeNfkc => text.nfkc().collect(),
                    IntrinsicOp::TextNormalizeNfkd => text.nfkd().collect(),
                    _ => unreachable!(),
                };
                self.ensure_text("text transform output bytes", output.len())?;
                self.charge_text(&output)?;
                Ok(Value::Text(output))
            }
            IntrinsicOp::TextContains
            | IntrinsicOp::TextStartsWith
            | IntrinsicOp::TextEndsWith
            | IntrinsicOp::TextIndexOf
            | IntrinsicOp::TextCount => {
                let [text, needle] = two_args(args);
                let text = text_arg(text)?;
                let needle = text_arg(needle)?;
                self.charge_text(&text)?;
                self.charge_text(&needle)?;
                match op {
                    IntrinsicOp::TextContains => Ok(Value::Bool(text.contains(&needle))),
                    IntrinsicOp::TextStartsWith => Ok(Value::Bool(text.starts_with(&needle))),
                    IntrinsicOp::TextEndsWith => Ok(Value::Bool(text.ends_with(&needle))),
                    IntrinsicOp::TextIndexOf => Ok(Value::Int(
                        text.find(&needle)
                            .map(|byte| text[..byte].graphemes(true).count() as i64)
                            .unwrap_or(-1),
                    )),
                    IntrinsicOp::TextCount => Ok(Value::Int(if needle.is_empty() {
                        text.graphemes(true).count() as i64 + 1
                    } else {
                        text.matches(&needle).count() as i64
                    })),
                    _ => unreachable!(),
                }
            }
            IntrinsicOp::TextReplace => {
                let [text, from, to] = three_args(args);
                let text = text_arg(text)?;
                let from = text_arg(from)?;
                let to = text_arg(to)?;
                self.charge_text(&text)?;
                self.charge_text(&from)?;
                self.charge_text(&to)?;
                let output = text.replace(&from, &to);
                self.ensure_text("text_replace output bytes", output.len())?;
                self.charge_text(&output)?;
                Ok(Value::Text(output))
            }
            IntrinsicOp::TextGraphemeSubstring => {
                let [text, start, length] = three_args(args);
                let text = text_arg(text)?;
                self.charge_text(&text)?;
                let pieces: Vec<&str> = text.graphemes(true).collect();
                let start = normalized_start(int_arg(start)?, pieces.len())?;
                let length = nonnegative_usize(int_arg(length)?, "text_grapheme_substring length")?;
                let end = start.saturating_add(length).min(pieces.len());
                let output = pieces[start..end].concat();
                self.ensure_text("text_grapheme_substring output bytes", output.len())?;
                Ok(Value::Text(output))
            }
            IntrinsicOp::TextRepeat => {
                let [text, count] = two_args(args);
                let text = text_arg(text)?;
                self.charge_text(&text)?;
                let count = nonnegative_usize(int_arg(count)?, "text_repeat count")?;
                self.ensure_items("text_repeat count", count)?;
                let bytes = text.len().checked_mul(count).ok_or_else(|| {
                    SpoonError::ArithmeticOverflow {
                        operation: "text_repeat output size".into(),
                    }
                })?;
                self.ensure_text("text_repeat output bytes", bytes)?;
                self.charge_intrinsic_work(bytes)?;
                Ok(Value::Text(text.repeat(count)))
            }
            IntrinsicOp::CollectionContains => {
                let [collection, sought] = two_args(args);
                match collection {
                    Value::List(items) => {
                        self.charge_items(items.len())?;
                        Ok(Value::Bool(items.contains(&sought)))
                    }
                    Value::Map(entries) => {
                        self.charge_items(entries.len())?;
                        let key = text_arg(sought)?;
                        Ok(Value::Bool(entries.contains_key(&key)))
                    }
                    other => Err(SpoonError::type_error("list or map", &other)),
                }
            }
            IntrinsicOp::CountEqual => {
                let [collection, sought] = two_args(args);
                let items = list_arg(collection)?;
                self.charge_items(items.len())?;
                Ok(Value::Int(
                    items.iter().filter(|item| **item == sought).count() as i64,
                ))
            }
            IntrinsicOp::MapKeys | IntrinsicOp::MapValues | IntrinsicOp::MapEntries => {
                let entries = map_arg(only_arg(args))?;
                self.charge_items(entries.len())?;
                self.ensure_items("map output items", entries.len())?;
                let values = match op {
                    IntrinsicOp::MapKeys => entries.into_keys().map(Value::Text).collect(),
                    IntrinsicOp::MapValues => entries.into_values().collect(),
                    IntrinsicOp::MapEntries => entries
                        .into_iter()
                        .map(|(k, v)| Value::List(vec![Value::Text(k), v]))
                        .collect(),
                    _ => unreachable!(),
                };
                Ok(Value::List(values))
            }
            IntrinsicOp::MapFromEntries => {
                let entries = list_arg(only_arg(args))?;
                self.charge_items(entries.len())?;
                let mut map = BTreeMap::new();
                for entry in entries {
                    let pair = list_arg(entry)?;
                    if pair.len() != 2 {
                        return Err(SpoonError::ArityMismatch {
                            name: intrinsic_name(op).into(),
                            expected: 2,
                            got: pair.len(),
                        });
                    }
                    let mut pair = pair.into_iter();
                    let key = text_arg(pair.next().expect("pair length checked"))?;
                    self.charge_text(&key)?;
                    let value = pair.next().expect("pair length checked");
                    if !map.contains_key(&key) {
                        self.ensure_items("map_from_entries output items", map.len() + 1)?;
                    }
                    map.insert(key, value);
                }
                Ok(Value::Map(map))
            }
            IntrinsicOp::MapSet => {
                let [map, key, value] = three_args(args);
                let mut map = map_arg(map)?;
                let key = text_arg(key)?;
                self.charge_items(map.len())?;
                self.charge_text(&key)?;
                if !map.contains_key(&key) {
                    self.ensure_items("map_set output items", map.len() + 1)?;
                }
                map.insert(key, value);
                Ok(Value::Map(map))
            }
            IntrinsicOp::MapDelete => {
                let [map, key] = two_args(args);
                let mut map = map_arg(map)?;
                let key = text_arg(key)?;
                self.charge_items(map.len())?;
                self.charge_text(&key)?;
                map.remove(&key);
                Ok(Value::Map(map))
            }
            IntrinsicOp::MapMerge => {
                let [left, right] = two_args(args);
                let mut left = map_arg(left)?;
                let right = map_arg(right)?;
                self.charge_items(left.len())?;
                self.charge_items(right.len())?;
                self.ensure_items(
                    "map_merge output items",
                    left.len().saturating_add(right.len()),
                )?;
                left.extend(right);
                self.ensure_items("map_merge output items", left.len())?;
                Ok(Value::Map(left))
            }
            IntrinsicOp::CollectionSlice => {
                let [items, start, length] = three_args(args);
                let items = list_arg(items)?;
                self.charge_items(items.len())?;
                let start = normalized_start(int_arg(start)?, items.len())?;
                let length = nonnegative_usize(int_arg(length)?, "collection_slice length")?;
                let end = start.saturating_add(length).min(items.len());
                self.ensure_items("collection_slice output items", end - start)?;
                Ok(Value::List(items[start..end].to_vec()))
            }
            IntrinsicOp::CollectionFindIndex => {
                let [items, sought] = two_args(args);
                let items = list_arg(items)?;
                self.charge_items(items.len())?;
                let index = items.iter().position(|item| *item == sought);
                Ok(Value::Int(index.map(|value| value as i64).unwrap_or(-1)))
            }
            IntrinsicOp::CollectionReverse => {
                let mut items = list_arg(only_arg(args))?;
                self.charge_items(items.len())?;
                items.reverse();
                Ok(Value::List(items))
            }
            IntrinsicOp::CollectionSort => {
                let mut items = list_arg(only_arg(args))?;
                self.charge_items(items.len())?;
                self.ensure_items("collection_sort items", items.len())?;
                // `sort_by` is stable: values that compare equal retain their
                // source order, while the total cross-type order is explicit.
                items.sort_by(value_sort_cmp);
                Ok(Value::List(items))
            }
            IntrinsicOp::CollectionUnique => {
                let items = list_arg(only_arg(args))?;
                self.charge_items(items.len())?;
                let mut output = Vec::new();
                for item in items {
                    if !output.contains(&item) {
                        self.ensure_items("collection_unique output items", output.len() + 1)?;
                        output.push(item);
                    }
                }
                Ok(Value::List(output))
            }
            IntrinsicOp::CollectionFlatten => {
                let items = list_arg(only_arg(args))?;
                self.charge_items(items.len())?;
                let mut output = Vec::new();
                for item in items {
                    let nested = list_arg(item)?;
                    self.charge_items(nested.len())?;
                    self.ensure_items(
                        "collection_flatten output items",
                        output.len().saturating_add(nested.len()),
                    )?;
                    output.extend(nested);
                }
                Ok(Value::List(output))
            }
            IntrinsicOp::CollectionZip => {
                let [left, right] = two_args(args);
                let left = list_arg(left)?;
                let right = list_arg(right)?;
                self.charge_items(left.len())?;
                self.charge_items(right.len())?;
                let length = left.len().min(right.len());
                self.ensure_items("collection_zip output items", length)?;
                Ok(Value::List(
                    left.into_iter()
                        .zip(right)
                        .map(|(a, b)| Value::List(vec![a, b]))
                        .collect(),
                ))
            }
            IntrinsicOp::Range => {
                let [start, end, step] = three_args(args);
                self.range(int_arg(start)?, int_arg(end)?, int_arg(step)?)
            }
            IntrinsicOp::TypeName => {
                let value = only_arg(args);
                Ok(Value::Text(value.type_name().to_owned()))
            }
            IntrinsicOp::ParseInt => {
                let text = text_arg(only_arg(args))?;
                self.charge_text(&text)?;
                Ok(text
                    .trim()
                    .parse::<i64>()
                    .map(Value::Int)
                    .unwrap_or(Value::Null))
            }
            IntrinsicOp::ParseFloat => {
                let text = text_arg(only_arg(args))?;
                self.charge_text(&text)?;
                Ok(text
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .map(Value::Float)
                    .unwrap_or(Value::Null))
            }
            IntrinsicOp::ParseBool => {
                let text = text_arg(only_arg(args))?;
                self.charge_text(&text)?;
                Ok(match text.trim() {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    _ => Value::Null,
                })
            }
            IntrinsicOp::ToText => {
                let value = only_arg(args);
                let text = match value {
                    Value::Text(text) => text,
                    Value::Null => "null".into(),
                    Value::Bool(value) => value.to_string(),
                    Value::Int(value) => value.to_string(),
                    Value::Float(value) if value.is_finite() => value.to_string(),
                    Value::Float(_) => {
                        return Err(SpoonError::InvalidJson(
                            "cannot convert a non-finite float to text".into(),
                        ));
                    }
                    value @ (Value::List(_) | Value::Map(_)) => self
                        .stringify_json(value)?
                        .as_text()
                        .expect("stringify_json returns text")
                        .to_owned(),
                };
                self.ensure_text("to_text output bytes", text.len())?;
                Ok(Value::Text(text))
            }
            IntrinsicOp::NumericAbs => numeric_abs(only_arg(args)),
            IntrinsicOp::NumericSign => numeric_sign(only_arg(args)),
            IntrinsicOp::NumericMin | IntrinsicOp::NumericMax => {
                let [left, right] = two_args(args);
                numeric_min_max(op, left, right)
            }
            IntrinsicOp::NumericClamp => {
                let [value, lower, upper] = three_args(args);
                numeric_clamp(value, lower, upper)
            }
            IntrinsicOp::NumericFloor
            | IntrinsicOp::NumericCeil
            | IntrinsicOp::NumericRound
            | IntrinsicOp::NumericTruncate => numeric_rounding(op, only_arg(args)),
            IntrinsicOp::NumericPowInt => {
                let [base, exponent] = two_args(args);
                self.numeric_pow_int(base, exponent)
            }
            IntrinsicOp::NumericPowFloat => {
                let [base, exponent] = two_args(args);
                numeric_pow_float(base, exponent)
            }
            IntrinsicOp::IntegerQuotient | IntrinsicOp::IntegerRemainder => {
                let [left, right] = two_args(args);
                integer_quotient_remainder(op, left, right)
            }
            IntrinsicOp::JsonParse => self.parse_json(text_arg(only_arg(args))?),
            IntrinsicOp::JsonStringify => self.stringify_json(only_arg(args)),
            IntrinsicOp::JsonPointerSet => {
                let [value, pointer, replacement] = three_args(args);
                self.set_json_pointer(value, text_arg(pointer)?, replacement)
            }
            IntrinsicOp::JsonPointerDelete => {
                let [value, pointer] = two_args(args);
                self.delete_json_pointer(value, text_arg(pointer)?)
            }
            IntrinsicOp::PathGet
            | IntrinsicOp::PathGetOptional
            | IntrinsicOp::JsonPointerGet
            | IntrinsicOp::JsonPointerGetOptional => {
                let optional = op == IntrinsicOp::PathGetOptional;
                let [value, path] = two_args(args);
                if matches!(
                    op,
                    IntrinsicOp::JsonPointerGet | IntrinsicOp::JsonPointerGetOptional
                ) {
                    self.get_json_pointer(
                        value,
                        text_arg(path)?,
                        op == IntrinsicOp::JsonPointerGetOptional,
                    )
                } else {
                    self.get_path(value, text_arg(path)?, optional)
                }
            }
            IntrinsicOp::Coalesce => Ok(args
                .into_iter()
                .find(|value| *value != Value::Null)
                .unwrap_or(Value::Null)),
        }
    }

    fn charge_text(&mut self, text: &str) -> Result<(), SpoonError> {
        self.ensure_text("text input bytes", text.len())?;
        self.charge_intrinsic_work(text.len())
    }

    fn text_tokenize(&mut self, text: String) -> Result<Value, SpoonError> {
        self.ensure_text("text_tokenize input bytes", text.len())?;
        if text.len() > MAX_TOKENIZE_INPUT_BYTES {
            return Err(SpoonError::IntrinsicLimitExceeded {
                operation: "text_tokenize input bytes".into(),
                limit: MAX_TOKENIZE_INPUT_BYTES,
            });
        }
        self.charge_intrinsic_work(text.len())?;
        let limits = LanguageLimits {
            max_input_bytes: MAX_TOKENIZE_INPUT_BYTES,
            max_tokens: MAX_TOKENIZE_ITEMS,
            ..LanguageLimits::default()
        };
        let stream = tokenize_with_limits(&text, &limits).map_err(|error| match error {
            LanguageError::LimitExceeded { limit, .. } => SpoonError::IntrinsicLimitExceeded {
                operation: "text_tokenize".into(),
                limit,
            },
            LanguageError::Invalid(message) => {
                SpoonError::Other(format!("text_tokenize: {message}"))
            }
        })?;
        self.ensure_items("text_tokenize output items", stream.tokens.len())?;
        self.charge_items(stream.tokens.len())?;
        let tokens = stream
            .tokens
            .iter()
            .map(|token| {
                let kind = match token.kind {
                    TokenKind::Word => "word",
                    TokenKind::Number => "number",
                    TokenKind::Whitespace => "whitespace",
                    TokenKind::Punctuation => "punctuation",
                    TokenKind::Symbol => "symbol",
                };
                let token_text = stream
                    .slice(&token.span)
                    .expect("validated token stream must slice its own spans");
                Value::Map(BTreeMap::from([
                    ("kind".into(), Value::Text(kind.into())),
                    ("text".into(), Value::Text(token_text.into())),
                    ("startByte".into(), Value::Int(token.span.start_byte as i64)),
                    ("endByte".into(), Value::Int(token.span.end_byte as i64)),
                ]))
            })
            .collect::<Vec<_>>();
        let value = Value::List(tokens);
        self.ensure_intrinsic_output("text_tokenize", &value)?;
        Ok(value)
    }
    fn charge_items(&mut self, items: usize) -> Result<(), SpoonError> {
        self.ensure_items("collection input items", items)?;
        self.charge_intrinsic_work(items)
    }
    fn ensure_text(&self, operation: &str, bytes: usize) -> Result<(), SpoonError> {
        if bytes > MAX_INTRINSIC_TEXT_BYTES {
            Err(SpoonError::IntrinsicLimitExceeded {
                operation: operation.into(),
                limit: MAX_INTRINSIC_TEXT_BYTES,
            })
        } else {
            Ok(())
        }
    }
    fn ensure_items(&self, operation: &str, items: usize) -> Result<(), SpoonError> {
        if items > MAX_INTRINSIC_ITEMS {
            Err(SpoonError::IntrinsicLimitExceeded {
                operation: operation.into(),
                limit: MAX_INTRINSIC_ITEMS,
            })
        } else {
            Ok(())
        }
    }

    fn ensure_intrinsic_output(
        &mut self,
        operation: &str,
        value: &Value,
    ) -> Result<(), SpoonError> {
        let bytes = self.intrinsic_value_bytes(value, 0)?;
        self.ensure_text(&format!("{operation} output bytes"), bytes)
    }

    fn intrinsic_value_bytes(&mut self, value: &Value, depth: usize) -> Result<usize, SpoonError> {
        if depth > MAX_JSON_DEPTH {
            return Err(SpoonError::IntrinsicLimitExceeded {
                operation: "intrinsic output depth".into(),
                limit: MAX_JSON_DEPTH,
            });
        }
        self.charge_intrinsic_work(1)?;
        match value {
            Value::Null => Ok(4),
            Value::Bool(value) => Ok(if *value { 4 } else { 5 }),
            Value::Int(value) => Ok(value.to_string().len()),
            Value::Float(value) => Ok(value.to_string().len()),
            Value::Text(value) => Ok(value.len()),
            Value::List(items) => {
                self.ensure_items("intrinsic output list items", items.len())?;
                items.iter().try_fold(2usize, |bytes, item| {
                    bytes
                        .checked_add(self.intrinsic_value_bytes(item, depth + 1)?)
                        .ok_or_else(|| SpoonError::ArithmeticOverflow {
                            operation: "intrinsic output size".into(),
                        })
                })
            }
            Value::Map(entries) => {
                self.ensure_items("intrinsic output map items", entries.len())?;
                entries.iter().try_fold(2usize, |bytes, (key, value)| {
                    let value_bytes = self.intrinsic_value_bytes(value, depth + 1)?;
                    bytes
                        .checked_add(key.len())
                        .and_then(|sum| sum.checked_add(value_bytes))
                        .ok_or_else(|| SpoonError::ArithmeticOverflow {
                            operation: "intrinsic output size".into(),
                        })
                })
            }
        }
    }
    fn range(&mut self, start: i64, end: i64, step: i64) -> Result<Value, SpoonError> {
        if step == 0 {
            return Err(SpoonError::Other("range step cannot be zero".into()));
        }
        if (step > 0 && start >= end) || (step < 0 && start <= end) {
            return Ok(Value::List(vec![]));
        }
        let distance = if step > 0 {
            end.checked_sub(start)
        } else {
            start.checked_sub(end)
        }
        .ok_or_else(|| SpoonError::ArithmeticOverflow {
            operation: "range distance".into(),
        })? as u64;
        let stride = step.unsigned_abs();
        let count = distance.div_ceil(stride) as usize;
        self.ensure_items("range output items", count)?;
        self.charge_items(count)?;
        let mut values = Vec::with_capacity(count);
        let mut current = start;
        for _ in 0..count {
            values.push(Value::Int(current));
            current = current
                .checked_add(step)
                .ok_or_else(|| SpoonError::ArithmeticOverflow {
                    operation: "range increment".into(),
                })?;
        }
        Ok(Value::List(values))
    }

    fn numeric_pow_int(&mut self, base: Value, exponent: Value) -> Result<Value, SpoonError> {
        let base = match base {
            Value::Int(value) => value,
            other => return Err(SpoonError::type_error("int", &other)),
        };
        let exponent = match exponent {
            Value::Int(value) => value,
            other => return Err(SpoonError::type_error("int", &other)),
        };
        if exponent < 0 {
            return Err(SpoonError::NegativeExponent {
                operation: "numeric_pow_int".into(),
            });
        }

        // Exponentiation by squaring is bounded by the width of i64 (at most
        // 63 loop iterations), and every multiplication is checked.
        let mut exponent = exponent as u64;
        let mut factor = base;
        let mut result = 1i64;
        while exponent != 0 {
            self.check_budget()?;
            if exponent & 1 == 1 {
                result =
                    result
                        .checked_mul(factor)
                        .ok_or_else(|| SpoonError::ArithmeticOverflow {
                            operation: "numeric_pow_int multiplication".into(),
                        })?;
            }
            exponent >>= 1;
            if exponent != 0 {
                factor =
                    factor
                        .checked_mul(factor)
                        .ok_or_else(|| SpoonError::ArithmeticOverflow {
                            operation: "numeric_pow_int squaring".into(),
                        })?;
            }
        }
        Ok(Value::Int(result))
    }
}

fn only_arg(mut args: Vec<Value>) -> Value {
    args.pop()
        .expect("arity validated before intrinsic evaluation")
}

fn two_args(args: Vec<Value>) -> [Value; 2] {
    args.try_into()
        .expect("arity validated before intrinsic evaluation")
}

fn three_args(args: Vec<Value>) -> [Value; 3] {
    args.try_into()
        .expect("arity validated before intrinsic evaluation")
}

fn text_arg(value: Value) -> Result<String, SpoonError> {
    match value {
        Value::Text(text) => Ok(text),
        other => Err(SpoonError::type_error("text", &other)),
    }
}

fn int_arg(value: Value) -> Result<i64, SpoonError> {
    match value {
        Value::Int(value) => Ok(value),
        other => Err(SpoonError::type_error("int", &other)),
    }
}

fn nonnegative_usize(value: i64, operation: &str) -> Result<usize, SpoonError> {
    usize::try_from(value)
        .map_err(|_| SpoonError::Other(format!("{operation} must be non-negative")))
}

fn normalized_start(start: i64, length: usize) -> Result<usize, SpoonError> {
    let length_i64 = i64::try_from(length).map_err(|_| SpoonError::ArithmeticOverflow {
        operation: "collection length conversion".into(),
    })?;
    let start = if start < 0 {
        start
            .checked_add(length_i64)
            .ok_or_else(|| SpoonError::ArithmeticOverflow {
                operation: "negative index".into(),
            })?
    } else {
        start
    };
    Ok(usize::try_from(start).unwrap_or(usize::MAX).min(length))
}

fn append_text(output: &mut String, text: &str, operation: &str) -> Result<(), SpoonError> {
    let total =
        output
            .len()
            .checked_add(text.len())
            .ok_or_else(|| SpoonError::ArithmeticOverflow {
                operation: format!("{operation} output size"),
            })?;
    if total > MAX_INTRINSIC_TEXT_BYTES {
        return Err(SpoonError::IntrinsicLimitExceeded {
            operation: format!("{operation} output bytes"),
            limit: MAX_INTRINSIC_TEXT_BYTES,
        });
    }
    output.push_str(text);
    Ok(())
}

fn invalid_number(operation: &str, reason: impl Into<String>) -> SpoonError {
    SpoonError::InvalidNumber {
        operation: operation.into(),
        reason: reason.into(),
    }
}

fn finite_numeric_float(value: &Value, operation: &str) -> Result<f64, SpoonError> {
    let number = match value {
        Value::Int(value) => *value as f64,
        Value::Float(value) => *value,
        other => return Err(SpoonError::type_error("numeric", other)),
    };
    if number.is_finite() {
        Ok(number)
    } else {
        Err(invalid_number(operation, "value must be finite"))
    }
}

fn numeric_abs(value: Value) -> Result<Value, SpoonError> {
    match value {
        Value::Int(value) => {
            value
                .checked_abs()
                .map(Value::Int)
                .ok_or_else(|| SpoonError::ArithmeticOverflow {
                    operation: "numeric_abs".into(),
                })
        }
        Value::Float(value) => {
            if !value.is_finite() {
                Err(invalid_number("numeric_abs", "value must be finite"))
            } else {
                Ok(Value::Float(value.abs()))
            }
        }
        other => Err(SpoonError::type_error("numeric", &other)),
    }
}

fn numeric_sign(value: Value) -> Result<Value, SpoonError> {
    match value {
        Value::Int(value) => Ok(Value::Int(value.signum())),
        Value::Float(value) if value.is_finite() => Ok(Value::Int(if value > 0.0 {
            1
        } else if value < 0.0 {
            -1
        } else {
            0
        })),
        Value::Float(_) => Err(invalid_number("numeric_sign", "value must be finite")),
        other => Err(SpoonError::type_error("numeric", &other)),
    }
}

fn numeric_ordering(left: &Value, right: &Value, operation: &str) -> Result<Ordering, SpoonError> {
    match (left, right) {
        (Value::Int(left), Value::Int(right)) => Ok(left.cmp(right)),
        _ => finite_numeric_float(left, operation)?
            .partial_cmp(&finite_numeric_float(right, operation)?)
            .ok_or_else(|| {
                invalid_number(operation, "comparison is undefined for non-finite values")
            }),
    }
}

fn numeric_min_max(op: IntrinsicOp, left: Value, right: Value) -> Result<Value, SpoonError> {
    let operation = intrinsic_name(op);
    let ordering = numeric_ordering(&left, &right, operation)?;
    let any_float = matches!(left, Value::Float(_)) || matches!(right, Value::Float(_));
    let selected = match op {
        IntrinsicOp::NumericMin if ordering != Ordering::Greater => left,
        IntrinsicOp::NumericMin => right,
        IntrinsicOp::NumericMax if ordering != Ordering::Less => left,
        IntrinsicOp::NumericMax => right,
        _ => unreachable!("numeric_min_max only handles min and max"),
    };
    if any_float {
        Ok(Value::Float(finite_numeric_float(&selected, operation)?))
    } else {
        Ok(selected)
    }
}

fn numeric_clamp(value: Value, lower: Value, upper: Value) -> Result<Value, SpoonError> {
    let operation = "numeric_clamp";
    let any_float = matches!(value, Value::Float(_))
        || matches!(lower, Value::Float(_))
        || matches!(upper, Value::Float(_));
    if numeric_ordering(&lower, &upper, operation)? == Ordering::Greater {
        return Err(invalid_number(
            operation,
            "lower bound must not exceed upper bound",
        ));
    }
    let selected = if numeric_ordering(&value, &lower, operation)? == Ordering::Less {
        lower
    } else if numeric_ordering(&value, &upper, operation)? == Ordering::Greater {
        upper
    } else {
        value
    };
    if any_float {
        Ok(Value::Float(finite_numeric_float(&selected, operation)?))
    } else {
        Ok(selected)
    }
}

fn numeric_rounding(op: IntrinsicOp, value: Value) -> Result<Value, SpoonError> {
    match value {
        Value::Int(value) => Ok(Value::Int(value)),
        Value::Float(value) if value.is_finite() => {
            let rounded = match op {
                IntrinsicOp::NumericFloor => value.floor(),
                IntrinsicOp::NumericCeil => value.ceil(),
                // Ties round away from zero, matching Rust's f64::round.
                IntrinsicOp::NumericRound => value.round(),
                IntrinsicOp::NumericTruncate => value.trunc(),
                _ => unreachable!("numeric_rounding only handles rounding operations"),
            };
            if rounded.is_finite() {
                Ok(Value::Float(rounded))
            } else {
                Err(invalid_number(intrinsic_name(op), "result must be finite"))
            }
        }
        Value::Float(_) => Err(invalid_number(intrinsic_name(op), "value must be finite")),
        other => Err(SpoonError::type_error("numeric", &other)),
    }
}

fn numeric_pow_float(base: Value, exponent: Value) -> Result<Value, SpoonError> {
    let operation = "numeric_pow_float";
    let base = finite_numeric_float(&base, operation)?;
    let exponent = finite_numeric_float(&exponent, operation)?;
    let result = base.powf(exponent);
    if result.is_finite() {
        Ok(Value::Float(result))
    } else {
        Err(invalid_number(operation, "result must be finite"))
    }
}

fn integer_quotient_remainder(
    op: IntrinsicOp,
    left: Value,
    right: Value,
) -> Result<Value, SpoonError> {
    let left = match left {
        Value::Int(value) => value,
        other => return Err(SpoonError::type_error("int", &other)),
    };
    let right = match right {
        Value::Int(value) => value,
        other => return Err(SpoonError::type_error("int", &other)),
    };
    if right == 0 {
        return Err(SpoonError::DivisionByZero);
    }
    let result = match op {
        IntrinsicOp::IntegerQuotient => left.checked_div(right),
        IntrinsicOp::IntegerRemainder => left.checked_rem(right),
        _ => unreachable!("integer_quotient_remainder only handles integer division operations"),
    };
    result
        .map(Value::Int)
        .ok_or_else(|| SpoonError::ArithmeticOverflow {
            operation: intrinsic_name(op).into(),
        })
}

fn value_sort_key(value: &Value) -> String {
    match value {
        Value::Null => "0:".into(),
        Value::Bool(value) => format!("1:{value}"),
        Value::Int(value) => format!("2:{value:020}"),
        Value::Float(value) => format!("3:{value:024.16}"),
        Value::Text(value) => format!("4:{value}"),
        Value::List(value) => format!("5:{value:?}"),
        Value::Map(value) => format!("6:{value:?}"),
    }
}

fn value_sort_cmp(left: &Value, right: &Value) -> Ordering {
    let rank = |value: &Value| match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Int(_) => 2,
        Value::Float(_) => 3,
        Value::Text(_) => 4,
        Value::List(_) => 5,
        Value::Map(_) => 6,
    };
    rank(left)
        .cmp(&rank(right))
        .then_with(|| match (left, right) {
            (Value::Null, Value::Null) => Ordering::Equal,
            (Value::Bool(left), Value::Bool(right)) => left.cmp(right),
            (Value::Int(left), Value::Int(right)) => left.cmp(right),
            (Value::Float(left), Value::Float(right)) => left.total_cmp(right),
            (Value::Text(left), Value::Text(right)) => left.cmp(right),
            _ => value_sort_key(left).cmp(&value_sort_key(right)),
        })
}

fn list_arg(value: Value) -> Result<Vec<Value>, SpoonError> {
    match value {
        Value::List(items) => Ok(items),
        other => Err(SpoonError::type_error("list", &other)),
    }
}

fn map_arg(value: Value) -> Result<std::collections::BTreeMap<String, Value>, SpoonError> {
    match value {
        Value::Map(entries) => Ok(entries),
        other => Err(SpoonError::type_error("map", &other)),
    }
}

impl Evaluator {
    fn parse_json(&mut self, text: String) -> Result<Value, SpoonError> {
        if text.len() > MAX_JSON_BYTES {
            return Err(SpoonError::IntrinsicLimitExceeded {
                operation: "json_parse input bytes".into(),
                limit: MAX_JSON_BYTES,
            });
        }
        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| SpoonError::InvalidJson(error.to_string()))?;
        self.json_to_value(json, 0)
    }

    fn json_to_value(
        &mut self,
        json: serde_json::Value,
        depth: usize,
    ) -> Result<Value, SpoonError> {
        self.charge_intrinsic_work(1)?;
        if depth > MAX_JSON_DEPTH {
            return Err(SpoonError::IntrinsicLimitExceeded {
                operation: "json_parse depth".into(),
                limit: MAX_JSON_DEPTH,
            });
        }
        match json {
            serde_json::Value::Null => Ok(Value::Null),
            serde_json::Value::Bool(value) => Ok(Value::Bool(value)),
            serde_json::Value::String(value) => Ok(Value::Text(value)),
            serde_json::Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    Ok(Value::Int(value))
                } else if value.as_u64().is_some() {
                    Err(SpoonError::InvalidJson(
                        "integer is outside Spoon's signed 64-bit range".into(),
                    ))
                } else if let Some(value) = value.as_f64() {
                    Ok(Value::Float(value))
                } else {
                    Err(SpoonError::InvalidJson("invalid JSON number".into()))
                }
            }
            serde_json::Value::Array(items) => {
                self.ensure_items("json_parse array items", items.len())?;
                items
                    .into_iter()
                    .map(|item| self.json_to_value(item, depth + 1))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::List)
            }
            serde_json::Value::Object(entries) => {
                self.ensure_items("json_parse object items", entries.len())?;
                entries
                    .into_iter()
                    .map(|(key, value)| Ok((key, self.json_to_value(value, depth + 1)?)))
                    .collect::<Result<_, SpoonError>>()
                    .map(Value::Map)
            }
        }
    }

    fn stringify_json(&mut self, value: Value) -> Result<Value, SpoonError> {
        let json = self.value_to_json(value, 0)?;
        let text = serde_json::to_string(&json)
            .map_err(|error| SpoonError::InvalidJson(format!("cannot stringify value: {error}")))?;
        if text.len() > MAX_JSON_BYTES {
            return Err(SpoonError::IntrinsicLimitExceeded {
                operation: "json_stringify output bytes".into(),
                limit: MAX_JSON_BYTES,
            });
        }
        Ok(Value::Text(text))
    }

    fn value_to_json(
        &mut self,
        value: Value,
        depth: usize,
    ) -> Result<serde_json::Value, SpoonError> {
        self.charge_intrinsic_work(1)?;
        if depth > MAX_JSON_DEPTH {
            return Err(SpoonError::IntrinsicLimitExceeded {
                operation: "json_stringify depth".into(),
                limit: MAX_JSON_DEPTH,
            });
        }
        match value {
            Value::Null => Ok(serde_json::Value::Null),
            Value::Bool(value) => Ok(serde_json::Value::Bool(value)),
            Value::Int(value) => Ok(serde_json::Value::Number(value.into())),
            Value::Float(value) => serde_json::Number::from_f64(value)
                .map(serde_json::Value::Number)
                .ok_or_else(|| {
                    SpoonError::InvalidJson("cannot stringify a non-finite float".into())
                }),
            Value::Text(value) => Ok(serde_json::Value::String(value)),
            Value::List(items) => {
                self.ensure_items("json_stringify array items", items.len())?;
                items
                    .into_iter()
                    .map(|item| self.value_to_json(item, depth + 1))
                    .collect::<Result<Vec<_>, _>>()
                    .map(serde_json::Value::Array)
            }
            Value::Map(entries) => {
                self.ensure_items("json_stringify object items", entries.len())?;
                entries
                    .into_iter()
                    .map(|(key, value)| Ok((key, self.value_to_json(value, depth + 1)?)))
                    .collect::<Result<serde_json::Map<_, _>, SpoonError>>()
                    .map(serde_json::Value::Object)
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PathSegment {
    Key(String),
    Index(i64),
}

impl Evaluator {
    fn get_path(
        &mut self,
        value: Value,
        path: String,
        optional: bool,
    ) -> Result<Value, SpoonError> {
        self.charge_text(&path)?;
        let segments = parse_path(&path)?;
        self.resolve_path(value, segments, optional, false)
    }

    fn get_json_pointer(
        &mut self,
        value: Value,
        pointer: String,
        optional: bool,
    ) -> Result<Value, SpoonError> {
        self.charge_text(&pointer)?;
        let segments = parse_json_pointer(&pointer)?;
        self.resolve_path(value, segments, optional, true)
    }

    fn set_json_pointer(
        &mut self,
        value: Value,
        pointer: String,
        replacement: Value,
    ) -> Result<Value, SpoonError> {
        self.charge_text(&pointer)?;
        let segments = parse_json_pointer(&pointer)?;
        self.charge_items(segments.len())?;
        if segments.is_empty() {
            return Ok(replacement);
        }
        update_json_pointer(self, value, &segments, replacement, false)
    }

    fn delete_json_pointer(&mut self, value: Value, pointer: String) -> Result<Value, SpoonError> {
        self.charge_text(&pointer)?;
        let segments = parse_json_pointer(&pointer)?;
        self.charge_items(segments.len())?;
        if segments.is_empty() {
            return Ok(Value::Null);
        }
        update_json_pointer(self, value, &segments, Value::Null, true)
    }

    fn resolve_path(
        &mut self,
        value: Value,
        segments: Vec<PathSegment>,
        optional: bool,
        pointer_mode: bool,
    ) -> Result<Value, SpoonError> {
        self.charge_items(segments.len())?;
        let mut current = value;
        for segment in segments {
            let next = match (current, segment) {
                (Value::Map(entries), PathSegment::Key(key)) => entries
                    .get(&key)
                    .cloned()
                    .ok_or(SpoonError::FieldNotFound(key)),
                (Value::List(items), PathSegment::Index(index)) => match usize::try_from(index) {
                    Ok(list_index) => {
                        items
                            .get(list_index)
                            .cloned()
                            .ok_or(SpoonError::IndexOutOfBounds {
                                index,
                                length: items.len(),
                            })
                    }
                    Err(_) => Err(SpoonError::IndexOutOfBounds {
                        index,
                        length: items.len(),
                    }),
                },
                (Value::List(items), PathSegment::Key(key)) if pointer_mode => {
                    let index = parse_pointer_index(&key)?;
                    items
                        .get(index)
                        .cloned()
                        .ok_or(SpoonError::IndexOutOfBounds {
                            index: index as i64,
                            length: items.len(),
                        })
                }
                (Value::Map(_), PathSegment::Index(_)) => Err(SpoonError::TypeError {
                    expected: "list for a bracket index path segment".into(),
                    got: "map".into(),
                }),
                (Value::List(_), PathSegment::Key(_)) => Err(SpoonError::TypeError {
                    expected: "map for a key path segment".into(),
                    got: "list".into(),
                }),
                (other, _) => Err(SpoonError::type_error("map or list", &other)),
            };
            match next {
                Ok(next) => current = next,
                Err(SpoonError::FieldNotFound(_)) | Err(SpoonError::IndexOutOfBounds { .. })
                    if optional =>
                {
                    return Ok(Value::Null);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(current)
    }
}

fn update_json_pointer(
    evaluator: &mut Evaluator,
    value: Value,
    segments: &[PathSegment],
    replacement: Value,
    delete: bool,
) -> Result<Value, SpoonError> {
    let segment = segments
        .first()
        .ok_or_else(|| SpoonError::Other("JSON Pointer update requires a target".into()))?;
    let last = segments.len() == 1;
    match (value, segment) {
        (Value::Map(mut entries), PathSegment::Key(key)) => {
            evaluator.charge_items(entries.len())?;
            if last {
                if delete {
                    entries
                        .remove(key)
                        .ok_or_else(|| SpoonError::FieldNotFound(key.clone()))?;
                } else {
                    if !entries.contains_key(key) {
                        evaluator
                            .ensure_items("json_pointer_set output items", entries.len() + 1)?;
                    }
                    entries.insert(key.clone(), replacement);
                }
                return Ok(Value::Map(entries));
            }
            let child = entries
                .remove(key)
                .ok_or_else(|| SpoonError::FieldNotFound(key.clone()))?;
            let updated =
                update_json_pointer(evaluator, child, &segments[1..], replacement, delete)?;
            entries.insert(key.clone(), updated);
            Ok(Value::Map(entries))
        }
        (Value::List(mut items), PathSegment::Key(key)) => {
            let index = parse_pointer_index(key)?;
            evaluator.charge_items(items.len())?;
            if index >= items.len() {
                return Err(SpoonError::IndexOutOfBounds {
                    index: index as i64,
                    length: items.len(),
                });
            }
            if last {
                if delete {
                    items.remove(index);
                } else {
                    items[index] = replacement;
                }
                return Ok(Value::List(items));
            }
            let child = items.remove(index);
            let updated =
                update_json_pointer(evaluator, child, &segments[1..], replacement, delete)?;
            items.insert(index, updated);
            Ok(Value::List(items))
        }
        (Value::Map(_), PathSegment::Index(_)) => Err(SpoonError::TypeError {
            expected: "map key path segment".into(),
            got: "index".into(),
        }),
        (Value::List(_), PathSegment::Index(_)) => Err(SpoonError::TypeError {
            expected: "JSON Pointer key segment".into(),
            got: "index".into(),
        }),
        (other, _) => Err(SpoonError::type_error("map or list", &other)),
    }
}

fn parse_json_pointer(pointer: &str) -> Result<Vec<PathSegment>, SpoonError> {
    if pointer.len() > MAX_PATH_BYTES {
        return Err(SpoonError::IntrinsicLimitExceeded {
            operation: "json pointer bytes".into(),
            limit: MAX_PATH_BYTES,
        });
    }
    if pointer.is_empty() {
        return Ok(Vec::new());
    }
    if !pointer.starts_with('/') {
        return Err(invalid_path(
            pointer,
            "JSON Pointer must be empty or start with '/'",
        ));
    }

    let mut segments = Vec::new();
    for raw in pointer[1..].split('/') {
        if segments.len() >= MAX_PATH_SEGMENTS {
            return Err(SpoonError::IntrinsicLimitExceeded {
                operation: "json pointer segments".into(),
                limit: MAX_PATH_SEGMENTS,
            });
        }
        let mut decoded = String::with_capacity(raw.len());
        let mut chars = raw.chars();
        while let Some(character) = chars.next() {
            if character == '~' {
                match chars.next() {
                    Some('0') => decoded.push('~'),
                    Some('1') => decoded.push('/'),
                    _ => return Err(invalid_path(pointer, "invalid JSON Pointer escape")),
                }
            } else {
                decoded.push(character);
            }
        }
        segments.push(PathSegment::Key(decoded));
    }
    Ok(segments)
}

fn parse_pointer_index(segment: &str) -> Result<usize, SpoonError> {
    if segment.is_empty() || segment == "-" {
        return Err(SpoonError::type_error(
            "array index",
            &Value::Text(segment.into()),
        ));
    }
    if segment.len() > 1 && segment.starts_with('0') {
        return Err(SpoonError::type_error(
            "canonical array index",
            &Value::Text(segment.into()),
        ));
    }
    segment
        .parse::<usize>()
        .map_err(|_| SpoonError::type_error("array index", &Value::Text(segment.into())))
}

fn parse_path(path: &str) -> Result<Vec<PathSegment>, SpoonError> {
    if path.len() > MAX_PATH_BYTES {
        return Err(SpoonError::IntrinsicLimitExceeded {
            operation: "path bytes".into(),
            limit: MAX_PATH_BYTES,
        });
    }
    if path.is_empty() {
        return Err(invalid_path(path, "path cannot be empty"));
    }

    let bytes = path.as_bytes();
    let mut cursor = 0;
    let mut segments = Vec::new();
    while cursor < bytes.len() {
        let segment = match bytes[cursor] {
            b'.' => return Err(invalid_path(path, "empty dot segment")),
            b'[' => parse_bracket_segment(path, &mut cursor)?,
            _ => {
                let start = cursor;
                while cursor < bytes.len() && bytes[cursor] != b'.' && bytes[cursor] != b'[' {
                    if bytes[cursor] == b']' {
                        return Err(invalid_path(path, "unexpected closing bracket"));
                    }
                    cursor += 1;
                }
                PathSegment::Key(path[start..cursor].to_owned())
            }
        };
        segments.push(segment);
        if segments.len() > MAX_PATH_SEGMENTS {
            return Err(SpoonError::IntrinsicLimitExceeded {
                operation: "path segments".into(),
                limit: MAX_PATH_SEGMENTS,
            });
        }
        if cursor == bytes.len() {
            break;
        }
        if bytes[cursor] == b'.' {
            cursor += 1;
            if cursor == bytes.len() {
                return Err(invalid_path(path, "path cannot end with a dot"));
            }
        } else if bytes[cursor] != b'[' {
            return Err(invalid_path(path, "segments must be separated by . or []"));
        }
    }
    Ok(segments)
}

fn parse_bracket_segment(path: &str, cursor: &mut usize) -> Result<PathSegment, SpoonError> {
    let bytes = path.as_bytes();
    *cursor += 1; // '['
    if *cursor >= bytes.len() {
        return Err(invalid_path(path, "unterminated bracket segment"));
    }
    if bytes[*cursor] == b'"' {
        let start = *cursor;
        *cursor += 1;
        let mut escaped = false;
        while *cursor < bytes.len() {
            let byte = bytes[*cursor];
            *cursor += 1;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                break;
            }
        }
        if *cursor > bytes.len() || bytes.get(*cursor - 1) != Some(&b'"') {
            return Err(invalid_path(path, "unterminated quoted key"));
        }
        let key: String = serde_json::from_str(&path[start..*cursor])
            .map_err(|error| invalid_path(path, format!("invalid quoted key: {error}")))?;
        if bytes.get(*cursor) != Some(&b']') {
            return Err(invalid_path(path, "quoted key must end with ]"));
        }
        *cursor += 1;
        return Ok(PathSegment::Key(key));
    }

    let start = *cursor;
    while *cursor < bytes.len() && bytes[*cursor].is_ascii_digit() {
        *cursor += 1;
    }
    if start == *cursor || bytes.get(*cursor) != Some(&b']') {
        return Err(invalid_path(
            path,
            "bracket index must be a non-negative integer",
        ));
    }
    let index = path[start..*cursor]
        .parse::<i64>()
        .map_err(|_| invalid_path(path, "bracket index is too large"))?;
    *cursor += 1;
    Ok(PathSegment::Index(index))
}

fn invalid_path(path: &str, reason: impl Into<String>) -> SpoonError {
    SpoonError::InvalidPath {
        path: path.to_owned(),
        reason: reason.into(),
    }
}

#[derive(Debug, Clone, Copy)]
enum ContractConditionKind {
    Requires,
    Promises,
    FailsWhen,
}

impl ContractConditionKind {
    fn label(self) -> &'static str {
        match self {
            Self::Requires => "requires",
            Self::Promises => "promises",
            Self::FailsWhen => "fails_when",
        }
    }
}

fn apply_binop(op: BinOp, l: Value, r: Value) -> Result<Value, SpoonError> {
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => arithmetic(op, l, r),
        BinOp::Eq => Ok(Value::Bool(l == r)),
        BinOp::Ne => Ok(Value::Bool(l != r)),
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => compare(op, &l, &r),
        BinOp::And | BinOp::Or => unreachable!("And/Or are short-circuited before reaching here"),
    }
}

fn arithmetic(op: BinOp, l: Value, r: Value) -> Result<Value, SpoonError> {
    match (&l, &r) {
        (Value::Int(a), Value::Int(b)) => int_op(op, *a, *b),
        (Value::Text(a), Value::Text(b)) if op == BinOp::Add => Ok(Value::Text(format!("{a}{b}"))),
        (Value::List(a), Value::List(b)) if op == BinOp::Add => {
            let mut items = a.clone();
            items.extend(b.clone());
            Ok(Value::List(items))
        }
        _ if l.is_numeric() && r.is_numeric() => {
            let a = l.as_float().expect("numeric");
            let b = r.as_float().expect("numeric");
            float_op(op, a, b)
        }
        _ => Err(SpoonError::TypeError {
            expected: "numeric, text, or list operands".to_string(),
            got: format!("{} and {}", l.type_name(), r.type_name()),
        }),
    }
}

fn int_op(op: BinOp, a: i64, b: i64) -> Result<Value, SpoonError> {
    match op {
        BinOp::Add => checked_int("integer addition", a.checked_add(b)),
        BinOp::Sub => checked_int("integer subtraction", a.checked_sub(b)),
        BinOp::Mul => checked_int("integer multiplication", a.checked_mul(b)),
        BinOp::Div => {
            if b == 0 {
                Err(SpoonError::DivisionByZero)
            } else {
                checked_int("integer division", a.checked_div(b))
            }
        }
        BinOp::Mod => {
            if b == 0 {
                Err(SpoonError::DivisionByZero)
            } else {
                checked_int("integer remainder", a.checked_rem(b))
            }
        }
        _ => unreachable!("int_op only called for arithmetic ops"),
    }
}

fn checked_int(operation: &str, value: Option<i64>) -> Result<Value, SpoonError> {
    value
        .map(Value::Int)
        .ok_or_else(|| SpoonError::ArithmeticOverflow {
            operation: operation.into(),
        })
}

fn float_op(op: BinOp, a: f64, b: f64) -> Result<Value, SpoonError> {
    match op {
        BinOp::Add => Ok(Value::Float(a + b)),
        BinOp::Sub => Ok(Value::Float(a - b)),
        BinOp::Mul => Ok(Value::Float(a * b)),
        BinOp::Div => {
            if b == 0.0 {
                Err(SpoonError::DivisionByZero)
            } else {
                Ok(Value::Float(a / b))
            }
        }
        BinOp::Mod => {
            if b == 0.0 {
                Err(SpoonError::DivisionByZero)
            } else {
                Ok(Value::Float(a % b))
            }
        }
        _ => unreachable!("float_op only called for arithmetic ops"),
    }
}

fn compare(op: BinOp, l: &Value, r: &Value) -> Result<Value, SpoonError> {
    let ordering = if l.is_numeric() && r.is_numeric() {
        l.as_float().unwrap().partial_cmp(&r.as_float().unwrap())
    } else if let (Value::Text(a), Value::Text(b)) = (l, r) {
        Some(a.cmp(b))
    } else {
        None
    };

    let ordering = ordering.ok_or_else(|| SpoonError::TypeError {
        expected: "comparable operands".to_string(),
        got: format!("{} and {}", l.type_name(), r.type_name()),
    })?;

    let result = match op {
        BinOp::Lt => ordering == Ordering::Less,
        BinOp::Le => ordering != Ordering::Greater,
        BinOp::Gt => ordering == Ordering::Greater,
        BinOp::Ge => ordering != Ordering::Less,
        _ => unreachable!("compare only called for ordering ops"),
    };

    Ok(Value::Bool(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{ConditionCheckStatus, ExecStepStatus};
    use spoon_core::{Condition, Contract, IntrinsicOp, Param};

    fn lit_int(n: i64) -> Expr {
        Expr::Literal(Value::Int(n))
    }

    fn lit_float(n: f64) -> Expr {
        Expr::Literal(Value::Float(n))
    }

    fn binop(op: BinOp, left: Expr, right: Expr) -> Expr {
        Expr::BinOp {
            op,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    fn intrinsic(op: IntrinsicOp, args: Vec<Expr>) -> Expr {
        Expr::Intrinsic {
            version: 1,
            op,
            args,
        }
    }

    fn lit_text(text: &str) -> Expr {
        Expr::Literal(Value::Text(text.to_string()))
    }

    #[test]
    fn text_split_filter_and_length_compose_into_letter_counting() {
        let split = intrinsic(
            IntrinsicOp::TextSplit,
            vec![lit_text("strawberry"), lit_text("")],
        );
        let only_rs = Expr::Filter {
            collection: Box::new(split),
            var: "character".to_string(),
            predicate: Box::new(binop(
                BinOp::Eq,
                Expr::Var("character".to_string()),
                lit_text("r"),
            )),
        };
        let expression = intrinsic(IntrinsicOp::Length, vec![only_rs]);

        let result = Evaluator::new().eval(&expression, &mut Env::new()).unwrap();

        assert_eq!(result, Value::Int(3));
    }

    #[test]
    fn text_lengths_make_unicode_units_explicit() {
        let text = lit_text("e\u{301}");
        let cases = [
            (IntrinsicOp::TextByteLength, Value::Int(3)),
            (IntrinsicOp::TextScalarLength, Value::Int(2)),
            (IntrinsicOp::TextGraphemeLength, Value::Int(1)),
        ];

        for (op, expected) in cases {
            let result = Evaluator::new()
                .eval(&intrinsic(op, vec![text.clone()]), &mut Env::new())
                .unwrap();
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn text_tokenize_preserves_utf8_byte_spans_and_token_kinds() {
        let expression = intrinsic(IntrinsicOp::TextTokenize, vec![lit_text("é 42!©")]);

        let result = Evaluator::new().eval(&expression, &mut Env::new()).unwrap();

        let token = |kind: &str, text: &str, start_byte, end_byte| {
            Value::Map(std::collections::BTreeMap::from([
                ("kind".into(), Value::Text(kind.into())),
                ("text".into(), Value::Text(text.into())),
                ("startByte".into(), Value::Int(start_byte)),
                ("endByte".into(), Value::Int(end_byte)),
            ]))
        };
        assert_eq!(
            result,
            Value::List(vec![
                token("word", "é", 0, 2),
                token("whitespace", " ", 2, 3),
                token("number", "42", 3, 5),
                token("punctuation", "!", 5, 6),
                token("symbol", "©", 6, 8),
            ])
        );
    }

    #[test]
    fn text_tokenize_enforces_its_stricter_token_limit() {
        let expression = intrinsic(
            IntrinsicOp::TextTokenize,
            vec![lit_text(&"!".repeat(4_097))],
        );

        assert!(matches!(
            Evaluator::new().eval(&expression, &mut Env::new()),
            Err(SpoonError::IntrinsicLimitExceeded { limit: 4_096, .. })
        ));
    }

    #[test]
    fn json_parse_and_paths_support_dot_index_and_quoted_keys() {
        let parsed = intrinsic(
            IntrinsicOp::JsonParse,
            vec![lit_text(
                r#"{"user":{"profile":{"name":"Spoon"}},"items":[{"id":7}],"a.b":"quoted"}"#,
            )],
        );
        let cases = [
            ("user.profile.name", Value::Text("Spoon".to_string())),
            ("items[0].id", Value::Int(7)),
            (r#"["a.b"]"#, Value::Text("quoted".to_string())),
        ];

        for (path, expected) in cases {
            let expression = intrinsic(IntrinsicOp::PathGet, vec![parsed.clone(), lit_text(path)]);
            let result = Evaluator::new().eval(&expression, &mut Env::new()).unwrap();
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn optional_path_only_turns_absence_into_null() {
        let parsed = intrinsic(
            IntrinsicOp::JsonParse,
            vec![lit_text(r#"{"present":null,"items":[]}"#)],
        );

        let present_null = intrinsic(
            IntrinsicOp::PathGet,
            vec![parsed.clone(), lit_text("present")],
        );
        let absent = intrinsic(
            IntrinsicOp::PathGetOptional,
            vec![parsed.clone(), lit_text("missing.value")],
        );
        let strict_absent = intrinsic(
            IntrinsicOp::PathGet,
            vec![parsed.clone(), lit_text("missing.value")],
        );
        let malformed = intrinsic(
            IntrinsicOp::PathGetOptional,
            vec![parsed, lit_text("items[")],
        );

        assert_eq!(
            Evaluator::new()
                .eval(&present_null, &mut Env::new())
                .unwrap(),
            Value::Null
        );
        assert_eq!(
            Evaluator::new().eval(&absent, &mut Env::new()).unwrap(),
            Value::Null
        );
        assert!(matches!(
            Evaluator::new().eval(&strict_absent, &mut Env::new()),
            Err(SpoonError::FieldNotFound(_))
        ));
        assert!(matches!(
            Evaluator::new().eval(&malformed, &mut Env::new()),
            Err(SpoonError::InvalidPath { .. })
        ));
    }

    #[test]
    fn json_and_intrinsic_failures_are_typed() {
        let invalid_json = intrinsic(IntrinsicOp::JsonParse, vec![lit_text("{")]);
        let wrong_arity = intrinsic(IntrinsicOp::TextTrim, vec![]);
        let unsupported_version = Expr::Intrinsic {
            version: 99,
            op: IntrinsicOp::Length,
            args: vec![lit_text("abc")],
        };

        assert!(matches!(
            Evaluator::new().eval(&invalid_json, &mut Env::new()),
            Err(SpoonError::InvalidJson(_))
        ));
        assert!(matches!(
            Evaluator::new().eval(&wrong_arity, &mut Env::new()),
            Err(SpoonError::ArityMismatch { .. })
        ));
        assert!(matches!(
            Evaluator::new().eval(&unsupported_version, &mut Env::new()),
            Err(SpoonError::UnsupportedIntrinsicVersion(99))
        ));
    }

    #[test]
    fn json_stringify_is_deterministic_and_round_trips_structure() {
        let expression = intrinsic(
            IntrinsicOp::JsonStringify,
            vec![intrinsic(
                IntrinsicOp::JsonParse,
                vec![lit_text(r#"{"z":1,"a":[true,null,2.5]}"#)],
            )],
        );

        let result = Evaluator::new().eval(&expression, &mut Env::new()).unwrap();

        assert_eq!(
            result,
            Value::Text(r#"{"a":[true,null,2.5],"z":1}"#.to_string())
        );
    }

    #[test]
    fn text_intrinsics_have_composable_v1_semantics() {
        let cases = [
            (
                intrinsic(
                    IntrinsicOp::TextJoin,
                    vec![
                        Expr::ListExpr(vec![lit_text("Spoon"), lit_text("runtime")]),
                        lit_text(" "),
                    ],
                ),
                Value::Text("Spoon runtime".into()),
            ),
            (
                intrinsic(IntrinsicOp::TextTrim, vec![lit_text("  spoon\n")]),
                Value::Text("spoon".into()),
            ),
            (
                intrinsic(IntrinsicOp::TextLowercase, vec![lit_text("SpOoN")]),
                Value::Text("spoon".into()),
            ),
            (
                intrinsic(IntrinsicOp::TextUppercase, vec![lit_text("spoon")]),
                Value::Text("SPOON".into()),
            ),
            (
                intrinsic(
                    IntrinsicOp::TextContains,
                    vec![lit_text("strawberry"), lit_text("raw")],
                ),
                Value::Bool(true),
            ),
            (
                intrinsic(
                    IntrinsicOp::TextStartsWith,
                    vec![lit_text("strawberry"), lit_text("straw")],
                ),
                Value::Bool(true),
            ),
            (
                intrinsic(
                    IntrinsicOp::TextEndsWith,
                    vec![lit_text("strawberry"), lit_text("berry")],
                ),
                Value::Bool(true),
            ),
            (
                intrinsic(
                    IntrinsicOp::TextReplace,
                    vec![lit_text("strawberry"), lit_text("r"), lit_text("R")],
                ),
                Value::Text("stRawbeRRy".into()),
            ),
        ];

        for (expression, expected) in cases {
            assert_eq!(
                Evaluator::new().eval(&expression, &mut Env::new()).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn collection_intrinsics_preserve_value_and_map_semantics() {
        let list = Expr::ListExpr(vec![lit_int(1), lit_int(2), lit_int(2)]);
        let map = Expr::Literal(Value::Map(std::collections::BTreeMap::from([
            ("a".into(), Value::Int(1)),
            ("b".into(), Value::Int(2)),
        ])));
        let cases = [
            (
                intrinsic(IntrinsicOp::Length, vec![list.clone()]),
                Value::Int(3),
            ),
            (
                intrinsic(
                    IntrinsicOp::CollectionContains,
                    vec![list.clone(), lit_int(2)],
                ),
                Value::Bool(true),
            ),
            (
                intrinsic(IntrinsicOp::CountEqual, vec![list, lit_int(2)]),
                Value::Int(2),
            ),
            (
                intrinsic(
                    IntrinsicOp::CollectionContains,
                    vec![map.clone(), lit_text("b")],
                ),
                Value::Bool(true),
            ),
            (
                intrinsic(IntrinsicOp::MapKeys, vec![map.clone()]),
                Value::List(vec![Value::Text("a".into()), Value::Text("b".into())]),
            ),
            (
                intrinsic(IntrinsicOp::MapValues, vec![map]),
                Value::List(vec![Value::Int(1), Value::Int(2)]),
            ),
        ];

        for (expression, expected) in cases {
            assert_eq!(
                Evaluator::new().eval(&expression, &mut Env::new()).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn collection_find_index_returns_first_structural_match() {
        let sought = Value::Map(std::collections::BTreeMap::from([
            ("kind".into(), Value::Text("answer".into())),
            ("value".into(), Value::Int(42)),
        ]));
        let collection = Expr::ListExpr(vec![
            Expr::Literal(Value::Map(std::collections::BTreeMap::from([(
                "kind".into(),
                Value::Text("question".into()),
            )]))),
            Expr::Literal(sought.clone()),
            Expr::Literal(sought.clone()),
        ]);
        let expression = intrinsic(
            IntrinsicOp::CollectionFindIndex,
            vec![collection, Expr::Literal(sought)],
        );

        assert_eq!(
            Evaluator::new().eval(&expression, &mut Env::new()).unwrap(),
            Value::Int(1)
        );
    }

    #[test]
    fn collection_find_index_returns_minus_one_when_absent() {
        let expression = intrinsic(
            IntrinsicOp::CollectionFindIndex,
            vec![
                Expr::ListExpr(vec![lit_int(1), lit_int(2), lit_int(3)]),
                lit_int(9),
            ],
        );

        assert_eq!(
            Evaluator::new().eval(&expression, &mut Env::new()).unwrap(),
            Value::Int(-1)
        );
    }

    #[test]
    fn collection_find_index_matches_null_maps_and_nested_lists_structurally() {
        let nested = Value::List(vec![
            Value::Null,
            Value::Map(std::collections::BTreeMap::from([(
                "ok".into(),
                Value::Bool(true),
            )])),
        ]);
        let expression = intrinsic(
            IntrinsicOp::CollectionFindIndex,
            vec![
                Expr::ListExpr(vec![
                    Expr::Literal(Value::Null),
                    Expr::Literal(Value::List(vec![Value::Null])),
                    Expr::Literal(nested.clone()),
                ]),
                Expr::Literal(nested),
            ],
        );

        assert_eq!(
            Evaluator::new().eval(&expression, &mut Env::new()).unwrap(),
            Value::Int(2)
        );
    }

    #[test]
    fn collection_find_index_rejects_wrong_types_and_arities() {
        let wrong_type = intrinsic(
            IntrinsicOp::CollectionFindIndex,
            vec![lit_text("not a list"), lit_int(1)],
        );
        assert!(matches!(
            Evaluator::new().eval(&wrong_type, &mut Env::new()),
            Err(SpoonError::TypeError { .. })
        ));

        for args in [
            vec![lit_int(1)],
            vec![],
            vec![lit_int(1), lit_int(2), lit_int(3)],
        ] {
            let expression = intrinsic(IntrinsicOp::CollectionFindIndex, args);
            assert!(matches!(
                Evaluator::new().eval(&expression, &mut Env::new()),
                Err(SpoonError::ArityMismatch { .. })
            ));
        }
    }

    #[test]
    fn collection_find_index_charges_linear_work_against_budget() {
        let expression = intrinsic(
            IntrinsicOp::CollectionFindIndex,
            vec![
                Expr::ListExpr(vec![lit_int(1), lit_int(2), lit_int(3), lit_int(4)]),
                lit_int(9),
            ],
        );

        assert!(matches!(
            Evaluator::new()
                .with_budget(4)
                .eval(&expression, &mut Env::new()),
            Err(SpoonError::BudgetExceeded)
        ));
    }

    #[test]
    fn map_from_entries_accepts_empty_input_and_builds_a_deterministic_map() {
        let entry =
            |key: &str, value: Value| Value::List(vec![Value::Text(key.to_string()), value]);
        let entries = Value::List(vec![
            entry("z", Value::Int(26)),
            entry("a", Value::List(vec![Value::Bool(true), Value::Null])),
        ]);

        let empty = intrinsic(
            IntrinsicOp::MapFromEntries,
            vec![Expr::Literal(Value::List(Vec::new()))],
        );
        let populated = intrinsic(IntrinsicOp::MapFromEntries, vec![Expr::Literal(entries)]);

        assert_eq!(
            Evaluator::new().eval(&empty, &mut Env::new()).unwrap(),
            Value::Map(BTreeMap::new())
        );
        assert_eq!(
            Evaluator::new().eval(&populated, &mut Env::new()).unwrap(),
            Value::Map(BTreeMap::from([
                (
                    "a".into(),
                    Value::List(vec![Value::Bool(true), Value::Null])
                ),
                ("z".into(), Value::Int(26)),
            ]))
        );
    }

    #[test]
    fn map_from_entries_uses_last_value_for_duplicate_keys() {
        let entries = Value::List(vec![
            Value::List(vec![Value::Text("answer".into()), Value::Int(7)]),
            Value::List(vec![Value::Text("answer".into()), Value::Int(42)]),
            Value::List(vec![Value::Text("other".into()), Value::Bool(true)]),
        ]);
        let expression = intrinsic(IntrinsicOp::MapFromEntries, vec![Expr::Literal(entries)]);

        assert_eq!(
            Evaluator::new().eval(&expression, &mut Env::new()).unwrap(),
            Value::Map(BTreeMap::from([
                ("answer".into(), Value::Int(42)),
                ("other".into(), Value::Bool(true)),
            ]))
        );
    }

    #[test]
    fn map_from_entries_rejects_malformed_entries_types_and_arities() {
        let wrong_collection = intrinsic(IntrinsicOp::MapFromEntries, vec![lit_text("not a list")]);
        assert!(matches!(
            Evaluator::new().eval(&wrong_collection, &mut Env::new()),
            Err(SpoonError::TypeError { .. })
        ));

        let wrong_entry_type = intrinsic(
            IntrinsicOp::MapFromEntries,
            vec![Expr::Literal(Value::List(vec![Value::Int(1)]))],
        );
        assert!(matches!(
            Evaluator::new().eval(&wrong_entry_type, &mut Env::new()),
            Err(SpoonError::TypeError { .. })
        ));

        for pair in [
            Value::List(vec![Value::Text("key".into())]),
            Value::List(vec![
                Value::Text("key".into()),
                Value::Int(1),
                Value::Int(2),
            ]),
        ] {
            let malformed_pair = intrinsic(
                IntrinsicOp::MapFromEntries,
                vec![Expr::Literal(Value::List(vec![pair]))],
            );
            assert!(matches!(
                Evaluator::new().eval(&malformed_pair, &mut Env::new()),
                Err(SpoonError::ArityMismatch { .. })
            ));
        }

        let wrong_key_type = intrinsic(
            IntrinsicOp::MapFromEntries,
            vec![Expr::Literal(Value::List(vec![Value::List(vec![
                Value::Int(1),
                Value::Bool(true),
            ])]))],
        );
        assert!(matches!(
            Evaluator::new().eval(&wrong_key_type, &mut Env::new()),
            Err(SpoonError::TypeError { .. })
        ));

        for args in [
            Vec::new(),
            vec![Expr::Literal(Value::List(Vec::new())), lit_int(1)],
        ] {
            let wrong_arity = intrinsic(IntrinsicOp::MapFromEntries, args);
            assert!(matches!(
                Evaluator::new().eval(&wrong_arity, &mut Env::new()),
                Err(SpoonError::ArityMismatch { .. })
            ));
        }
    }

    #[test]
    fn map_from_entries_charges_work_and_enforces_output_item_limits() {
        let entries = Value::List(
            (0..=100_000)
                .map(|index| {
                    Value::List(vec![Value::Text(format!("key-{index}")), Value::Int(index)])
                })
                .collect(),
        );
        let expression = intrinsic(IntrinsicOp::MapFromEntries, vec![Expr::Literal(entries)]);

        assert!(matches!(
            Evaluator::new().eval(&expression, &mut Env::new()),
            Err(SpoonError::IntrinsicLimitExceeded { .. })
        ));

        let small = Value::List(vec![
            Value::List(vec![Value::Text("a".into()), Value::Int(1)]),
            Value::List(vec![Value::Text("b".into()), Value::Int(2)]),
            Value::List(vec![Value::Text("c".into()), Value::Int(3)]),
        ]);
        let expression = intrinsic(IntrinsicOp::MapFromEntries, vec![Expr::Literal(small)]);
        assert!(matches!(
            Evaluator::new()
                .with_budget(2)
                .eval(&expression, &mut Env::new()),
            Err(SpoonError::BudgetExceeded)
        ));
    }

    #[test]
    fn optional_path_does_not_hide_type_errors() {
        let expression = intrinsic(
            IntrinsicOp::PathGetOptional,
            vec![lit_int(4), lit_text("missing")],
        );
        assert!(matches!(
            Evaluator::new().eval(&expression, &mut Env::new()),
            Err(SpoonError::TypeError { .. })
        ));
    }

    #[test]
    fn json_pointer_get_supports_root_arrays_and_rfc6901_escaped_keys() {
        let document = Value::Map(std::collections::BTreeMap::from([
            (
                "a/b".into(),
                Value::Map(std::collections::BTreeMap::from([(
                    "m~n".into(),
                    Value::Text("escaped".into()),
                )])),
            ),
            (
                "items".into(),
                Value::List(vec![
                    Value::Map(std::collections::BTreeMap::from([(
                        "name".into(),
                        Value::Text("first".into()),
                    )])),
                    Value::Map(std::collections::BTreeMap::from([(
                        "name".into(),
                        Value::Text("second".into()),
                    )])),
                ]),
            ),
        ]));

        let cases = [
            ("", document.clone()),
            ("/a~1b/m~0n", Value::Text("escaped".into())),
            ("/items/0/name", Value::Text("first".into())),
            ("/items/1/name", Value::Text("second".into())),
        ];

        for (pointer, expected) in cases {
            let expression = intrinsic(
                IntrinsicOp::JsonPointerGet,
                vec![Expr::Literal(document.clone()), lit_text(pointer)],
            );
            let result = Evaluator::new().eval(&expression, &mut Env::new()).unwrap();
            assert_eq!(result, expected, "pointer {pointer:?}");
        }
    }

    #[test]
    fn json_pointer_optional_distinguishes_present_null_from_missing() {
        let document = Value::Map(std::collections::BTreeMap::from([(
            "present".into(),
            Value::Null,
        )]));
        let strict_present = intrinsic(
            IntrinsicOp::JsonPointerGet,
            vec![Expr::Literal(document.clone()), lit_text("/present")],
        );
        let optional_present = intrinsic(
            IntrinsicOp::JsonPointerGetOptional,
            vec![Expr::Literal(document.clone()), lit_text("/present")],
        );
        let optional_missing = intrinsic(
            IntrinsicOp::JsonPointerGetOptional,
            vec![Expr::Literal(document.clone()), lit_text("/missing")],
        );
        let strict_missing = intrinsic(
            IntrinsicOp::JsonPointerGet,
            vec![Expr::Literal(document), lit_text("/missing")],
        );

        for expression in [strict_present, optional_present, optional_missing] {
            assert_eq!(
                Evaluator::new().eval(&expression, &mut Env::new()).unwrap(),
                Value::Null
            );
        }
        assert!(matches!(
            Evaluator::new().eval(&strict_missing, &mut Env::new()),
            Err(SpoonError::FieldNotFound(_))
        ));
    }

    #[test]
    fn json_pointer_rejects_malformed_pointers_and_type_mismatches() {
        let document = Value::Map(std::collections::BTreeMap::from([
            ("items".into(), Value::List(vec![Value::Int(1)])),
            ("scalar".into(), Value::Int(7)),
        ]));
        let malformed = ["items", "/bad~2", "/trailing~"];
        for pointer in malformed {
            let expression = intrinsic(
                IntrinsicOp::JsonPointerGetOptional,
                vec![Expr::Literal(document.clone()), lit_text(pointer)],
            );
            assert!(
                matches!(
                    Evaluator::new().eval(&expression, &mut Env::new()),
                    Err(SpoonError::InvalidPath { .. })
                ),
                "pointer {pointer:?}"
            );
        }

        let list_with_non_numeric_index = intrinsic(
            IntrinsicOp::JsonPointerGetOptional,
            vec![Expr::Literal(document.clone()), lit_text("/items/name")],
        );
        let scalar_as_container = intrinsic(
            IntrinsicOp::JsonPointerGetOptional,
            vec![Expr::Literal(document), lit_text("/scalar/value")],
        );
        assert!(matches!(
            Evaluator::new().eval(&list_with_non_numeric_index, &mut Env::new()),
            Err(SpoonError::TypeError { .. })
        ));
        assert!(matches!(
            Evaluator::new().eval(&scalar_as_container, &mut Env::new()),
            Err(SpoonError::TypeError { .. })
        ));
    }

    #[test]
    fn json_pointer_set_replaces_root_nested_values_and_escaped_keys_immutably() {
        let document = Value::Map(std::collections::BTreeMap::from([
            (
                "a/b".into(),
                Value::Map(std::collections::BTreeMap::from([(
                    "m~n".into(),
                    Value::Text("before".into()),
                )])),
            ),
            (
                "items".into(),
                Value::List(vec![Value::Int(1), Value::Int(2)]),
            ),
        ]));
        let original = document.clone();

        let nested = intrinsic(
            IntrinsicOp::JsonPointerSet,
            vec![
                Expr::Literal(document.clone()),
                lit_text("/a~1b/m~0n"),
                lit_text("after"),
            ],
        );
        let nested_result = Evaluator::new().eval(&nested, &mut Env::new()).unwrap();
        assert_eq!(
            nested_result,
            Value::Map(std::collections::BTreeMap::from([
                (
                    "a/b".into(),
                    Value::Map(std::collections::BTreeMap::from([(
                        "m~n".into(),
                        Value::Text("after".into()),
                    )])),
                ),
                (
                    "items".into(),
                    Value::List(vec![Value::Int(1), Value::Int(2)])
                ),
            ]))
        );
        assert_eq!(document, original, "set must not mutate its input value");

        let root = intrinsic(
            IntrinsicOp::JsonPointerSet,
            vec![Expr::Literal(document), lit_text(""), lit_int(42)],
        );
        assert_eq!(
            Evaluator::new().eval(&root, &mut Env::new()).unwrap(),
            Value::Int(42)
        );
    }

    #[test]
    fn json_pointer_set_replaces_array_elements() {
        let document = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let expression = intrinsic(
            IntrinsicOp::JsonPointerSet,
            vec![Expr::Literal(document.clone()), lit_text("/1"), lit_int(20)],
        );

        assert_eq!(
            Evaluator::new().eval(&expression, &mut Env::new()).unwrap(),
            Value::List(vec![Value::Int(1), Value::Int(20), Value::Int(3)])
        );
        assert_eq!(
            document,
            Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
        );
    }

    #[test]
    fn json_pointer_delete_removes_root_map_fields_and_array_elements_immutably() {
        let document = Value::Map(std::collections::BTreeMap::from([
            ("keep".into(), Value::Bool(true)),
            ("remove".into(), Value::Int(7)),
            (
                "items".into(),
                Value::List(vec![Value::Text("a".into()), Value::Text("b".into())]),
            ),
        ]));
        let original = document.clone();

        let remove_field = intrinsic(
            IntrinsicOp::JsonPointerDelete,
            vec![Expr::Literal(document.clone()), lit_text("/remove")],
        );
        assert_eq!(
            Evaluator::new()
                .eval(&remove_field, &mut Env::new())
                .unwrap(),
            Value::Map(std::collections::BTreeMap::from([
                ("keep".into(), Value::Bool(true)),
                (
                    "items".into(),
                    Value::List(vec![Value::Text("a".into()), Value::Text("b".into())]),
                ),
            ]))
        );
        assert_eq!(document, original, "delete must not mutate its input value");

        let remove_item = intrinsic(
            IntrinsicOp::JsonPointerDelete,
            vec![Expr::Literal(document.clone()), lit_text("/items/0")],
        );
        assert_eq!(
            Evaluator::new()
                .eval(&remove_item, &mut Env::new())
                .unwrap(),
            Value::Map(std::collections::BTreeMap::from([
                ("keep".into(), Value::Bool(true)),
                ("remove".into(), Value::Int(7)),
                ("items".into(), Value::List(vec![Value::Text("b".into())]),),
            ]))
        );

        let delete_root = intrinsic(
            IntrinsicOp::JsonPointerDelete,
            vec![Expr::Literal(document), lit_text("")],
        );
        assert_eq!(
            Evaluator::new()
                .eval(&delete_root, &mut Env::new())
                .unwrap(),
            Value::Null
        );
    }

    #[test]
    fn json_pointer_update_rejects_missing_type_and_malformed_targets() {
        let document = Value::Map(std::collections::BTreeMap::from([
            ("items".into(), Value::List(vec![Value::Int(1)])),
            ("scalar".into(), Value::Int(7)),
        ]));

        let new_set = intrinsic(
            IntrinsicOp::JsonPointerSet,
            vec![
                Expr::Literal(document.clone()),
                lit_text("/missing"),
                lit_int(1),
            ],
        );
        let missing_delete = intrinsic(
            IntrinsicOp::JsonPointerDelete,
            vec![Expr::Literal(document.clone()), lit_text("/missing")],
        );
        let wrong_type_set = intrinsic(
            IntrinsicOp::JsonPointerSet,
            vec![
                Expr::Literal(document.clone()),
                lit_text("/items/name"),
                lit_int(1),
            ],
        );
        let wrong_type_delete = intrinsic(
            IntrinsicOp::JsonPointerDelete,
            vec![Expr::Literal(document.clone()), lit_text("/scalar/value")],
        );
        let malformed = intrinsic(
            IntrinsicOp::JsonPointerSet,
            vec![Expr::Literal(document), lit_text("/bad~2"), lit_int(1)],
        );

        assert_eq!(
            Evaluator::new().eval(&new_set, &mut Env::new()).unwrap(),
            Value::Map(std::collections::BTreeMap::from([
                ("items".into(), Value::List(vec![Value::Int(1)])),
                ("missing".into(), Value::Int(1)),
                ("scalar".into(), Value::Int(7)),
            ]))
        );
        assert!(matches!(
            Evaluator::new().eval(&missing_delete, &mut Env::new()),
            Err(SpoonError::FieldNotFound(_))
        ));
        assert!(matches!(
            Evaluator::new().eval(&wrong_type_set, &mut Env::new()),
            Err(SpoonError::TypeError { .. })
        ));
        assert!(matches!(
            Evaluator::new().eval(&wrong_type_delete, &mut Env::new()),
            Err(SpoonError::TypeError { .. })
        ));
        assert!(matches!(
            Evaluator::new().eval(&malformed, &mut Env::new()),
            Err(SpoonError::InvalidPath { .. })
        ));
    }

    #[test]
    fn json_pointer_update_enforces_intrinsic_arities() {
        for args in [
            vec![lit_int(1), lit_text("/value")],
            vec![lit_int(1), lit_text("/value"), lit_int(2), lit_int(3)],
        ] {
            let expression = intrinsic(IntrinsicOp::JsonPointerSet, args);
            assert!(matches!(
                Evaluator::new().eval(&expression, &mut Env::new()),
                Err(SpoonError::ArityMismatch { .. })
            ));
        }

        for args in [
            vec![lit_int(1)],
            vec![lit_int(1), lit_text("/value"), lit_int(2)],
        ] {
            let expression = intrinsic(IntrinsicOp::JsonPointerDelete, args);
            assert!(matches!(
                Evaluator::new().eval(&expression, &mut Env::new()),
                Err(SpoonError::ArityMismatch { .. })
            ));
        }
    }

    #[test]
    fn coalesce_returns_first_non_null_without_treating_values_as_falsey() {
        let cases = [
            (Value::Bool(false), Value::Bool(false)),
            (Value::Int(0), Value::Int(0)),
            (Value::Text(String::new()), Value::Text(String::new())),
            (Value::List(Vec::new()), Value::List(Vec::new())),
            (
                Value::Map(std::collections::BTreeMap::new()),
                Value::Map(std::collections::BTreeMap::new()),
            ),
        ];

        for (first, expected) in cases {
            let expression = intrinsic(
                IntrinsicOp::Coalesce,
                vec![Expr::Literal(Value::Null), Expr::Literal(first)],
            );
            let result = Evaluator::new().eval(&expression, &mut Env::new()).unwrap();
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn coalesce_skips_nulls_and_returns_null_when_all_values_are_null() {
        let expression = intrinsic(
            IntrinsicOp::Coalesce,
            vec![Expr::Literal(Value::Null), lit_text("fallback")],
        );
        assert_eq!(
            Evaluator::new().eval(&expression, &mut Env::new()).unwrap(),
            Value::Text("fallback".into())
        );

        let all_null = intrinsic(
            IntrinsicOp::Coalesce,
            vec![Expr::Literal(Value::Null), Expr::Literal(Value::Null)],
        );
        assert_eq!(
            Evaluator::new().eval(&all_null, &mut Env::new()).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn coalesce_rejects_empty_argument_lists() {
        let expression = intrinsic(IntrinsicOp::Coalesce, vec![]);
        assert!(matches!(
            Evaluator::new().eval(&expression, &mut Env::new()),
            Err(SpoonError::ArityMismatch { .. })
        ));
    }

    #[test]
    fn arithmetic_add() {
        let mut ev = Evaluator::new();
        let mut env = Env::new();
        let expr = binop(BinOp::Add, lit_int(2), lit_int(3));
        assert_eq!(ev.eval(&expr, &mut env).unwrap(), Value::Int(5));
    }

    #[test]
    fn arithmetic_mul() {
        let mut ev = Evaluator::new();
        let mut env = Env::new();
        let expr = binop(BinOp::Mul, lit_int(7), lit_int(2));
        assert_eq!(ev.eval(&expr, &mut env).unwrap(), Value::Int(14));
    }

    #[test]
    fn arithmetic_float_promotion() {
        let mut ev = Evaluator::new();
        let mut env = Env::new();
        let expr = binop(
            BinOp::Add,
            Expr::Literal(Value::Int(2)),
            Expr::Literal(Value::Float(0.5)),
        );
        assert_eq!(ev.eval(&expr, &mut env).unwrap(), Value::Float(2.5));
    }

    #[test]
    fn variables_let_binding() {
        let mut ev = Evaluator::new();
        let mut env = Env::new();
        // let x = 5 in x + 1
        let expr = Expr::Let {
            name: "x".to_string(),
            value: Box::new(lit_int(5)),
            body: Box::new(binop(BinOp::Add, Expr::Var("x".to_string()), lit_int(1))),
        };
        assert_eq!(ev.eval(&expr, &mut env).unwrap(), Value::Int(6));
    }

    #[test]
    fn undefined_variable_errors() {
        let mut ev = Evaluator::new();
        let mut env = Env::new();
        let expr = Expr::Var("nope".to_string());
        let err = ev.eval(&expr, &mut env).unwrap_err();
        assert!(matches!(err, SpoonError::UndefinedVar(name) if name == "nope"));
    }

    fn double_procedure() -> Procedure {
        // DOUBLE(x) = x * 2
        Procedure::new(
            "DOUBLE",
            vec![Param::named("x")],
            binop(BinOp::Mul, Expr::Var("x".to_string()), lit_int(2)),
        )
    }

    fn checked_double_procedure() -> Procedure {
        let mut contract = Contract::default();
        contract
            .requires
            .push(Condition::described("x is positive").with_check(binop(
                BinOp::Gt,
                Expr::Var("x".to_string()),
                lit_int(0),
            )));
        contract
            .requires
            .push(Condition::described("caller accepts integer output"));
        contract
            .fails_when
            .push(Condition::described("x is thirteen").with_check(binop(
                BinOp::Eq,
                Expr::Var("x".to_string()),
                lit_int(13),
            )));
        contract
            .promises
            .push(Condition::described("result is double x").with_check(binop(
                BinOp::Eq,
                Expr::Var("result".to_string()),
                binop(BinOp::Mul, Expr::Var("x".to_string()), lit_int(2)),
            )));

        double_procedure().with_contract(contract)
    }

    #[test]
    fn procedure_call() {
        let mut ev = Evaluator::new();
        let double = double_procedure();
        let id = double.id;
        ev.register_procedure(double);

        let result = ev.exec_procedure(&id, vec![Value::Int(7)]).unwrap();
        assert_eq!(result.value, Value::Int(14));
    }

    #[test]
    fn contract_checks_are_enforced_and_recorded_in_the_trace() {
        let mut ev = Evaluator::new();
        let checked_double = checked_double_procedure();
        let id = checked_double.id;
        ev.register_procedure(checked_double);

        let result = ev.exec_procedure(&id, vec![Value::Int(4)]).unwrap();
        let checks = &result.trace.steps[0].contract_checks;

        assert_eq!(result.value, Value::Int(8));
        assert_eq!(checks.requires.len(), 2);
        assert_eq!(checks.requires[0].status, ConditionCheckStatus::Passed);
        assert_eq!(
            checks.requires[1].status,
            ConditionCheckStatus::NotExecutable
        );
        assert_eq!(checks.fails_when[0].status, ConditionCheckStatus::Passed);
        assert_eq!(checks.promises[0].status, ConditionCheckStatus::Passed);
    }

    #[test]
    fn failed_requirement_stops_execution_before_the_body() {
        let mut ev = Evaluator::new();
        let mut contract = Contract::default();
        contract
            .requires
            .push(Condition::described("x must be positive").with_check(binop(
                BinOp::Gt,
                Expr::Var("x".to_string()),
                lit_int(0),
            )));
        let procedure = Procedure::new(
            "GUARDED",
            vec![Param::named("x")],
            binop(BinOp::Div, lit_int(1), lit_int(0)),
        )
        .with_contract(contract);
        let id = procedure.id;
        ev.register_procedure(procedure);

        let error = ev.exec_procedure(&id, vec![Value::Int(-1)]).unwrap_err();

        assert!(matches!(
            error,
            SpoonError::ContractViolation(message) if message.contains("requires") && message.contains("x must be positive")
        ));
    }

    #[test]
    fn captured_requirement_failure_retains_the_failed_call_and_check() {
        let mut ev = Evaluator::new();
        let mut procedure = checked_double_procedure();
        procedure.version = 7;
        let id = procedure.id;
        ev.register_procedure(procedure);

        let attempt = ev.exec_procedure_captured(&id, vec![Value::Int(-1)]);

        assert!(matches!(
            attempt.result,
            Err(SpoonError::ContractViolation(ref message))
                if message.contains("requires") && message.contains("x is positive")
        ));
        assert_eq!(attempt.trace.len(), 1);
        let step = &attempt.trace.steps[0];
        assert_eq!(step.procedure_called, Some(id));
        assert_eq!(step.procedure_version, Some(7));
        assert_eq!(
            step.contract_checks.requires[0].status,
            ConditionCheckStatus::Violated
        );
        assert!(matches!(
            &step.status,
            ExecStepStatus::Failed { error } if error.contains("requires")
        ));
    }

    #[test]
    fn triggered_fails_when_stops_execution_before_the_body() {
        let mut ev = Evaluator::new();
        let checked_double = checked_double_procedure();
        let id = checked_double.id;
        ev.register_procedure(checked_double);

        let error = ev.exec_procedure(&id, vec![Value::Int(13)]).unwrap_err();

        assert!(matches!(
            error,
            SpoonError::ContractViolation(message) if message.contains("fails_when") && message.contains("x is thirteen")
        ));
    }

    #[test]
    fn captured_fails_when_failure_retains_the_failed_call_and_check() {
        let mut ev = Evaluator::new();
        let checked_double = checked_double_procedure();
        let id = checked_double.id;
        ev.register_procedure(checked_double);

        let attempt = ev.exec_procedure_captured(&id, vec![Value::Int(13)]);

        assert!(attempt.result.is_err());
        let step = attempt.trace.steps.last().unwrap();
        assert_eq!(step.procedure_called, Some(id));
        assert_eq!(step.procedure_version, Some(1));
        assert_eq!(
            step.contract_checks.fails_when[0].status,
            ConditionCheckStatus::Violated
        );
        assert!(matches!(step.status, ExecStepStatus::Failed { .. }));
    }

    #[test]
    fn failed_promise_is_reported_after_the_body() {
        let mut ev = Evaluator::new();
        let mut contract = Contract::default();
        contract
            .promises
            .push(Condition::described("result is ten").with_check(binop(
                BinOp::Eq,
                Expr::Var("result".to_string()),
                lit_int(10),
            )));
        let procedure = double_procedure().with_contract(contract);
        let id = procedure.id;
        ev.register_procedure(procedure);

        let error = ev.exec_procedure(&id, vec![Value::Int(3)]).unwrap_err();

        assert!(matches!(
            error,
            SpoonError::ContractViolation(message) if message.contains("promises") && message.contains("result is ten")
        ));
    }

    #[test]
    fn captured_promise_failure_retains_the_candidate_output() {
        let mut ev = Evaluator::new();
        let mut contract = Contract::default();
        contract
            .promises
            .push(Condition::described("result is ten").with_check(binop(
                BinOp::Eq,
                Expr::Var("result".to_string()),
                lit_int(10),
            )));
        let procedure = double_procedure().with_contract(contract);
        let id = procedure.id;
        ev.register_procedure(procedure);

        let attempt = ev.exec_procedure_captured(&id, vec![Value::Int(3)]);

        assert!(attempt.result.is_err());
        let step = attempt.trace.steps.last().unwrap();
        assert_eq!(step.output, Value::Int(6));
        assert_eq!(
            step.contract_checks.promises[0].status,
            ConditionCheckStatus::Violated
        );
        assert!(matches!(step.status, ExecStepStatus::Failed { .. }));
    }

    #[test]
    fn captured_body_error_records_failed_nested_and_parent_calls() {
        let mut ev = Evaluator::new();
        let failing = Procedure::new("FAIL", vec![], binop(BinOp::Div, lit_int(1), lit_int(0)));
        let failing_id = failing.id;
        ev.register_procedure(failing);
        let parent = Procedure::new(
            "PARENT",
            vec![],
            Expr::Call {
                procedure: failing_id,
                args: vec![],
            },
        );
        let parent_id = parent.id;
        ev.register_procedure(parent);

        let attempt = ev.exec_procedure_captured(&parent_id, vec![]);

        assert!(matches!(attempt.result, Err(SpoonError::DivisionByZero)));
        assert_eq!(attempt.trace.len(), 2);
        assert_eq!(attempt.trace.steps[0].procedure_called, Some(failing_id));
        assert_eq!(attempt.trace.steps[0].procedure_version, Some(1));
        assert!(matches!(
            attempt.trace.steps[0].status,
            ExecStepStatus::Failed { .. }
        ));
        assert_eq!(attempt.trace.steps[1].procedure_called, Some(parent_id));
        assert_eq!(attempt.trace.steps[1].procedure_version, Some(1));
        assert!(matches!(
            attempt.trace.steps[1].status,
            ExecStepStatus::Failed { .. }
        ));
    }

    #[test]
    fn explicit_step_status_distinguishes_successful_null_from_failure() {
        let mut ev = Evaluator::new();
        let succeeds = Procedure::new("NULL", vec![], Expr::Literal(Value::Null));
        let succeeds_id = succeeds.id;
        ev.register_procedure(succeeds);
        let fails = Procedure::new("FAIL", vec![], binop(BinOp::Div, lit_int(1), lit_int(0)));
        let fails_id = fails.id;
        ev.register_procedure(fails);

        let success = ev.exec_procedure_captured(&succeeds_id, vec![]);
        let failure = ev.exec_procedure_captured(&fails_id, vec![]);

        assert_eq!(success.trace.steps[0].output, Value::Null);
        assert_eq!(success.trace.steps[0].status, ExecStepStatus::Succeeded);
        assert_eq!(failure.trace.steps[0].output, Value::Null);
        assert!(matches!(
            failure.trace.steps[0].status,
            ExecStepStatus::Failed { .. }
        ));
    }

    #[test]
    fn trace_pins_the_exact_procedure_version() {
        let mut ev = Evaluator::new();
        let mut double = double_procedure();
        double.version = 7;
        let id = double.id;
        ev.register_procedure(double);

        let result = ev.exec_procedure(&id, vec![Value::Int(3)]).unwrap();

        assert_eq!(result.trace.steps[0].procedure_called, Some(id));
        assert_eq!(result.trace.steps[0].procedure_version, Some(7));
    }

    #[test]
    fn replay_replaces_top_level_arguments_deterministically() {
        let mut ev = Evaluator::new().with_budget(4);
        let double = double_procedure();
        let id = double.id;
        ev.register_procedure(double);
        let original = ev.exec_procedure(&id, vec![Value::Int(3)]).unwrap();

        let replayed = ev.replay(&original.trace, vec![Value::Int(9)]).unwrap();

        assert_eq!(replayed.value, Value::Int(18));
        assert_eq!(
            replayed.trace.steps.last().unwrap().input,
            Some(Value::List(vec![Value::Int(9)]))
        );
        assert_eq!(
            replayed.trace.steps.last().unwrap().procedure_version,
            Some(1)
        );
    }

    #[test]
    fn replay_rejects_a_trace_when_any_procedure_version_changed() {
        let mut ev = Evaluator::new();
        let double = double_procedure();
        let id = double.id;
        ev.register_procedure(double.clone());
        let original = ev.exec_procedure(&id, vec![Value::Int(3)]).unwrap();

        let mut revised = double;
        revised.version = 2;
        ev.register_procedure(revised);

        let error = ev.replay(&original.trace, vec![Value::Int(9)]).unwrap_err();

        assert!(matches!(
            error,
            SpoonError::Other(message) if message.contains("version") && message.contains("replay")
        ));
    }

    #[test]
    fn replay_rejects_a_changed_nested_procedure_before_execution() {
        let mut ev = Evaluator::new();
        let double = double_procedure();
        let double_id = double.id;
        ev.register_procedure(double.clone());

        let quadruple = Procedure::new(
            "QUADRUPLE",
            vec![Param::named("x")],
            Expr::Call {
                procedure: double_id,
                args: vec![Expr::Call {
                    procedure: double_id,
                    args: vec![Expr::Var("x".to_string())],
                }],
            },
        );
        let quadruple_id = quadruple.id;
        ev.register_procedure(quadruple);
        let original = ev
            .exec_procedure(&quadruple_id, vec![Value::Int(3)])
            .unwrap();

        let mut revised_double = double;
        revised_double.version = 2;
        ev.register_procedure(revised_double);

        let error = ev.replay(&original.trace, vec![Value::Int(4)]).unwrap_err();

        assert!(matches!(
            error,
            SpoonError::Other(message)
                if message.contains(&double_id.to_string()) && message.contains("version")
        ));
    }

    #[test]
    fn nested_procedure_calls() {
        let mut ev = Evaluator::new();
        let double = double_procedure();
        let double_id = double.id;
        ev.register_procedure(double);

        // QUADRUPLE(x) = DOUBLE(DOUBLE(x))
        let quadruple = Procedure::new(
            "QUADRUPLE",
            vec![Param::named("x")],
            Expr::Call {
                procedure: double_id,
                args: vec![Expr::Call {
                    procedure: double_id,
                    args: vec![Expr::Var("x".to_string())],
                }],
            },
        );
        let quadruple_id = quadruple.id;
        ev.register_procedure(quadruple);

        let result = ev
            .exec_procedure(&quadruple_id, vec![Value::Int(3)])
            .unwrap();
        assert_eq!(result.value, Value::Int(12));
    }

    #[test]
    fn exact_procedure_call_rejects_a_different_registered_revision() {
        let mut evaluator = Evaluator::new();
        let double = double_procedure();
        let double_id = double.id;
        evaluator.register_procedure(double.clone());
        let caller = Procedure::new(
            "PINNED DOUBLE",
            vec![Param::named("x")],
            Expr::CallExact {
                procedure: double_id,
                version: 1,
                args: vec![Expr::Var("x".into())],
            },
        );
        let caller_id = caller.id;
        evaluator.register_procedure(caller);
        assert_eq!(
            evaluator
                .exec_procedure(&caller_id, vec![Value::Int(4)])
                .unwrap()
                .value,
            Value::Int(8)
        );

        let mut revised = double;
        revised.version = 2;
        evaluator.register_procedure(revised);
        let error = evaluator
            .exec_procedure(&caller_id, vec![Value::Int(4)])
            .unwrap_err();
        assert!(matches!(
            error,
            SpoonError::Other(message) if message.contains("exact call") && message.contains("version 1")
        ));
    }

    #[test]
    fn conditionals() {
        let mut ev = Evaluator::new();
        let mut env = Env::new();
        let expr = Expr::If {
            cond: Box::new(Expr::Literal(Value::Bool(true))),
            then: Box::new(lit_int(1)),
            else_: Box::new(lit_int(2)),
        };
        assert_eq!(ev.eval(&expr, &mut env).unwrap(), Value::Int(1));
    }

    #[test]
    fn list_map() {
        let mut ev = Evaluator::new();
        let mut env = Env::new();
        // map [1,2,3] with x -> x * 2
        let expr = Expr::Map {
            collection: Box::new(Expr::ListExpr(vec![lit_int(1), lit_int(2), lit_int(3)])),
            var: "x".to_string(),
            body: Box::new(binop(BinOp::Mul, Expr::Var("x".to_string()), lit_int(2))),
        };
        assert_eq!(
            ev.eval(&expr, &mut env).unwrap(),
            Value::List(vec![Value::Int(2), Value::Int(4), Value::Int(6)])
        );
    }

    #[test]
    fn list_filter() {
        let mut ev = Evaluator::new();
        let mut env = Env::new();
        // filter [1,2,3,4] where x % 2 == 0
        let expr = Expr::Filter {
            collection: Box::new(Expr::ListExpr(vec![
                lit_int(1),
                lit_int(2),
                lit_int(3),
                lit_int(4),
            ])),
            var: "x".to_string(),
            predicate: Box::new(binop(
                BinOp::Eq,
                binop(BinOp::Mod, Expr::Var("x".to_string()), lit_int(2)),
                lit_int(0),
            )),
        };
        assert_eq!(
            ev.eval(&expr, &mut env).unwrap(),
            Value::List(vec![Value::Int(2), Value::Int(4)])
        );
    }

    #[test]
    fn list_reduce() {
        let mut ev = Evaluator::new();
        let mut env = Env::new();
        // reduce [1,2,3] from 0 with (acc, x) -> acc + x
        let expr = Expr::Reduce {
            collection: Box::new(Expr::ListExpr(vec![lit_int(1), lit_int(2), lit_int(3)])),
            init: Box::new(lit_int(0)),
            acc: "acc".to_string(),
            var: "x".to_string(),
            body: Box::new(binop(
                BinOp::Add,
                Expr::Var("acc".to_string()),
                Expr::Var("x".to_string()),
            )),
        };
        assert_eq!(ev.eval(&expr, &mut env).unwrap(), Value::Int(6));
    }

    #[test]
    fn budget_exceeded() {
        let mut ev = Evaluator::new().with_budget(3);
        let mut env = Env::new();
        // Nested arithmetic that needs more than 3 evaluation steps:
        // (1 + 2) + (3 + 4) requires 5 eval() calls (root + 2 sums + 2 leftovers... )
        let expr = binop(
            BinOp::Add,
            binop(BinOp::Add, lit_int(1), lit_int(2)),
            binop(BinOp::Add, lit_int(3), lit_int(4)),
        );
        let err = ev.eval(&expr, &mut env).unwrap_err();
        assert!(matches!(err, SpoonError::BudgetExceeded));
    }

    #[test]
    fn type_error_on_mismatched_add() {
        let mut ev = Evaluator::new();
        let mut env = Env::new();
        let expr = binop(
            BinOp::Add,
            lit_int(5),
            Expr::Literal(Value::Text("hello".to_string())),
        );
        let err = ev.eval(&expr, &mut env).unwrap_err();
        assert!(matches!(err, SpoonError::TypeError { .. }));
    }

    #[test]
    fn division_by_zero() {
        let mut ev = Evaluator::new();
        let mut env = Env::new();
        let expr = binop(BinOp::Div, lit_int(1), lit_int(0));
        let err = ev.eval(&expr, &mut env).unwrap_err();
        assert!(matches!(err, SpoonError::DivisionByZero));
    }

    #[test]
    fn integer_overflow_is_an_execution_error_instead_of_wrapping() {
        let cases = [
            binop(BinOp::Add, lit_int(i64::MAX), lit_int(1)),
            binop(BinOp::Sub, lit_int(i64::MIN), lit_int(1)),
            binop(BinOp::Mul, lit_int(i64::MAX), lit_int(2)),
            binop(BinOp::Div, lit_int(i64::MIN), lit_int(-1)),
        ];

        for expr in cases {
            let error = Evaluator::new().eval(&expr, &mut Env::new()).unwrap_err();
            assert!(matches!(error, SpoonError::ArithmeticOverflow { .. }));
        }
    }

    #[test]
    fn negating_the_minimum_integer_returns_overflow_without_panicking() {
        let expr = Expr::UnOp {
            op: UnOp::Neg,
            operand: Box::new(lit_int(i64::MIN)),
        };

        let error = Evaluator::new().eval(&expr, &mut Env::new()).unwrap_err();
        assert!(matches!(error, SpoonError::ArithmeticOverflow { .. }));
    }

    #[test]
    fn trace_captures_procedure_calls() {
        let mut ev = Evaluator::new();
        let double = double_procedure();
        let double_id = double.id;
        ev.register_procedure(double);

        let quadruple = Procedure::new(
            "QUADRUPLE",
            vec![Param::named("x")],
            Expr::Call {
                procedure: double_id,
                args: vec![Expr::Call {
                    procedure: double_id,
                    args: vec![Expr::Var("x".to_string())],
                }],
            },
        );
        let quadruple_id = quadruple.id;
        ev.register_procedure(quadruple);

        let result = ev
            .exec_procedure(&quadruple_id, vec![Value::Int(3)])
            .unwrap();

        // Two inner DOUBLE calls, then the outer QUADRUPLE call.
        assert_eq!(result.trace.len(), 3);
        assert_eq!(result.trace.steps[0].procedure_called, Some(double_id));
        assert_eq!(result.trace.steps[0].output, Value::Int(6));
        assert_eq!(result.trace.steps[1].procedure_called, Some(double_id));
        assert_eq!(result.trace.steps[1].output, Value::Int(12));
        assert_eq!(result.trace.steps[2].procedure_called, Some(quadruple_id));
        assert_eq!(result.trace.steps[2].output, Value::Int(12));
    }

    #[test]
    fn arity_mismatch_errors() {
        let mut ev = Evaluator::new();
        let double = double_procedure();
        let id = double.id;
        ev.register_procedure(double);

        let err = ev
            .exec_procedure(&id, vec![Value::Int(1), Value::Int(2)])
            .unwrap_err();
        assert!(matches!(err, SpoonError::ArityMismatch { .. }));
    }

    #[test]
    fn undefined_procedure_errors() {
        let mut ev = Evaluator::new();
        let err = ev.exec_procedure(&ProcedureId::new(), vec![]).unwrap_err();
        assert!(matches!(err, SpoonError::UndefinedProcedure(_)));
    }

    #[test]
    fn v1_expansion_has_explicit_text_map_collection_and_conversion_semantics() {
        let map = Expr::Literal(Value::Map(std::collections::BTreeMap::from([
            ("b".into(), Value::Int(2)),
            ("a".into(), Value::Int(1)),
        ])));
        let list = Expr::ListExpr(vec![lit_int(3), lit_int(1), lit_int(3), lit_int(2)]);
        let cases = [
            (
                intrinsic(IntrinsicOp::TextNormalizeNfc, vec![lit_text("e\u{301}")]),
                Value::Text("é".into()),
            ),
            (
                intrinsic(IntrinsicOp::TextNormalizeNfd, vec![lit_text("é")]),
                Value::Text("e\u{301}".into()),
            ),
            (
                intrinsic(IntrinsicOp::TextNormalizeNfkc, vec![lit_text("Ａ")]),
                Value::Text("A".into()),
            ),
            (
                intrinsic(IntrinsicOp::TextNormalizeNfkd, vec![lit_text("é")]),
                Value::Text("e\u{301}".into()),
            ),
            (
                intrinsic(IntrinsicOp::TextTrimStart, vec![lit_text("  x  ")]),
                Value::Text("x  ".into()),
            ),
            (
                intrinsic(IntrinsicOp::TextTrimEnd, vec![lit_text("  x  ")]),
                Value::Text("  x".into()),
            ),
            (
                intrinsic(
                    IntrinsicOp::TextGraphemeSubstring,
                    vec![lit_text("a👩‍💻b"), lit_int(1), lit_int(1)],
                ),
                Value::Text("👩‍💻".into()),
            ),
            (
                intrinsic(
                    IntrinsicOp::TextIndexOf,
                    vec![lit_text("a👩‍💻b"), lit_text("b")],
                ),
                Value::Int(2),
            ),
            (
                intrinsic(
                    IntrinsicOp::TextCount,
                    vec![lit_text("banana"), lit_text("an")],
                ),
                Value::Int(2),
            ),
            (
                intrinsic(IntrinsicOp::TextRepeat, vec![lit_text("ab"), lit_int(3)]),
                Value::Text("ababab".into()),
            ),
            (
                intrinsic(
                    IntrinsicOp::TextConcatMany,
                    vec![Expr::ListExpr(vec![lit_text("a"), lit_text("b")])],
                ),
                Value::Text("ab".into()),
            ),
            (
                intrinsic(IntrinsicOp::MapEntries, vec![map.clone()]),
                Value::List(vec![
                    Value::List(vec![Value::Text("a".into()), Value::Int(1)]),
                    Value::List(vec![Value::Text("b".into()), Value::Int(2)]),
                ]),
            ),
            (
                intrinsic(
                    IntrinsicOp::MapSet,
                    vec![map.clone(), lit_text("c"), lit_int(3)],
                ),
                Value::Map(std::collections::BTreeMap::from([
                    ("a".into(), Value::Int(1)),
                    ("b".into(), Value::Int(2)),
                    ("c".into(), Value::Int(3)),
                ])),
            ),
            (
                intrinsic(IntrinsicOp::MapDelete, vec![map, lit_text("a")]),
                Value::Map(std::collections::BTreeMap::from([(
                    "b".into(),
                    Value::Int(2),
                )])),
            ),
            (
                intrinsic(
                    IntrinsicOp::MapMerge,
                    vec![
                        Expr::Literal(Value::Map(std::collections::BTreeMap::from([(
                            "a".into(),
                            Value::Int(1),
                        )]))),
                        Expr::Literal(Value::Map(std::collections::BTreeMap::from([
                            ("a".into(), Value::Int(9)),
                            ("b".into(), Value::Int(2)),
                        ]))),
                    ],
                ),
                Value::Map(std::collections::BTreeMap::from([
                    ("a".into(), Value::Int(9)),
                    ("b".into(), Value::Int(2)),
                ])),
            ),
            (
                intrinsic(
                    IntrinsicOp::CollectionSlice,
                    vec![list.clone(), lit_int(-3), lit_int(2)],
                ),
                Value::List(vec![Value::Int(1), Value::Int(3)]),
            ),
            (
                intrinsic(IntrinsicOp::CollectionReverse, vec![list.clone()]),
                Value::List(vec![
                    Value::Int(2),
                    Value::Int(3),
                    Value::Int(1),
                    Value::Int(3),
                ]),
            ),
            (
                intrinsic(IntrinsicOp::CollectionSort, vec![list.clone()]),
                Value::List(vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Int(3),
                    Value::Int(3),
                ]),
            ),
            (
                intrinsic(IntrinsicOp::CollectionUnique, vec![list]),
                Value::List(vec![Value::Int(3), Value::Int(1), Value::Int(2)]),
            ),
            (
                intrinsic(
                    IntrinsicOp::CollectionFlatten,
                    vec![Expr::ListExpr(vec![
                        Expr::ListExpr(vec![lit_int(1)]),
                        Expr::ListExpr(vec![lit_int(2), lit_int(3)]),
                    ])],
                ),
                Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
            ),
            (
                intrinsic(
                    IntrinsicOp::CollectionZip,
                    vec![
                        Expr::ListExpr(vec![lit_int(1), lit_int(2)]),
                        Expr::ListExpr(vec![lit_text("a")]),
                    ],
                ),
                Value::List(vec![Value::List(vec![
                    Value::Int(1),
                    Value::Text("a".into()),
                ])]),
            ),
            (
                intrinsic(IntrinsicOp::Range, vec![lit_int(1), lit_int(6), lit_int(2)]),
                Value::List(vec![Value::Int(1), Value::Int(3), Value::Int(5)]),
            ),
            (
                intrinsic(IntrinsicOp::TypeName, vec![lit_int(4)]),
                Value::Text("int".into()),
            ),
            (
                intrinsic(IntrinsicOp::ParseInt, vec![lit_text(" 42 ")]),
                Value::Int(42),
            ),
            (
                intrinsic(IntrinsicOp::ParseFloat, vec![lit_text("1.5")]),
                Value::Float(1.5),
            ),
            (
                intrinsic(IntrinsicOp::ParseBool, vec![lit_text("true")]),
                Value::Bool(true),
            ),
            (
                intrinsic(IntrinsicOp::ToText, vec![lit_int(42)]),
                Value::Text("42".into()),
            ),
        ];
        for (expression, expected) in cases {
            assert_eq!(
                Evaluator::new().eval(&expression, &mut Env::new()).unwrap(),
                expected
            );
        }
        assert_eq!(
            Evaluator::new()
                .eval(
                    &intrinsic(IntrinsicOp::ParseBool, vec![lit_text("TRUE")]),
                    &mut Env::new()
                )
                .unwrap(),
            Value::Null
        );
        assert!(matches!(
            Evaluator::new().eval(
                &intrinsic(
                    IntrinsicOp::MapSet,
                    vec![Expr::Literal(Value::Map(Default::default()))],
                ),
                &mut Env::new(),
            ),
            Err(SpoonError::ArityMismatch { .. })
        ));
    }

    #[test]
    fn intrinsic_linear_work_consumes_budget_and_limits_are_typed() {
        let expensive = intrinsic(
            IntrinsicOp::TextGraphemeLength,
            vec![lit_text(&"x".repeat(256))],
        );
        assert!(matches!(
            Evaluator::new()
                .with_budget(4)
                .eval(&expensive, &mut Env::new()),
            Err(SpoonError::BudgetExceeded)
        ));
        let structured = intrinsic(
            IntrinsicOp::JsonParse,
            vec![lit_text(r#"[[[1,2,3],[4,5,6]]]"#)],
        );
        assert!(matches!(
            Evaluator::new()
                .with_budget(4)
                .eval(&structured, &mut Env::new()),
            Err(SpoonError::BudgetExceeded)
        ));
        let output_limit = intrinsic(
            IntrinsicOp::TextRepeat,
            vec![lit_text("x"), lit_int(1_048_577)],
        );
        assert!(matches!(
            Evaluator::new().eval(&output_limit, &mut Env::new()),
            Err(SpoonError::IntrinsicLimitExceeded { .. })
        ));
        let item_limit = intrinsic(
            IntrinsicOp::Range,
            vec![lit_int(0), lit_int(100_001), lit_int(1)],
        );
        assert!(matches!(
            Evaluator::new().eval(&item_limit, &mut Env::new()),
            Err(SpoonError::IntrinsicLimitExceeded { .. })
        ));
    }

    #[test]
    fn numeric_intrinsics_have_bounded_explicit_semantics() {
        let cases = [
            (
                intrinsic(IntrinsicOp::NumericAbs, vec![lit_int(-7)]),
                Value::Int(7),
            ),
            (
                intrinsic(IntrinsicOp::NumericSign, vec![lit_float(-0.25)]),
                Value::Int(-1),
            ),
            (
                intrinsic(IntrinsicOp::NumericMin, vec![lit_int(9), lit_float(2.5)]),
                Value::Float(2.5),
            ),
            (
                intrinsic(IntrinsicOp::NumericMax, vec![lit_int(9), lit_float(2.5)]),
                Value::Float(9.0),
            ),
            (
                intrinsic(
                    IntrinsicOp::NumericClamp,
                    vec![lit_int(10), lit_float(0.5), lit_int(5)],
                ),
                Value::Float(5.0),
            ),
            (
                intrinsic(IntrinsicOp::NumericFloor, vec![lit_float(-1.5)]),
                Value::Float(-2.0),
            ),
            (
                intrinsic(IntrinsicOp::NumericCeil, vec![lit_float(-1.5)]),
                Value::Float(-1.0),
            ),
            (
                intrinsic(IntrinsicOp::NumericRound, vec![lit_float(-1.5)]),
                Value::Float(-2.0),
            ),
            (
                intrinsic(IntrinsicOp::NumericTruncate, vec![lit_float(-1.5)]),
                Value::Float(-1.0),
            ),
            (
                intrinsic(IntrinsicOp::NumericPowInt, vec![lit_int(2), lit_int(10)]),
                Value::Int(1024),
            ),
            (
                intrinsic(IntrinsicOp::IntegerQuotient, vec![lit_int(-7), lit_int(3)]),
                Value::Int(-2),
            ),
            (
                intrinsic(IntrinsicOp::IntegerRemainder, vec![lit_int(-7), lit_int(3)]),
                Value::Int(-1),
            ),
        ];
        for (expression, expected) in cases {
            assert_eq!(
                Evaluator::new().eval(&expression, &mut Env::new()).unwrap(),
                expected
            );
        }

        let float_power = intrinsic(
            IntrinsicOp::NumericPowFloat,
            vec![lit_int(9), lit_float(0.5)],
        );
        let Value::Float(value) = Evaluator::new()
            .eval(&float_power, &mut Env::new())
            .unwrap()
        else {
            panic!("numeric_pow_float must return a float")
        };
        assert!((value - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn numeric_intrinsics_reject_invalid_numbers_and_report_typed_failures() {
        let non_finite = intrinsic(IntrinsicOp::NumericAbs, vec![lit_float(f64::NAN)]);
        assert!(matches!(
            Evaluator::new().eval(&non_finite, &mut Env::new()),
            Err(SpoonError::InvalidNumber { .. })
        ));

        let negative_exponent =
            intrinsic(IntrinsicOp::NumericPowInt, vec![lit_int(2), lit_int(-1)]);
        assert!(matches!(
            Evaluator::new().eval(&negative_exponent, &mut Env::new()),
            Err(SpoonError::NegativeExponent { .. })
        ));

        let integer_overflow = intrinsic(
            IntrinsicOp::NumericPowInt,
            vec![lit_int(i64::MAX), lit_int(2)],
        );
        assert!(matches!(
            Evaluator::new().eval(&integer_overflow, &mut Env::new()),
            Err(SpoonError::ArithmeticOverflow { .. })
        ));

        let invalid_range = intrinsic(
            IntrinsicOp::NumericClamp,
            vec![lit_int(1), lit_int(3), lit_int(2)],
        );
        assert!(matches!(
            Evaluator::new().eval(&invalid_range, &mut Env::new()),
            Err(SpoonError::InvalidNumber { .. })
        ));

        let float_overflow = intrinsic(
            IntrinsicOp::NumericPowFloat,
            vec![lit_float(1e308), lit_float(2.0)],
        );
        assert!(matches!(
            Evaluator::new().eval(&float_overflow, &mut Env::new()),
            Err(SpoonError::InvalidNumber { .. })
        ));

        let zero_quotient = intrinsic(IntrinsicOp::IntegerQuotient, vec![lit_int(1), lit_int(0)]);
        assert!(matches!(
            Evaluator::new().eval(&zero_quotient, &mut Env::new()),
            Err(SpoonError::DivisionByZero)
        ));
    }
}
