//! Dialog move primitives (dialog.*).
//!
//! Each constructor returns Value::Json whose serde tag matches Move::<Variant>.
//! The orchestrator wraps these in a ResponsePlan; the mouth renders them.
//! Effect::Pure - they are pure constructors, not side effects.

use crate::types::*;

use super::super::{Ctx, EvalError, Kernel};

pub fn register(k: &mut Kernel) {
    use Effect::Pure;
    use Role::Dialog;

    let dm = || Type::Concept(ConceptId("dialog.Move".into()));
    let t = || Type::Text;
    let lt = || Type::list(Type::Text);

    macro_rules! p {
        ($id:expr, $verbs:expr, $inputs:expr, $desc:expr, $f:expr) => {
            k.register(
                Action::primitive($id, $verbs, $inputs, dm(), Pure, $desc).with_role(Dialog),
                $f,
            )
        };
    }

    p!("dialog.greet",    &["greet"],      vec![Input::required("returning", Type::Bool)], "greet the user", dialog_greet);
    p!("dialog.farewell", &["farewell", "say-goodbye"], vec![], "say goodbye", dialog_farewell);
    p!("dialog.thanks",   &["thank"],      vec![], "express thanks", dialog_thanks);
    p!("dialog.ack",      &["acknowledge"],vec![Input::required("summary", t())], "acknowledge a user statement with a summary", dialog_ack);
    p!("dialog.empathize",&["empathize"],  vec![Input::required("feeling", t()), Input::required("about", t())], "express empathy for a feeling about a topic", dialog_empathize);
    p!("dialog.reflect",  &["reflect"],    vec![Input::required("summary", t())], "reflect the user's situation back", dialog_reflect);
    p!("dialog.clarify",  &["clarify"],    vec![Input::required("question", t()), Input::required("options", lt())], "ask a clarifying question with options", dialog_clarify);
    p!("dialog.refuse",   &["refuse"],     vec![Input::required("reason", t())], "refuse a request with a reason", dialog_refuse);
    p!("dialog.explain",  &["explain"],    vec![Input::required("text", t())], "explain something in text", dialog_explain);
    p!("dialog.opinion",  &["opine"],      vec![
        Input::required("topic", t()),
        Input::required("stance", t()),
        Input::required("reasons", lt()),
        Input::required("confidence", Type::Float),
    ], "state an opinion with reasons and confidence", dialog_opinion);
    p!("dialog.advise",   &["advise"],     vec![
        Input::required("situation", t()),
        Input::required("options", Type::Json),
        Input::required("leaning", t()),
    ], "give advice with options and a recommended direction", dialog_advise);
    p!("dialog.cannot_do",&["cannot-do"],  vec![Input::required("what", t()), Input::required("reason", t())], "say Spoon cannot do something and why", dialog_cannot_do);
    p!("dialog.learned",  &["learned"],    vec![Input::required("what", t())], "announce that Spoon learned something", dialog_learned);
}

// ---- helpers -----------------------------------------------------------

fn to_json_move(m: &Move) -> Result<Value, EvalError> {
    serde_json::to_value(m)
        .map(Value::Json)
        .map_err(|e| EvalError::Other(format!("dialog serialization error: {e}")))
}

fn text_arg<'a>(id: &str, args: &'a [Value], i: usize) -> Result<&'a str, EvalError> {
    args.get(i).and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("text", args.get(i).unwrap_or(&Value::Null), id))
}

fn bool_arg(id: &str, args: &[Value], i: usize) -> Result<bool, EvalError> {
    args.get(i).and_then(Value::as_bool)
        .ok_or_else(|| EvalError::ty("bool", args.get(i).unwrap_or(&Value::Null), id))
}

// ---- primitives --------------------------------------------------------

fn dialog_greet(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let returning = bool_arg("dialog.greet", args, 0)?;
    to_json_move(&Move::Greet { returning })
}

fn dialog_farewell(_ctx: &mut Ctx<'_>, _args: &[Value]) -> Result<Value, EvalError> {
    to_json_move(&Move::Farewell)
}

fn dialog_thanks(_ctx: &mut Ctx<'_>, _args: &[Value]) -> Result<Value, EvalError> {
    to_json_move(&Move::Thanks)
}

fn dialog_ack(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let summary = text_arg("dialog.ack", args, 0)?.to_string();
    to_json_move(&Move::Ack { summary })
}

fn dialog_empathize(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let feeling = text_arg("dialog.empathize", args, 0)?.to_string();
    let about = text_arg("dialog.empathize", args, 1)?.to_string();
    to_json_move(&Move::Empathize { feeling, about })
}

fn dialog_reflect(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let summary = text_arg("dialog.reflect", args, 0)?.to_string();
    to_json_move(&Move::Reflect { summary })
}

fn dialog_clarify(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let question = text_arg("dialog.clarify", args, 0)?.to_string();
    let opts = args.get(1).and_then(Value::as_list)
        .ok_or_else(|| EvalError::ty("list", args.get(1).unwrap_or(&Value::Null), "dialog.clarify"))?;
    let options: Vec<String> = opts.iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    to_json_move(&Move::Clarify { question, options, slot_type: None })
}

fn dialog_refuse(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let reason = text_arg("dialog.refuse", args, 0)?.to_string();
    to_json_move(&Move::Refuse { reason })
}

fn dialog_explain(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let text = text_arg("dialog.explain", args, 0)?.to_string();
    to_json_move(&Move::Explain { text })
}

fn dialog_opinion(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let topic = text_arg("dialog.opinion", args, 0)?.to_string();
    let stance = text_arg("dialog.opinion", args, 1)?.to_string();
    let reasons_list = args.get(2).and_then(Value::as_list)
        .ok_or_else(|| EvalError::ty("list", args.get(2).unwrap_or(&Value::Null), "dialog.opinion"))?;
    let reasons: Vec<String> = reasons_list.iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let confidence = args.get(3).and_then(Value::as_f64)
        .ok_or_else(|| EvalError::ty("float", args.get(3).unwrap_or(&Value::Null), "dialog.opinion"))? as f32;
    to_json_move(&Move::Opinion { topic, stance, reasons, confidence })
}

fn dialog_advise(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let situation = text_arg("dialog.advise", args, 0)?.to_string();
    let opts_json = match args.get(1) {
        Some(Value::Json(j)) => j.clone(),
        other => return Err(EvalError::ty("json", other.unwrap_or(&Value::Null), "dialog.advise")),
    };
    let leaning = text_arg("dialog.advise", args, 2)?.to_string();
    let options: Vec<AdviceOption> = serde_json::from_value(opts_json).unwrap_or_default();
    to_json_move(&Move::Advise { situation, options, leaning: Some(leaning) })
}

fn dialog_cannot_do(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let what = text_arg("dialog.cannot_do", args, 0)?.to_string();
    let reason = text_arg("dialog.cannot_do", args, 1)?.to_string();
    to_json_move(&Move::CannotDo { what, reason })
}

fn dialog_learned(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let what = text_arg("dialog.learned", args, 0)?.to_string();
    to_json_move(&Move::Learned { what })
}
