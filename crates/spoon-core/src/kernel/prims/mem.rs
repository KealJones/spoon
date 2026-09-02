//! Memory primitives (mem.*): recall from episodic memory and current time.

use crate::types::*;

use super::super::{Ctx, EvalError, Kernel};

pub fn register(k: &mut Kernel) {
    k.register(
        Action::primitive(
            "mem.recall",
            &["recall"],
            vec![Input::required("query", Type::Text)],
            Type::list(Type::Text),
            Effect::Read,
            "recall recent or relevant memory fragments matching a query",
        ),
        mem_recall,
    );
    k.register(
        Action::primitive(
            "mem.facts_about",
            &["facts-about"],
            vec![Input::required("name", Type::Name)],
            Type::list(Type::Text),
            Effect::Read,
            "recall stored facts about a named entity",
        ),
        mem_facts_about,
    );
    k.register(
        Action::primitive(
            "mem.now_ms",
            &["now-ms"],
            vec![],
            Type::Int,
            Effect::Read,
            "current Unix timestamp in milliseconds",
        ),
        mem_now_ms,
    );
}

fn mem_recall(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let query = args.first().and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("text", args.first().unwrap_or(&Value::Null), "mem.recall"))?;
    let results = ctx.host.recall(query, 8);
    Ok(Value::List(results.into_iter().map(Value::Text).collect()))
}

fn mem_facts_about(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let name = args.first().and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("name", args.first().unwrap_or(&Value::Null), "mem.facts_about"))?;
    let query = format!("facts about {name}");
    let results = ctx.host.recall(&query, 8);
    Ok(Value::List(results.into_iter().map(Value::Text).collect()))
}

fn mem_now_ms(ctx: &mut Ctx<'_>, _args: &[Value]) -> Result<Value, EvalError> {
    Ok(Value::Int(ctx.host.now_ms()))
}
