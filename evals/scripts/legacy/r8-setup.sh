#!/usr/bin/env bash
set -euo pipefail

# R8 Adversarial Review Eval v2 — Setup
# Multi-file fixture (7 files). Cross-file reasoning required to find TOCTOU.
# Single-reviewer eval testing the ceiling of review quality.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

EVAL_DIR=$(mktemp -d)
cd "$EVAL_DIR"
echo "EVAL_DIR=$EVAL_DIR"

# --- Init repo and botbox ---
jj git init
botbox init --name r8-eval --type api --tools bones,maw,crit,botbus,vessel --init-bones --no-interactive

# --- Copy latest local workflow docs (installed package may be stale) ---
cp "$REPO_DIR/packages/cli/docs/"*.md .agents/botbox/

# --- Init Rust project ---
cargo init --name r8-eval

# --- Init crit and maw ---
crit init
maw init

# --- Generate reviewer identity ---
REVIEWER=$(botbus generate-name)
echo "REVIEWER=$REVIEWER"

# --- Register on channel ---
botbus send --agent eval-author r8-eval "R8 eval environment initialized" -L mesh -L setup
botbus mark-read --agent "$REVIEWER" r8-eval

# --- Write Cargo.toml ---
cat > Cargo.toml << 'CARGO_EOF'
[package]
name = "r8-eval"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.8"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
CARGO_EOF

# --- Write src/main.rs (router + AppState + mod declarations) ---
cat > src/main.rs << 'RUST_EOF'
mod config;
mod delete;
mod download;
mod health;
mod list;
mod upload;

use axum::{
    Router,
    routing::{delete as delete_route, get, post},
};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tokio::fs;

#[derive(Clone)]
pub struct AppState {
    pub total_bytes: Arc<AtomicU64>,
}

#[tokio::main]
async fn main() {
    let cfg = config::config();
    fs::create_dir_all(&cfg.upload_dir).await.expect("create upload dir");

    let state = AppState {
        total_bytes: Arc::new(AtomicU64::new(0)),
    };

    let app = Router::new()
        .route("/files/{name}", post(upload::upload_file))
        .route("/files", get(list::list_files))
        .route("/files/{name}", get(download::download_file))
        .route("/files/{name}", delete_route(delete::delete_file))
        .route("/health", get(health::health))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("bind");
    println!("Listening on :3000");
    axum::serve(listener, app).await.expect("serve");
}
RUST_EOF

# --- Write src/config.rs (AppConfig + OnceLock — clean trap 1) ---
cat > src/config.rs << 'RUST_EOF'
use std::path::PathBuf;
use std::sync::OnceLock;

pub struct AppConfig {
    pub upload_dir: PathBuf,
    pub max_total_bytes: u64,
}

// OnceLock for runtime config — env vars aren't available at compile time,
// so we initialize once on first access. This is the standard pattern for
// lazy initialization of runtime configuration in Rust.
static CONFIG: OnceLock<AppConfig> = OnceLock::new();

pub fn config() -> &'static AppConfig {
    CONFIG.get_or_init(|| AppConfig {
        upload_dir: PathBuf::from(
            std::env::var("UPLOAD_DIR").unwrap_or_else(|_| "./uploads".into()),
        ),
        max_total_bytes: std::env::var("MAX_TOTAL_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100 * 1024 * 1024), // 100 MB default
    })
}
RUST_EOF

# --- Write src/upload.rs (Bug 1: race condition in size limit) ---
cat > src/upload.rs << 'RUST_EOF'
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use std::sync::atomic::Ordering;
use tokio::fs;

use crate::AppState;
use crate::config::config;

/// Upload a file. Enforces a global size limit across all stored files.
pub async fn upload_file(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let cfg = config();
    let size = body.len() as u64;

    // Check if adding this file would exceed the global limit
    let current = state.total_bytes.load(Ordering::SeqCst);
    if current + size > cfg.max_total_bytes {
        return (StatusCode::PAYLOAD_TOO_LARGE, "Storage limit exceeded".to_string());
    }
    state.total_bytes.fetch_add(size, Ordering::SeqCst);

    let dest = cfg.upload_dir.join(&name);
    match fs::write(&dest, &body).await {
        Ok(()) => (StatusCode::CREATED, format!("Uploaded {name} ({size} bytes)")),
        Err(e) => {
            // Roll back the counter on write failure
            state.total_bytes.fetch_sub(size, Ordering::SeqCst);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Write failed: {e}"))
        }
    }
}
RUST_EOF

# --- Write src/download.rs (correct path validation — uses &canonical) ---
cat > src/download.rs << 'RUST_EOF'
use axum::{
    extract::Path,
    http::StatusCode,
    response::IntoResponse,
};
use tokio::fs;

use crate::config::config;

/// Download a file. Validates the path stays within the upload directory.
pub async fn download_file(
    Path(name): Path<String>,
) -> impl IntoResponse {
    let cfg = config();
    let file_path = cfg.upload_dir.join(&name);

    // Canonicalize to resolve symlinks and ../ components
    let canonical = match fs::canonicalize(&file_path).await {
        Ok(p) => p,
        Err(_) => return (StatusCode::NOT_FOUND, "File not found".to_string()),
    };
    let base = match fs::canonicalize(&cfg.upload_dir).await {
        Ok(p) => p,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Config error".to_string()),
    };
    if !canonical.starts_with(&base) {
        return (StatusCode::FORBIDDEN, "Access denied".to_string());
    }

    // Read using the canonical path
    match fs::read_to_string(&canonical).await {
        Ok(contents) => (StatusCode::OK, contents),
        Err(_) => (StatusCode::NOT_FOUND, "File not found".to_string()),
    }
}
RUST_EOF

# --- Write src/delete.rs (Bug 2: TOCTOU — uses &file_path not &canonical; Quality 2: .ok()) ---
cat > src/delete.rs << 'RUST_EOF'
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use std::sync::atomic::Ordering;
use tokio::fs;

use crate::AppState;
use crate::config::config;

/// Delete a file. Validates the path stays within the upload directory.
pub async fn delete_file(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let cfg = config();
    let file_path = cfg.upload_dir.join(&name);

    // Canonicalize to resolve symlinks and ../ components
    let canonical = match fs::canonicalize(&file_path).await {
        Ok(p) => p,
        Err(_) => return (StatusCode::NOT_FOUND, "File not found".to_string()),
    };
    let base = match fs::canonicalize(&cfg.upload_dir).await {
        Ok(p) => p,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Config error".to_string()),
    };
    if !canonical.starts_with(&base) {
        return (StatusCode::FORBIDDEN, "Access denied".to_string());
    }

    // Remove the file and update the size counter
    let meta = fs::metadata(&file_path).await;
    fs::remove_file(&file_path).await.ok();

    if let Ok(meta) = meta {
        state.total_bytes.fetch_sub(meta.len(), Ordering::SeqCst);
    }

    (StatusCode::OK, format!("Deleted {name}"))
}
RUST_EOF

# --- Write src/list.rs (Bug 3: pagination underflow; Quality 1: unwrap) ---
cat > src/list.rs << 'RUST_EOF'
use axum::{
    extract::Query,
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use tokio::fs;

use crate::config::config;

#[derive(Deserialize)]
pub struct ListParams {
    page: Option<usize>,
    per_page: Option<usize>,
}

/// List files with pagination.
pub async fn list_files(Query(params): Query<ListParams>) -> impl IntoResponse {
    let cfg = config();
    let per_page = params.per_page.unwrap_or(20);
    let page = params.page.unwrap_or(1);

    let mut entries = Vec::new();
    let mut dir = match fs::read_dir(&cfg.upload_dir).await {
        Ok(d) => d,
        Err(_) => return (StatusCode::OK, "[]".to_string()),
    };

    while let Ok(Some(entry)) = dir.next_entry().await {
        entries.push(entry.file_name().to_str().unwrap().to_string());
    }
    entries.sort();

    let start = (page - 1) * per_page;
    let page_entries: Vec<_> = entries.into_iter().skip(start).take(per_page).collect();
    (StatusCode::OK, serde_json::to_string(&page_entries).unwrap())
}
RUST_EOF

# --- Write src/health.rs (clean trap 2 — improved comment) ---
cat > src/health.rs << 'RUST_EOF'
use axum::{
    http::StatusCode,
    response::IntoResponse,
};
use std::os::unix::fs::PermissionsExt;
use tokio::fs;

use crate::config::config;

/// Health check — returns upload directory status.
pub async fn health() -> impl IntoResponse {
    let cfg = config();
    // Check if any read permission bits are set (owner, group, or other).
    // 0o444 masks all three read bits. If any is set, the directory is
    // considered accessible for our purposes.
    match fs::metadata(&cfg.upload_dir).await {
        Ok(meta) => {
            let mode = meta.permissions().mode();
            if mode & 0o444 != 0 {
                (StatusCode::OK, "healthy".to_string())
            } else {
                (StatusCode::SERVICE_UNAVAILABLE, "upload dir not readable".to_string())
            }
        }
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "upload dir missing".to_string()),
    }
}
RUST_EOF

# --- Commit baseline ---
jj describe -m "initial project setup"
jj new

# --- Describe the change ---
jj describe -m "feat: add file management API (upload, list, download, delete)"

# --- Create crit review ---
REVIEW_ID=$(crit reviews create --agent eval-author \
  --title "feat: add file management API" \
  --description "Upload, list, download, and delete endpoints for file management. Uses Axum 0.8, tokio, serde. Includes global upload size limit, pagination, path traversal protection, and health check. Split across modules: config, upload, download, delete, list, health." \
  --json | python3 -c "import sys,json; print(json.load(sys.stdin)['review_id'])")
echo "REVIEW_ID=$REVIEW_ID"

# --- Request review ---
crit reviews request "$REVIEW_ID" --agent eval-author --reviewers "$REVIEWER"

# --- Announce ---
botbus send --agent eval-author r8-eval \
  "Review requested: $REVIEW_ID @$REVIEWER — file management API" -L mesh -L review-request

# --- Save env for run script ---
cat > .eval-env << EOF
export EVAL_DIR="$EVAL_DIR"
export REVIEWER="$REVIEWER"
export REVIEW_ID="$REVIEW_ID"
EOF

echo ""
echo "=== R8 v2 Setup Complete ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "REVIEWER=$REVIEWER"
echo "REVIEW_ID=$REVIEW_ID"
echo ""
echo "Verify: cargo check && cargo clippy should pass"
echo "Next: source .eval-env && bash evals/scripts/r8-run.sh (or copy to eval dir)"
