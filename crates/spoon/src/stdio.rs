use std::sync::Arc;
use serde::Deserialize;
use serde_json::json;
use spoon_mind::brain::Brain;

#[derive(Deserialize)]
struct StdioInput {
    #[serde(default = "default_session")]
    session: String,
    text: String,
}

fn default_session() -> String {
    "stdio".into()
}

/// Process one JSON line and return a JSON line response.
/// Exported so tests can call it directly.
pub async fn handle_line(brain: &Brain, line: &str) -> String {
    let input: StdioInput = match serde_json::from_str(line.trim()) {
        Ok(v) => v,
        Err(e) => {
            return json!({"error": e.to_string()}).to_string();
        }
    };

    match brain.turn(&input.session, &input.text).await {
        Ok(result) => {
            let out = json!({
                "text": result.text,
                "episode_id": result.episode.id,
                "mouth_path": result.mouth_path,
                "metrics": result.episode.metrics,
                "trace": result.trace,
            });
            out.to_string()
        }
        Err(e) => json!({"error": e.to_string()}).to_string(),
    }
}

pub async fn run(brain: Arc<Brain>) -> anyhow::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_line(&brain, &line).await;
        stdout.write_all(response.as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
    }

    Ok(())
}
