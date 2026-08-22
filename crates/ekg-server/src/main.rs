use std::io::{self, BufReader};

use ekg_server::{RpcServer, run_stdio};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database = std::env::var("EKG_DB").unwrap_or_else(|_| "ekg.db".into());
    let server = RpcServer::open(&database)?;
    run_stdio(
        &server,
        BufReader::new(io::stdin().lock()),
        io::stdout().lock(),
    )?;
    Ok(())
}
