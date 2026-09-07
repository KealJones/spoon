//! The world outside the process: the network, the filesystem, stdout.
//!
//! Everything here declares an effect above `Pure`, so the permission layer
//! decides whether it runs. That layer trusts these declarations completely and
//! the evaluator takes the maximum of what a realization claims and what its
//! native says, so understating one is the single thing that actually defeats
//! the safeguard.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use spoon_concept::{Concept, Effect};
use spoon_eval::{ArgStrategy, Arity, Ctx, EvalResult, NativeRegistry, native_error};

use crate::text::want_text;

/// How long a fetch may take. Short on purpose: a request that hangs holds the
/// whole turn, and the evaluator's node budget cannot see time spent inside a
/// native.
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

/// Cap on a response body. Without one, a single fetch can exhaust memory
/// outside any budget the evaluator knows about, because the entire payload is
/// one concept node.
const MAX_BODY: usize = 8 * 1024 * 1024;

/// Blocking HTTP from inside a native.
///
/// The evaluator is synchronous while the binary runs on tokio, and
/// `reqwest::blocking` panics if constructed inside a runtime. Doing the work on
/// a plain thread sidesteps that rather than restructuring evaluation to be
/// async, which would push `.await` into every realization for the sake of the
/// handful that need it.
fn http_get(url: &str) -> Result<(u16, String), String> {
    let url = url.to_string();
    std::thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .user_agent("spoon/0.2")
            .build()
            .map_err(|e| e.to_string())?;
        let response = client.get(&url).send().map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let body = response.text().map_err(|e| e.to_string())?;
        if body.len() > MAX_BODY {
            return Err(format!(
                "response is {} bytes, over the {MAX_BODY} cap",
                body.len()
            ));
        }
        Ok((status, body))
    })
    .join()
    .map_err(|_| "the fetch thread panicked".to_string())?
}

fn fetch(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let url = want_text("io-fetch", &args[0])?;
    let (status, body) = http_get(url).map_err(|e| native_error("io-fetch", e))?;
    if !(200..300).contains(&status) {
        return Err(native_error("io-fetch", format!("{url} answered {status}")));
    }
    Ok(Concept::text(body))
}

/// Fetch and parse in one step, because the pair is what anyone actually wants
/// and keeping them apart means every call site repeats the join.
fn fetch_json(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let url = want_text("io-fetch-json", &args[0])?;
    let (status, body) = http_get(url).map_err(|e| native_error("io-fetch-json", e))?;
    if !(200..300).contains(&status) {
        return Err(native_error(
            "io-fetch-json",
            format!("{url} answered {status}"),
        ));
    }
    let value: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| native_error("io-fetch-json", format!("{url} did not return JSON: {e}")))?;
    Ok(Concept::json(value))
}

/// Reject a path that climbs out of where it started.
///
/// Not a sandbox, and it does not pretend to be one: the permission layer is
/// what actually stands between a learned realization and the filesystem. This
/// only stops a path built from user text walking somewhere surprising through
/// `..`, which is the accident rather than the attack.
fn safe_path(native: &str, raw: &str) -> Result<PathBuf, spoon_eval::EvalError> {
    let path = Path::new(raw);
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(native_error(
            native,
            format!("{raw} climbs out of its directory"),
        ));
    }
    Ok(path.to_path_buf())
}

fn read_file(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let raw = want_text("io-read-file", &args[0])?;
    let path = safe_path("io-read-file", raw)?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| native_error("io-read-file", format!("{}: {e}", path.display())))?;
    Ok(Concept::text(text))
}

fn write_file(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let raw = want_text("io-write-file", &args[0])?;
    let body = want_text("io-write-file", &args[1])?;
    let path = safe_path("io-write-file", raw)?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| native_error("io-write-file", format!("{}: {e}", parent.display())))?;
    }
    std::fs::write(&path, body)
        .map_err(|e| native_error("io-write-file", format!("{}: {e}", path.display())))?;
    // The path comes back rather than a bare true, so a plan can keep using it.
    Ok(Concept::text(raw))
}

fn append_file(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    use std::io::Write;
    let raw = want_text("io-append-file", &args[0])?;
    let body = want_text("io-append-file", &args[1])?;
    let path = safe_path("io-append-file", raw)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| native_error("io-append-file", format!("{}: {e}", path.display())))?;
    file.write_all(body.as_bytes())
        .map_err(|e| native_error("io-append-file", format!("{}: {e}", path.display())))?;
    Ok(Concept::text(raw))
}

fn file_exists(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let raw = want_text("io-file-exists", &args[0])?;
    Ok(Concept::bool(
        safe_path("io-file-exists", raw)
            .map(|p| p.exists())
            .unwrap_or(false),
    ))
}

/// Print, and hand back what was printed so it can keep flowing through a plan.
fn print(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let text = match args[0].as_ground() {
        Some(spoon_concept::Ground::Text(t)) => t.to_string(),
        Some(other) => format!("{other:?}"),
        None => format!("{:?}", args[0]),
    };
    println!("{text}");
    Ok(args[0].clone())
}

pub fn register(registry: &mut NativeRegistry) {
    registry.register(
        "io-fetch",
        fetch,
        Arity::Exact(1),
        ArgStrategy::Eager,
        Effect::Network,
        "fetch a URL and return the body as text",
    );
    registry.register(
        "io-fetch-json",
        fetch_json,
        Arity::Exact(1),
        ArgStrategy::Eager,
        Effect::Network,
        "fetch a URL and parse the body as JSON",
    );
    registry.register(
        "io-read-file",
        read_file,
        Arity::Exact(1),
        ArgStrategy::Eager,
        Effect::Read,
        "read a file as text",
    );
    registry.register(
        "io-write-file",
        write_file,
        Arity::Exact(2),
        ArgStrategy::Eager,
        Effect::Write,
        "write text to a file, creating directories as needed",
    );
    registry.register(
        "io-append-file",
        append_file,
        Arity::Exact(2),
        ArgStrategy::Eager,
        Effect::Write,
        "append text to a file",
    );
    registry.register(
        "io-file-exists",
        file_exists,
        Arity::Exact(1),
        ArgStrategy::Eager,
        Effect::Read,
        "whether a path exists",
    );
    registry.register(
        "io-print",
        print,
        Arity::Exact(1),
        ArgStrategy::Eager,
        Effect::Write,
        "print a value and pass it through",
    );
}
