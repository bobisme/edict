# Build the Rust binary
build:
    cargo build

# Run tests
test:
    cargo test

# Install the binary to ~/.cargo/bin
install:
    cargo install --path . --locked

# Lint with clippy
lint:
    cargo clippy --all-targets -- -D warnings

# Format with rustfmt
fmt:
    cargo fmt

# Check types without building
check:
    cargo check

# e.g. just eval worker current sonnet smoke
# Run one hermetic agent eval (real model calls). See evals/README.md.
eval scenario variant="current" model="sonnet" label="run":
    EDICT_AGENTS_VARIANT={{variant}} EVAL_MODEL={{model}} EVAL_LABEL={{label}} evals/scripts/run.sh {{scenario}}

# Eval harness self-test with a scripted agent (no model calls)
eval-selftest scenario="worker":
    evals/scripts/selftest.sh {{scenario}}
