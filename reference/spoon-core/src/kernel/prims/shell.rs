//! Shell primitive (shell.*). Effect::Shell.
//!
//! Uses a channel-based timeout: the command runs in a thread and the main
//! thread waits with recv_timeout. If it times out, we give up on the thread
//! (the process orphans, which is acceptable for Stage 0; a production version
//! would track the pid and kill it).

use crate::types::*;

use super::super::{Ctx, EvalError, Kernel};

pub fn register(k: &mut Kernel) {
    k.register(
        Action::primitive(
            "shell.run",
            &["run", "execute"],
            vec![Input::required("command", Type::Text)],
            Type::Text,
            Effect::Shell,
            "run a shell command, returns combined stdout+stderr with exit code on last line",
        ),
        shell_run,
    );
}

fn shell_run(ctx: &mut Ctx<'_>, args: &[Value]) -> Result<Value, EvalError> {
    let id = ActionId("shell.run".into());
    if !ctx.kernel.sandbox.allow_shell {
        return Err(EvalError::Permission { action: id, effect: Effect::Shell });
    }
    let cmd = args.first().and_then(Value::as_str)
        .ok_or_else(|| EvalError::ty("text", args.first().unwrap_or(&Value::Null), "shell.run"))?
        .to_string();
    let timeout_secs = ctx.kernel.sandbox.shell_timeout_secs;

    let (tx, rx) = std::sync::mpsc::channel::<Result<std::process::Output, String>>();
    std::thread::spawn(move || {
        let result = std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .output()
            .map_err(|e| e.to_string());
        let _ = tx.send(result);
    });

    let output = match rx.recv_timeout(std::time::Duration::from_secs(timeout_secs)) {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(EvalError::runtime(&id, e)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            return Err(EvalError::runtime(&id, format!("command timed out after {timeout_secs}s")));
        }
        Err(_) => return Err(EvalError::runtime(&id, "shell thread disconnected")),
    };

    let mut result = String::from_utf8_lossy(&output.stdout).into_owned();
    result.push_str(&String::from_utf8_lossy(&output.stderr));
    result.push_str(&format!("\nexit: {}", output.status.code().unwrap_or(-1)));
    Ok(Value::Text(result))
}
