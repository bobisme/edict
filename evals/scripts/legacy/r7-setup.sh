#!/usr/bin/env bash
set -euo pipefail

# R7 Planning Eval — Setup
# Creates a fresh eval environment with a Rust/Axum project and a large feature
# request bone. The bone describes SQLite persistence but Cargo.toml has no DB
# crate — the agent must notice the gap and adapt.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

EVAL_DIR=$(mktemp -d)
cd "$EVAL_DIR"
echo "EVAL_DIR=$EVAL_DIR"

# --- Init repo and botbox ---
jj git init
botbox init --name r7-eval --type api --tools bones,maw,crit,botbus,vessel --init-bones --no-interactive

# --- Copy latest local workflow docs (installed package may be stale) ---
cp "$REPO_DIR/packages/cli/docs/"*.md .agents/botbox/

# --- Init Rust project ---
cargo init --name r7-eval

# --- Write Cargo.toml (axum, tokio, serde, chrono, uuid — NO SQLite crate) ---
cat > Cargo.toml << 'CARGO_EOF'
[package]
name = "r7-eval"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.8"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4", "serde"] }
CARGO_EOF

# --- Write minimal src/main.rs with /health endpoint ---
cat > src/main.rs << 'RUST_EOF'
use axum::{routing::get, Router};

async fn health() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() {
    let app = Router::new().route("/health", get(health));

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

# --- Generate agent identity ---
DEV_AGENT=$(botbus generate-name)
echo "DEV_AGENT=$DEV_AGENT"

# --- Register on channel ---
botbus send --agent setup r7-eval "R7 eval environment initialized" -L mesh -L setup
botbus mark-read --agent "$DEV_AGENT" r7-eval

# --- Create the feature request bone ---
PARENT_BEAD=$(bn create \
  --title "Build task management API" \
  --description "$(cat << 'DESC_EOF'
Build a complete task management API using Rust and Axum.

## Data Model

Each task has:
- id: UUID (auto-generated)
- title: String (required)
- description: Optional<String>
- status: enum (todo, in_progress, done)
- priority: enum (low, medium, high, critical)
- tags: Vec<String>
- due_date: Optional<DateTime>
- created_at: DateTime
- updated_at: DateTime

## Endpoints

### CRUD
- POST /tasks — create a task (returns 201 + task JSON)
- GET /tasks/:id — get a task by ID (404 if not found)
- PUT /tasks/:id — update a task (partial update, 404 if not found)
- DELETE /tasks/:id — delete a task (204 on success, 404 if not found)

### Tag Management
- POST /tasks/:id/tags — add tags to a task (body: {"tags": ["bug", "urgent"]})
- DELETE /tasks/:id/tags/:tag — remove a tag from a task

### Filtering + Pagination
- GET /tasks?status=todo&priority=high&tag=bug&page=1&per_page=20
  - Filter by any combination of status, priority, tag
  - Paginated response with total count in header or envelope

### Overdue Query
- GET /tasks/overdue — return all tasks where due_date < now and status != done

## Persistence

Store tasks in SQLite for persistence across restarts. Use a migrations pattern
for the schema.

## Testing

Write integration tests that exercise each endpoint. Use axum::test helpers or
spawn the server and test with reqwest.

## Acceptance Criteria

- All endpoints return proper HTTP status codes
- JSON serialization/deserialization works correctly
- Filtering supports multiple criteria simultaneously
- Pagination returns correct slices
- Overdue query correctly compares dates
- Tests pass and cover the happy path for each endpoint
DESC_EOF
)" \
  --kind task)

echo "PARENT_BEAD=$PARENT_BEAD"

# --- Commit baseline ---
jj describe -m "initial project setup"
jj new

# --- Save env for phases ---
cat > .eval-env << EOF
export EVAL_DIR="$EVAL_DIR"
export DEV_AGENT="$DEV_AGENT"
export PARENT_BEAD="$PARENT_BEAD"
EOF

echo ""
echo "=== R7 Setup Complete ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "DEV_AGENT=$DEV_AGENT"
echo "PARENT_BEAD=$PARENT_BEAD"
echo ""
echo "Verify: cargo check clean, bn next shows 1 bone"
echo "Next: source .eval-env && bash $REPO_DIR/evals/scripts/r7-phase1.sh"
