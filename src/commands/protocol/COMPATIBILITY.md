# Protocol Guidance Output Contract - Compatibility Policy

## Schema Version

The protocol guidance output adheres to the **protocol-guidance.v1** schema. This version is immutable once released.

## Core Guarantee: Additive-Only Evolution

Future versions (v1.1, v1.2, v2.0) MUST maintain backwards compatibility with clients consuming v1 output using these rules:

### 1. Required Fields Are Immutable
These fields MUST NOT be:
- Removed
- Renamed
- Retyped
- Reordered in JSON output

**v1 Required Fields:**
```
schema: &'static str
command: &'static str
status: ProtocolStatus
snapshot_at: String
valid_for_sec: u32
steps: Vec<String>
diagnostics: Vec<String>
```

### 2. Optional Fields Can Evolve
New optional fields MUST be:
- Added with an `Option<T>` or `Vec<T>` wrapper
- Documented in this policy
- Include a default value if omitted

**v1 Optional Fields:**
```
bead: Option<BeadRef>
workspace: Option<String>
review: Option<ReviewRef>
revalidate_cmd: Option<String>
advice: Option<String>
```

### 3. Field Addition Process
When adding a new field in a future version:

1. **Assess cardinality:**
   - If 0 or 1: use `Option<T>`
   - If 0 or many: use `Vec<T>`

2. **Document the default:**
   - If omitted, what value should be assumed?
   - Example: `valid_for_sec` defaults to 300 if missing

3. **Add test:**
   - Add golden schema test asserting field structure
   - Ensure JSON roundtrip includes new field

4. **Update this policy:**
   - Record the new field here
   - Explain client expectations

### 4. Enum Evolution (ProtocolStatus)
Adding new status variants is NOT backwards compatible.

**Safe approach for new statuses:**
- Add to a separate enum (e.g., `ProtocolStatusExt`)
- Or bump to v2 and migrate all clients

**Current variants (v1):**
- Ready
- Blocked
- Resumable
- NeedsReview
- HasResources
- Clean
- HasWork
- Fresh

### 5. Output Format Stability
All three renderers (text, json, pretty) must be derivable from the same internal ProtocolGuidance model:

- **JSON:** Direct serde serialization of ProtocolGuidance
- **Text:** Human-readable lines derived from all fields
- **Pretty:** Colored TTY output derived from all fields

No renderer should add fields or data not in the core struct.

## Freshness Semantics

The freshness metadata enables clients to detect and refresh stale guidance:

### snapshot_at
- **Type:** `String` (UTC ISO 8601)
- **Meaning:** When this guidance was generated
- **Client behavior:** Parse with a robust RFC 3339 parser
- **Immutable:** Yes, this is the generation timestamp

### valid_for_sec
- **Type:** `u32` (duration in seconds)
- **Meaning:** How long this guidance remains fresh
- **Default:** 300 (5 minutes)
- **Client behavior:**
  ```
  let expiry = parse_rfc3339(snapshot_at) + Duration::secs(valid_for_sec as i64)
  if now > expiry { revalidate() }
  ```
- **Immutable:** No, may change between versions

### revalidate_cmd
- **Type:** `Option<String>`
- **Meaning:** Shell command to re-fetch fresh guidance
- **Examples:**
  - `"edict protocol start"`
  - `"edict protocol resume"`
  - `None` for final states (finish, cleanup)
- **Client behavior:** Run the command if guidance is stale

## Exit Code Policy

Protocol commands use a three-tier exit-code scheme. The key principle: **agents branch on stdout status fields, NOT on shell exit codes.**

### Exit Codes

| Code | Meaning | Stderr | Stdout |
|------|---------|--------|--------|
| **0** | Command succeeded | Empty | Valid guidance (JSON/text/pretty) |
| **1** | Operational failure | Error message | Empty |
| **2** | Usage error | Error message | Empty |

### Exit 0: Success (all ProtocolStatus variants)

All guidance states exit 0, including states that indicate the agent cannot proceed:

- **Ready** — commands are ready to run
- **Blocked** — cannot proceed; diagnostics explain why
- **Resumable** — work in progress from a previous session
- **NeedsReview** — awaiting review approval
- **HasResources** — workspace/claims still held
- **Clean** — no held resources
- **HasWork** — ready bones available
- **Fresh** — starting fresh (no prior state)

The agent reads the `status` field in stdout to decide what to do next. Exit 0 means "I produced valid guidance output."

### Exit 1: Operational Failure

Returned when the protocol command cannot produce valid guidance at all:

- `.edict.toml (or legacy .botbox.toml) config not found
- Companion tool missing or unavailable (bus, maw, br, crit)
- Subprocess output cannot be parsed
- Command not yet implemented

Exit 1 writes a diagnostic to stderr in the format:
```
edict protocol: <command>: <detail>
```

### Exit 2: Usage Error

Returned for invalid arguments. Typically handled by clap before protocol code runs.

### Stderr Policy

Stderr is reserved exclusively for operational errors (exit 1 and exit 2). Protocol commands MUST NOT write to stderr when producing valid guidance (exit 0).

Specifically:
- Status information (blocked, needs-review, etc.) goes to **stdout** as part of guidance
- Diagnostic details about why the agent is blocked go to **stdout** in the `diagnostics` array
- Warnings about held resources go to **stdout** in the `diagnostics` array
- Only true failures (config missing, tool crashed, parse error) go to **stderr**

### Client Integration

Agents consuming protocol commands should:

1. **Check exit code first**: non-zero means no guidance was produced
2. **On exit 0**: parse stdout for guidance, branch on the `status` field
3. **On exit 1**: read stderr for the error message, retry or escalate
4. **On exit 2**: fix the invocation (bad arguments)
5. **Never branch on exit code to determine status**: use `status` field instead

### Implementation

Exit codes are enforced through `ProtocolExitError` (in `exit_policy.rs`), which integrates with the `ExitError` pattern in `main.rs`. All guidance rendering goes through `render_guidance()` which always returns `Ok(())` (exit 0).

## Diagnostics Reason Codes

The `diagnostics` array may contain reason codes in future versions. Current codes are free-form strings. If codes are introduced, they will be documented here.

## Testing Strategy

### Golden Schema Tests
Validate that the contract is preserved:
- `golden_schema_version_is_stable` — schema version never changes
- `golden_status_variants_are_complete` — no variants silently removed
- `golden_guidance_json_structure` — required fields always present
- `golden_minimal_guidance_json` — minimal valid output structure
- `golden_full_guidance_json` — all optional fields serialize correctly
- `golden_text_render_includes_all_fields` — text output is complete
- `golden_compatibility_additive_only` — optional fields are truly optional

### Freshness Tests
Validate that clients can detect and react to stale guidance:
- `guidance_default_freshness` — default TTL is 300 seconds
- `guidance_set_freshness` — TTL and revalidate command are configurable
- `guidance_stale_window_logic` — timestamp + duration arithmetic works
- `render_json_includes_freshness` — JSON always includes freshness metadata
- `render_text_includes_freshness` — text output shows freshness

### Exit Code Policy Tests
Validate the exit-code contract:
- `all_statuses_map_to_success` — every ProtocolStatus variant exits 0
- `exit_code_values` — Success=0, OperationalError=1, UsageError=2
- `blocked_status_still_exits_zero` — blocked guidance exits 0
- `needs_review_status_still_exits_zero` — needs-review guidance exits 0
- `protocol_exit_error_operational` — operational errors produce exit 1
- `protocol_exit_error_to_exit_error` — integrates with main.rs ExitError

### Integration Tests
(Implemented in higher-level bones)
- Agents receiving stale guidance re-run the revalidate command
- Commands executed from stale guidance fail or warn appropriately

## Migration Path (For v1 → v2)

If backwards incompatibility is necessary:

1. **Release v1.1** with all additive changes
2. **Deprecate** in client docs (e.g., "v1 will be EOL on date X")
3. **Implement v2** alongside v1 for 2+ releases
4. **Migrate clients** during overlap period
5. **Sunset v1** after all clients upgraded

## Client Responsibilities

Clients consuming protocol guidance MUST:

1. **Ignore unknown fields** — if v1.1 adds a field, v1 clients should skip it
2. **Handle missing optional fields** — treat `None` as the documented default
3. **Parse timestamps robustly** — use a standard RFC 3339 parser
4. **Respect freshness semantics** — check expiry before executing steps
5. **Treat diagnostics as advisory** — don't crash on unexpected diagnostic text

## Version Mismatch Handling

If a client sees schema != "protocol-guidance.v1":

- **Newer schema (v1.1, v1.2):** Attempt to parse as v1; skip unknown fields
- **Older schema (v0.9):** Error; request user to upgrade client
- **Unrecognized schema:** Error with guidance to file an issue

## Questions and Issues

For questions about this compatibility policy, see [CLAUDE.md](../../CLAUDE.md) — cross-channel communication process.

File issues at: `bus send --agent $AGENT edict "Issue: <details>" -L tool-issue`
