#!/usr/bin/env bash
set -euo pipefail

# E11-L5v2 Coordination Eval — Setup
# Creates a Rust CLI project (flowlog) — a data pipeline tool where three subcommands
# each CONTRIBUTE fields to a shared Record struct and steps to a shared Pipeline trait.
#
# WHY THIS FORCES REAL COORDINATION:
# The old taskr project had a clean core module that could be fully implemented first,
# turning coordination into a unidirectional waterfall. flowlog is different:
#   - Record struct starts with ONLY an id + data fields
#   - Each subcommand needs to ADD its own fields to Record during implementation
#   - Specs use DOMAIN language ("track provenance", "verify integrity", "record lineage")
#     so workers must make implementation decisions about field names and types
#   - No single worker can implement Record upfront because each stage's fields
#     emerge from that stage's domain requirements
#   - Workers must announce what they added (coord:interface) and read what siblings added
#
# flowlog CLI:
#   flowlog ingest <source>                       — read data, track provenance (todo!)
#   flowlog transform <rules-file> --input <file> — apply rules, validate integrity (todo!)
#   flowlog emit <output> --input <file>          — write output, record lineage (todo!)
#
# Shared modules:
#   src/record.rs   — Record struct (MINIMAL stub — workers add fields)
#   src/pipeline.rs — PipelineStage trait + PipelineError (MINIMAL stub — workers add variants/impls)
#
# Does NOT send the task-request — that goes in the run script.

# --- Preflight ---
REQUIRED_CMDS=(botbox bus bn maw crit vessel jj cargo jq)
for cmd in "${REQUIRED_CMDS[@]}"; do
  command -v "$cmd" >/dev/null || { echo "Missing required command: $cmd" >&2; exit 1; }
done

# --- Paths and identities ---
EVAL_BASE="${EVAL_BASE:-${HOME}/.cache/botbox-evals}"
mkdir -p "$EVAL_BASE"
EVAL_DIR=$(mktemp -d "$EVAL_BASE/e11-l5-XXXXXXXXXX")
PROJECT_DIR="$EVAL_DIR/flowlog"
PROJECT_REMOTE="$EVAL_DIR/flowlog-remote.git"
mkdir -p "$PROJECT_DIR" "$EVAL_DIR/artifacts"

FLOWLOG_DEV="flowlog-dev"

export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
bus init

# --- Tool versions ---
{
  echo "timestamp=$(date -Iseconds)"
  for cmd in botbox bus bn maw crit vessel jj cargo; do
    echo "$cmd=$($cmd --version 2>/dev/null || echo unknown)"
  done
  echo "eval=e11-l5v2"
} > "$EVAL_DIR/artifacts/tool-versions.env"

# ============================================================
# Fake git remote (so maw push / maw release succeed)
# ============================================================
git init --bare "$PROJECT_REMOTE"

# ============================================================
# Create flowlog project
# ============================================================
cd "$PROJECT_DIR"
jj git init
jj git remote add origin "$PROJECT_REMOTE"

mkdir -p src/commands data

# --- Cargo.toml ---
cat > Cargo.toml << 'CARGO_EOF'
[package]
name = "flowlog"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "flowlog"
path = "src/main.rs"

[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
csv = "1"
chrono = { version = "0.4", features = ["serde"] }
thiserror = "2"
CARGO_EOF

# --- src/main.rs (clap dispatch — DO NOT MODIFY) ---
cat > src/main.rs << 'RUST_EOF'
use clap::{Parser, Subcommand};

mod record;
mod pipeline;
mod commands;

#[derive(Parser)]
#[command(name = "flowlog", version, about = "Data pipeline CLI — ingest, transform, emit")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Ingest data from a source file, tracking where it came from
    Ingest {
        /// Path to source file (CSV or JSON)
        source: String,
        /// Override detected format (csv or json)
        #[arg(long)]
        format: Option<String>,
        /// Output ingested records as JSON
        #[arg(long)]
        json: bool,
    },
    /// Apply transformation rules and validate data integrity
    Transform {
        /// Path to rules file (JSON format)
        rules_file: String,
        /// Read records from this file (JSON, one per line)
        #[arg(long)]
        input: String,
        /// Write transformed records to this file
        #[arg(long)]
        output: Option<String>,
        /// Strict mode: reject records with any validation error
        #[arg(long)]
        strict: bool,
    },
    /// Emit records to an output destination with lineage tracking
    Emit {
        /// Output destination path
        output: String,
        /// Read records from this file (JSON, one per line)
        #[arg(long)]
        input: String,
        /// Output format: json, csv, or summary
        #[arg(long, default_value = "json")]
        format: String,
        /// Include full lineage chain in output
        #[arg(long)]
        lineage: bool,
    },
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Ingest { source, format, json } => {
            commands::ingest::execute(&source, format.as_deref(), json)
        }
        Commands::Transform { rules_file, input, output, strict } => {
            commands::transform::execute(&rules_file, &input, output.as_deref(), strict)
        }
        Commands::Emit { output, input, format, lineage } => {
            commands::emit::execute(&output, &input, &format, lineage)
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
RUST_EOF

# --- src/commands/mod.rs ---
cat > src/commands/mod.rs << 'RUST_EOF'
pub mod ingest;
pub mod transform;
pub mod emit;
RUST_EOF

# --- src/record.rs (MINIMAL shared types — workers MUST add fields) ---
# THIS IS THE KEY DESIGN DECISION: Record starts nearly empty.
# Each worker adds fields as they discover domain requirements from the specs.
# Specs DON'T name fields — they say "track provenance", "verify integrity", "record lineage".
cat > src/record.rs << 'RUST_EOF'
//! Shared record type for the flowlog pipeline.
//!
//! Record represents a single data item flowing through the pipeline.
//! Each pipeline stage (ingest, transform, emit) needs Record to carry
//! information relevant to that stage's concerns.
//!
//! ## Current state
//!
//! Record currently has ONLY a unique identifier and a data payload.
//! Each pipeline stage will need to extend this struct with fields for
//! its own domain concerns as it is implemented.
//!
//! ## Guidelines
//!
//! - All fields must be serializable (derive Serialize, Deserialize)
//! - Use Option<T> for fields that are only set by certain stages
//! - Keep field names descriptive of their domain purpose
//! - When adding fields, coordinate with sibling workers via bus
//!   to avoid naming conflicts or redundant fields

use std::collections::HashMap;

/// A single record flowing through the pipeline.
///
/// TODO: This struct needs fields added by each pipeline stage:
/// - The ingestion stage needs to track where data came from
/// - The transformation stage needs to track what happened to data
/// - The emission stage needs to track where data went
///
/// Workers implementing each stage should add the fields they need
/// and announce changes via coord:interface bus messages.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Record {
    /// Unique identifier for this record
    pub id: String,
    /// The actual data payload (key-value pairs from the source)
    pub data: HashMap<String, serde_json::Value>,
}

compile_error!("Each pipeline stage must add its own fields to Record — remove this line after adding fields");
RUST_EOF

# --- src/pipeline.rs (MINIMAL shared trait — workers MUST add error variants and impls) ---
cat > src/pipeline.rs << 'RUST_EOF'
//! Shared pipeline traits and error types for flowlog.
//!
//! The PipelineStage trait defines the interface that each stage implements.
//! PipelineError is the shared error type used across all stages.
//!
//! ## Current state
//!
//! The trait and error type are stubs. Each pipeline stage will need
//! to add error variants for its failure modes and may need to extend
//! the trait with stage-specific lifecycle methods.
//!
//! ## Guidelines
//!
//! - Add error variants as you discover failure modes in your stage
//! - Keep error messages descriptive for end users
//! - When adding new error variants, announce via coord:interface bus message
//! - The trait should remain object-safe if possible

use crate::record::Record;

/// A pipeline stage that processes records.
///
/// TODO: Each stage may need additional lifecycle methods beyond process().
/// For example:
/// - Initialization (opening files, connecting to sources)
/// - Finalization (flushing buffers, writing summaries)
/// - Validation (checking configuration before processing)
///
/// Workers should add methods as needed and coordinate on the trait shape.
pub trait PipelineStage: std::fmt::Debug {
    /// Human-readable name of this stage
    fn name(&self) -> &str;

    /// Process a single record through this stage.
    /// May modify the record in place (adding fields, transforming data).
    fn process(&self, record: &mut Record) -> Result<(), PipelineError>;
}

/// Errors that can occur during pipeline execution.
///
/// TODO: Each stage needs to add its own error variants here.
/// The ingestion stage has different failure modes than transformation
/// or emission. Add variants as you discover them during implementation.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// Generic I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Placeholder — remove this and add real variants
    #[error("{0}")]
    Other(String),
}

compile_error!("Each stage must add error variants to PipelineError — remove this line after adding variants");
compile_error!("Implement PipelineStage for each stage (ingest, transform, emit) — remove this line after implementing");
RUST_EOF

# --- src/commands/ingest.rs (todo stub — DOMAIN language, not code language) ---
cat > src/commands/ingest.rs << 'RUST_EOF'
//! flowlog ingest — read data from source files and track provenance.
//!
//! The ingest stage is the entry point for data into the pipeline.
//! It must handle multiple source formats and track where each record came from.
//!
//! ## Domain requirements
//!
//! Data provenance is critical for auditing. Every ingested record must carry
//! enough context to answer: "Where did this data come from?" This includes
//! the source file, the original format, and when it was ingested.
//!
//! The ingest stage must also detect the source format automatically (CSV vs JSON)
//! unless overridden by the user, and handle format-specific parsing:
//! - CSV: each row becomes a record, headers become field names
//! - JSON: each object becomes a record (supports both arrays and line-delimited)
//!
//! Raw data size tracking helps downstream stages estimate resource needs.
//!
//! ## Implementation notes
//! - Uses the shared Record type from record.rs — ADD fields Record needs
//!   for provenance tracking (coordinate with siblings via bus)
//! - Uses PipelineStage trait from pipeline.rs — IMPLEMENT the trait
//!   and ADD error variants for ingestion failures
//! - Generate unique IDs for each record (e.g., source-file + line-number hash)
//! - --json flag: output ingested records as JSON array to stdout
//! - Without --json: print summary (N records ingested from <source>)
//! - --format override: skip auto-detection, parse as specified format
//!
//! ## Dependencies
//! - record::Record (MUST ADD provenance fields)
//! - pipeline::{PipelineStage, PipelineError} (MUST ADD error variants, MUST IMPLEMENT trait)

use crate::record;
use crate::pipeline;

pub fn execute(
    _source: &str,
    _format: Option<&str>,
    _json: bool,
) -> Result<(), pipeline::PipelineError> {
    todo!("Implement ingest: detect format, parse records, track provenance, output results")
}
RUST_EOF

# --- src/commands/transform.rs (todo stub — DOMAIN language) ---
cat > src/commands/transform.rs << 'RUST_EOF'
//! flowlog transform — apply transformation rules and validate data integrity.
//!
//! The transform stage is the processing core of the pipeline.
//! It applies user-defined rules to records and validates data integrity.
//!
//! ## Domain requirements
//!
//! Data integrity verification ensures pipeline reliability. Every transformed
//! record must carry enough context to answer: "What happened to this data?"
//! This includes a log of transformations applied and any validation failures.
//!
//! Rules are defined in a JSON rules file with this structure:
//! ```json
//! {
//!   "rules": [
//!     { "field": "name", "action": "uppercase" },
//!     { "field": "age", "action": "validate_range", "min": 0, "max": 150 },
//!     { "field": "email", "action": "validate_pattern", "pattern": ".*@.*" },
//!     { "field": "score", "action": "default", "value": 0 }
//!   ]
//! }
//! ```
//!
//! Supported actions:
//! - uppercase / lowercase: transform string values
//! - validate_range: check numeric values are in range (records validation outcome)
//! - validate_pattern: check string matches regex pattern (records validation outcome)
//! - default: set field to value if missing
//!
//! --strict mode: if any validation fails, reject the record entirely.
//! Without --strict: keep the record but mark it as having validation issues.
//!
//! ## Implementation notes
//! - Reads records from --input file (JSON, one record per line — as output by ingest --json)
//! - Uses the shared Record type — ADD fields Record needs for transformation
//!   tracking and validation state (coordinate with siblings via bus)
//! - Uses PipelineStage trait — IMPLEMENT the trait and ADD error variants
//!   for transformation failures (bad rules, validation errors, etc.)
//! - Writes transformed records to --output file (or stdout if not specified)
//! - Print summary: N records transformed, M validation errors
//!
//! ## Dependencies
//! - record::Record (MUST ADD transformation tracking fields)
//! - pipeline::{PipelineStage, PipelineError} (MUST ADD error variants, MUST IMPLEMENT trait)

use crate::record;
use crate::pipeline;

pub fn execute(
    _rules_file: &str,
    _input: &str,
    _output: Option<&str>,
    _strict: bool,
) -> Result<(), pipeline::PipelineError> {
    todo!("Implement transform: load rules, apply transforms, validate integrity, output results")
}
RUST_EOF

# --- src/commands/emit.rs (todo stub — DOMAIN language) ---
cat > src/commands/emit.rs << 'RUST_EOF'
//! flowlog emit — write records to output destination with lineage tracking.
//!
//! The emit stage is the exit point for data from the pipeline.
//! It writes records in the requested format and tracks the full data lineage.
//!
//! ## Domain requirements
//!
//! Data lineage captures the complete journey of each record through the pipeline.
//! Every emitted record must carry enough context to answer: "What is the full
//! history of this data?" This means combining provenance from ingestion,
//! transformation history, and emission metadata into a complete chain.
//!
//! Output formats:
//! - json: write records as a JSON array (with optional lineage)
//! - csv: write records as CSV (data fields only, lineage in separate file)
//! - summary: human-readable summary — record count, source breakdown,
//!   transformation stats, validation pass rate
//!
//! --lineage flag: include full lineage chain in JSON output.
//! For CSV, write a companion .lineage.json file alongside the CSV.
//!
//! ## Implementation notes
//! - Reads records from --input file (JSON, one record per line)
//! - Uses the shared Record type — ADD fields Record needs for emission
//!   metadata and lineage assembly (coordinate with siblings via bus)
//! - Uses PipelineStage trait — IMPLEMENT the trait and ADD error variants
//!   for emission failures (write errors, format errors, etc.)
//! - Each format handler should be its own function
//! - Summary format should aggregate across all records
//!
//! ## Dependencies
//! - record::Record (MUST ADD emission and lineage fields)
//! - pipeline::{PipelineStage, PipelineError} (MUST ADD error variants, MUST IMPLEMENT trait)

use crate::record;
use crate::pipeline;

pub fn execute(
    _output: &str,
    _input: &str,
    _format: &str,
    _lineage: bool,
) -> Result<(), pipeline::PipelineError> {
    todo!("Implement emit: read records, format output, track lineage, write to destination")
}
RUST_EOF

# --- Sample data files ---
cat > data/sample.csv << 'DATA_EOF'
name,age,email,score
Alice,30,alice@example.com,95
Bob,25,bob@example.com,87
Charlie,35,charlie@test.org,92
Diana,28,diana@example.com,78
Eve,42,eve@nowhere.net,88
DATA_EOF

cat > data/sample.json << 'DATA_EOF'
[
  {"name": "Alice", "age": 30, "email": "alice@example.com", "score": 95},
  {"name": "Bob", "age": 25, "email": "bob@example.com", "score": 87},
  {"name": "Charlie", "age": 35, "email": "charlie@test.org", "score": 92},
  {"name": "Diana", "age": 28, "email": "diana@example.com", "score": 78},
  {"name": "Eve", "age": 42, "email": "eve@nowhere.net", "score": 88}
]
DATA_EOF

cat > data/rules.json << 'DATA_EOF'
{
  "rules": [
    { "field": "name", "action": "uppercase" },
    { "field": "age", "action": "validate_range", "min": 0, "max": 150 },
    { "field": "email", "action": "validate_pattern", "pattern": ".*@.*\\..*" },
    { "field": "score", "action": "default", "value": 0 }
  ]
}
DATA_EOF

cat > data/strict-rules.json << 'DATA_EOF'
{
  "rules": [
    { "field": "age", "action": "validate_range", "min": 0, "max": 30 },
    { "field": "email", "action": "validate_pattern", "pattern": ".*@example\\.com$" }
  ]
}
DATA_EOF

cat > data/bad-records.json << 'DATA_EOF'
{"id":"r1","data":{"name":"Alice","age":30}}
{"id":"r2","data":{"name":"Bob"}}
not-valid-json
{"id":"r3","data":{"name":"Charlie","age":"not-a-number"}}
DATA_EOF

# --- .gitignore ---
cat > .gitignore << 'GITIGNORE_EOF'
/target/
GITIGNORE_EOF

# --- Verify project structure ---
# The skeleton has todo!() at module level which won't compile — intentional.
echo "Project structure created (todo!() stubs — won't compile until implemented)"

# --- Initial commit ---
jj describe -m "flowlog: data pipeline CLI skeleton with co-evolving shared types"
jj bookmark create main -r @
jj git push --bookmark main
jj new

# ============================================================
# Initialize with botbox (maw v2 bare repo layout)
# ============================================================
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" \
  botbox init --name flowlog --type cli --tools bones,maw,crit,botbus,vessel --init-bones --no-interactive

# ============================================================
# Patch .botbox.json: enable missions, disable review, set models
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
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus send --agent "$FLOWLOG_DEV" projects \
  "project: flowlog  repo: $PROJECT_DIR  lead: $FLOWLOG_DEV  tools: cli, data-pipeline"

# Mark registry read
BOTBUS_DATA_DIR="$EVAL_DIR/.botbus" bus inbox --agent "$FLOWLOG_DEV" --channels projects --mark-read >/dev/null 2>&1 || true

# ============================================================
# Save env
# ============================================================
cat > "$EVAL_DIR/.eval-env" << EOF
export EVAL_DIR="$EVAL_DIR"
export BOTBUS_DATA_DIR="$EVAL_DIR/.botbus"
export PROJECT_DIR="$PROJECT_DIR"
export FLOWLOG_DEV="$FLOWLOG_DEV"
EOF

echo ""
echo "=== E11-L5v2 Setup Complete ==="
echo "EVAL_DIR=$EVAL_DIR"
echo "PROJECT_DIR=$PROJECT_DIR"
echo "FLOWLOG_DEV=$FLOWLOG_DEV"
echo ""
echo "Mission config:"
echo "  missions.enabled=true, maxWorkers=3, maxChildren=8"
echo "  review.enabled=false"
echo "  worker.model=sonnet"
echo ""
echo "Project: flowlog — data pipeline CLI with co-evolving shared types:"
echo "  src/record.rs             — Record struct (MINIMAL: id + data only, workers ADD fields)"
echo "  src/pipeline.rs           — PipelineStage trait + PipelineError (MINIMAL, workers ADD)"
echo "  src/commands/ingest.rs    — flowlog ingest <source>"
echo "  src/commands/transform.rs — flowlog transform <rules-file> --input <file>"
echo "  src/commands/emit.rs      — flowlog emit <output> --input <file>"
echo ""
echo "CO-EVOLUTION REQUIREMENT:"
echo "  Record starts with ONLY id + data fields. Each worker must ADD fields"
echo "  for their stage's concerns (provenance, transformation history, lineage)."
echo "  The full Record shape is not knowable until ALL stages are implemented."
echo "  Workers MUST announce additions via coord:interface and read siblings'."
echo ""
echo "No bone seeded — !mission handler in respond.mjs creates it."
echo ""
echo "Source .eval-env before running:"
echo "  source $EVAL_DIR/.eval-env"
echo ""
