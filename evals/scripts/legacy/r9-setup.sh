#!/usr/bin/env bash
set -euo pipefail

# R9 Crash Recovery Eval — Setup
# Creates a fresh eval environment with a Rust/Axum CRUD project,
# simulates 2 completed subtasks, and leaves subtask 3 in a crashed state
# (doing, workspace exists with partial code, bone comments trail).

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

EVAL_DIR=$(mktemp -d)
cd "$EVAL_DIR"
echo "EVAL_DIR=$EVAL_DIR"

# --- Init repo and botbox ---
jj git init
botbox init --name r9-eval --type api --tools bones,maw,crit,botbus,vessel --init-bones --no-interactive

# --- Copy latest local workflow docs ---
cp "$REPO_DIR/packages/cli/docs/"*.md .agents/botbox/

# --- Init Rust project ---
cargo init --name r9-eval

# --- Write Cargo.toml ---
cat > Cargo.toml << 'CARGO_EOF'
[package]
name = "r9-eval"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.8"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
CARGO_EOF

# --- Write baseline main.rs (health endpoint + Item struct + AppState) ---
cat > src/main.rs << 'RUST_EOF'
use axum::{routing::get, Router};
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

mod list;
mod create;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub id: u64,
    pub name: String,
}

#[derive(Clone)]
pub struct AppState {
    pub items: Arc<Mutex<Vec<Item>>>,
    pub next_id: Arc<AtomicU64>,
}

async fn health() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() {
    let state = AppState {
        items: Arc::new(Mutex::new(Vec::new())),
        next_id: Arc::new(AtomicU64::new(1)),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/items", get(list::list_items).post(create::create_item))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("bind");
    println!("Listening on :3000");
    axum::serve(listener, app).await.expect("serve");
}
RUST_EOF

# --- Write src/list.rs (subtask 1: GET /items) ---
cat > src/list.rs << 'RUST_EOF'
use axum::extract::State;
use axum::Json;

use crate::{AppState, Item};

pub async fn list_items(State(state): State<AppState>) -> Json<Vec<Item>> {
    let items = state.items.lock().unwrap();
    Json(items.clone())
}
RUST_EOF

# --- Write src/create.rs (subtask 2: POST /items) ---
cat > src/create.rs << 'RUST_EOF'
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use std::sync::atomic::Ordering;

use crate::{AppState, Item};

#[derive(Deserialize)]
pub struct CreateItem {
    pub name: String,
}

pub async fn create_item(
    State(state): State<AppState>,
    Json(input): Json<CreateItem>,
) -> (StatusCode, Json<Item>) {
    let id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let item = Item {
        id,
        name: input.name,
    };
    state.items.lock().unwrap().push(item.clone());
    (StatusCode::CREATED, Json(item))
}
RUST_EOF

# --- Verify baseline compiles ---
cargo check 2>&1
echo "cargo check (baseline + subtasks 1-2): OK"

# --- Init crit and maw ---
crit init
maw init

# --- Generate agent identity ---
AGENT=$(bus generate-name)
echo "AGENT=$AGENT"

# --- Register on channel ---
bus send --agent setup r9-eval "R9 eval environment initialized" -L mesh -L setup
bus mark-read --agent "$AGENT" r9-eval

# --- Create parent bone ---
PARENT=$(bn create \
  --title "Build items CRUD API" \
  --description "$(cat << 'DESC_EOF'
Build a complete CRUD API for items with the following endpoints:
- GET /items — list all items
- POST /items — create a new item
- GET /items/:id — get a single item by ID
- DELETE /items/:id — delete an item by ID
- PUT /items/:id — update an item by ID

The Item struct and AppState with in-memory storage already exist in main.rs.

## Acceptance Criteria

- All 5 endpoints work correctly
- cargo check passes
- Each endpoint is in its own module (src/list.rs, src/create.rs, etc.)

## Testing

Manual curl tests for each endpoint.
DESC_EOF
)" \
  --kind task)
echo "PARENT=$PARENT"

# --- Create 5 subtask bones ---
S1=$(bn create \
  --title "Add GET /items endpoint" \
  --description "Add a GET /items handler in src/list.rs that returns all items as JSON. Wire into the router in main.rs." \
  --kind task)

S2=$(bn create \
  --title "Add POST /items endpoint" \
  --description "Add a POST /items handler in src/create.rs that accepts {name} JSON and creates a new item with auto-incremented ID. Return 201 with the created item. Wire into the router in main.rs." \
  --kind task)

S3=$(bn create \
  --title "Add GET /items/:id endpoint" \
  --description "$(cat << 'DESC_EOF'
Add a GET /items/:id handler in src/get_item.rs that returns a single item by ID.

## Requirements

- Parse the :id path parameter as u64
- Look up the item in AppState.items
- Return 200 with the item as JSON if found
- Return 404 with {"error": "not found"} if not found
- Create src/get_item.rs module and wire into the router in main.rs

## Acceptance Criteria

- GET /items/1 returns the item if it exists
- GET /items/999 returns 404
- cargo check passes
DESC_EOF
)" \
  --kind task)

S4=$(bn create \
  --title "Add DELETE /items/:id endpoint" \
  --description "$(cat << 'DESC_EOF'
Add a DELETE /items/:id handler in src/delete_item.rs that removes an item by ID.

## Requirements

- Parse the :id path parameter as u64
- Remove the item from AppState.items if it exists
- Return 204 (no content) on success
- Return 404 with {"error": "not found"} if not found
- Create src/delete_item.rs module and wire into the router in main.rs

## Acceptance Criteria

- DELETE /items/1 removes the item and returns 204
- DELETE /items/999 returns 404
- cargo check passes
DESC_EOF
)" \
  --kind task)

S5=$(bn create \
  --title "Add PUT /items/:id endpoint" \
  --description "$(cat << 'DESC_EOF'
Add a PUT /items/:id handler in src/update_item.rs that updates an item by ID.

## Requirements

- Parse the :id path parameter as u64
- Accept JSON body: {"name": "new name"}
- Find the item in AppState.items and update its name
- Return 200 with the updated item as JSON
- Return 404 with {"error": "not found"} if not found
- Create src/update_item.rs module and wire into the router in main.rs

## Acceptance Criteria

- PUT /items/1 with {"name":"updated"} returns the updated item
- PUT /items/999 returns 404
- cargo check passes
DESC_EOF
)" \
  --kind task)

echo "S1=$S1  S2=$S2  S3=$S3  S4=$S4  S5=$S5"

# --- Wire dependencies ---
# Parent is blocked by all children (can't close until all subtasks done)
bn triage dep add "$S1" --blocks "$PARENT"
bn triage dep add "$S2" --blocks "$PARENT"
bn triage dep add "$S3" --blocks "$PARENT"
bn triage dep add "$S4" --blocks "$PARENT"
bn triage dep add "$S5" --blocks "$PARENT"

# Linear chain: S1 blocks S2, S2 blocks S3, etc.
bn triage dep add "$S1" --blocks "$S2"
bn triage dep add "$S2" --blocks "$S3"
bn triage dep add "$S3" --blocks "$S4"
bn triage dep add "$S4" --blocks "$S5"

# --- Simulate subtask 1 completed ---
bn do "$S1"
bn bone comment add "$S1" "Starting: implementing GET /items in src/list.rs"
bn bone comment add "$S1" "Progress: wrote list_items handler, wired into router"
bn bone comment add "$S1" "Completed by $AGENT"
bn done "$S1" --reason "Completed"

# --- Simulate subtask 2 completed ---
bn do "$S2"
bn bone comment add "$S2" "Starting: implementing POST /items in src/create.rs"
bn bone comment add "$S2" "Progress: wrote create_item handler with CreateItem struct, wired into router"
bn bone comment add "$S2" "Completed by $AGENT"
bn done "$S2" --reason "Completed"

# --- Mark parent doing ---
bn do "$PARENT"

# --- Commit baseline + subtasks 1-2 ---
jj describe -m "baseline + subtasks 1-2: GET /items, POST /items"
jj new

# --- Simulate crash state for subtask 3 ---
bn do "$S3"
bus claims stake --agent "$AGENT" "bone://r9-eval/$S3" -m "$S3"

# Create workspace for subtask 3
maw ws create --random
# Find the workspace by scanning .workspaces/
WS=""
WS_PATH=""
for d in .workspaces/*/; do
  if [[ -d "$d" ]]; then
    WS=$(basename "$d")
    WS_PATH="$(cd "$d" && pwd)"
    break
  fi
done
if [[ -z "$WS" ]]; then
  echo "ERROR: no workspace found in .workspaces/" >&2
  exit 1
fi

echo "WS=$WS"
echo "WS_PATH=$WS_PATH"

bus claims stake --agent "$AGENT" "workspace://r9-eval/$WS" -m "$S3"

# Write partial get_item.rs in the workspace (handler stub, not wired into router)
cat > "$WS_PATH/src/get_item.rs" << 'RUST_EOF'
use axum::extract::{Path, State};
use axum::Json;

use crate::{AppState, Item};

pub async fn get_item(
    Path(id): Path<u64>,
    State(state): State<AppState>,
) -> Json<Item> {
    let items = state.items.lock().unwrap();
    let item = items.iter().find(|i| i.id == id).unwrap();
    Json(item.clone())
}
RUST_EOF

# Note: the handler is deliberately incomplete:
# - Uses unwrap() instead of returning 404 for missing items
# - Not wired into the router in main.rs
# - No mod declaration in main.rs

# Describe the workspace change
maw ws jj "$WS" describe -m "$S3: Add GET /items/:id endpoint (WIP)"

# Add bone comments showing the agent was working when it crashed
bn bone comment add "$S3" "Starting: implementing GET /items/:id in src/get_item.rs"
bn bone comment add "$S3" "Progress: created workspace $WS ($WS_PATH), wrote handler stub in src/get_item.rs. Still need to: add 404 error handling (currently unwraps), wire mod get_item into main.rs, add route to router"

# Do NOT close subtask 3. Do NOT merge workspace. Simulate crash.

# --- Save env for run script ---
cat > .eval-env << EOF
export EVAL_DIR="$EVAL_DIR"
export AGENT="$AGENT"
export PARENT_BEAD="$PARENT"
export S1="$S1"
export S2="$S2"
export S3="$S3"
export S4="$S4"
export S5="$S5"
export WS="$WS"
export WS_PATH="$WS_PATH"
EOF

echo ""
echo "=== R9 Setup Complete ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "AGENT=$AGENT"
echo "PARENT_BEAD=$PARENT"
echo "S1=$S1 (GET /items — closed)"
echo "S2=$S2 (POST /items — closed)"
echo "S3=$S3 (GET /items/:id — doing, crash state)"
echo "S4=$S4 (DELETE /items/:id — ready, blocked by S3)"
echo "S5=$S5 (PUT /items/:id — ready, blocked by S4)"
echo "WS=$WS  WS_PATH=$WS_PATH"
echo ""
echo "State:"
echo "  Subtasks 1-2: done, code merged into main"
echo "  Subtask 3: doing, workspace has partial get_item.rs (missing 404 + router wiring)"
echo "  Subtasks 4-5: ready (blocked by dependency chain)"
echo "  Agent $AGENT holds claim on bone://r9-eval/$S3 and workspace://r9-eval/$WS"
echo ""
echo "Verify: cargo check (main branch), bn next, bn show $S3, bus claims --agent $AGENT --mine"
echo "Next: source .eval-env && bash $REPO_DIR/evals/scripts/r9-run.sh"
