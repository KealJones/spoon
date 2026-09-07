default:
    @just --list

# ---- build and test ----

check:
    cargo check --workspace --all-targets

test:
    cargo test --workspace

test-crate crate:
    cargo test -p {{crate}}

lint:
    cargo clippy --workspace --all-targets -- -D warnings

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

# Everything CI would run.
ci: fmt-check lint test

# ---- run ----

serve port="8787" host="127.0.0.1":
    cargo run -p spoon -- serve --port {{port}} --host {{host}}

# ---- v1 reference ----

# The v1 tree lives in reference/ and is deliberately outside the workspace so
# it stays readable without being built. Point cargo at it explicitly to run it.
v1-repl:
    cargo run --manifest-path reference/spoon/Cargo.toml -- --ephemeral --offline repl
