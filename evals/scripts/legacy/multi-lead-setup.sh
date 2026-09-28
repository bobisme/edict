#!/usr/bin/env bash
set -euo pipefail

# Multi-Lead Eval — Setup
# Creates the same futil Rust CLI project as E11-L4, but with multi-lead enabled.
# Two independent missions will test concurrent lead orchestrators.
#
# Config differences from E11-L4:
#   agents.dev.multiLead.enabled = true
#   agents.dev.multiLead.maxLeads = 3
#   agents.dev.multiLead.mergeTimeoutSec = 60

# --- Preflight ---
REQUIRED_CMDS=(botbox bus bn maw crit vessel jj cargo jq)
for cmd in "${REQUIRED_CMDS[@]}"; do
  command -v "$cmd" >/dev/null || { echo "Missing required command: $cmd" >&2; exit 1; }
done

# --- Paths and identities ---
EVAL_BASE="${EVAL_BASE:-${HOME}/.cache/botbox-evals}"
mkdir -p "$EVAL_BASE"
EVAL_DIR=$(mktemp -d "$EVAL_BASE/multi-lead-XXXXXXXXXX")
PROJECT_DIR="$EVAL_DIR/futil"
PROJECT_REMOTE="$EVAL_DIR/futil-remote.git"
mkdir -p "$PROJECT_DIR" "$EVAL_DIR/artifacts"

FUTIL_DEV="futil-dev"

export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
bus init

# --- Tool versions ---
{
  echo "timestamp=$(date -Iseconds)"
  for cmd in botbox bus bn maw crit vessel jj cargo; do
    echo "$cmd=$($cmd --version 2>/dev/null || echo unknown)"
  done
  echo "eval=multi-lead"
} > "$EVAL_DIR/artifacts/tool-versions.env"

# ============================================================
# Fake git remote
# ============================================================
git init --bare "$PROJECT_REMOTE"

# ============================================================
# Create futil project (same as E11-L4)
# ============================================================
cd "$PROJECT_DIR"
jj git init
jj git remote add origin "$PROJECT_REMOTE"

mkdir -p src data

# --- Cargo.toml ---
cat > Cargo.toml << 'CARGO_EOF'
[package]
name = "futil"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "futil"
path = "src/main.rs"

[dependencies]
clap = { version = "4", features = ["derive"] }
regex = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
csv = "1"
thiserror = "2"
CARGO_EOF

# --- src/main.rs (clap dispatch — same as E11-L4) ---
cat > src/main.rs << 'RUST_EOF'
use clap::{Parser, Subcommand};

mod error;
mod stats;
mod search;
mod convert;

#[derive(Parser)]
#[command(name = "futil", version, about = "File utility CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Analyze file statistics: lines, words, bytes
    Stats {
        #[arg(required = true)]
        paths: Vec<String>,
        #[arg(long)]
        json: bool,
        #[arg(long, value_name = "N")]
        top_words: Option<usize>,
        #[arg(long)]
        chars: bool,
    },
    /// Search for regex patterns
    Search {
        pattern: String,
        #[arg(required = true)]
        paths: Vec<String>,
        #[arg(short = 'A', long, default_value = "0")]
        after_context: usize,
        #[arg(short = 'B', long, default_value = "0")]
        before_context: usize,
        #[arg(short = 'C', long)]
        context: Option<usize>,
        #[arg(short = 'i', long)]
        ignore_case: bool,
        #[arg(short = 'c', long)]
        count: bool,
        #[arg(short = 'l', long)]
        files_with_matches: bool,
        #[arg(short = 'v', long)]
        invert_match: bool,
        #[arg(long)]
        json: bool,
    },
    /// Convert between JSON, CSV, and JSONL formats
    Convert {
        input: String,
        #[arg(short, long)]
        format: String,
        #[arg(long, value_delimiter = ',')]
        fields: Option<Vec<String>>,
        #[arg(long)]
        sort_by: Option<String>,
        #[arg(long)]
        pretty: bool,
        #[arg(short, long)]
        output: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Stats { paths, json, top_words, chars } => {
            stats::run(&paths, json, top_words, chars)
        }
        Commands::Search {
            pattern, paths, after_context, before_context,
            context, ignore_case, count, files_with_matches,
            invert_match, json,
        } => {
            let (before, after) = if let Some(c) = context {
                (c, c)
            } else {
                (before_context, after_context)
            };
            search::run(&pattern, &paths, before, after, ignore_case,
                        count, files_with_matches, invert_match, json)
        }
        Commands::Convert { input, format, fields, sort_by, pretty, output } => {
            convert::run(&input, &format, fields.as_deref(), sort_by.as_deref(),
                         pretty, output.as_deref())
        }
    };
    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
RUST_EOF

# --- src/error.rs ---
cat > src/error.rs << 'RUST_EOF'
use thiserror::Error;

#[derive(Error, Debug)]
pub enum FutilError {
    #[error("File not found: {0}")]
    FileNotFound(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid regex pattern: {0}")]
    InvalidRegex(String),
    #[error("Invalid format '{0}' — expected 'json', 'csv', or 'jsonl'")]
    InvalidFormat(String),
    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("No data in input file: {0}")]
    EmptyInput(String),
    #[error("Field '{0}' not found in data")]
    FieldNotFound(String),
}

pub fn validate_file(_path: &str) -> Result<String, FutilError> {
    todo!("Check file exists, read to string, return contents or FileNotFound")
}

pub fn detect_format(_path: &str) -> Result<&'static str, FutilError> {
    todo!("Check extension: .json→json, .csv→csv, .jsonl→jsonl, else InvalidFormat")
}

pub fn write_output(_content: &str, _output_path: Option<&str>) -> Result<(), FutilError> {
    todo!("If output_path is Some, write to file; if None, print to stdout")
}
RUST_EOF

# --- src/stats.rs ---
cat > src/stats.rs << 'RUST_EOF'
use crate::error::FutilError;

pub fn run(
    _paths: &[String], _json: bool, _top_words: Option<usize>, _chars: bool,
) -> Result<(), FutilError> {
    todo!("Implement stats: multi-file, optional JSON output, top-words frequency, char counting")
}
RUST_EOF

# --- src/search.rs ---
cat > src/search.rs << 'RUST_EOF'
use crate::error::FutilError;

#[allow(clippy::too_many_arguments)]
pub fn run(
    _pattern: &str, _paths: &[String], _before_context: usize, _after_context: usize,
    _ignore_case: bool, _count: bool, _files_with_matches: bool,
    _invert_match: bool, _json: bool,
) -> Result<(), FutilError> {
    todo!("Implement search: regex with context, case-insensitive, count, files-only, invert, JSON output")
}
RUST_EOF

# --- src/convert.rs ---
cat > src/convert.rs << 'RUST_EOF'
use crate::error::FutilError;

pub fn run(
    _input: &str, _format: &str, _fields: Option<&[String]>,
    _sort_by: Option<&str>, _pretty: bool, _output: Option<&str>,
) -> Result<(), FutilError> {
    todo!("Implement convert: 6 format pairs, field selection, sorting, pretty-print, file output")
}
RUST_EOF

# --- Sample data files ---
cat > data/sample.txt << 'DATA_EOF'
Hello world
This is a sample file for testing futil.
It has multiple lines with various words.
Numbers like 42 and 100 appear here too.
The quick brown fox jumps over the lazy dog.
DATA_EOF

cat > data/words.txt << 'DATA_EOF'
the cat sat on the mat
the dog lay on the rug
the bird flew over the house
the fish swam in the pond
the cat chased the bird
the dog barked at the cat
the bird sang in the tree
the fish jumped out of the pond
DATA_EOF

cat > data/sample.csv << 'DATA_EOF'
name,age,city,email,score
Alice,30,New York,alice@example.com,92.5
Bob,25,San Francisco,bob@test.org,88.0
Charlie,35,Chicago,charlie@example.com,95.2
Diana,28,Boston,diana@test.org,91.0
Eve,32,Seattle,eve@example.com,87.5
DATA_EOF

cat > data/sample.json << 'DATA_EOF'
[
  {"name": "Alice", "age": 30, "city": "New York", "email": "alice@example.com", "score": 92.5},
  {"name": "Bob", "age": 25, "city": "San Francisco", "email": "bob@test.org", "score": 88.0},
  {"name": "Charlie", "age": 35, "city": "Chicago", "email": "charlie@example.com", "score": 95.2},
  {"name": "Diana", "age": 28, "city": "Boston", "email": "diana@test.org", "score": 91.0},
  {"name": "Eve", "age": 32, "city": "Seattle", "email": "eve@example.com", "score": 87.5}
]
DATA_EOF

cat > data/sample.jsonl << 'DATA_EOF'
{"name":"Alice","age":30,"city":"New York","score":92.5}
{"name":"Bob","age":25,"city":"San Francisco","score":88.0}
{"name":"Charlie","age":35,"city":"Chicago","score":95.2}
{"name":"Diana","age":28,"city":"Boston","score":91.0}
{"name":"Eve","age":32,"city":"Seattle","score":87.5}
DATA_EOF

cat > data/log.txt << 'DATA_EOF'
2024-01-15 08:00:01 INFO  Server started on port 8080
2024-01-15 08:00:02 DEBUG Loading configuration from config.toml
2024-01-15 08:00:03 INFO  Database connection established
2024-01-15 08:01:15 WARN  Slow query detected: SELECT * FROM users (2.3s)
2024-01-15 08:02:30 ERROR Connection refused: redis://localhost:6379
2024-01-15 08:02:31 WARN  Falling back to in-memory cache
2024-01-15 08:04:00 ERROR Timeout: GET /api/reports took 30.1s
DATA_EOF

cat > .gitignore << 'GITIGNORE_EOF'
/target/
GITIGNORE_EOF

# --- Compile to verify skeleton ---
cargo check

# --- Initial commit ---
jj describe -m "futil: CLI skeleton with three todo subcommands"
jj bookmark create main -r @
jj git push --bookmark main
jj new

# ============================================================
# Initialize with botbox
# ============================================================
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" \
  botbox init --name futil --type cli --tools bones,maw,crit,botbus,vessel --init-bones --no-interactive

# ============================================================
# Patch .botbox.json: enable multi-lead + missions, disable review
# ============================================================
cd "$PROJECT_DIR"
CONFIG_FILE=".botbox.json"
PATCHED=$(jq '
  .review.enabled = false |
  .agents.dev.model = "opus" |
  .agents.dev.timeout = 900 |
  .agents.dev.missions.enabled = true |
  .agents.dev.missions.maxWorkers = 3 |
  .agents.dev.missions.maxChildren = 8 |
  .agents.dev.missions.checkpointIntervalSec = 30 |
  .agents.dev.multiLead.enabled = true |
  .agents.dev.multiLead.maxLeads = 3 |
  .agents.dev.multiLead.mergeTimeoutSec = 60 |
  .agents.worker.model = "sonnet" |
  .agents.worker.timeout = 600
' "$CONFIG_FILE")
echo "$PATCHED" > "$CONFIG_FILE"

echo "Patched .botbox.json:"
jq '{review: .review, agents: .agents}' "$CONFIG_FILE"

# ============================================================
# Fix hooks: add BOTBUS_DATA_DIR to --env-inherit
# ============================================================
_fix_hooks() {
  ALL_HOOKS=$(BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus hooks list --format json 2>/dev/null)
  HOOK_COUNT=$(echo "$ALL_HOOKS" | jq '.hooks | length' 2>/dev/null || echo "0")

  for i in $(seq 0 $((HOOK_COUNT - 1))); do
    HOOK=$(echo "$ALL_HOOKS" | jq ".hooks[$i]" 2>/dev/null)
    hook_id=$(echo "$HOOK" | jq -r '.id')
    HOOK_CHANNEL=$(echo "$HOOK" | jq -r '.channel')
    HOOK_CWD=$(echo "$HOOK" | jq -r '.cwd')
    COND_TYPE=$(echo "$HOOK" | jq -r '.condition.type')

    readarray -t CMD_ARRAY < <(echo "$HOOK" | jq -r '.command[] | if startswith("BOTBUS_CHANNEL") then . + ",BOTBUS_DATA_DIR" else . end')

    BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus hooks remove "$hook_id" 2>/dev/null || true

    ADD_CMD=(bus hooks add --channel "$HOOK_CHANNEL" --cwd "$HOOK_CWD")
    if [[ "$COND_TYPE" == "claim_available" ]]; then
      CLAIM_PATTERN=$(echo "$HOOK" | jq -r '.condition.pattern')
      ADD_CMD+=(--claim "$CLAIM_PATTERN" --release-on-exit)
    elif [[ "$COND_TYPE" == "mention_received" ]]; then
      MENTION_AGENT=$(echo "$HOOK" | jq -r '.condition.agent')
      ADD_CMD+=(--mention "$MENTION_AGENT")
    fi

    ADD_CMD+=(--)
    ADD_CMD+=("${CMD_ARRAY[@]}")

    BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" "${ADD_CMD[@]}" 2>&1 || \
      echo "WARNING: Failed to update hook $hook_id with BOTBUS_DATA_DIR"
  done
}
_fix_hooks

BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus hooks list > "$EVAL_DIR/artifacts/hooks-after-init.txt" 2>&1

# ============================================================
# Projects registry
# ============================================================
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus send --agent "$FUTIL_DEV" projects \
  "project: futil  repo: $PROJECT_DIR  lead: $FUTIL_DEV  tools: cli, file-utils"
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus inbox --agent "$FUTIL_DEV" --channels projects --mark-read >/dev/null 2>&1 || true

# ============================================================
# Save env
# ============================================================
cat > "$EVAL_DIR/.eval-env" << EOF
export EVAL_DIR="$EVAL_DIR"
export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
export PROJECT_DIR="$PROJECT_DIR"
export FUTIL_DEV="$FUTIL_DEV"
EOF

echo ""
echo "=== Multi-Lead Setup Complete ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "FUTIL_DEV=$FUTIL_DEV"
echo ""
echo "Multi-lead config:"
echo "  multiLead.enabled=true, maxLeads=3, mergeTimeoutSec=60"
echo "  missions.enabled=true, maxWorkers=3, maxChildren=8"
echo "  review.enabled=false"
echo "  worker.model=sonnet"
echo ""
echo "Two missions will be sent:"
echo "  Mission A: error.rs + stats subcommand"
echo "  Mission B: search + convert subcommands"
echo ""
echo "Source .eval-env before running:"
echo "  source $EVAL_DIR/.eval-env"
echo ""
