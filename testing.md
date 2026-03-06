# Testing Plan (Legacy — JS era)

> **Note**: This testing plan was written for the JavaScript version of botbox. The project has been rewritten in Rust. For current development, use `cargo test`. The E2E eval scripts in `evals/scripts/` have been updated for the Rust binary.

End-to-end testing of `botbox` CLI against real repos using `vessel` for interactive session control.

## Prerequisites

```bash
cd ~/src/botbox/packages/cli && bun install
bun link  # makes botbox available globally
```

Confirm tools are available:
```bash
botbox --version
vessel --version
jj --version
```

## 1. Fresh repo — non-interactive init

Create a brand-new repo and bootstrap it entirely via CLI flags (simulates what an agent would do).

```bash
WORKDIR=$(mktemp -d)
cd "$WORKDIR" && jj git init

botbox init \
  --name test-fresh \
  --type api \
  --tools beads,maw,seal,rite,vessel \
  --reviewers security \
  --no-interactive

botbox doctor
botbox sync --check
```

**Verify:**
- [x] `.agents/botbox/` exists with all 9 workflow docs
- [x] `.agents/botbox/.version` contains a 12-char hex hash
- [x] `AGENTS.md` exists with managed section markers
- [x] `CLAUDE.md` is a symlink to `AGENTS.md`
- [x] Managed section contains all expected headings (Identity, Lifecycle, Quick Start, Beads Conventions, Mesh Protocol, Spawning Agents, Reviews, Stack Reference)
- [x] `doctor` exits 0
- [x] `sync --check` exits 0 (already up to date)

**Cleanup:**
```bash
rm -rf "$WORKDIR"
```

## 2. Fresh repo — interactive init via vessel

Test the interactive prompts by spawning botbox inside vessel and sending keystrokes.

```bash
WORKDIR=$(mktemp -d)
cd "$WORKDIR" && jj git init

vessel spawn -n init-test -- bash -c "cd $WORKDIR && botbox init"
```

**Drive the prompts:**
```bash
# Project name
sleep 1  # give spawn time to start
vessel snapshot init-test
vessel send init-test "my-interactive-project"

# Project type — select with arrow keys + enter
sleep 0.5
vessel snapshot init-test
vessel send init-test ""  # enter selects first option (api)

# Tools — all checked by default, just confirm
sleep 0.5
vessel snapshot init-test
vessel send init-test ""  # enter confirms defaults

# Reviewer roles — select security
sleep 0.5
vessel snapshot init-test
vessel send init-test " "  # space to toggle first option (now selected)
sleep 0.5
vessel snapshot init-test  # optional: verify selection

# Initialize beads — default yes
sleep 0.5
vessel snapshot init-test
vessel send init-test ""   # enter for default

# Wait for completion
sleep 1
vessel snapshot init-test  # should show "Done."
```

**Verify (after completion):**
```bash
test -d "$WORKDIR/.agents/botbox" && echo "PASS: agents dir" || echo "FAIL"
test -L "$WORKDIR/CLAUDE.md" && echo "PASS: symlink" || echo "FAIL"
grep -q "my-interactive-project" "$WORKDIR/AGENTS.md" && echo "PASS: name" || echo "FAIL"
grep -q "Reviewer roles: security" "$WORKDIR/AGENTS.md" && echo "PASS: reviewers" || echo "FAIL"
```

**Cleanup:**
```bash
vessel kill init-test 2>&1 || true  # may already be exited
rm -rf "$WORKDIR"
```

## 3. Existing repo — clone and init

Clone a real project and bootstrap it. Uses seal as the guinea pig since it's a known Rust project.

```bash
WORKDIR=$(mktemp -d)
cp -r ~/src/seal "$WORKDIR/seal"
cd "$WORKDIR/seal"

botbox init \
  --name seal \
  --type library \
  --tools beads,maw,seal,rite \
  --no-interactive \
  --force
```

**Verify:**
- [x] Existing files untouched (Cargo.toml, src/, etc. still present)
- [x] `.agents/botbox/` created alongside existing project files
- [x] `AGENTS.md` generated with `Project type: library`
- [x] `CLAUDE.md` symlinked (or overwritten if one existed)
- [x] `doctor` exits 0 (all tools available)

**Cleanup:**
```bash
rm -rf "$WORKDIR"
```

## 4. Sync after doc change

Simulate a botbox upgrade by modifying a bundled doc, then running sync.

```bash
WORKDIR=$(mktemp -d)
cd "$WORKDIR" && jj git init

# Init
botbox init --name sync-test --type api --tools beads --no-interactive

# Verify sync says up to date
botbox sync --check && echo "PASS: up to date" || echo "FAIL: unexpected stale"

# Tamper with version marker to simulate stale docs
echo "000000000000" > .agents/botbox/.version

# sync --check should now fail
botbox sync --check 2>&1 && echo "FAIL: should be stale" || echo "PASS: detected stale"

# Run actual sync
botbox sync

# Verify it updated
botbox sync --check && echo "PASS: synced" || echo "FAIL"
```

**Verify:**
- [x] `sync --check` exits non-zero when stale
- [x] `sync` updates docs and version marker
- [x] `sync --check` exits 0 after sync
- [x] AGENTS.md managed section is refreshed (contains current headings)

**Cleanup:**
```bash
rm -rf "$WORKDIR"
```

## 5. Sync preserves user content

Ensure the managed section replacement doesn't eat user-written content.

```bash
WORKDIR=$(mktemp -d)
cd "$WORKDIR" && jj git init

botbox init --name preserve-test --type frontend --tools beads --no-interactive

# Add custom content above and below managed section
sed -i '1i\# My Custom Header\n\nDo not delete this.\n' AGENTS.md
echo -e "\n## My Custom Footer\n\nThis should survive sync." >> AGENTS.md

# Force stale
echo "000000000000" > .agents/botbox/.version

# Sync
botbox sync

# Check preservation
grep -q "My Custom Header" AGENTS.md && echo "PASS: header preserved" || echo "FAIL"
grep -q "Do not delete this" AGENTS.md && echo "PASS: custom content" || echo "FAIL"
grep -q "My Custom Footer" AGENTS.md && echo "PASS: footer preserved" || echo "FAIL"
grep -q "botbox:managed-start" AGENTS.md && echo "PASS: markers present" || echo "FAIL"
```

**Cleanup:**
```bash
rm -rf "$WORKDIR"
```

## 6. Doctor on a healthy vs broken setup

```bash
WORKDIR=$(mktemp -d)
cd "$WORKDIR" && jj git init

# Doctor before init — should fail
botbox init --name doctor-test --type api --tools beads,maw,seal,rite,vessel --no-interactive
botbox doctor && echo "PASS: healthy" || echo "FAIL"

# Break things
rm -rf .agents/botbox
botbox doctor 2>&1 && echo "FAIL: should detect missing dir" || echo "PASS: detected"

# Partially break — remove symlink
botbox init --name doctor-test --type api --tools beads,maw,seal,rite,vessel --no-interactive --force
rm CLAUDE.md
botbox doctor 2>&1  # should report missing CLAUDE.md
```

**Cleanup:**
```bash
rm -rf "$WORKDIR"
```

## 7. Interactive init via vessel — edge cases

Test prompt validation and unusual inputs.

```bash
WORKDIR=$(mktemp -d)
cd "$WORKDIR" && jj git init

vessel spawn -n edge-test -- bash -c "cd $WORKDIR && botbox init"
sleep 1

# Project name
vessel snapshot edge-test
vessel send edge-test "test-edge"

# Navigate project type with arrow keys — select "monorepo" (4th option)
sleep 0.5
vessel snapshot edge-test
vessel send-bytes edge-test "1b5b42"  # down arrow
sleep 0.5
vessel send-bytes edge-test "1b5b42"  # down arrow
sleep 0.5
vessel send-bytes edge-test "1b5b42"  # down arrow
sleep 0.5
vessel snapshot edge-test  # should show monorepo selected
vessel send edge-test ""   # enter on monorepo

# Deselect all tools
sleep 0.5
vessel snapshot edge-test
vessel send edge-test "a"  # press 'a' to toggle all off
# Note: 'a' in inquirer toggles all — if all are selected, they all deselect
# Wait briefly for the action to take effect before confirming

# Skip reviewers
sleep 0.5
vessel snapshot edge-test
vessel send edge-test ""

# No beads
sleep 0.5
vessel snapshot edge-test
vessel send edge-test "n"

sleep 1
vessel snapshot edge-test  # should show "Done."
```

**Verify:**
```bash
grep -q "monorepo" "$WORKDIR/AGENTS.md" && echo "PASS: type" || echo "FAIL"
grep "^Tools:" "$WORKDIR/AGENTS.md"  # should show "Tools:" with no items
```

**Cleanup:**
```bash
vessel kill edge-test 2>&1 || true
rm -rf "$WORKDIR"
```

## 8. Init on existing repo — --force vs no --force

```bash
WORKDIR=$(mktemp -d)
cd "$WORKDIR" && jj git init

# First init
botbox init --name force-test --type api --tools beads --no-interactive

# Second init without --force — should warn about AGENTS.md
botbox init --name force-test-2 --type library --tools beads --no-interactive 2>&1 \
  | grep -q "already exists" && echo "PASS: warned" || echo "FAIL"

# Verify AGENTS.md still has original name
grep -q "force-test" AGENTS.md && echo "PASS: not overwritten" || echo "FAIL"

# With --force — should overwrite
botbox init --name force-test-2 --type library --tools beads --no-interactive --force
grep -q "force-test-2" AGENTS.md && echo "PASS: overwritten" || echo "FAIL"
```

**Cleanup:**
```bash
rm -rf "$WORKDIR"
```

## Test Results

All 8 tests passed as of 2026-01-29.

**Non-interactive tests (1, 3, 4, 5, 6, 8):** Can be scripted and run in parallel. All passed.

**Interactive tests (2, 7):** Require `vessel spawn/send/snapshot`. Both passed.

## Notes and Tweaks

- **Use `bun link`** in packages/cli/ to make `botbox` available globally instead of PATH manipulation.
- **`vessel wait --contains`** can race if the output appears before the wait starts. Use `sleep` + `snapshot` instead for reliability.
- **`vessel send " "`** (space) toggles checkboxes in inquirer prompts. Pressing `a` toggles all items.
- **`vessel kill`** exits non-zero if the agent already exited. Use `|| true` to avoid script failure.
- **`vessel send-bytes "1b5b42"`** sends down arrow. `1b5b41` is up arrow. Useful for menu navigation.
- **Empty tools list** renders as `Tools:` with no items in AGENTS.md (not omitted).
- **beads init** prints a multi-line box to stdout. Wait for "Done." before proceeding.

## Future Work

A `scripts/e2e-test.sh` could automate the non-interactive suite (tests 1, 3, 4, 5, 6, 8) and report pass/fail.
