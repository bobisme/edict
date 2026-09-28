#!/usr/bin/env bash
set -euo pipefail

# E10 Phase 2: Beta Investigates + Responds (beta-dev, Sonnet)
# Beta-dev reads alpha-dev's question, investigates validate_email,
# responds with domain expertise, and creates a bug bone.

source "${1:?Usage: e10-phase2.sh <path-to-.eval-env>}"

PHASE="phase2"
ARTIFACTS="$EVAL_DIR/artifacts"
mkdir -p "$ARTIFACTS"

cd "$BETA_DIR"

echo "=== E10 Phase 2: Beta Investigates + Responds ==="
echo "BETA_DEV=$BETA_DEV"
echo "BETA_DIR=$BETA_DIR"

PROMPT="You are dev agent \"${BETA_DEV}\" for the \"beta\" validation library.
Your project directory is: ${BETA_DIR}
This project uses maw v2 (bare repo layout). Source files are in ws/default/.
Use --agent ${BETA_DEV} on ALL bus mutation commands.
Set BOTBUS_DATA_DIR=${BOTBUS_DATA_DIR} in your environment for all bus commands.

Another developer (alpha-dev) has asked you about validate_email behavior. Check your inbox and respond.

1. CHECK INBOX:
   - bus inbox --agent ${BETA_DEV} --channels beta --mentions --mark-read
   - Read alpha-dev's question about validate_email rejecting + characters

2. INVESTIGATE:
   - Read your own code: ${BETA_DIR}/ws/default/src/lib.rs — examine the character whitelist in validate_email
   - Consider: Does RFC 5321 allow + in the local part of email addresses?
   - Consider: Do major email providers (Gmail, etc.) support plus-addressing (user+tag@)?

3. RESPOND:
   - If alpha-dev is right (the whitelist is overly restrictive), acknowledge it:
     - Create a bug bone to track the fix:
       maw exec default -- bn create --title \"validate_email: allow + in local part\" --description \"The character whitelist in validate_email excludes +, which is valid per RFC 5321 and widely used for subaddressing. Fix the whitelist to allow + in the email local part.\" --kind bug
     - Respond to alpha-dev on the alpha channel with domain expertise — reference the RFC, explain why the exclusion was wrong, and say you'll fix it:
       bus send --agent ${BETA_DEV} alpha \"Good catch @alpha-dev — the + exclusion was overly conservative. RFC 5321 allows printable characters including + in the local part. I'll fix this and add test coverage. Created a bone to track it.\" -L feedback
   - If you believe the restriction is intentional, explain why on the alpha channel.

4. STOP HERE. Do NOT fix the code yet — that's the next phase.

Key rules:
- Run bn commands via: maw exec default -- bn ...
- Source files are at ${BETA_DIR}/ws/default/"

echo "$PROMPT" > "$ARTIFACTS/$PHASE.prompt.md"

botbox run-agent claude -p "$PROMPT" -m sonnet -t 600 \
  > "$ARTIFACTS/$PHASE.stdout.log" 2> "$ARTIFACTS/$PHASE.stderr.log" \
  || echo "PHASE $PHASE exited with code $?"

# --- Discover bug bone ID for Phase 3 ---
# Filter for the validate_email bug bone (not seed bones from --init-bones)
BUG_BEAD=$(cd "$BETA_DIR" && maw exec default -- bn next 2>&1 | grep -i 'validate\|email\|plus' | grep -oP 'bn-\w+' | head -1)
if [[ -z "$BUG_BEAD" ]]; then
  # Fallback: most recently created bone (last in list)
  BUG_BEAD=$(cd "$BETA_DIR" && maw exec default -- bn next 2>&1 | grep -oP 'bn-\w+' | tail -1)
fi

if [[ -n "$BUG_BEAD" ]]; then
  echo "export BUG_BEAD=\"$BUG_BEAD\"" >> "$EVAL_DIR/.eval-env"
  echo "Bug bone: $BUG_BEAD"
else
  echo "WARNING: No bug bone found after Phase 2"
fi

echo ""
echo "=== Phase 2 Complete ==="
