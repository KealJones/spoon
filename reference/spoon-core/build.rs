use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");

    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() { Some(o.stdout) } else { None })
        .and_then(|b| String::from_utf8(b).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    // Use a timestamp so dirty builds are distinguished from each other.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| {
            let secs = d.as_secs();
            let h = (secs / 3600) % 24;
            let m = (secs / 60) % 60;
            format!("{h:02}:{m:02}")
        })
        .unwrap_or_else(|_| "??:??".to_string());

    let build_id = if dirty {
        format!("{sha}-dirty@{ts}")
    } else {
        sha
    };

    println!("cargo:rustc-env=SPOON_BUILD_ID={build_id}");
}
