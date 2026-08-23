use std::io::{self, BufReader};

use spoon_capability::ResourceBounds;
use spoon_server::{CapabilityHostAdapters, RpcServer, run_stdio};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database = std::env::var("SPOON_DB").unwrap_or_else(|_| "spoon.db".into());
    let mut server = RpcServer::open(&database)?;
    if let Ok(token) = std::env::var("SPOON_ADMIN_TOKEN") {
        server = server.with_admin_token(token)?;
    }
    if let Ok(identity) = std::env::var("SPOON_FEEDBACK_SOURCE_ID") {
        server = server.with_feedback_source_identity(identity);
    }
    if let Ok(root) = std::env::var("SPOON_FILE_ROOT") {
        let binding = std::env::var("SPOON_FILE_BINDING").unwrap_or_else(|_| "workspace".into());
        let adapters = CapabilityHostAdapters::with_scoped_files(
            binding,
            root,
            ResourceBounds {
                max_bytes: 1024 * 1024,
                max_steps: 16,
                max_millis: 2_000,
            },
        )?;
        server = server.with_capability_host_adapters(adapters);
    }
    run_stdio(
        &mut server,
        BufReader::new(io::stdin().lock()),
        io::stdout().lock(),
    )?;
    Ok(())
}
