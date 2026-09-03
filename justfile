default:
    @just --list

repl:
    cargo run -p spoon -- repl

repl-offline:
    cargo run -p spoon -- --ephemeral --offline repl

serve port="8787" host="127.0.0.1":
    cargo run -p spoon -- serve --port {{port}} --host {{host}}

stdio:
    cargo run -q -p spoon -- --offline stdio

bench-demo:
    cargo run -p spoon -- --offline bench demo

bench suite="convo20":
    cargo run -p spoon -- bench {{suite}}

teach lessons="6":
    cargo run -p spoon -- teach --lessons {{lessons}}

export out="seed.json":
    cargo run -p spoon -- export --out {{out}}

import file="seed.json":
    cargo run -p spoon -- import {{file}}

test:
    cargo test --workspace

build:
    cargo build --workspace

check:
    cargo check --workspace

fmt:
    cargo fmt --all

clippy:
    cargo clippy --workspace --all-targets
