use std::io::{self, BufReader};

use spoon_capability::ResourceBounds;
use spoon_server::{CapabilityHostAdapters, RpcServer, run_stdio};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let http_mode = args.iter().any(|a| a == "--http");
    let http_port: u16 = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(4318);

    let runtime = spoon_server::config::resolve_from_process()?;
    let database = runtime.database_path.to_string_lossy().into_owned();
    let mut server = RpcServer::open(&database)?;
    if let Ok(token) = std::env::var("SPOON_ADMIN_TOKEN") {
        server = server.with_admin_token(token)?;
    }
    if let Ok(identity) = std::env::var("SPOON_FEEDBACK_SOURCE_ID") {
        server = server.with_feedback_source_identity(identity);
    }
    let web_hosts = std::env::var("SPOON_WEB_FETCH_HOSTS").ok().map(|hosts| {
        hosts
            .split(',')
            .map(str::trim)
            .filter(|host| !host.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    });
    let file_root = std::env::var("SPOON_FILE_ROOT").ok();
    if let (Some(root), Some(hosts)) = (file_root.as_deref(), web_hosts.as_ref()) {
        let binding = std::env::var("SPOON_FILE_BINDING").unwrap_or_else(|_| "workspace".into());
        let adapters = CapabilityHostAdapters::with_scoped_files_and_web_fetch(
            binding,
            root,
            ResourceBounds {
                max_bytes: 1024 * 1024,
                max_steps: 16,
                max_millis: 2_000,
            },
            hosts.clone(),
            ResourceBounds {
                max_bytes: 1024 * 1024,
                max_steps: 16,
                max_millis: 10_000,
            },
        )?;
        server = server.with_capability_host_adapters(adapters);
    } else if let Some(root) = file_root.as_deref() {
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
    } else if let Some(hosts) = web_hosts {
        let adapters = CapabilityHostAdapters::with_web_fetch(
            hosts,
            ResourceBounds {
                max_bytes: 1024 * 1024,
                max_steps: 16,
                max_millis: 10_000,
            },
        )?;
        server = server.with_capability_host_adapters(adapters);
    }

    if http_mode {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(spoon_server::http::serve(server, http_port, runtime))?;
    } else {
        run_stdio(
            &mut server,
            BufReader::new(io::stdin().lock()),
            io::stdout().lock(),
        )?;
    }
    Ok(())
}
