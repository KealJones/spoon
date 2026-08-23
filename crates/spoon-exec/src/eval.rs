use std::cmp::Ordering;
use std::collections::HashMap;

use spoon_core::{BinOp, Condition, Expr, Procedure, ProcedureId, SpoonError, UnOp, Value};

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
        }
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
    use spoon_core::{Condition, Contract, Param};

    fn lit_int(n: i64) -> Expr {
        Expr::Literal(Value::Int(n))
    }

    fn binop(op: BinOp, left: Expr, right: Expr) -> Expr {
        Expr::BinOp {
            op,
            left: Box::new(left),
            right: Box::new(right),
        }
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
}
