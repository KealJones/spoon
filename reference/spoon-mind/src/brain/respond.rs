//! Response building helpers for the brain turn loop.

use spoon_core::can::Can;
use spoon_core::types::*;

use crate::dispatch::Present;

/// Present a finished plan's value the way the dispatcher asked: a command
/// result, a computed answer, or a yes/no comparison.
pub fn present_result(
    present: &Present,
    value: Value,
    action_id: &ActionId,
    steps: usize,
    moves_before: Vec<Move>,
) -> ResponsePlan {
    match present {
        Present::Result => exec_result_plan(value, action_id, steps, moves_before),
        Present::Answer { question } => {
            let mut moves = moves_before;
            moves.push(Move::Answer { question: question.clone(), values: vec![value], source: Some("computed".into()) });
            ResponsePlan::new(moves)
        }
        Present::YesNo { question, subject, want } => {
            let mut moves = moves_before;
            moves.push(Move::YesNo {
                question: question.clone(),
                answer: values_match(&value, want),
                because: Some(format!("{subject} is {}", value.render())),
            });
            ResponsePlan::new(moves)
        }
    }
}

/// Equality as a user means it: 6 == 6.0, "Wednesday" == Wednesday.
fn values_match(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < 1e-9,
        _ => match (a.as_str(), b.as_str()) {
            (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
            _ => a == b,
        },
    }
}

/// Build a ResponsePlan from executor outcome, merging accumulated moves.
pub fn exec_result_plan(
    value: Value,
    action_id: &ActionId,
    steps: usize,
    moves_before: Vec<Move>,
) -> ResponsePlan {
    let mut moves = moves_before;
    // dialog.* primitives return Value::Json that deserializes into Move.
    if let Value::Json(ref j) = value {
        if let Ok(m) = serde_json::from_value::<Move>(j.clone()) {
            moves.push(m);
            return ResponsePlan::new(moves);
        }
    }
    moves.push(Move::Result {
        action: action_id.clone(),
        value,
        steps,
    });
    ResponsePlan::new(moves)
}

/// Permission answer heuristic.
pub fn parse_permission(text: &str) -> Option<bool> {
    let lower = text.trim().to_lowercase();
    let stripped = lower
        .trim_end_matches(|c: char| ".!?".contains(c))
        .trim();
    let yes = [
        "yes", "y", "ok", "okay", "sure", "go ahead", "go", "do it", "yep",
        "yup", "yeah", "affirmative", "proceed",
    ];
    let no = [
        "no", "n", "nope", "stop", "cancel", "abort", "don't", "nah",
        "negative",
    ];
    if yes.contains(&stripped) {
        return Some(true);
    }
    if no.contains(&stripped) {
        return Some(false);
    }
    None
}

/// Choice answer: try to parse a 1-based number.
pub fn parse_choice(text: &str) -> Option<usize> {
    text.trim()
        .parse::<usize>()
        .ok()
        .filter(|&n| n >= 1)
        .map(|n| n - 1)
}

/// A user's answer to an `Elicit` for a text input: drop surrounding quotes
/// and a sentence-final period so `"cd".` becomes `cd`.
pub fn elicited_text(text: &str) -> String {
    let t = text.trim().trim_end_matches('.').trim();
    let t = t.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(t);
    t.to_string()
}

// ---------------------------------------------------------------------------
// Program pretty-printer
// ---------------------------------------------------------------------------

const PARAM_NAMES: [&str; 6] = ["x", "y", "z", "u", "v", "w"];
const LAMBDA_NAMES: [&str; 3] = ["a", "b", "c"];

/// Human-readable program description: `(math.add p0 p0)` -> `add(x, x)`.
/// Learned actions render by their canonical verb, kernel ones lose their
/// namespace, params become x, y, z.
pub fn describe_program(program: &Program, can: &Can) -> String {
    describe_expr(&program.body, can)
}

fn action_name(id: &ActionId, can: &Can) -> String {
    if let Some(a) = can.action(id) {
        if a.tier != Tier::Kernel {
            return a.canonical_verb().to_string();
        }
    }
    id.0.rsplit('.').next().unwrap_or(&id.0).to_string()
}

fn describe_expr(expr: &Expr, can: &Can) -> String {
    match expr {
        Expr::Const { value: Value::Text(s) } => format!("\"{s}\""),
        Expr::Const { value } => value.render(),
        Expr::Param { index } => {
            PARAM_NAMES.get(*index).map(|s| s.to_string()).unwrap_or_else(|| format!("x{index}"))
        }
        Expr::LambdaParam { index } => {
            LAMBDA_NAMES.get(*index).map(|s| s.to_string()).unwrap_or_else(|| format!("a{index}"))
        }
        Expr::Call { action, args } => {
            let parts: Vec<String> = args.iter().map(|a| describe_expr(a, can)).collect();
            format!("{}({})", action_name(action, can), parts.join(", "))
        }
        Expr::Lambda { lambda } => {
            let params: Vec<&str> =
                (0..lambda.params.len()).map(|i| LAMBDA_NAMES.get(i).copied().unwrap_or("_")).collect();
            format!("({}) -> {}", params.join(", "), describe_expr(&lambda.body, can))
        }
        Expr::If { cond, then, otherwise } => format!(
            "if {} then {} else {}",
            describe_expr(cond, can),
            describe_expr(then, can),
            describe_expr(otherwise, can)
        ),
        Expr::ListLit { items } => {
            let parts: Vec<String> = items.iter().map(|i| describe_expr(i, can)).collect();
            format!("[{}]", parts.join(", "))
        }
        Expr::Struct { concept, fields } => {
            let parts: Vec<String> =
                fields.iter().map(|(k, v)| format!("{k}: {}", describe_expr(v, can))).collect();
            format!("{}{{{}}}", concept.0, parts.join(", "))
        }
        Expr::Field { of, name } => format!("{}.{name}", describe_expr(of, can)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_prints_kernel_and_learned_calls() {
        let mut can = Can::new();
        let mut learned = Action::primitive("learned.double_abcd", &["double"], vec![], Type::Float, Effect::Pure, "");
        learned.tier = Tier::Provisional;
        can.add_action(learned);
        let body = Expr::call(
            "math.add",
            vec![Expr::param(0), Expr::call("learned.double_abcd", vec![Expr::param(0)])],
        );
        let program = Program::new(vec![Type::Float], Type::Float, body);
        assert_eq!(describe_program(&program, &can), "add(x, double(x))");
    }

    #[test]
    fn elicited_text_strips_quotes_and_period() {
        assert_eq!(elicited_text("\"cd\"."), "cd");
        assert_eq!(elicited_text(" , "), ",");
    }

    #[test]
    fn yes_no_presentation_compares_like_a_user() {
        assert!(values_match(&Value::Float(6.0), &Value::Int(6)));
        assert!(!values_match(&Value::Float(6.0), &Value::Int(7)));
        assert!(values_match(&Value::text("Wednesday"), &Value::name("wednesday")));
        let present = Present::YesNo { question: "Is the double of 3 6?".into(), subject: "the double of 3".into(), want: Value::Int(6) };
        let plan = present_result(&present, Value::Float(6.0), &ActionId("learned.double".into()), 1, vec![]);
        assert!(
            matches!(plan.moves.as_slice(), [Move::YesNo { answer: true, because: Some(b), .. }] if b == "the double of 3 is 6"),
            "got {:?}",
            plan.moves
        );
        let present = Present::Answer { question: "What is the double of 100?".into() };
        let plan = present_result(&present, Value::Float(200.0), &ActionId("learned.double".into()), 1, vec![]);
        assert!(
            matches!(plan.moves.as_slice(), [Move::Answer { source: Some(s), .. }] if s == "computed"),
            "got {:?}",
            plan.moves
        );
    }
}
