use std::sync::Arc;
use spoon_mind::brain::Brain;

pub async fn run(brain: Arc<Brain>) -> anyhow::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let session_id = format!("repl-{}", uuid::Uuid::new_v4());
    // Toggle independently of cfg so `:debug` works at runtime.
    let mut debug = brain.cfg.debug;

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    loop {
        stdout.write_all(b"you> ").await?;
        stdout.flush().await?;

        match lines.next_line().await? {
            None => break, // EOF / Ctrl-D
            Some(line) => {
                let line = line.trim().to_string();
                match line.as_str() {
                    "" => continue,
                    ":q" | ":quit" => break,
                    ":metrics" => {
                        let m = brain.metrics();
                        let json = serde_json::to_string_pretty(&m)?;
                        stdout.write_all(json.as_bytes()).await?;
                        stdout.write_all(b"\n").await?;
                    }
                    ":snapshot" => {
                        let s = brain.snapshot();
                        let json = serde_json::to_string_pretty(&s)?;
                        stdout.write_all(json.as_bytes()).await?;
                        stdout.write_all(b"\n").await?;
                    }
                    ":debug" => {
                        debug = !debug;
                        let msg = if debug { "debug on\n" } else { "debug off\n" };
                        stdout.write_all(msg.as_bytes()).await?;
                    }
                    text => match brain.turn(&session_id, text).await {
                        Ok(result) => {
                            if debug {
                                for t in &result.trace {
                                    stdout.write_all(format!("  | {t}\n").as_bytes()).await?;
                                }
                            }
                            stdout
                                .write_all(format!("spoon> {}\n", result.text).as_bytes())
                                .await?;
                            if debug {
                                stdout
                                    .write_all(
                                        format!(
                                            "  | path={} ms={}\n",
                                            result.mouth_path, result.episode.metrics.ms_mouth
                                        )
                                        .as_bytes(),
                                    )
                                    .await?;
                            }
                        }
                        Err(e) => {
                            stdout
                                .write_all(format!("spoon> error: {e}\n").as_bytes())
                                .await?;
                        }
                    },
                }
                stdout.flush().await?;
            }
        }
    }

    Ok(())
}
