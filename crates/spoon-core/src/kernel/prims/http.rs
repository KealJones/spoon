//! HTTP primitives (http.*). Effect::Network.
//!
//! HTTP calls are run on a fresh std::thread so they never panic inside a
//! tokio runtime context. reqwest's async API is driven by a per-call
//! single-threaded tokio runtime on that thread.

use crate::types::*;

use super::super::{Ctx, EvalError, Kernel};

pub fn register(k: &mut Kernel) {
    use Effect::Network;

    macro_rules! p {
        ($id:expr, $verbs:expr, $inputs:expr, $out:expr, $desc:expr, $f:expr) => {
            k.register(Action::primitive($id, $verbs, $inputs, $out, Network, $desc), $f)
        };
    }

    p!("http.get",       &["http-get"],      vec![Input::required("url", Type::Url)], Type::Text, "HTTP GET, returns response body as text",       http_get);
    p!("http.get_json",  &["http-get-json"], vec![Input::required("url", Type::Url)], Type::Json, "HTTP GET, parses response body as JSON",         http_get_json);
    p!("http.post_json", &["http-post-json"],vec![Input::required("url", Type::Url), Input::required("body", Type::Json)], Type::Json, "HTTP POST JSON body, returns JSON response", http_post_json);
    k.register(
        Action::primitive("http.url_encode", &["url-encode"], vec![Input::required("text", Type::Text)], Type::Text, Effect::Pure, "URL-encode a string (percent-encoding)"),
        http_url_encode,
    );
}

// ---- async HTTP via fresh thread ---------------------------------------

fn run_in_thread<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    std::thread::spawn(f)
        .join()
        .map_err(|_| "HTTP thread panicked".to_string())?
}

fn make_client(timeout_secs: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| e.to_string())
}

fn new_rt() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

// ---- primitives --------------------------------------------------------

fn http_get(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("http.get".into());
    if !ctx.kernel.sandbox.allow_network {
        return Err(EvalError::Permission { action: id, effect: Effect::Network });
    }
    let url = url_arg("http.get", args, 0)?;
    let timeout = ctx.kernel.sandbox.http_timeout_secs;

    let result = run_in_thread(move || {
        let rt = new_rt()?;
        rt.block_on(async {
            let client = make_client(timeout)?;
            client.get(&url).send().await.map_err(|e| e.to_string())?
                .text().await.map_err(|e| e.to_string())
        })
    });
    result.map(Value::Text).map_err(|e| EvalError::runtime(&id, e))
}

fn http_get_json(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("http.get_json".into());
    if !ctx.kernel.sandbox.allow_network {
        return Err(EvalError::Permission { action: id, effect: Effect::Network });
    }
    let url = url_arg("http.get_json", args, 0)?;
    let timeout = ctx.kernel.sandbox.http_timeout_secs;

    let result = run_in_thread(move || {
        let rt = new_rt()?;
        rt.block_on(async {
            let client = make_client(timeout)?;
            let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
            resp.json::<serde_json::Value>().await.map_err(|e| e.to_string())
        })
    });
    result.map(Value::Json).map_err(|e| EvalError::runtime(&id, e))
}

fn http_post_json(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("http.post_json".into());
    if !ctx.kernel.sandbox.allow_network {
        return Err(EvalError::Permission { action: id, effect: Effect::Network });
    }
    let url = url_arg("http.post_json", args, 0)?;
    let body = match args.get(1) {
        Some(Value::Json(j)) => j.clone(),
        other => return Err(EvalError::ty("json", other.unwrap_or(&Value::Null), "http.post_json")),
    };
    let timeout = ctx.kernel.sandbox.http_timeout_secs;

    let result = run_in_thread(move || {
        let rt = new_rt()?;
        rt.block_on(async {
            let client = make_client(timeout)?;
            let resp = client.post(&url).json(&body).send().await.map_err(|e| e.to_string())?;
            resp.json::<serde_json::Value>().await.map_err(|e| e.to_string())
        })
    });
    result.map(Value::Json).map_err(|e| EvalError::runtime(&id, e))
}

fn http_url_encode(_ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let s = args.first().and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("text", args.first().unwrap_or(&Value::Null), "http.url_encode"))?;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    Ok(Value::Text(out))
}

// ---- helper -----------------------------------------------------------

fn url_arg(id: &str, args: &[Value], i: usize) -> Result<String, EvalError> {
    match args.get(i) {
        Some(Value::Url(s)) | Some(Value::Text(s)) => Ok(s.clone()),
        other => Err(EvalError::ty("url", other.unwrap_or(&Value::Null), id)),
    }
}
