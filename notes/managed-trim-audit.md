# Managed-section trim: enforcement audit (bn-97do)

Goal: bn-30a0. Subject: `src/templates/agents-managed.md.jinja` (375 lines; `Lnn` below = template line).

For each rule in the template, this audit records where else it is enforced or taught, and what the rewrite should do with it.

## Sources and how they were checked

| Abbrev | Source |
|---|---|
| P/`f` | `src/commands/protocol/f` (what `edict protocol …` prints or blocks) |
| DEV | `src/commands/dev_loop/prompt.rs` (lead loop prompt; `format!` literal L158-508) |
| WRK | `src/commands/worker_loop.rs` (worker prompt) |
| RSP | `src/commands/responder.rs` |
| RPL | `src/reply.rs`: `anchor_section` L98-121, `ask_and_wait_section` L128-161, `review_recipe` L176-219. These are appended to every DEV/WRK iteration (DEV L146-150, WRK L417-425); `anchor_section` also goes into RSP (L1019). |
| doc `x`:n | `src/templates/docs/x.md` (rendered to `.agents/edict/x.md`) |
| tool help | Captured with rite 0.35.1, maw 1.0.0-pre.16, seal 0.29.0 (seal source 0.30.0) and bones-cli 0.26.0: `bn tldr`, `rite tldr`, `rite agentsmd show`, `maw --help` (= `maw tldr` + workflow), `maw agents show`, `seal agents show`, `seal --help`. `seal tldr` does not exist. |

Tool behaviour was checked three ways: against tool source (`/home/bob/src/{maw,seal,rite,bones}`), against `--help`, and by runs in a sandbox. Each run used a tempdir for HOME, XDG_*, and RITE_DATA_DIR, with AGENT, RITE_AGENT and BOTBUS_AGENT unset. The live `rite hooks list | wc -l` count was 27 before and 27 after.

Edict does not inject `maw agents show`, `seal agents show` or `rite agentsmd show` into AGENTS.md. One downstream repo carries the maw block by hand: `botcrit-ui/ws/default/AGENTS.md:249-304`.

## Rule table

Recommendation key:
- **KEEP**: the rule goes in the Rules list.
- **MOVE**: the rule belongs in a workflow doc (most are already there).
- **POINTER**: a tool's help covers it, so a one-line pointer is enough.
- **DROP**: remove the rule.

| # | Rule (short) | Template section | Covered by | Recommendation |
|---|---|---|---|---|
| R1 | Create a bone before work; open → doing → done | How to Make Changes L5; Bones Conventions L143 | The mechanics are covered: `bn tldr` L3-16, P/mod.rs L486 (`bn do` step), DEV L308/L483, WRK L535/L391, doc worker-loop:60,78, doc start:13. **"Before you start work"** appears nowhere for ad-hoc sessions; the loops only work bones that already exist. | **KEEP** (rule 1) |
| R2 | `maw ws create <bone-id> --from main --description` | L6, maw table L91 | `maw --help` L68 gives the exact form. maw refuses to create without `--from` ("Workspace create requires an explicit source. Use one of: maw ws create --from main …", maw `workspace/mod.rs:1752`). P/mod.rs L471-476 (`protocol start` emits it), DEV L310/L371, doc start:15 | **POINTER** (maw help and `protocol start`); fold the bone-id name into rule 2 |
| R3 | Edit only in a workspace, never in trunk/`default` | L7 | Implicit only: DEV L317, WRK L540-542/L578, doc start:16, `maw --help` L12/L23. No source says "never in default". DEV L557 tells the lead to fix things in default after the merge; that exception is sanctioned. | **KEEP** (rule 2) |
| R4 | Reviewed work merges only through `edict protocol merge`; run its steps in order; bare merge only when there is no review | L8, L94, L117-118 | The protocol emits the steps: P/merge.rs L735-792, P/finish.rs L335-360. It blocks risk:critical: P/merge.rs L322-327. DEV L468-480 says "ALWAYS try protocol merge first". Doc merge-check:5-13. **Contradicted** by doc worker-loop:180-182 and doc finish:36, where the worker runs a bare `maw ws merge`. | **KEEP** (rule 5) and fix the docs (see b) |
| R5 | Conventional-commit prefix in `--message` | L8 | Without a TTY, `maw ws merge` exits with an error when `--message` is missing ("No --message provided and stdin is not a terminal … A commit message is required", maw `workspace/mod.rs:2144-2157`). P/merge.rs L38-39 gives the same error, with an example. `edict protocol merge --help` (`-m`). `maw --help` L89. No tool enforces the prefix. | **POINTER**; the rule 5 example shows `feat:` |
| R6 | Use a change id instead of `default` when merging into a tracked change | L8 | P/merge.rs L708/L754-757 (`merge_target`); `maw ws merge --help` example `--into change:…`; doc finish:36, doc merge-check:35 | **MOVE** (already in merge-check.md) **Lives in:** merge-check.md › Merge steps (bn-3ox9). |
| R7 | Close the bone with `bn done` | L9 | `bn tldr` L14-16; P/finish.rs L363; DEV L483; WRK L391; doc finish:17 | **KEEP** (part of rule 1) |
| R8 | Do not create git branches by hand | L11 | **Nowhere.** The wording is also inaccurate: maw workspaces are detached-HEAD worktrees (sandbox: `## HEAD (no branch)`), so maw does not "handle branching". | **KEEP** (in rule 2), reworded to "no git branches; maw workspaces replace them" |
| R9 | All tools have `--help` | L13 | `maw --help` L107-108, `rite tldr` L82-85 | **DROP**; the pointer line replaces it |
| R10 | Conflicts are data; `ws sync` commits markers and sets `lifecycle:conflicted` | L15-17 | `maw ws sync --help` ("Conflicts are recorded as data … marked as 'conflicted'"); `maw --help` L97; P/merge.rs L813-815 ("Conflicts are data, not failure"); doc merge-check:50-52, doc finish:69-70. Confirmed in the sandbox (`[lifecycle:conflicted]` in `maw ws list`). | **KEEP** as one line (rule 6); the details are a **POINTER** |
| R11 | `resolve --list/--keep`, merge `--resolve/--resolve-all`, `ws conflicts`, "merge auto-syncs stale sources" | L19-21 | `maw --help` L99-100; `maw ws merge --help`; P/merge.rs L817-846 (conflict diagnostic lists every command); doc finish:86-103. **The claim that merge auto-syncs stale sources is false as stated.** A source that is still stale is refused: "Workspace '<ws>' is stale … To fix: maw ws sync <ws>" (maw `merge.rs:5456`, 1555). Sources stay fresh through post-merge sibling auto-rebase, but a dirty sibling can be left stale. | **POINTER** (maw help and protocol merge output). Drop the auto-sync claim. **Lives in:** merge-check.md › Merge gates (bn-3ox9). |
| R12 | Merge refuses a source with conflict markers; bypass with `--force` | L22 | maw `merge.rs:3265`: the refusal message names `maw ws resolve <ws> --list` and `--force`. A second gate, for placeholder blobs in HEAD, **cannot** be bypassed with `--force` (`merge.rs:3276-3300`). Doc finish:120-122. | **POINTER** (the tool error teaches the fix). Drop the `--force` advice. **Lives in:** merge-check.md › Merge gates (bn-3ox9). |
| R13 | Directory layout description and tree | L24-68 | DEV `command_pattern(layout)` L884-903; WRK L434-437 (rewritten); doc worker-loop:20,81 and doc finish:5 are layout-conditional; `maw --help` L5 covers the root layout only. No source has the tree diagram. | **DROP** the tree; keep one conditional line inside rule 2 |
| R14 | Never merge or destroy `default` | L41, L62, L121 | **maw enforces it.** Merge: "Cannot merge the default workspace — it is the merge target, not a source" (`merge.rs:5434`, also 1766 and 2648). Destroy: "Cannot destroy the default workspace" (`create.rs:746,755`). maw also refuses `ws create default`, `ws restore default`, and `ws clean default` without `--force`. P/merge.rs L159-167 blocks too. Sandbox-confirmed. Quirk: with no `-m` and no TTY, the missing-message error fires first. | **KEEP** as a half-line in rule 3 (it states intent: default is the target). A **POINTER** would also be safe. |
| R15 | `maw exec <ws> -- <cmd>` | L43, L64 | `maw --help` L76-78; `maw agents show` L27; DEV L318/L661; WRK L541/L578 | **POINTER** |
| R16 | Where `bn` runs: directly at the root (root layout), or `maw exec default -- bn` (bare layout) | L44, L65, L67 | DEV L884-903 (layout-specific); WRK via `Layout::rewrite_prompt`; doc worker-loop:20. **Contradicted** in two places. `maw --help` L80 always says `maw exec default -- bn`. Protocol step builders (P/shell.rs L387, L418) always emit the bare-layout form, even in root layout. | **KEEP** (conditional line inside rule 2) |
| R17 | `seal` always through `maw exec <ws> -- seal` | L45, L66 | Every P/shell `seal_*` builder (L518, L610, L648, L680, L701); DEV L661; WRK L580; doc review-request:14, doc security-review:89. **Contradicted** by `seal agents show`, which runs `seal --agent …` directly. | **KEEP** (in rule 8) |
| R18 | Bones quick-reference table; identity comes from `$AGENT` | L70-85 | `bn tldr` covers every row. bones-cli `agent.rs:60` reads `AGENT`. DEV L161, WRK L430. | **POINTER** (`bn tldr`) |
| R19 | maw quick-reference table and inspect commands | L87-113 | The QUICK REFERENCE in `maw --help` (L63-106) covers create, list, diff, exec, sync, merge --check, merge --destroy --message, recover and --to, conflicts and resolve. It does not cover `overlap`, `history`, `undo`, `recover --search` or `--show`; those are in `maw ws … --help`. | **POINTER** (`maw --help`) |
| R20 | Lead: look for `+N to merge` in `maw ws list` | L116 | `maw --help` L73; DEV L442 | **POINTER** |
| R21 | Always run `--check` before `--destroy` | L122 | Redundant. The real merge runs the same gates and refuses before it commits, and `--destroy` runs only after a successful merge (maw help; confirmed in the sandbox). `protocol merge` also runs `--check` itself (P/merge.rs L231, L704-719). | **DROP** |
| R22 | Commit before you request review; after the LGTM, commit only the review log | L123 | DEV L490-497, L528-529; WRK L377-380; P/merge.rs L536-557 (diagnostic for a stale approval); doc worker-loop:113-115,175; doc review-response:76-80. seal exempts commits that touch only `.seal/` (seal `range.rs:171-190`). | "After the LGTM" part: **KEEP** in rule 5. "Commit before review" part: **MOVE** (review-request.md) **Lives in:** review-request.md › Before you request review (bn-3ox9). |
| R23 | Run `maw ws recover` before you conclude work is lost; never start over without it | L124 | The recovery mechanics are real: destroy snapshots confirmed in the sandbox, and `maw --help` L91-95 says "nothing is ever lost". **No source makes the agent check first**: P/merge.rs L862-864 mentions recover only in the conflict diagnostic. **Contradicted** in three places: DEV L209-211, WRK L476 and doc worker-loop:44 all say "workspace destroyed: re-create and resume from scratch". | **KEEP** (rule 7) and fix the prompts and doc |
| R24 | Run the protocol command at each transition; `--format json`; if one fails, fall back to the docs | L126-139 | Covered for the loops: DEV L218/L232/L305/L331/L470/L580/L618 and WRK L450/L532/L295/L352/L568, with the fallback wording checked by tests (DEV L996, L1024; WRK L1123, L1200). `edict protocol --help` lists the subcommands. Manual sessions: only doc merge-check:5-13. worker-loop.md, start.md, finish.md and review-request.md never mention `edict protocol`. | **KEEP** (rule 4). The table is a **POINTER** (`edict protocol --help`). |
| R25 | Post progress comments for crash recovery | L144 | DEV L319; WRK L552 ("Add at least one progress comment"); P/mod.rs L489; doc worker-loop:89-92 | **KEEP** as a clause in rule 1 (manual sessions get it nowhere else) |
| R26 | Run the check command before committing (`check_command`) | L145 | Protocol and prompts: only DEV L548-560, a lead check after the merge. No pre-commit check in WRK. Docs: worker-loop:112, finish:24-28, review-response:39. | **KEEP** (rule 11, conditional on `check_command`) |
| R27 | Follow finish.md; workers do NOT push | L146 | WRK L358-359 and L441 ("the lead handles merging"). No explicit "do not push" anywhere. **Contradicted** in three places: doc worker-loop:183 and doc finish:43-44 (`maw push` when pushMain), and `maw agents show` L81-103 (push with no role scope). A non-dispatched WRK merges its own work (WRK L387-388). | **KEEP** (rule 11) and fix the docs |
| R28 | Install locally after release (`install_command`) | L147 | **Nowhere.** | **KEEP** as a conditional release block (not in the Rules list) |
| R29 | Release instructions (`release_instructions`) | L148-151 | Project text; nowhere else (DEV L636-654 is a generic release check) | **KEEP** as a conditional block, unchanged |
| R30 | Identity: `$AGENT`; manual sessions use `<project>-dev` | L153-156 | `$AGENT`: DEV L161, WRK L430, P/mod.rs L44, doc worker-loop:16. `<project>-dev` naming: **nowhere**. **Contradicted** by `rite tldr` L5 and `rite agentsmd` L17 (`export RITE_AGENT=$(rite generate-name)`). | **KEEP** as one line in the pointer header |
| R31 | Stake `bone://` and `workspace://` claims; `release --all` | L158-166 | `protocol start` emits both stakes (P/mod.rs L464-483). `protocol review` and `protocol finish` **block** without the bone claim (P/review.rs L76-82, P/finish.rs L96-101). Release: P/shell.rs L263/L303, P/cleanup.rs L83. Docs: start:14,17, finish:42. **Contradicted** by `rite agentsmd show` L34-36, which uses `rite claim`, `check-claim` and `release`; none of these subcommands exist. | **DROP** (the protocol enforces it; the docs teach it) |
| R32 | `--reviewers` is a gate and spawns nothing; security review through security-review.md; never `@<project>-security` | L170-174 | Confirmed in seal: `reviews request` only records reviewers and spawns nothing (`reviews.rs:285-346`). RPL L190-197 and L207 ("Do NOT … send an @mention"); DEV L337; WRK L305; P/finish.rs L425/L466/L500; doc review-request:3-5, doc security-review:3-9. **Contradicted by edict itself**: `protocol review` announces `"Review requested: <bone> @<project>-security" -L review-request` (P/review.rs L215-221; re-request at L385-390 and L452). | **KEEP** (rule 8) and fix `protocol review` |
| R33 | Retarget before re-requesting; the re-review snippet | L177-185 | Confirmed in seal: `request` never retargets. RPL L200-202; DEV L198-201; WRK L466-469; P/review.rs L370; P/finish.rs L410; P/merge.rs L517/L548-552. Tests: DEV L954, WRK L1060, P/finish L721, P/merge L1026, template.rs L447. Docs: review-request:54-66, security-review:152-156. | **MOVE** (already in review-request.md; the protocol emits it) **Lives in:** review-request.md › Create and launch…; review-response.md › Steps 3c (bn-3ox9). |
| R34 | Agentbus completion is not approval; confirm with `seal review --format json` | L187-188 | RPL L198-199 (every loop iteration); doc review-request:73-74; doc security-review:118-119,129. Nothing covers manual sessions outside the docs. **Weakened** by doc worker-loop:143 (a subagent does the review, and no Seal vote check follows). | **KEEP** (rule 8) |
| R35 | Agentbus timeout or failure: one anchored `task-blocked`, release the claim, stop | L189-191 | RPL L204-207 (fuller than the template); P/shell.rs L363-375 (`review_wait_advice`); doc review-request:75-79; doc security-review:118-127 | **MOVE** (already covered) **Lives in:** security-review.md › Launch contract, › Who sends what (bn-3ox9). |
| R36 | Dedicated reviewers reply with `--reply-to` and `-L review-done` | L193-195 | **Contradicted**: doc security-review:90-91 forbids the reviewer from sending Rite messages, and doc security-review:142-144 has the **author** send `review-done`. The template is wrong. | **DROP** (security-review.md is the authority) **Lives in:** security-review.md › Who sends what (bn-3ox9). |
| R37 | `seal reviews create` uses the fork point; check the range; `--base` | L197-202 | `seal reviews create --help --base` ("seal discovers the fork point … Pass HEAD~1"). Prints `Range: a..b (N commits)` (`reviews.rs:200-210`). The base persists (`range.rs`). Doc worker-loop:134-136. | **POINTER** (`seal reviews create --help`) **Lives in:** review-request.md › What a review covers (bn-3ox9). |
| R38 | No code after the LGTM; `mark-merged` exits 1 on a stale approval; a fresh LGTM fixes it; `--allow-stale-approval`; `seal diff approval_stale` | L204-215 | **seal enforces it.** `reviews.rs:56` fails with exit 1: "Cannot merge <id>: the approval does not cover the current code … ask a reviewer to vote again with 'seal lgtm <id>', or merge anyway with --allow-stale-approval". A repeat LGTM moves `approved_commit` (`projection/mod.rs:1773-1793`). `seal diff --format json` emits `approval_stale`, `approved_commit` and `uncovered_commits` (`status.rs:251-262`). Also: `seal reviews mark-merged --help`, the P/merge.rs L536-557 diagnostic, DEV L530-532, WRK L382-384, and docs review-response:76-103 and merge-check:29-32. | **MOVE**. The one-line "no code after the LGTM" goes in rule 5; the rest is in the tool error and review-response.md. **Lives in:** review-response.md › Commit no code after the LGTM (bn-3ox9). |
| R39 | Review-log commit sequence right before the merge; clean check in the same command; never `mark-merged` after the merge; never move or delete `.seal/reviews` | L217-232 | The protocol emits the exact sequence (P/shell.rs L500-580, used by P/merge.rs L752-758 and P/finish.rs L352-360). DEV L521-539; WRK L377-389 ("Never run mark-merged after this"); docs merge-check:27-35, finish:29-36. **"Never move/delete `.seal/reviews` to get past `maw ws sync`" is nowhere else.** `seal agents show` L41 lists mark-merged last, with no order against the merge. | **MOVE** (merge-check.md already has the sequence; add the `.seal/reviews` line there) **Lives in:** merge-check.md › The review log and the clean check (bn-3ox9). |
| R40 | `maw ws merge` also merges uncommitted additions and deletions | L230 | **Confirmed in the sandbox**: an untracked file, a deletion and an edit, none committed, all landed in the merge commit with no warning. No tool guards against this. DEV L523, WRK L379, docs merge-check:28, finish:30. DEV L495 contradicts it ("may miss uncommitted worker changes"). | **MOVE** (the reason behind rule 5; already in merge-check.md) **Lives in:** merge-check.md › The review log and the clean check (bn-3ox9). |
| R41 | rite bus command table | L234-248 | `rite tldr` covers every row (send L11, `--reply-to` L40, `--format json` id L39, inbox L26, `wait --reply-to` L46, `wait --mentions` L34, `history --thread` L42, search L68) | **POINTER** (`rite tldr`) |
| R42 | Stuck on a companion tool: ask its channel; cross-project ask-and-wait; tracking bone on exit 1 | L250, L289-307 | Full workflow in doc cross-channel:7-103. WRK L581 posts `-L tool-issue` to the agent's own channel only. **Weakened** by doc worker-loop:101-103 (no anchor, no wait). | **MOVE** (cross-channel.md; listed in the docs index) **Lives in:** cross-channel.md › Steps: ask another project (bn-3ox9). |
| R43 | Anchoring: answer `$RITE_MESSAGE_ID`; a batch anchor is the LAST id; a missing anchor is a warning; `complete:false` means a fragment; never reuse an earlier turn's anchor | L252-267 | RPL L98-121 covers all of it and is injected every turn (DEV, WRK, RSP), but only when a ULID anchor exists. Manual sessions get nothing. `rite tldr` L37-42 and `rite send --help` cover `$RITE_MESSAGE_ID` and the missing-anchor warning (sandbox: exit 0 plus a warning). `rite history --thread` reports `complete`. **The batch LAST-id rule and "never reuse an earlier anchor" are in no tool doc.** The LAST-id claim was not verified in rite help. | **KEEP** (rule 9) **Lives in:** cross-channel.md › Threads (bn-3ox9). |
| R44 | Ask and wait: anchor, then `rite wait --reply-to`; exit 0/1/2; never re-send; how the wait filters narrow | L269-287 | RPL L128-161 (a near-verbatim copy, in every DEV/WRK iteration; not in RSP). `rite tldr` L44-47 has the exit codes. **Confirmed in rite**: 1 is `EXIT_TIMEOUT`, 2 is `EXIT_BAD_PARENT` (`wait.rs:56,65`). Your own reply never satisfies your wait (`wait.rs:350`). A reply sent before the wait counts (`wait.rs:495-499`). On timeout, the JSON `advice` says "Do not send the request again." The default `-t` is 0, which means no timeout. Doc cross-channel:31-54. | **KEEP** (rule 10). Exit-code details are a **POINTER** (`rite wait --help`). **Lives in:** cross-channel.md › Ask and wait (bn-3ox9). |
| R45 | STE Language rules | L309-321 | Nowhere | **DROP** (user decision) |
| R46 | rite message shape: one or two lines, lead with the subject, labelled examples; anchor instead of quoting | L323-332 | Only as literal message templates: DEV L314, WRK L557, RSP L1047 ("brief one-line acknowledgment"). `rite agentsmd` L57-63 says "concise", with unlabelled examples. | **KEEP** one rule (user decision; rule 12) **Lives in:** cross-channel.md › Message shape (bn-3ox9). |
| R47 | Replies to a human, plus exceptions | L334-357 | Only RSP L975 ("helpfully and concisely") | **DROP** (user decision). This also drops "Confirm before destructive actions" (L351), which is stated nowhere else. |
| R48 | `cass search` (optional) | L359-361 | doc worker-loop:106, doc triage:46 | **DROP** (the docs cover it) |
| R49 | Design-docs and workflow-docs index | L363-375 | Nowhere else (the protocol points only at finish.md and merge-check.md; the prompts point at security-review.md) | **KEEP** (the index is the pointer target) |

## (a) Rules enforced or taught nowhere outside the managed section

These rules would be lost if removed. Each one either stays in the Rules list or moves into a workflow doc.

1. **R8 no manual git branches.** Nothing covers it. Keep it, reworded (maw uses detached worktrees).
2. **R23 recover before starting over.** No source makes the agent check first, and three sources teach the opposite (DEV L209-211, WRK L476, doc worker-loop:44).
3. **R24 run protocol at every transition, for manual sessions.** Only merge-check.md mentions `edict protocol`. start.md, finish.md, review-request.md and worker-loop.md do not.
4. **R4 reviewed work merges only through protocol merge, for a worker reading the docs.** worker-loop.md and finish.md teach a bare merge.
5. **R43 batch anchor is the LAST id, and never reuse an earlier turn's anchor.** Only RPL covers these, and only when a hook anchor exists. No rite doc covers them. Not verified in rite help.
6. **R30 `<project>-dev` for manual sessions.**
7. **R39 never move or delete `.seal/reviews` to get past `maw ws sync`.** Add this to merge-check.md.
8. **R27 workers do not push.** No prompt says it, and two docs and `maw agents show` contradict it.
9. **R26 run the check command before committing.** The workflow docs teach it; no prompt or protocol output does.
10. **R28 `install_command`** and **R29 `release_instructions`.** These are project-specific and exist only here.
11. **R1 "create a bone before you start"** and **R25 "progress comments"** for ad-hoc sessions. The loops only cover bones that already exist.
12. **"Confirm before destructive actions"** (R47 exceptions, L351). It disappears with the dropped section. Flag it for the user and decide whether it earns a bullet.

## (b) Tool help or edict text that contradicts edict policy

### Tool help (raw captures are in the bn-97do scratchpad: `audit/tooldocs/`)

| # | Where | Says | Edict policy |
|---|---|---|---|
| T1 | `maw --help` QUICK START L10; `maw agents show` L12 | `maw ws create <your-name> --from origin/main` | `<bone-id> --from main` |
| T2 | `maw --help` WORKFLOW L26; `maw agents show` L28 | `maw ws merge <name1> <name2> --into default` | Protocol merge, with `--destroy --message`, the review gate and the clean check. `maw --help`'s own QUICK REFERENCE (L88-89) is correct, so maw's help contradicts itself. |
| T3 | `maw --help` L80 | `maw exec default -- bn <args>  # bones always runs through the default workspace` | Root layout: run `bn` directly at the repo root |
| T4 | `maw agents show` L70-79 | Resolve conflicts by editing out the markers by hand, then `git add -A` and commit | `maw ws resolve --keep`. A `git add -A` after the LGTM leaves the approval stale. |
| T5 | `maw agents show` L81-103 | `maw push` after merging, with no role scope | Only the lead pushes |
| T6 | `maw ws merge --help` | "Stale workspaces are automatically synced before merge" | False per maw's own source (`merge.rs:5456`, which refuses a stale source). The template repeats this claim (R11). |
| T7 | `seal agents show` L10-41 | `seal --agent … ` run directly | `maw exec <ws> -- seal …` |
| T8 | `seal agents show` L31 | `mark-merged <id> --self-approve  # solo workflow` | Skips the reviewer gate; edict never allows it |
| T9 | `seal agents show` L41 | mark-merged as the last step, with no order against the maw merge | mark-merged goes before the merge, while HEAD is still the approved commit; never after |
| T10 | `seal agents show` L45, L48 | "jj Change IDs", `--json` | Stale: edict uses git worktrees and `--format json` |
| T11 | `rite agentsmd show` L34-36 | `rite claim`, `rite check-claim`, `rite release --all` | None of these subcommands exist in rite 0.35.1 (verified). Correct form: `rite claims stake/check/release`. |
| T12 | `rite agentsmd show` L73 | `rite wait --mention -t 120` | Fails in the sandbox with exit 2: "unexpected argument '--mention' … tip: '--mentions'". This is the error that reached 15 repos. |
| T13 | `rite agentsmd show` L68-70; `rite tldr` L54-59 | Ask and wait without an anchor (`rite wait -c … --from …`) | Capture the id, then `rite wait --reply-to <id>` |
| T14 | `rite tldr` L5; `rite agentsmd` L17 | `export RITE_AGENT=$(rite generate-name)` | `$AGENT` from the launcher, or `<project>-dev` |
| T15 | `rite agentsmd` L29-30, L60-63 | Example messages with no label and no bone id | A labelled one-liner that leads with the bone id |
| T16 | `rite wait --help` | `-t` defaults to 0, meaning no timeout | Always pass `-t`; a wait without it blocks forever |

The tool-side fixes (T1-T16) belong in bones filed against maw, seal and rite. They are out of scope for this goal. Until they land, the pointer lines must say that edict's rules override tool quick-starts.

### Edict's own text (fix in bn-30a0 or follow-up bones)

| # | Where | Problem |
|---|---|---|
| E1 | `src/commands/protocol/review.rs` L215-221, L385-390, L452 | `protocol review` emits `"Review requested: <bone> @<project>-security" -L review-request`. That is the forbidden @-mention (R32, and RPL L207). |
| E2 | DEV L209-211, WRK L476, doc worker-loop:44 | Workspace destroyed → "re-create and resume from scratch". This skips `maw ws recover` (R23). |
| E3 | DEV L495 vs DEV L523, WRK L379 | "maw ws merge may miss uncommitted worker changes" vs "merges uncommitted files". The sandbox shows the second is true. |
| E4 | doc worker-loop:180-183, doc finish:36, 43-44 | The worker runs a bare `maw ws merge` for reviewed work, then `maw push`. This contradicts R4 and R27. |
| E5 | Template L193-195 vs doc security-review:90-91, 142-144 | Who sends `review-done`: the template says the reviewer; the doc says the author, and the doc is right. |
| E6 | doc worker-loop:143 | A general review is done by a subagent, with no Seal verdict check (R34) |
| E7 | doc worker-loop:101-103 | A cross-project ask without an anchor or a wait (R42/R44) |
| E8 | doc cross-channel:107 | "@mention … so their hook fires". Stale: the router hook fires on claims, so the mention is harmless but the reason is wrong. |
| E9 | P/shell.rs L387, L418 | Protocol output always emits `maw exec default -- bn`, even in root layout. It works, but differs from the root-layout rule in the template. |
| E10 | Template L8/L9 order vs docs finish:17, worker-loop:179 | The template merges, then runs `bn done`. The docs and protocol merge (which requires the bone closed, merge-check:18) do `bn done` first. The rewrite should follow the protocol order. |
| E11 | Template R8, R11, R12, R21 | False or overstated tool claims (see the table). Do not carry them into the rewrite. |

## (c) Conditionals the rewrite must preserve

The template context comes from `src/template.rs` `TemplateContext` and `LayoutVars` (L20-72).

- **`is_root_layout`**: L7 (trunk = repo root vs `ws/default/`) and L25-68 (the whole Directory Structure block, including root-direct `bn` vs `maw exec default -- bn`). A test, `src/template.rs` L565-579, asserts the following:
  - bare output contains `"bare repo"`, `"maw exec default -- bn triage"` and ``"never in `ws/default/`"``, and no `.maw/workspaces`;
  - root output contains `.maw/workspaces` and the literal ``"| Triage (scores) | `bn triage` |"`` (from the bones table, which is recommended for removal), and no `maw exec default -- bn` or `bare repo`.

  The rewrite must update these assertions, not just drop the table.
- **`{{ bn }}`** (`bn` or `maw exec default -- bn`): L5, L9, L74-83, L182, L303.
- **`{{ ws_prefix }}`** (`.maw/workspaces/` or `ws/`): L6, L7.
- **`check_command`**: L145, with the fallback text "`just check` (or your project's build/test command)" when unset.
- **`install_command`**: L147 (legacy).
- **`release_instructions`**: L148-151 (dedented block).
- **`design_docs`**: L363-370 (filtered by project type).
- **`workflow_docs`**: L371-375.
  - The list is hard-coded in `template.rs` L135-205.
  - `coordination.md` and `mission.md` exist in `src/templates/docs/` but are **not** in the index.
  - `groom.md`'s description is just "groom".
- **Also in the context but unused by the template:** `tools` (bones/maw/seal/rite/vessel toggles), `review` (`enabled`, `reviewers`), `project.*`, `seal_default`, `trunk_path`, `default_prefix`.
  - **The current section ignores tool toggles.** A project with `seal: false` or `rite: false` still gets the Seal and rite rules.
  - The rewrite can gate the seal and rite bullets and pointers on `tools.seal` and `tools.rite`, or keep today's behaviour on purpose; it should decide explicitly.
  - Gating is cheap and cuts tokens further for projects that don't use those tools.
- **Outside the markers**, `render_agents_md` writes the `Tools:` and `Reviewer roles:` header (`template.rs` L241-270). A test asserts `"## Edict Workflow"` in the output (L621), so keep that heading or update the test.

## (d) Draft target Rules list

Pointer header (one line per tool):

```
Tools: bones `bn tldr` · rite `rite tldr` · maw `maw --help` · seal `seal --help` · edict `edict protocol --help`.
Where a tool's quick start differs from the rules below (e.g. `--from origin/main`, bare `maw ws merge`,
`rite wait --mention`), these rules win. Identity: `$AGENT` (manual sessions: `<project>-dev`).
```

Rules (12 bullets; conditionals shown as Jinja):

1. Track all work in a bone. Create it before you start (`{{ bn }} create`), start it with `{{ bn }} do`, post progress with `{{ bn }} bone comment add`, and close it with `{{ bn }} done`.
2. Edit only in a maw workspace named after the bone: `maw ws create <bone-id> --from main`, at `{{ ws_prefix }}<bone-id>/`. Never edit {% if is_root_layout %}the trunk at the repo root{% else %}`ws/default/`{% endif %} directly, and never create git branches. {% if is_root_layout %}Run `bn` at the repo root{% else %}Run bones through `maw exec default -- bn`{% endif %}; run seal through `maw exec <ws> -- seal`.
3. Never merge or destroy `default`. It is the merge target.
4. At each transition, run the matching `edict protocol resume|start|review|finish|merge|cleanup … --agent $AGENT`, and run the steps it prints, in order. If it exits 1, follow the workflow doc.
5. Reviewed work merges only through `edict protocol merge <ws> --message "feat: …"`. A bare `maw ws merge` skips the review log, the uncommitted-changes check and the `risk:critical` gate. Do not commit code after the LGTM; only the review log.
6. A conflicted workspace is a normal state, not a failure: `maw ws resolve <ws> --list`.
7. Before you conclude work is lost, or start a bone over, run `maw ws recover`.
8. Seal: completion of a review session is not approval. Confirm the verdict with `seal review <id> --format json`. `--reviewers` spawns nobody. For security review, follow security-review.md, and never `@<project>-security`.
9. Answer the message that woke you with `rite send --reply-to`. The anchor is `$RITE_MESSAGE_ID`, or the LAST id in `$RITE_BATCH_MESSAGE_IDS`. Never reuse an earlier turn's anchor.
10. To ask, capture the id (`--format json | jq -r .id`), then `rite wait --reply-to <id> -t 300`. On exit 1, post one `-L task-blocked` and move on. Never re-send.
11. Run {% if check_command %}`{{ check_command }}`{% else %}the project's check command{% endif %} before committing. Workers do not push; the lead merges and pushes.
12. Keep rite messages to one labelled line that leads with the bone id, e.g. `[task-blocked] Blocked on <thing>: <what unblocks it>`.

Kept outside the Rules list: the conditional Release block (`install_command`, `release_instructions`), Design Guidelines, and the Workflow Docs index.

Open questions for the user:
- Rule 3 is tool-enforced by maw, so it could become a pointer. It stays here because it states intent in one line.
- "Confirm before destructive actions" disappears with R47. Decide whether it earns a bullet.
- Gate rules 8-10 and 12 on `tools.seal` / `tools.rite`?

## (e) Where the dropped detail now lives (bn-3ox9)

Every MOVE row, and the long-form detail behind KEEP and POINTER rows, now has one home. Paths are `src/templates/docs/` (rendered to `.agents/edict/`). The rewrite (bn-21zp) can drop the template text in the "Template text" column and link to the heading instead.

| # | Template text | Now lives in (doc › heading) |
|---|---|---|
| R6 | Swap `default` for a change id | merge-check.md › Merge steps (step 2); finish.md › Steps 7 |
| R10-R11 | Conflicts are data; `resolve --list/--keep`; `merge --resolve/--resolve-all`; `ws conflicts` | merge-check.md › Conflict recovery (steps 1-3). The false "merge auto-syncs stale sources" is removed there. |
| R11, R12 | Stale sources; the hard gate and `--force` | merge-check.md › Conflict recovery › **Merge gates** (new). finish.md › Full recovery now links there instead of repeating it. |
| R22 | Commit before you request review | review-request.md › **Before you request review** (new) |
| R33 | Retarget before re-requesting; the re-review snippet | review-request.md › Create and launch an exact security review; review-response.md › Steps 3c; security-review.md › Launch contract (closing paragraph). Protocol output emits it too. |
| R32, R34, R35 | `--reviewers` spawns nobody; Agentbus completion is not approval; Agentbus failure → one anchored `task-blocked` | security-review.md › **Who sends what** (new) and › Launch contract; review-request.md › Terminal rules |
| R36 | Who sends `review-done` | security-review.md › **Who sends what**: the author sends it after verifying Seal; the reviewer sends no Rite messages. |
| R37 | Fork point, printed range, `--base`, persisted base | review-request.md › **What a review covers** (new). worker-loop.md step 5 now links there. |
| R38 | No code after the LGTM; `mark-merged` exits 1; fresh LGTM; `--allow-stale-approval` with a bone comment; `seal diff --format json` fields | review-response.md › Commit no code after the LGTM (plus "a re-review must end in a vote") |
| R39, R40 | Review-log sequence; clean check in the same command; never `mark-merged` after the merge; never move or delete `.seal/reviews`; merge also merges uncommitted changes | merge-check.md › **The review log and the clean check** (new, the one source for the rules) and › Merge steps (the exact commands). finish.md step 6 summarizes and links there. |
| R42 | Ask a companion tool's channel; cross-project ask-and-wait; tracking bone on exit 1 | cross-channel.md › Steps: ask another project (1-4) |
| R43 | Threads: reply anchoring, `$RITE_MESSAGE_ID`, batch anchor = LAST id, missing-anchor warning, `complete:false`, never reuse an earlier anchor | cross-channel.md › **Threads** (new) |
| R44 | Ask and wait: anchored send, `rite wait --reply-to <id> -t <secs>`, exit 0/1/2 table, how filters narrow, own reply never satisfies, always pass `-t` | cross-channel.md › **Ask and wait** (new) |
| R46 | Message shape and the `[task-claim]` / `[review-request]` / `[task-blocked]` examples | cross-channel.md › **Message shape** (new) |

Notes for bn-21zp:

- **Batch anchor.** Verified in rite `src/cli/hooks.rs` (spawn path, ~L1512-1521): `RITE_BATCH_MESSAGE_IDS` is chronological with the triggering message last, and `RITE_MESSAGE_ID` is that same triggering id. Rule 9 can say "answer `$RITE_MESSAGE_ID`" and link to cross-channel.md › Threads for the batch detail. Per the bn-3ox9 comment, do not restate the LAST-id rule in the Rules list as well.
- **`rite wait` default.** `-t` defaults to 0, meaning no timeout (`rite wait --help`, rite 0.35.1). Rule 10's `-t 300` example covers this.
- **Merge gates, corrected** (maw 1.0.0-pre.16, `crates/maw-cli/src/workspace/merge.rs`): a stale source is refused (L1555-1567, called at L2166 and L5456); `--force` bypasses only the recorded-conflict gate (L3255-3270); the placeholder-in-HEAD and header-only gates cannot be bypassed (L3216-3300).
- **Retarget clears votes** (seal 0.30.0 `core/reviews.rs` L182-187): now stated in review-request.md › What a review covers.
- **Template lines corrected in place** (not deleted, so the rewrite still owns the removal): the `auto-syncs stale sources` bullet, the `--force` hard-gate bullet, and the "Dedicated reviewers reply … review-done" paragraph.
- **Also fixed:** worker-loop.md step 4 (E7) now asks with an anchor and a wait; cross-channel.md Notes (E8) no longer says the mention fires a hook.
- **Still wrong, outside this bone:** `src/commands/protocol/merge.rs` L671 and L814 say "`maw ws merge` auto-syncs stale sources". Protocol code was out of scope for bn-3ox9.
- **Docs index descriptions** (template.rs `list_workflow_docs`) are unchanged. Suggested for the rewrite: cross-channel.md "Rite threads, ask-and-wait, message shape, cross-project asks"; review-request.md "Request a review: commit first, review range, retarget before re-request"; merge-check.md "Merge a workspace: review log, clean check, merge gates, conflict recovery".

