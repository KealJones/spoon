//! Bottom-up typed enumerative synthesizer with observational-equivalence
//! pruning. No LLM anywhere - search over typed Programs.
//!
//! Budget axes: nodes evaluated, wall-clock time, OE table entries (memory).
//!
//! Determinism: actions are sorted by ID before any iteration. The bank is a
//! Vec (insertion order). Same spec + Can -> identical search order every run.
//!
//! Completeness: the bank has no lossy cap. OE dedup bounds size.
//! `max_table_entries` is the hard memory limit and fails loudly with Budget.
//!
//! Pruning:
//! - Backward type reachability: a bank entry of type T is only kept if T can
//!   lead to spec.ret within the remaining size budget.
//! - All-Any-param actions (value.*) are skipped unless the spec uses Any/Json.
//! - Constant-only sub-expressions (no Param anywhere) are skipped at size >= 2.

use std::collections::HashSet;
use std::time::Instant;

use spoon_core::can::Can;
use spoon_core::kernel::eval::eval_program;
use spoon_core::kernel::{Budget, Ctx, Kernel, NoHost};
use spoon_core::types::{Action, Expr, Lambda, Program, Spec, Type, Value};

use super::oe::{oe_key, outputs_match, OeInsert, OeTable};

// ---- public types -------------------------------------------------------

/// Search budget for the synthesizer.
#[derive(Debug, Clone)]
pub struct SynthBudget {
    pub max_nodes: usize,
    pub max_millis: u64,
    /// Maximum expression size (node count) to enumerate.
    pub max_size: usize,
    /// Maximum OE-table entries (memory proxy).
    pub max_table_entries: usize,
}

impl Default for SynthBudget {
    fn default() -> Self {
        SynthBudget {
            max_nodes: 200_000,
            max_millis: 3_000,
            max_size: 7,
            max_table_entries: 500_000,
        }
    }
}

#[derive(Debug)]
pub enum SynthOutcome {
    Found { program: Program, tried: usize, millis: u64, table_size: usize },
    Exhausted { tried: usize, millis: u64 },
    Budget { tried: usize, millis: u64, reason: String },
}

// ---- internal bank entry ------------------------------------------------

#[derive(Clone)]
struct Entry {
    expr: Expr,
    output_ty: Type,
    size: usize,
    /// True if the expression contains at least one Param{} node.
    has_param: bool,
}

// ---- main entry point ---------------------------------------------------

pub fn synthesize(spec: &Spec, can: &Can, kernel: &Kernel, budget: &SynthBudget) -> SynthOutcome {
    if let Err(reason) = spec.validate() {
        return SynthOutcome::Budget { tried: 0, millis: 0, reason };
    }
    let start = Instant::now();
    let expected: Vec<Value> = spec.examples.iter().map(|ex| ex.output.clone()).collect();

    // Sort for determinism.
    let mut pure_actions: Vec<Action> = can.pure_actions().into_iter().cloned().collect();
    pure_actions.sort_by(|a, b| a.id.0.cmp(&b.id.0));

    let spec_uses_any_json = spec_touches_any_json(spec);

    // Filter out all-Any-param actions (value.*) unless spec touches Any/Json.
    let working_actions: Vec<Action> = pure_actions
        .into_iter()
        .filter(|a| spec_uses_any_json || !is_all_any_params(a))
        .collect();

    let (hof_actions, regular_actions): (Vec<Action>, Vec<Action>) =
        working_actions.into_iter().partition(|a| has_func_input(a));

    let constants = gather_constants(spec);
    // Lambda bodies built from sorted regular_actions - deterministic.
    let lambda_bodies = build_lambda_bodies(&regular_actions, &constants, 3);

    let max_size = budget.max_size;

    // Backward reachability: reach[k] = types that can contribute to spec.ret
    // in at most k more nodes. Used to prune bank entries.
    let reach = build_reach(spec, can, max_size, &regular_actions);

    // bank[size] = OE-distinct entries of exactly that node count.
    let mut bank: Vec<Vec<Entry>> = vec![vec![]; max_size + 1];
    let mut oe = OeTable::new(budget.max_table_entries);
    let mut nodes = 0usize;
    let mut tried = 0usize;

    // ---- Level 1: terminals ----

    for (i, ty) in spec.params.iter().enumerate() {
        let expr = Expr::Param { index: i };
        let outputs: Vec<Value> = spec.examples.iter().map(|ex| ex.inputs[i].clone()).collect();
        let output_ty = ty.clone();
        if !type_in_reach(&output_ty, &reach[max_size.saturating_sub(1)]) {
            continue;
        }
        let key = oe_key(&output_ty, &outputs);
        match oe.try_insert(&key) {
            OeInsert::TableFull => return mk_budget(tried, &start, "table full"),
            OeInsert::New => {
                tried += 1;
                if outputs_match(&outputs, &expected) {
                    return mk_found(spec, expr, tried, &start, oe.len());
                }
                bank[1].push(Entry { expr, output_ty, size: 1, has_param: true });
            }
            OeInsert::Dominated => {}
        }
    }

    for val in &constants {
        let expr = Expr::Const { value: val.clone() };
        let outputs: Vec<Value> = spec.examples.iter().map(|_| val.clone()).collect();
        let output_ty = val.type_of();
        if !type_in_reach(&output_ty, &reach[max_size.saturating_sub(1)]) {
            continue;
        }
        let key = oe_key(&output_ty, &outputs);
        match oe.try_insert(&key) {
            OeInsert::TableFull => return mk_budget(tried, &start, "table full"),
            OeInsert::New => {
                tried += 1;
                if outputs_match(&outputs, &expected) {
                    return mk_found(spec, expr, tried, &start, oe.len());
                }
                bank[1].push(Entry { expr, output_ty, size: 1, has_param: false });
            }
            OeInsert::Dominated => {}
        }
    }

    // ---- Levels 2 .. max_size ----

    for n in 2..=max_size {
        if start.elapsed().as_millis() as u64 >= budget.max_millis {
            return mk_budget(tried, &start, &format!("{} ms", budget.max_millis));
        }

        let remaining = max_size.saturating_sub(n);
        // Reachability set for entries that will go INTO bank[n].
        let reach_for_n = &reach[remaining];

        // ---- HOF actions FIRST ----
        let has_list = (1..n).any(|s| bank[s].iter().any(|e| is_list_type(&e.output_ty)));
        if has_list {
            for action in &hof_actions {
                match action.arity() {
                    2 => {
                        // (list, fn): Call size = 1 + list_size + lambda_size
                        for (body_expr, body_size) in &lambda_bodies {
                            let lambda_size = 1 + body_size;
                            let list_size = match (n - 1).checked_sub(lambda_size) {
                                Some(s) if s >= 1 && s < bank.len() => s,
                                _ => continue,
                            };
                            let lambda = Lambda {
                                params: vec![Type::Any],
                                ret: Type::Any,
                                body: body_expr.clone(),
                            };
                            let lambda_expr = Expr::Lambda { lambda: Box::new(lambda) };
                            for list_entry in bank[list_size].clone() {
                                if !can.assignable(&action.inputs[0].ty, &list_entry.output_ty) {
                                    continue;
                                }
                                let expr = Expr::Call {
                                    action: action.id.clone(),
                                    args: vec![list_entry.expr.clone(), lambda_expr.clone()],
                                };
                                tried += 1;
                                nodes += 1;
                                if let Some(o) = budget_check(nodes, &start, tried, budget) {
                                    return o;
                                }
                                let Some(outputs) = eval_all(&expr, spec, can, kernel) else {
                                    continue;
                                };
                                let output_ty = infer_output_ty(&action.output, &outputs);
                                if !type_in_reach(&output_ty, reach_for_n) {
                                    continue;
                                }
                                let key = oe_key(&output_ty, &outputs);
                                match oe.try_insert(&key) {
                                    OeInsert::TableFull => {
                                        return mk_budget(tried, &start, "table full")
                                    }
                                    OeInsert::Dominated => {}
                                    OeInsert::New => {
                                        if outputs_match(&outputs, &expected) {
                                            return mk_found(spec, expr, tried, &start, oe.len());
                                        }
                                        bank[n].push(Entry {
                                            expr,
                                            output_ty,
                                            size: n,
                                            has_param: list_entry.has_param,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    3 => {
                        // Fold: (list, init, fn)
                        for (body_expr, body_size) in &lambda_bodies {
                            let lambda_size = 1 + body_size;
                            let remaining_args = match (n - 1).checked_sub(lambda_size) {
                                Some(r) if r >= 2 => r,
                                _ => continue,
                            };
                            let lambda = Lambda {
                                params: vec![Type::Any, Type::Any],
                                ret: Type::Any,
                                body: body_expr.clone(),
                            };
                            let lambda_expr = Expr::Lambda { lambda: Box::new(lambda) };
                            for list_size in 1..remaining_args {
                                let init_size = remaining_args - list_size;
                                if list_size >= bank.len() || init_size >= bank.len() {
                                    continue;
                                }
                                for list_entry in bank[list_size].clone() {
                                    if !can.assignable(
                                        &action.inputs[0].ty,
                                        &list_entry.output_ty,
                                    ) {
                                        continue;
                                    }
                                    for init_entry in bank[init_size].clone() {
                                        let has_p =
                                            list_entry.has_param || init_entry.has_param;
                                        if !has_p {
                                            continue;
                                        }
                                        let expr = Expr::Call {
                                            action: action.id.clone(),
                                            args: vec![
                                                list_entry.expr.clone(),
                                                init_entry.expr.clone(),
                                                lambda_expr.clone(),
                                            ],
                                        };
                                        tried += 1;
                                        nodes += 1;
                                        if let Some(o) = budget_check(nodes, &start, tried, budget) {
                                            return o;
                                        }
                                        let Some(outputs) = eval_all(&expr, spec, can, kernel)
                                        else {
                                            continue;
                                        };
                                        let output_ty =
                                            infer_output_ty(&action.output, &outputs);
                                        if !type_in_reach(&output_ty, reach_for_n) {
                                            continue;
                                        }
                                        let key = oe_key(&output_ty, &outputs);
                                        match oe.try_insert(&key) {
                                            OeInsert::TableFull => {
                                                return mk_budget(tried, &start, "table full")
                                            }
                                            OeInsert::Dominated => {}
                                            OeInsert::New => {
                                                if outputs_match(&outputs, &expected) {
                                                    return mk_found(
                                                        spec, expr, tried, &start, oe.len(),
                                                    );
                                                }
                                                bank[n].push(Entry {
                                                    expr,
                                                    output_ty,
                                                    size: n,
                                                    has_param: has_p,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // ---- Regular (non-HOF) actions ----
        for action in &regular_actions {
            let arity = action.arity();
            if arity == 0 {
                continue;
            }
            // Pre-check: output must be reachable for remaining budget.
            if !type_in_reach(&action.output, reach_for_n) {
                // Also skip if Any (Any outputs are inferred at runtime - always keep).
                if !type_has_any(&action.output) {
                    continue;
                }
            }
            let input_types = action.input_types();
            let mut combos: Vec<Vec<Entry>> = Vec::new();
            collect_combos(&input_types, n - 1, &bank, can, &mut Vec::new(), &mut combos);

            for combo in combos {
                // Skip constant-only sub-expressions.
                if !combo.iter().any(|e| e.has_param) {
                    continue;
                }
                let args: Vec<Expr> = combo.iter().map(|e| e.expr.clone()).collect();
                let expr = Expr::Call { action: action.id.clone(), args };
                tried += 1;
                nodes += 1;
                if let Some(o) = budget_check(nodes, &start, tried, budget) {
                    return o;
                }
                let Some(outputs) = eval_all(&expr, spec, can, kernel) else { continue };
                let output_ty = infer_output_ty(&action.output, &outputs);
                if !type_in_reach(&output_ty, reach_for_n) {
                    continue;
                }
                let key = oe_key(&output_ty, &outputs);
                match oe.try_insert(&key) {
                    OeInsert::TableFull => return mk_budget(tried, &start, "table full"),
                    OeInsert::Dominated => {}
                    OeInsert::New => {
                        if outputs_match(&outputs, &expected) {
                            return mk_found(spec, expr, tried, &start, oe.len());
                        }
                        let has_p = combo.iter().any(|e| e.has_param);
                        bank[n].push(Entry { expr, output_ty, size: n, has_param: has_p });
                    }
                }
            }
        }
    }

    SynthOutcome::Exhausted { tried, millis: start.elapsed().as_millis() as u64 }
}

// ---- pretty-printer ----------------------------------------------------

/// Render a Program body as a Lisp s-expression for logs.
/// Example: `(math.mul p0 2)`
pub fn describe(program: &Program) -> String {
    describe_expr(&program.body)
}

fn describe_expr(expr: &Expr) -> String {
    match expr {
        Expr::Const { value } => value.render(),
        Expr::Param { index } => format!("p{}", index),
        Expr::LambdaParam { index } => format!("lp{}", index),
        Expr::Call { action, args } => {
            if args.is_empty() {
                action.0.clone()
            } else {
                let parts: Vec<String> = args.iter().map(describe_expr).collect();
                format!("({} {})", action.0, parts.join(" "))
            }
        }
        Expr::Lambda { lambda } => format!("(lambda {})", describe_expr(&lambda.body)),
        Expr::If { cond, then, otherwise } => format!(
            "(if {} {} {})",
            describe_expr(cond),
            describe_expr(then),
            describe_expr(otherwise)
        ),
        Expr::ListLit { items } => {
            let parts: Vec<String> = items.iter().map(describe_expr).collect();
            format!("[{}]", parts.join(" "))
        }
        Expr::Struct { concept, fields } => {
            let parts: Vec<String> = fields
                .iter()
                .map(|(k, v)| format!(":{} {}", k, describe_expr(v)))
                .collect();
            format!("(struct {} {})", concept.0, parts.join(" "))
        }
        Expr::Field { of, name } => format!("(. {} {})", describe_expr(of), name),
    }
}

// ---- reachability -------------------------------------------------------

/// Backward type reachability. reach[k] = set of normalized type keys whose
/// expressions can contribute to spec.ret in at most k more nodes.
///
/// Built via BFS over pure, non-HOF, non-bare-Any-param actions.
/// Numeric promotion: Int and Float are treated as interchangeable.
fn build_reach(
    spec: &Spec,
    _can: &Can,
    max_size: usize,
    regular_actions: &[Action],
) -> Vec<HashSet<String>> {
    // Actions usable in the backward BFS: no bare Any param, no HOF.
    let bfs_actions: Vec<&Action> = regular_actions
        .iter()
        .filter(|a| !has_bare_any_param(a))
        .collect();

    let mut reach: Vec<HashSet<String>> = (0..=max_size).map(|_| HashSet::new()).collect();

    // Seed reach[0] with goal type + compatible variants.
    let goal_key = type_reach_key(&spec.ret);
    reach[0].insert(goal_key.clone());
    // Any-output actions (list.first, list.fold, etc.) may produce the goal at runtime.
    reach[0].insert("any".to_string());
    // Numeric promotion: Int and Float are interchangeable.
    if goal_key == "int" || goal_key == "float" {
        reach[0].insert("int".to_string());
        reach[0].insert("float".to_string());
    }
    // If goal is a specific list type, List(Any) is also useful (runtime-inferred).
    if goal_key.starts_with("list.") && goal_key != "list.any" {
        reach[0].insert("list.any".to_string());
    }

    for k in 1..=max_size {
        // Inherit.
        let prev: Vec<String> = reach[k - 1].iter().cloned().collect();
        for key in &prev {
            reach[k].insert(key.clone());
        }

        // Backward step: if A.output is reachable, A's input types become reachable.
        for action in &bfs_actions {
            let out_key = type_reach_key(&action.output);
            let useful = reach[k - 1].contains(&out_key)
                || type_has_any(&action.output) // Any output: runtime-inferred
                || (out_key.starts_with("list.") && reach[k - 1].contains("list.any"));
            if !useful {
                continue;
            }
            for input in &action.inputs {
                if !matches!(input.ty, Type::Func(_, _)) {
                    let in_key = type_reach_key(&input.ty);
                    reach[k].insert(in_key.clone());
                    // Numeric promotion.
                    if in_key == "int" || in_key == "float" {
                        reach[k].insert("int".to_string());
                        reach[k].insert("float".to_string());
                    }
                    // Any list type makes "list.any" available.
                    if in_key.starts_with("list.") {
                        reach[k].insert("list.any".to_string());
                    }
                }
            }
        }

        // HOF actions can accept list inputs; ensure "list.any" propagates back.
        // (HOFs themselves are excluded from BFS but their list inputs count.)
        if reach[k - 1].contains("list.any") || reach[k - 1].starts_with_any_list() {
            reach[k].insert("list.any".to_string());
        }
    }

    // Also: any type in spec.params is always reachable (it's a direct input).
    for ty in &spec.params {
        let key = type_reach_key(ty);
        for r in reach.iter_mut() {
            r.insert(key.clone());
        }
    }
    // Null is never useful unless the goal is Null.
    if goal_key != "null" {
        for r in reach.iter_mut() {
            r.remove("null");
        }
    }

    reach
}

/// Normalized type key for reachability (finer-grained than OE key).
fn type_reach_key(ty: &Type) -> String {
    match ty {
        Type::Any => "any".to_string(),
        Type::Int => "int".to_string(),
        Type::Float => "float".to_string(),
        Type::Text => "text".to_string(),
        Type::Bool => "bool".to_string(),
        Type::Null => "null".to_string(),
        Type::Path => "path".to_string(),
        Type::Json => "json".to_string(),
        Type::List(inner) => format!("list.{}", type_reach_key(inner)),
        Type::Func(_, _) => "func".to_string(),
        other => format!("{:?}", other).to_lowercase(),
    }
}

fn type_in_reach(ty: &Type, reach_set: &HashSet<String>) -> bool {
    if reach_set.is_empty() {
        return true; // reach[0] from saturating_sub when max_size=0; keep everything
    }
    let key = type_reach_key(ty);
    if reach_set.contains(&key) {
        return true;
    }
    // Any type is always accepted when "any" is in reach.
    if reach_set.contains("any") {
        return true;
    }
    // Numeric promotion.
    if (key == "int" || key == "float") && (reach_set.contains("int") || reach_set.contains("float")) {
        return true;
    }
    // Any list is compatible with List(Any).
    if key.starts_with("list.") && reach_set.contains("list.any") {
        return true;
    }
    false
}

// ---- helpers -----------------------------------------------------------

fn spec_touches_any_json(spec: &Spec) -> bool {
    let any_ty = |ty: &Type| matches!(ty, Type::Any | Type::Json);
    spec.params.iter().any(any_ty) || any_ty(&spec.ret)
}

fn is_all_any_params(action: &Action) -> bool {
    !action.inputs.is_empty() && action.inputs.iter().all(|i| matches!(i.ty, Type::Any))
}

fn has_func_input(action: &Action) -> bool {
    action.inputs.iter().any(|i| matches!(i.ty, Type::Func(_, _)))
}

fn has_bare_any_param(action: &Action) -> bool {
    action.inputs.iter().any(|i| matches!(i.ty, Type::Any))
}

fn is_list_type(ty: &Type) -> bool {
    matches!(ty, Type::List(_))
}

/// Gather constants from spec examples plus a small fixed pool.
/// Returns values deduplicated by their debug representation.
pub(crate) fn gather_constants(spec: &Spec) -> Vec<Value> {
    let mut vals = vec![
        Value::Int(0),
        Value::Int(1),
        Value::Int(2),
        Value::Int(10),
        Value::Int(-1),
        Value::Text(String::new()),
        Value::Text(" ".to_string()),
    ];
    for ex in &spec.examples {
        for v in ex.inputs.iter().chain(std::iter::once(&ex.output)) {
            collect_scalars(v, &mut vals);
        }
    }
    dedup_values(vals)
}

fn collect_scalars(v: &Value, out: &mut Vec<Value>) {
    match v {
        Value::Int(_) | Value::Float(_) | Value::Text(_) | Value::Bool(_) => {
            out.push(v.clone())
        }
        Value::List(items) => {
            for item in items {
                collect_scalars(item, out);
            }
        }
        _ => {}
    }
}

fn dedup_values(vals: Vec<Value>) -> Vec<Value> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for v in vals {
        let key = format!("{:?}", v);
        if seen.insert(key) {
            out.push(v);
        }
    }
    out
}

/// Build lambda bodies `(body_expr, body_size)` for HOF enumeration.
/// Uses sorted actions for determinism.
fn build_lambda_bodies(
    regular_actions: &[Action],
    constants: &[Value],
    max_body_size: usize,
) -> Vec<(Expr, usize)> {
    let mut bodies: Vec<(Expr, usize)> = Vec::new();

    // Size 1: lambda params and constants.
    bodies.push((Expr::LambdaParam { index: 0 }, 1));
    bodies.push((Expr::LambdaParam { index: 1 }, 1));
    for c in constants {
        bodies.push((Expr::Const { value: c.clone() }, 1));
    }

    if max_body_size >= 2 {
        for action in regular_actions.iter().filter(|a| a.arity() == 1) {
            bodies.push((
                Expr::Call {
                    action: action.id.clone(),
                    args: vec![Expr::LambdaParam { index: 0 }],
                },
                2,
            ));
        }
    }

    if max_body_size >= 3 {
        for action in regular_actions.iter().filter(|a| a.arity() == 2) {
            bodies.push((
                Expr::Call {
                    action: action.id.clone(),
                    args: vec![Expr::LambdaParam { index: 0 }, Expr::LambdaParam { index: 0 }],
                },
                3,
            ));
            bodies.push((
                Expr::Call {
                    action: action.id.clone(),
                    args: vec![Expr::LambdaParam { index: 0 }, Expr::LambdaParam { index: 1 }],
                },
                3,
            ));
            for c in constants {
                let ce = Expr::Const { value: c.clone() };
                bodies.push((
                    Expr::Call {
                        action: action.id.clone(),
                        args: vec![Expr::LambdaParam { index: 0 }, ce.clone()],
                    },
                    3,
                ));
                bodies.push((
                    Expr::Call {
                        action: action.id.clone(),
                        args: vec![ce, Expr::LambdaParam { index: 0 }],
                    },
                    3,
                ));
            }
        }
        // size-3: unary(unary(lp0))
        for outer in regular_actions.iter().filter(|a| a.arity() == 1) {
            for inner in regular_actions.iter().filter(|a| a.arity() == 1) {
                let inner_e = Expr::Call {
                    action: inner.id.clone(),
                    args: vec![Expr::LambdaParam { index: 0 }],
                };
                bodies.push((
                    Expr::Call { action: outer.id.clone(), args: vec![inner_e] },
                    3,
                ));
            }
        }
    }

    bodies
}

/// Enumerate all k-tuples from the bank whose sizes sum to target_size and
/// whose types are accepted by input_types.
fn collect_combos(
    input_types: &[Type],
    target_size: usize,
    bank: &[Vec<Entry>],
    can: &Can,
    prefix: &mut Vec<Entry>,
    results: &mut Vec<Vec<Entry>>,
) {
    if input_types.is_empty() {
        if target_size == 0 {
            results.push(prefix.clone());
        }
        return;
    }
    let k = input_types.len();
    let max_for_this = target_size.saturating_sub(k - 1);
    let input_ty = &input_types[0];

    for size in 1..=max_for_this {
        if size >= bank.len() {
            break;
        }
        for entry in &bank[size] {
            if !can.assignable(input_ty, &entry.output_ty) {
                continue;
            }
            prefix.push(entry.clone());
            collect_combos(&input_types[1..], target_size - size, bank, can, prefix, results);
            prefix.pop();
        }
    }
}

/// Evaluate an expression on all spec examples. Returns None if any eval fails.
fn eval_all(expr: &Expr, spec: &Spec, can: &Can, kernel: &Kernel) -> Option<Vec<Value>> {
    let mut outs = Vec::with_capacity(spec.examples.len());
    for ex in &spec.examples {
        let prog = Program::new(spec.params.clone(), spec.ret.clone(), expr.clone());
        let mut ctx = Ctx::pure(can, kernel, &NoHost, Budget::tiny());
        match eval_program(&mut ctx, &prog, &ex.inputs) {
            Ok(v) => outs.push(v),
            Err(_) => return None,
        }
    }
    Some(outs)
}

/// Resolve the output type for an expression, inferring from actual outputs
/// when the declared type contains Any.
fn infer_output_ty(declared: &Type, outputs: &[Value]) -> Type {
    if type_has_any(declared) {
        outputs.first().map(|v| v.type_of()).unwrap_or_else(|| declared.clone())
    } else {
        declared.clone()
    }
}

fn type_has_any(ty: &Type) -> bool {
    match ty {
        Type::Any => true,
        Type::List(inner) => type_has_any(inner),
        Type::Func(params, ret) => params.iter().any(type_has_any) || type_has_any(ret),
        _ => false,
    }
}

/// Check node + time budget. Returns Some(Budget outcome) if exceeded.
fn budget_check(
    nodes: usize,
    start: &Instant,
    tried: usize,
    budget: &SynthBudget,
) -> Option<SynthOutcome> {
    if nodes >= budget.max_nodes {
        return Some(SynthOutcome::Budget {
            tried,
            millis: start.elapsed().as_millis() as u64,
            reason: format!("{} nodes", budget.max_nodes),
        });
    }
    if nodes % 256 == 0 && start.elapsed().as_millis() as u64 >= budget.max_millis {
        return Some(SynthOutcome::Budget {
            tried,
            millis: start.elapsed().as_millis() as u64,
            reason: format!("{} ms", budget.max_millis),
        });
    }
    None
}

fn mk_budget(tried: usize, start: &Instant, reason: &str) -> SynthOutcome {
    SynthOutcome::Budget {
        tried,
        millis: start.elapsed().as_millis() as u64,
        reason: reason.to_string(),
    }
}

fn mk_found(
    spec: &Spec,
    expr: Expr,
    tried: usize,
    start: &Instant,
    table_size: usize,
) -> SynthOutcome {
    SynthOutcome::Found {
        program: Program::new(spec.params.clone(), spec.ret.clone(), expr),
        tried,
        millis: start.elapsed().as_millis() as u64,
        table_size,
    }
}

// ---- extension trait for reach sets ------------------------------------

trait ReachSetExt {
    fn starts_with_any_list(&self) -> bool;
}

impl ReachSetExt for HashSet<String> {
    fn starts_with_any_list(&self) -> bool {
        self.iter().any(|k| k.starts_with("list."))
    }
}
