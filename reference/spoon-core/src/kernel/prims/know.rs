//! World knowledge primitives (know.*). Effect::Network.
//! Queries Wikidata via its public API.

use crate::types::*;

use super::super::{Ctx, EvalError, Kernel};

const USER_AGENT: &str = "spoon/0.1 (https://github.com/kealjones/spoon)";
const WIKIDATA_API: &str = "https://www.wikidata.org/w/api.php";

pub fn register(k: &mut Kernel) {
    use Effect::Network;

    k.register(
        Action::primitive(
            "know.wikidata_search",
            &["search-wikidata", "wikidata-search"],
            vec![Input::required("query", Type::Name)],
            Type::Json,
            Network,
            "search Wikidata and return a JSON list of {id, label, description}",
        ),
        wikidata_search,
    );
    k.register(
        Action::primitive(
            "know.wikidata_describe",
            &["describe-wikidata"],
            vec![Input::required("query", Type::Name)],
            Type::Text,
            Network,
            "describe the top Wikidata result as 'label: description'",
        ),
        wikidata_describe,
    );
}

// ---- internal ----------------------------------------------------------

fn search_wikidata(query: String, timeout: u64) -> Result<serde_json::Value, String> {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        rt.block_on(async {
            let url = format!(
                "{WIKIDATA_API}?action=wbsearchentities&search={}&language=en&format=json&limit=10",
                percent_encode(&query)
            );
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(timeout))
                .user_agent(USER_AGENT)
                .build()
                .map_err(|e| e.to_string())?;
            let resp: serde_json::Value = client.get(&url).send().await
                .map_err(|e| e.to_string())?
                .json().await
                .map_err(|e| e.to_string())?;
            Ok(resp)
        })
    })
    .join()
    .map_err(|_| "wikidata thread panicked".to_string())?
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn extract_hits(resp: &serde_json::Value) -> Vec<serde_json::Value> {
    resp.get("search")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter().map(|item| {
                serde_json::json!({
                    "id": item.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                    "label": item.get("label").and_then(|v| v.as_str()).unwrap_or(""),
                    "description": item.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                })
            }).collect()
        })
        .unwrap_or_default()
}

// ---- primitives --------------------------------------------------------

fn wikidata_search(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("know.wikidata_search".into());
    if !ctx.kernel.sandbox.allow_network {
        return Err(EvalError::Permission { action: id, effect: Effect::Network });
    }
    let query = args.first().and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("name", args.first().unwrap_or(&Value::Null), "know.wikidata_search"))?
        .to_string();
    let timeout = ctx.kernel.sandbox.http_timeout_secs;

    let resp = search_wikidata(query, timeout)
        .map_err(|e| EvalError::runtime(&id, e))?;
    let hits = extract_hits(&resp);
    Ok(Value::Json(serde_json::Value::Array(hits)))
}

fn wikidata_describe(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("know.wikidata_describe".into());
    if !ctx.kernel.sandbox.allow_network {
        return Err(EvalError::Permission { action: id, effect: Effect::Network });
    }
    let query = args.first().and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("name", args.first().unwrap_or(&Value::Null), "know.wikidata_describe"))?
        .to_string();
    let timeout = ctx.kernel.sandbox.http_timeout_secs;

    let resp = search_wikidata(query, timeout)
        .map_err(|e| EvalError::runtime(&id, e))?;
    let hits = extract_hits(&resp);
    let description = hits.first()
        .map(|h| {
            let label = h.get("label").and_then(|v| v.as_str()).unwrap_or("");
            let desc = h.get("description").and_then(|v| v.as_str()).unwrap_or("");
            if desc.is_empty() {
                label.to_string()
            } else {
                format!("{label}: {desc}")
            }
        })
        .unwrap_or_else(|| "no results".to_string());
    Ok(Value::Text(description))
}
