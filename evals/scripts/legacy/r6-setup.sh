#!/usr/bin/env bash
set -euo pipefail

# R6 Parallel Dispatch Eval — Setup
# Creates a fresh eval environment with a Rust/Axum project and 3 independent,
# pre-groomed bones. The dev agent must dispatch Haiku workers in parallel
# rather than doing the work sequentially.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

EVAL_DIR=$(mktemp -d)
cd "$EVAL_DIR"
echo "EVAL_DIR=$EVAL_DIR"

# --- Init repo and botbox ---
jj git init
botbox init --name r6-eval --type api --tools bones,maw,crit,botbus,vessel --init-bones --no-interactive

# --- Copy latest local workflow docs (installed package may be stale) ---
cp "$REPO_DIR/packages/cli/docs/"*.md .agents/botbox/

# --- Init Rust project ---
cargo init --name r6-eval

# --- Write Cargo.toml ---
cat > Cargo.toml << 'CARGO_EOF'
[package]
name = "r6-eval"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.8"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
CARGO_EOF

# --- Write minimal src/main.rs with AppState and /health ---
cat > src/main.rs << 'RUST_EOF'
use axum::{routing::get, Router};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

#[derive(Clone)]
pub struct AppState {
    pub request_count: Arc<AtomicU64>,
}

async fn health() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() {
    let state = AppState {
        request_count: Arc::new(AtomicU64::new(0)),
    };

    let app = Router::new()
        .route("/health", get(health))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("bind");
    println!("Listening on :3000");
    axum::serve(listener, app).await.expect("serve");
}
RUST_EOF

# --- Verify it compiles ---
cargo check 2>&1
echo "cargo check: OK"

# --- Init crit and maw ---
crit init
maw init

# --- Generate dev agent identity ---
DEV_AGENT=$(bus generate-name)
echo "DEV_AGENT=$DEV_AGENT"

# --- Register on channel ---
bus send --agent setup r6-eval "R6 eval environment initialized" -L mesh -L setup
bus mark-read --agent "$DEV_AGENT" r6-eval

# --- Create 3 independent, pre-groomed bones ---

BEAD1=$(bn create \
  --title "Add GET /version endpoint" \
  --description "$(cat << 'DESC_EOF'
Add a GET /version endpoint that returns the application version as JSON.

## Requirements

- GET /version returns 200 with JSON body: {"name": "r6-eval", "version": "0.1.0"}
- Create a new module src/version.rs with the handler
- Add the route to the Router in src/main.rs

## Acceptance Criteria

- Endpoint returns 200 with correct JSON
- cargo check passes

## Testing

Verify with: curl http://localhost:3000/version
Expected: {"name":"r6-eval","version":"0.1.0"}
DESC_EOF
)" \
  --kind task)
echo "BEAD1=$BEAD1"

BEAD2=$(bn create \
  --title "Add POST /echo endpoint" \
  --description "$(cat << 'DESC_EOF'
Add a POST /echo endpoint that returns the request body wrapped in JSON.

## Requirements

- POST /echo with a text body returns 200 with JSON: {"echo": "<body>"}
- Use axum::body::Bytes to read the request body
- Handle empty body: return {"echo": ""}
- Create a new module src/echo.rs with the handler
- Add the route to the Router in src/main.rs (use axum::routing::post)

## Acceptance Criteria

- Endpoint returns 200 with correct JSON
- Empty body returns {"echo": ""}
- cargo check passes

## Testing

Verify with: curl -X POST -d "hello world" http://localhost:3000/echo
Expected: {"echo":"hello world"}
DESC_EOF
)" \
  --kind task)
echo "BEAD2=$BEAD2"

BEAD3=$(bn create \
  --title "Add GET /metrics endpoint with request counter" \
  --description "$(cat << 'DESC_EOF'
Add a /metrics endpoint and request-counting middleware.

## Requirements

- Add axum middleware that increments AppState.request_count on every request
- The counter field already exists: request_count: Arc<AtomicU64> in AppState
- GET /metrics returns 200 with JSON: {"total_requests": <count>}
- Use axum::middleware::from_fn with State extractor for the middleware
- Create a new module src/metrics.rs with the handler and middleware function
- Add the middleware layer and /metrics route to the Router in src/main.rs

## Acceptance Criteria

- /metrics returns correct count after N requests
- Counter increments on ALL routes (including /health, /metrics)
- cargo check passes

## Testing

curl /health 3 times, then curl /metrics should show {"total_requests": 4}
DESC_EOF
)" \
  --kind task)
echo "BEAD3=$BEAD3"

# --- Commit baseline ---
jj describe -m "initial project setup"
jj new

# --- Save env for run script ---
cat > .eval-env << EOF
export EVAL_DIR="$EVAL_DIR"
export DEV_AGENT="$DEV_AGENT"
export BEAD1="$BEAD1"
export BEAD2="$BEAD2"
export BEAD3="$BEAD3"
EOF

echo ""
echo "=== R6 Setup Complete ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "DEV_AGENT=$DEV_AGENT"
echo "BEAD1=$BEAD1 (version endpoint)"
echo "BEAD2=$BEAD2 (echo endpoint)"
echo "BEAD3=$BEAD3 (metrics endpoint)"
echo ""
echo "Verify: cargo check clean, bn next shows 3 bones"
echo "Next: source .eval-env && bash $REPO_DIR/evals/scripts/r6-run.sh"
