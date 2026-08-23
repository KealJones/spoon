use std::io::{self, BufReader};

use spoon_server::{RpcServer, run_stdio};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database = std::env::var("SPOON_DB").unwrap_or_else(|_| "spoon.db".into());
    let mut server = RpcServer::open(&database)?;
    if let Ok(token) = std::env::var("SPOON_ADMIN_TOKEN") {
        server = server.with_admin_token(token)?;
    }
    if let Ok(identity) = std::env::var("SPOON_FEEDBACK_SOURCE_ID") {
        server = server.with_feedback_source_identity(identity);
    }
    run_stdio(
        &mut server,
        BufReader::new(io::stdin().lock()),
        io::stdout().lock(),
    )?;
    Ok(())
}
