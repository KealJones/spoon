use std::path::Path;
use anyhow::Context;
use spoon_core::store::{Seed, Store};

pub async fn export(db_path: Option<&Path>, name: &str, out: Option<&Path>) -> anyhow::Result<()> {
    let store = open_store(db_path)?;
    let seed = store.export_seed(name)?;
    let json = serde_json::to_string_pretty(&seed)?;
    match out {
        Some(path) => std::fs::write(path, &json).with_context(|| format!("writing {}", path.display()))?,
        None => println!("{json}"),
    }
    Ok(())
}

pub async fn import(db_path: Option<&Path>, file: &Path) -> anyhow::Result<()> {
    let json = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let seed: Seed = serde_json::from_str(&json).context("parsing seed JSON")?;
    let store = open_store(db_path)?;
    let written = store.import_seed(&seed)?;
    println!("imported: {written} records written");
    Ok(())
}

fn open_store(db_path: Option<&Path>) -> anyhow::Result<Store> {
    match db_path {
        Some(p) => {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir)?;
            }
            Store::open(p)
        }
        None => Store::open_memory(),
    }
}
