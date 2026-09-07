//! Stitch-lite library consolidation.
//!
//! Finds subtrees that recur across the program library, anti-unifies them
//! into a new CAN action, and rewrites the affected programs to call it.
//! One pattern per call; the orchestrator loops if it wants more.

use std::collections::HashMap;

use spoon_core::can::Can;
use spoon_core::types::{
    Action, ActionId, Effect, Expr, Impl, Input, Program, Provenance, Stats, Tier, Type, Value,
};

// ---- public types -------------------------------------------------------

/// Result of one consolidation pass.
#[derive(Debug, Default)]
pub struct ConsolidationReport {
    /// Newly minted CAN actions (Provisional, Consolidated provenance).
    pub new_actions: Vec<Action>,
    /// Programs that were rewritten: (original action id, new program).
    pub rewritten: Vec<(ActionId, Program)>,
}

// ---- main entry point ---------------------------------------------------

/// Scan `library` for repeated subtrees of size >= min_size that appear in
/// >= min_occurrences programs.  Pick the pattern with the best compression,
/// create a new action, and rewrite the matching programs.
pub fn consolidate(
    library: &[(ActionId, Program)],
    can: &Can,
    min_occurrences: usize,
    min_size: usize,
) -> ConsolidationReport {
    if library.is_empty() {
        return ConsolidationReport::default();
    }

    // Step 1: collect subtrees per structural shape.
    // shape_key -> Vec<(subtree_expr, prog_idx)>
    let mut shape_map: HashMap<String, Vec<(Expr, usize)>> = HashMap::new();
    for (prog_idx, (_id, program)) in library.iter().enumerate() {
        for subtree in all_subtrees(&program.body) {
            if subtree.size() < min_size {
                continue;
            }
            // Only abstract over Call subtrees (not bare params/constants).
            if !matches!(subtree, Expr::Call { .. }) {
                continue;
            }
            let key = structural_key(subtree);
            shape_map.entry(key).or_default().push((subtree.clone(), prog_idx));
        }
    }

    // Step 2: keep only shapes with enough occurrences (count by program, not
    // by raw subtree count, to avoid inflating within a single program).
    let candidates: Vec<(String, Vec<(Expr, usize)>)> = shape_map
        .into_iter()
        .filter(|(_, instances)| {
            let unique_progs: std::collections::HashSet<usize> =
                instances.iter().map(|(_, idx)| *idx).collect();
            unique_progs.len() >= min_occurrences
        })
        .collect();

    if candidates.is_empty() {
        return ConsolidationReport::default();
    }

    // Step 3: for each candidate, anti-unify instances and compute compression.
    let mut best: Option<(String, Vec<(Expr, usize)>, Expr, Vec<Type>, Type, i64, usize)> = None;

    for (shape_key, instances) in candidates {
        let prog_params: Vec<Vec<Type>> = instances
            .iter()
            .map(|(_, idx)| library[*idx].1.params.clone())
            .collect();
        let expr_params: Vec<(Expr, Vec<Type>)> = instances
            .iter()
            .zip(prog_params.iter())
            .map(|((e, _), p)| (e.clone(), p.clone()))
            .collect();

        let mut holes: Vec<(Vec<Expr>, Vec<Type>)> = Vec::new();
        let abstract_body = match anti_unify(&expr_params, &mut holes) {
            Some(b) => b,
            None => continue,
        };
        if holes.is_empty() {
            continue; // no parameters -> not useful
        }

        let pattern_size = instances[0].0.size();
        // Call to abstract action has size 1 + holes.len()
        let call_size = 1 + holes.len();
        let n_occurrences = instances.len() as i64;
        let compression = n_occurrences * (pattern_size as i64 - call_size as i64)
            - abstract_body.size() as i64;
        if compression <= 0 {
            continue;
        }

        let hole_types: Vec<Type> = holes
            .iter()
            .map(|(exprs, params)| common_leaf_type(exprs, params))
            .collect();

        let n_holes = hole_types.len();
        let output_ty = root_output_type(&abstract_body, can);

        match &best {
            None => {
                best = Some((shape_key, instances, abstract_body, hole_types, output_ty, compression, n_holes));
            }
            Some((_, _, _, _, _, best_comp, _)) if compression > *best_comp => {
                best = Some((shape_key, instances, abstract_body, hole_types, output_ty, compression, n_holes));
            }
            _ => {}
        }
    }

    let (shape_key, instances, abstract_body, hole_types, output_ty, _, n_holes) = match best {
        Some(b) => b,
        None => return ConsolidationReport::default(),
    };

    // Step 4: create the new action.
    let abstract_prog = Program::new(hole_types.clone(), output_ty.clone(), abstract_body.clone());
    let hash = &abstract_prog.hash()[..8];
    let new_id = ActionId(format!("learned.abs_{}", hash));

    let inputs: Vec<Input> = hole_types
        .iter()
        .enumerate()
        .map(|(i, ty)| Input::required(&format!("arg{}", i), ty.clone()))
        .collect();

    // Provenance: track which library programs contributed.
    let from: Vec<String> = {
        let mut prog_ids: Vec<String> = instances
            .iter()
            .map(|(_, idx)| library[*idx].0 .0.clone())
            .collect();
        prog_ids.sort();
        prog_ids.dedup();
        prog_ids
    };

    let new_action = Action {
        id: new_id.clone(),
        inputs,
        output: output_ty,
        effect: Effect::Pure,
        imp: Impl::Program { program: abstract_prog },
        role: spoon_core::types::Role::Command,
        verbs: vec![],
        phrasings: vec![],
        description: String::new(),
        tier: Tier::Provisional,
        provenance: Provenance::Consolidated { from },
        stats: Stats::default(),
    };

    // Step 5: rewrite affected programs.
    let prog_indices: std::collections::HashSet<usize> =
        instances.iter().map(|(_, idx)| *idx).collect();

    let mut rewritten = Vec::new();
    for idx in prog_indices {
        let (orig_id, orig_prog) = &library[idx];
        let new_body = rewrite_expr(&orig_prog.body, &shape_key, &new_id, &abstract_body, n_holes);
        let new_prog = Program::new(orig_prog.params.clone(), orig_prog.ret.clone(), new_body);
        rewritten.push((orig_id.clone(), new_prog));
    }

    ConsolidationReport { new_actions: vec![new_action], rewritten }
}

// ---- subtree enumeration ------------------------------------------------

fn all_subtrees(expr: &Expr) -> Vec<&Expr> {
    let mut result = Vec::new();
    collect_subtrees(expr, &mut result);
    result
}

fn collect_subtrees<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    out.push(expr);
    match expr {
        Expr::Call { args, .. } => {
            for a in args {
                collect_subtrees(a, out);
            }
        }
        Expr::Lambda { lambda } => collect_subtrees(&lambda.body, out),
        Expr::If { cond, then, otherwise } => {
            collect_subtrees(cond, out);
            collect_subtrees(then, out);
            collect_subtrees(otherwise, out);
        }
        _ => {}
    }
}

// ---- structural key ----------------------------------------------------

/// Hash an expression by structure only, replacing all Param/Const/LambdaParam
/// leaves with "?".
fn structural_key(expr: &Expr) -> String {
    match expr {
        Expr::Param { .. } | Expr::LambdaParam { .. } | Expr::Const { .. } => "?".to_string(),
        Expr::Call { action, args } => {
            let parts: Vec<String> = args.iter().map(structural_key).collect();
            format!("({} {})", action.0, parts.join(" "))
        }
        Expr::Lambda { lambda } => format!("(lambda {})", structural_key(&lambda.body)),
        Expr::Field { of, name } => format!("(.{} {})", name, structural_key(of)),
        Expr::If { cond, then, otherwise } => format!(
            "(if {} {} {})",
            structural_key(cond),
            structural_key(then),
            structural_key(otherwise)
        ),
        Expr::ListLit { items } => {
            let parts: Vec<String> = items.iter().map(structural_key).collect();
            format!("[{}]", parts.join(" "))
        }
        Expr::Struct { concept, fields } => {
            let parts: Vec<String> = fields.iter().map(|(k, v)| format!("{}:{}", k, structural_key(v))).collect();
            format!("(s/{} {})", concept.0, parts.join(" "))
        }
    }
}

// ---- anti-unification --------------------------------------------------

/// Anti-unify a list of (expression, program_params) into an abstract
/// expression where holes become `Expr::Param{i}`.  Returns None if the
/// result would be trivially equal to one of the inputs.
///
/// `holes` receives one entry per introduced parameter: the concrete leaf
/// expressions and the program.params slice they came from.
fn anti_unify(
    instances: &[(Expr, Vec<Type>)],
    holes: &mut Vec<(Vec<Expr>, Vec<Type>)>,
) -> Option<Expr> {
    if instances.is_empty() {
        return None;
    }
    Some(au_expr(instances, holes))
}

fn au_expr(
    instances: &[(Expr, Vec<Type>)],
    holes: &mut Vec<(Vec<Expr>, Vec<Type>)>,
) -> Expr {
    let first = &instances[0].0;

    // If all are identical constants, keep the constant (no hole needed).
    if let Expr::Const { value: v } = first {
        if instances
            .iter()
            .all(|(e, _)| matches!(e, Expr::Const { value: w } if w == v))
        {
            return Expr::Const { value: v.clone() };
        }
    }

    // If all are Calls with the same action and arity, recurse into args.
    if let Expr::Call { action, args } = first {
        let arity = args.len();
        if instances.iter().all(|(e, _)| {
            matches!(e, Expr::Call { action: a, args: aa } if a == action && aa.len() == arity)
        }) {
            let mut new_args = Vec::with_capacity(arity);
            for i in 0..arity {
                let sub: Vec<(Expr, Vec<Type>)> = instances
                    .iter()
                    .map(|(e, p)| match e {
                        Expr::Call { args: aa, .. } => (aa[i].clone(), p.clone()),
                        _ => unreachable!(),
                    })
                    .collect();
                new_args.push(au_expr(&sub, holes));
            }
            return Expr::Call { action: action.clone(), args: new_args };
        }
    }

    // Otherwise: introduce a hole parameter.
    let hole_exprs: Vec<Expr> = instances.iter().map(|(e, _)| e.clone()).collect();
    let hole_params: Vec<Type> = instances.iter().map(|(_, p)| {
        // Use empty if no params available.
        p.clone().into_iter().collect::<Vec<_>>()
    }).flat_map(|v| v).collect::<Vec<_>>();
    let hole_idx = holes.len();
    holes.push((hole_exprs, hole_params));
    Expr::Param { index: hole_idx }
}

// ---- type helpers -------------------------------------------------------

fn common_leaf_type(exprs: &[Expr], all_params: &[Type]) -> Type {
    let types: Vec<Type> = exprs
        .iter()
        .map(|e| match e {
            Expr::Param { index } => all_params.get(*index).cloned().unwrap_or(Type::Any),
            Expr::Const { value } => value.type_of(),
            _ => Type::Any,
        })
        .collect();
    if types.is_empty() {
        return Type::Any;
    }
    let first = &types[0];
    if types.iter().all(|t| t == first) { first.clone() } else { Type::Any }
}

fn root_output_type(body: &Expr, can: &Can) -> Type {
    match body {
        Expr::Call { action: aid, .. } => {
            can.action(aid).map(|a| a.output.clone()).unwrap_or(Type::Any)
        }
        Expr::Const { value } => value.type_of(),
        _ => Type::Any,
    }
}

// ---- rewriting ---------------------------------------------------------

/// Replace every subtree that matches `shape_key` with a call to `new_id`,
/// threading the hole arguments through.
fn rewrite_expr(
    expr: &Expr,
    shape_key: &str,
    new_id: &ActionId,
    abstract_body: &Expr,
    n_holes: usize,
) -> Expr {
    if structural_key(expr) == shape_key {
        let hole_args = extract_hole_args(abstract_body, expr);
        return Expr::Call { action: new_id.clone(), args: hole_args };
    }
    match expr {
        Expr::Call { action, args } => {
            let new_args: Vec<Expr> = args
                .iter()
                .map(|a| rewrite_expr(a, shape_key, new_id, abstract_body, n_holes))
                .collect();
            Expr::Call { action: action.clone(), args: new_args }
        }
        Expr::Lambda { lambda } => {
            let new_body = rewrite_expr(&lambda.body, shape_key, new_id, abstract_body, n_holes);
            Expr::Lambda {
                lambda: Box::new(spoon_core::types::Lambda {
                    params: lambda.params.clone(),
                    ret: lambda.ret.clone(),
                    body: new_body,
                }),
            }
        }
        Expr::If { cond, then, otherwise } => Expr::If {
            cond: Box::new(rewrite_expr(cond, shape_key, new_id, abstract_body, n_holes)),
            then: Box::new(rewrite_expr(then, shape_key, new_id, abstract_body, n_holes)),
            otherwise: Box::new(rewrite_expr(otherwise, shape_key, new_id, abstract_body, n_holes)),
        },
        other => other.clone(),
    }
}

/// Given the abstract body (with Param{i} as holes) and a concrete subtree,
/// extract the concrete expressions that correspond to each hole.
fn extract_hole_args(pattern: &Expr, concrete: &Expr) -> Vec<Expr> {
    let mut holes: Vec<Option<Expr>> = Vec::new();
    fill_holes(pattern, concrete, &mut holes);
    holes.into_iter().map(|h| h.unwrap_or(Expr::Const { value: Value::Null })).collect()
}

fn fill_holes(pattern: &Expr, concrete: &Expr, holes: &mut Vec<Option<Expr>>) {
    match pattern {
        Expr::Param { index } => {
            while holes.len() <= *index {
                holes.push(None);
            }
            holes[*index] = Some(concrete.clone());
        }
        Expr::Call { args: p_args, .. } => {
            if let Expr::Call { args: c_args, .. } = concrete {
                for (pa, ca) in p_args.iter().zip(c_args.iter()) {
                    fill_holes(pa, ca, holes);
                }
            }
        }
        _ => {}
    }
}
