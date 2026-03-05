use std::path::{Path, PathBuf};
use std::process;

use anyhow::Context;

use crate::config::Config;
use crate::subprocess::Tool;

/// Worker loop state and configuration.
pub struct WorkerLoop {
    project_root: PathBuf,
    agent: String,
    project: String,
    model_pool: Vec<String>,
    timeout: u64,
    review_enabled: bool,
    critical_approvers: Vec<String>,
    dispatched_bone: Option<String>,
    dispatched_workspace: Option<String>,
    dispatched_mission: Option<String>,
    dispatched_siblings: Option<String>,
    dispatched_mission_outcome: Option<String>,
    dispatched_file_hints: Option<String>,
}

impl WorkerLoop {
    /// Create a new worker loop instance.
    pub fn new(
        project_root: Option<PathBuf>,
        agent: Option<String>,
        model: Option<String>,
    ) -> anyhow::Result<Self> {
        let project_root = project_root
            .or_else(|| std::env::current_dir().ok())
            .context("determining project root")?;

        // Find and load config
        let config = load_config(&project_root)?;

        // Agent name: CLI arg > auto-generated (empty for worker)
        let agent = agent.unwrap_or_default();

        // Set AGENT and BOTBUS_AGENT env so spawned tools resolve identity correctly
        // SAFETY: single-threaded at this point in startup, before spawning any threads
        if !agent.is_empty() {
            unsafe {
                std::env::set_var("AGENT", &agent);
                std::env::set_var("BOTBUS_AGENT", &agent);
            }
        }

        // Apply config [env] vars to our own process so tools we invoke (cargo, etc.) inherit them
        let resolved_env = config.resolved_env();
        for (k, v) in &resolved_env {
            // SAFETY: single-threaded at startup
            unsafe {
                std::env::set_var(k, v);
            }
        }

        // Emit startup diagnostic for build-related env vars.
        // This confirms whether vars from .botbox.toml [env] actually reach
        // the process where cargo runs, helping diagnose OOM issues from
        // unthrottled parallel builds in multi-agent setups.
        emit_build_env_diagnostic(&resolved_env);

        // Project name from config
        let project = config.channel();

        // Model: CLI arg > config > default, then resolve to pool for fallback
        let worker_config = config.agents.worker.as_ref();
        let model_raw = model
            .or_else(|| worker_config.map(|w| w.model.clone()))
            .unwrap_or_default();
        let model_pool = config.resolve_model_pool(&model_raw);

        let timeout = worker_config.map(|w| w.timeout).unwrap_or(900);
        let review_enabled = config.review.enabled;
        let critical_approvers = config
            .project
            .critical_approvers
            .clone()
            .unwrap_or_default();

        // Dispatched worker env vars (set by dev-loop)
        // Validate bone ID format: bd-XXXX (alphanumeric + hyphens)
        let dispatched_bone = std::env::var("EDICT_BONE")
            .or_else(|_| std::env::var("EDICT_BEAD"))
            .ok()
            .filter(|v| {
                !v.is_empty()
                    && v.len() <= 20
                    && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            });
        // Validate workspace name: lowercase alphanumeric + hyphens, no path components
        let dispatched_workspace = std::env::var("EDICT_WORKSPACE").ok().filter(|v| {
            !v.is_empty()
                && v.len() <= 64
                && v.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                && !v.starts_with('-')
                && !v.contains("..")
        });
        // Mission ID has same format as bone ID
        let dispatched_mission = std::env::var("EDICT_MISSION").ok().filter(|v| {
            !v.is_empty()
                && v.len() <= 20
                && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
        // Siblings and file hints are informational — limit size to prevent prompt bloat
        let dispatched_siblings = std::env::var("EDICT_SIBLINGS").ok().map(|v| {
            if v.len() > 4096 {
                v[..v.floor_char_boundary(4096)].to_string()
            } else {
                v
            }
        });
        let dispatched_mission_outcome = std::env::var("EDICT_MISSION_OUTCOME").ok().map(|v| {
            if v.len() > 2048 {
                v[..v.floor_char_boundary(2048)].to_string()
            } else {
                v
            }
        });
        let dispatched_file_hints = std::env::var("EDICT_FILE_HINTS").ok().map(|v| {
            if v.len() > 4096 {
                v[..v.floor_char_boundary(4096)].to_string()
            } else {
                v
            }
        });

        Ok(Self {
            project_root,
            agent,
            project,
            model_pool,
            timeout,
            review_enabled,
            critical_approvers,
            dispatched_bone,
            dispatched_workspace,
            dispatched_mission,
            dispatched_siblings,
            dispatched_mission_outcome,
            dispatched_file_hints,
        })
    }

    /// Run one iteration of the worker loop.
    pub fn run_once(&self) -> anyhow::Result<LoopStatus> {
        // Set up cleanup handlers
        register_cleanup_handlers(&self.agent, &self.project);

        // Build prompt for Claude
        let prompt = self.build_prompt();

        // Run agent via edict run agent (Pi by default), with rate limit fallback
        let start = crate::telemetry::metrics::time_start();
        let output = run_agent_with_fallback(&prompt, &self.model_pool, self.timeout)?;
        crate::telemetry::metrics::time_record(
            "edict.worker.agent_run_duration_seconds",
            start,
            &[("agent", &self.agent), ("project", &self.project)],
        );

        // Parse completion signal
        let status = parse_completion_signal(&output);
        crate::telemetry::metrics::counter(
            "edict.worker.runs_total",
            1,
            &[("agent", &self.agent), ("project", &self.project)],
        );

        Ok(status)
    }

    /// Build the worker loop prompt.
    fn build_prompt(&self) -> String {
        let dispatched_section = if let (Some(bone), Some(ws)) =
            (&self.dispatched_bone, &self.dispatched_workspace)
        {
            let mission_section = if let Some(ref mission) = self.dispatched_mission {
                let outcome = if let Some(ref outcome) = self.dispatched_mission_outcome {
                    format!("Mission outcome: {outcome}")
                } else {
                    format!("Read mission context: maw exec default -- bn show {mission}")
                };

                let siblings = if let Some(ref sibs) = self.dispatched_siblings {
                    format!("\nSibling bones (other workers in this mission):\n{sibs}")
                } else {
                    String::new()
                };

                let file_hints = if let Some(ref hints) = self.dispatched_file_hints {
                    format!(
                        "\nAdvisory file ownership (avoid editing files owned by siblings):\n{hints}"
                    )
                } else {
                    String::new()
                };

                format!("Mission: {mission}\n{outcome}{siblings}{file_hints}")
            } else {
                String::new()
            };

            let ws_path = self.project_root.join("ws").join(ws);

            format!(
                r#"## DISPATCHED WORKER — FAST PATH

You were dispatched by a lead dev agent with a pre-assigned bone and workspace.
Skip steps 0 (RESUME CHECK), 1 (INBOX), and 2 (TRIAGE) entirely.

Pre-assigned bone: {bone}
Pre-assigned workspace: {ws}
Workspace path: {ws_path}
{mission_section}

Go directly to:
1. Verify your bone: maw exec default -- bn show {bone}
2. Verify your workspace: maw ws list (confirm {ws} exists)
3. Your bone is already doing and claimed. Proceed to step 4 (WORK).
   Use absolute workspace path: {ws_path}
   For commands in workspace: maw exec {ws} -- <command>

"#,
                bone = bone,
                ws = ws,
                ws_path = ws_path.display(),
                mission_section = mission_section,
            )
        } else {
            String::new()
        };

        let dispatched_intro = if self.dispatched_bone.is_some() {
            "You are a dispatched worker — follow the FAST PATH section below."
        } else {
            r#"Execute exactly ONE cycle of the worker loop. Complete one task (or determine there is no work),
then STOP. Do not start a second task — the outer loop handles iteration."#
        };

        let review_step_6 = if self.review_enabled {
            format!(
                r#"6. REVIEW REQUEST (risk-aware):
   First, check the bone's risk label: maw exec default -- bn show <id> — look for risk:low, risk:high, or risk:critical labels.
   No risk label = risk:medium (standard review, current default).

   RISK:LOW PATH (evals, docs, tests, config) — Self-review and merge directly:
     No security review needed regardless of REVIEW setting.
     Add self-review comment: maw exec default -- bn bone comment add <id> "Self-review (risk:low): <what you verified>"
     Proceed directly to step 7 (FINISH).

   RISK:MEDIUM PATH — Standard review (current default):
     Try protocol command: edict protocol review <bone-id> --agent {agent}
     Read the output carefully. If status is Ready, run the suggested commands.
     If it fails (exit 1 = command unavailable), fall back to manual review request:
       CHECK for existing review first:
         - Run: maw exec default -- bn comments <id> | grep "Review created:"
         - If found, extract <review-id> and skip to requesting review (don't create duplicate)
       Create review with reviewer assignment (only if none exists):
         - maw exec $WS -- crit reviews create --agent {agent} --title "<id>: <title>" --description "<summary>" --reviewers {project}-security
         - IMMEDIATELY record: maw exec default -- bn bone comment add <id> "Review created: <review-id> in workspace $WS"
       bus statuses set --agent {agent} "Review: <review-id>".
       Spawn reviewer via @mention: bus send --agent {agent} {project} "Review requested: <review-id> for <id> @{project}-security" -L review-request
     Do NOT close the bone. Do NOT merge. Do NOT release claims.
     Output: <promise>COMPLETE</promise>
     STOP this iteration.

   RISK:HIGH PATH — Security review + failure-mode checklist:
     Same as risk:medium, but when creating the review, add to description: "risk:high — failure-mode checklist required."
     The security reviewer will include the 5 failure-mode questions in their review:
       1. What could fail in production?  2. How would we detect it quickly?
       3. What is the fastest safe rollback?  4. What dependency could invalidate this plan?
       5. What assumption is least certain?
     MUST request security reviewer. Do not skip.
     STOP this iteration.

   RISK:CRITICAL PATH — Security review + human approval required:
     Same as risk:high, but ALSO:
     - Add to review description: "risk:critical — REQUIRES HUMAN APPROVAL before merge."
     - Post to bus requesting human approval:
       bus send --agent {agent} {project} "risk:critical review for <id>: requires human approval before merge. {critical_approvers}" -L review-request
     STOP this iteration."#,
                agent = self.agent,
                project = self.project,
                critical_approvers = if self.critical_approvers.is_empty() {
                    "Check project.critical_approvers in .edict.toml".to_string()
                } else {
                    format!("Approvers: {}", self.critical_approvers.join(", "))
                }
            )
        } else {
            r#"   REVIEW is disabled. Skip code review.
   Proceed directly to step 7 (FINISH)."#
                .to_string()
        };

        let finish_step_7 = if self.dispatched_bone.is_some() {
            format!(
                r#"7. FINISH (dispatched worker — lead handles merge):
   Try protocol command: edict protocol finish <bone-id> --agent {agent} --no-merge
   Read the output carefully. If status is Ready, run the suggested commands.
   If it fails (exit 1 = command unavailable), fall back to manual finish:
     Close bone: maw exec default -- bn done <id> --reason "Completed"
     Announce: bus send --agent {agent} {project} "Completed <id>: <title>" -L task-done
     Release bone claim: bus claims release --agent {agent} "bone://{project}/<id>"
     Do NOT merge the workspace — the lead dev will handle merging via the merge protocol.
     Do NOT run the release check — the lead handles releases.
   Output: <promise>COMPLETE</promise>"#,
                agent = self.agent,
                project = self.project,
            )
        } else {
            format!(
                r#"7. FINISH (only reached after LGTM from step 0, or after step 6 when REVIEW is false):
   Try protocol command: edict protocol finish <bone-id> --agent {agent}
   Read the output carefully. If status is Ready, run the suggested commands.
   If it fails (exit 1 = command unavailable), fall back to manual finish:
     If a review was conducted:
       maw exec default -- crit reviews mark-merged <review-id> --agent {agent}.
     RISK:CRITICAL CHECK — Before merging a risk:critical bone:
       Verify human approval exists: bus history {project} -n 50 -L review-request | look for approval message referencing this bone/review from an authorized approver.
       If no approval found, do NOT merge. Post: bus send --agent {agent} {project} "Waiting for human approval on risk:critical <id>" -L review-request. STOP.
       If approval found, record it: maw exec default -- bn bone comment add <id> "Human approval: <approver> via bus message <msg-id>"
     maw exec default -- bn bone comment add <id> "Completed by {agent}".
     maw exec default -- bn done <id> --reason "Completed" --suggest-next.
     bus send --agent {agent} {project} "Completed <id>: <title>" -L task-done.
     bus claims release --agent {agent} "bone://{project}/<id>".
     Keep workspace claim — the lead will merge it.
   STOP — do not proceed to RELEASE CHECK (only leads check for releases after merging)."#,
                agent = self.agent,
                project = self.project,
            )
        };

        let review_status_str = if self.review_enabled { "true" } else { "false" };
        let review_note = if self.review_enabled {
            "risk:low (evals, docs, tests, config) skips security review — self-review and merge directly. risk:medium gets standard review. risk:high requires failure-mode checklist. risk:critical requires human approval."
        } else {
            "Review is disabled. Skip review and proceed to FINISH after describing commit."
        };

        format!(
            r#"You are worker agent "{agent}" for project "{project}".

IMPORTANT: Use --agent {agent} on ALL bus and crit commands. bn resolves agent identity from $AGENT/$BOTBUS_AGENT env automatically. Set EDICT_PROJECT={project}.

CRITICAL - HUMAN MESSAGE PRIORITY: If you see a system reminder with "STOP:" showing unread bus messages, these are from humans or other agents trying to reach you. IMMEDIATELY check inbox and respond before continuing your current task. Human questions, clarifications, and redirects take priority over heads-down work.

COMMAND PATTERN — maw exec: All bn commands run in the default workspace. All crit commands run in their workspace.
  bn:   maw exec default -- bn <args>
  crit: maw exec $WS -- crit <args>
  other: maw exec $WS -- <command>           (cargo test, etc.)

VERSION CONTROL: This project uses Git + maw. Do NOT run jj commands.
  Workers commit with: maw exec $WS -- git add -A && maw exec $WS -- git commit -m "<message>"
  The lead handles merging workspaces into main.

{dispatched}{dispatched_intro}

At the end of your work, output exactly one of these completion signals:
- <promise>COMPLETE</promise> if you completed a task or determined there is no work
- <promise>BLOCKED</promise> if you are stuck and cannot proceed

0. RESUME CHECK (do this FIRST):
   Try protocol command: edict protocol resume --agent {agent}
   If it fails (exit 1 = command unavailable), fall back to manual resume check:
     Run: bus claims list --agent {agent} --mine
     If you hold a bone:// claim, you have an in-progress bone from a previous iteration.
     - Run: maw exec default -- bn comments <bone-id> to understand what was done before and what remains.
     - Look for workspace info in comments (workspace name and path).
     - If a "Review created: <review-id>" comment exists:
       * Find the review: maw exec $WS -- crit review <review-id>
       * Check review status: maw exec $WS -- crit review <review-id>
       * If LGTM (approved): proceed to FINISH (step 7) — merge the review and close the bone.
       * If BLOCKED (changes requested): fix the issues, then re-request review:
         1. Read threads: maw exec $WS -- crit review <review-id> (threads show inline with comments)
         2. For each unresolved thread with reviewer feedback:
            - Fix the code in the workspace (use absolute WS_PATH for file edits)
            - Reply: maw exec $WS -- crit reply <thread-id> --agent {agent} "Fixed: <what you did>"
            - Resolve: maw exec $WS -- crit threads resolve <thread-id> --agent {agent}
         3. Re-request: maw exec $WS -- crit reviews request <review-id> --reviewers {project}-security --agent {agent}
         5. Announce: bus send --agent {agent} {project} "Review updated: <review-id> — addressed feedback @{project}-security" -L review-response
         STOP this iteration — wait for re-review.
       * If PENDING (no votes yet): STOP this iteration. Wait for the reviewer.
       * If review not found: DO NOT merge or create a new review. The reviewer may still be starting up (hooks have latency). STOP this iteration and wait. Only create a new review if the workspace was destroyed AND 3+ iterations have passed since the review comment.
     - If no review comment (work was in progress when session ended):
       * Read the workspace code to see what's already done.
       * Complete the remaining work in the EXISTING workspace — do NOT create a new one.
       * After completing: maw exec default -- bn bone comment add <id> "Resumed and completed: <what you finished>".
       * Then proceed to step 6 (REVIEW REQUEST) or step 7 (FINISH).
     If no active claims: proceed to step 1 (INBOX).

1. INBOX (do this before triaging):
   Run: bus inbox --agent {agent} --channels {project} --mark-read
   For each message:
   - Task request (-L task-request or asks for work): create a bone with maw exec default -- bn create.
   - Status check or question: reply on bus, do NOT create a bone.
   - Feedback (-L feedback): if it contains a bug report, feature request, or actionable work — create a bone. Evaluate critically: is this a real issue? Is it well-scoped? Set priority accordingly. Then acknowledge on bus.
   - Announcements from other agents ("Working on...", "Completed...", "online"): ignore, no action.
   - Duplicate of existing bone: do NOT create another bone, note it covers the request.

2. TRIAGE: Check maw exec default -- bn next. If no ready bones and inbox created none, say "NO_WORK_AVAILABLE" and stop.
   GROOM each ready bone (maw exec default -- bn show <id>): ensure clear title, description with acceptance criteria
   and testing strategy, appropriate priority, and risk label. Fix anything missing, comment what you changed.
   RISK LABELS: Assess each bone for risk using these dimensions: blast radius, data sensitivity, reversibility, dependency uncertainty.
   - risk:low — typo fixes, doc updates, config tweaks (add label: bn bone tag <id> risk:low)
   - risk:medium — standard features/bugs (default, no label needed)
   - risk:high — security-sensitive, data integrity, user-visible behavior changes (add label)
   - risk:critical — irreversible actions, migrations, regulated changes (add label)
   Any agent can escalate risk upward. Downgrades require lead approval with justification comment.
   Use maw exec default -- bn --robot-next to pick exactly one small task. If the task is large, break it down with
   maw exec default -- bn create + bn triage dep add, then bn next again. If a bone is claimed
   (bus claims check --agent {agent} "bone://{project}/<id>"), skip it.

   MISSION CONTEXT: After picking a bone, check if it has a mission:bd-xxx label (visible in bn show output).
   If it does, read the mission bone for shared context:
     maw exec default -- bn show <mission-id>
   Note the mission's Outcome, Constraints, and Stop criteria. Check siblings:
     maw exec default -- bn list -l "mission:<mission-id>"
   Use this context to understand how your work fits into the larger effort.

   SIBLING COORDINATION (missions only):
   When working on a mission bone, you share the codebase with sibling workers. Coordinate through bus:

   READ siblings: Before editing a file listed in EDICT_FILE_HINTS as owned by a sibling, and periodically
   during work (~every 5 minutes), check for sibling messages:
     bus history {project} -n 10 -L "mission:<mission-id>" --since "5 minutes ago"
   Look for coord:interface messages — these tell you about API/schema/config changes siblings made.
   If a sibling changed something you depend on, adapt your implementation to match.

   POST discoveries: When you change an API, schema, config format, shared type, or exported interface
   that siblings might depend on, announce it immediately:
     bus send --agent {agent} {project} "<file>: <what changed and why>" -L coord:interface -L "mission:<mission-id>"

   COORDINATION LABELS on bus messages:
   - coord:interface — API/schema/config changes that affect siblings
   - coord:blocker — You need something from a sibling: bus send --agent {agent} {project} "Blocked by <sibling-bone>: <reason>" -L coord:blocker -L "mission:<mission-id>"
   - task-done — Signal completion: bus send --agent {agent} {project} "Completed <id>" -L task-done -L "mission:<mission-id>"

3. START: Try protocol command: edict protocol start <bone-id> --agent {agent}
   Read the output carefully. If status is Ready, run the suggested commands.
   If it fails (exit 1 = command unavailable), fall back to manual start:
     maw exec default -- bn do <id>.
     bus claims stake --agent {agent} "bone://{project}/<id>" -m "<id>".
     Create workspace: run maw ws create --random. Note the workspace name AND absolute path
     from the output (e.g., name "frost-castle", path "/abs/path/ws/frost-castle").
     Store the name as WS and the absolute path as WS_PATH.
     IMPORTANT: All file operations (Read, Write, Edit) must use the absolute WS_PATH.
     For commands in the workspace: maw exec $WS -- <command>.
     Do NOT cd into the workspace and stay there — the workspace is destroyed during finish.
     bus claims stake --agent {agent} "workspace://{project}/$WS" -m "<id>".
     maw exec default -- bn bone comment add <id> "Started in workspace $WS ($WS_PATH)".
     bus statuses set --agent {agent} "Working: <id>" --ttl 30m.
     Announce: bus send --agent {agent} {project} "Working on <id>: <title>" -L task-claim.

4. WORK: maw exec default -- bn show <id>, then implement the task in the workspace.
   If this bone is part of a mission, check bus for sibling updates BEFORE starting implementation:
     bus history {project} -n 10 -L "mission:<mission-id>" -L coord:interface
   Adapt your approach if siblings have already defined interfaces you need to consume or conform to.
   Add at least one progress comment: maw exec default -- bn bone comment add <id> "Progress: ...".

5. STUCK CHECK: If same approach tried twice, info missing, or tool fails repeatedly — you are
   stuck. maw exec default -- bn bone comment add <id> "Blocked: <details>".
   bus statuses set --agent {agent} "Blocked: <short reason>".
   bus send --agent {agent} {project} "Stuck on <id>: <reason>" -L task-blocked.
   maw exec default -- bn bone tag <id> blocked.
   Release: bus claims release --agent {agent} "bone://{project}/<id>".
   Output: <promise>BLOCKED</promise>
   Stop this cycle.

{review_step}

{finish_step}

8. CLEANUP (always run before stopping, even on error or BLOCKED):
   Try protocol command: edict protocol cleanup --agent {agent}
   If it fails (exit 1 = command unavailable), fall back to manual cleanup:
     bus statuses clear --agent {agent}
     (bn is event-sourced — no sync needed)

Key rules:
- Exactly one small task per cycle.
- Always finish or release before stopping.
- If claim denied, pick something else.
- All bus and crit commands use --agent {agent}.
- All file operations use the absolute workspace path from maw ws create output. Do NOT cd into the workspace and stay there.
- All bn commands: maw exec default -- bn ...
- All crit/git commands in a workspace: maw exec $WS -- crit/git ...
- If a tool behaves unexpectedly, report it: bus send --agent {agent} {project} "Tool issue: <details>" -L tool-issue.
- STOP after completing one task or determining no work. Do not loop.
- Always output <promise>COMPLETE</promise> or <promise>BLOCKED</promise> at the end.
- RISK LABELS: Check bone risk labels before review. REVIEW={review_status}. {review_note}"#,
            agent = self.agent,
            project = self.project,
            dispatched = dispatched_section,
            dispatched_intro = dispatched_intro,
            review_step = review_step_6,
            finish_step = finish_step_7,
            review_status = review_status_str,
            review_note = review_note,
        )
    }
}

/// Status of a loop iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopStatus {
    Complete,
    Blocked,
    Unknown,
}

/// Emit startup diagnostic for build-related environment variables.
///
/// Logs the effective values of CARGO_BUILD_JOBS, RUSTC_WRAPPER, and SCCACHE_DIR
/// to stderr. These vars control cargo parallelism and caching — critical for
/// preventing OOM when multiple agents build concurrently.
///
/// Warns if any of these vars are unset or empty, which could indicate that
/// the .edict.toml [env] configuration isn't reaching the build process.
fn emit_build_env_diagnostic(config_env: &std::collections::HashMap<String, String>) {
    use std::env;

    const BUILD_VARS: &[(&str, &str)] = &[
        ("CARGO_BUILD_JOBS", "limits parallel rustc processes"),
        ("RUSTC_WRAPPER", "enables sccache for build caching"),
        ("SCCACHE_DIR", "sccache cache directory"),
    ];

    eprintln!("--- build env diagnostic ---");

    let mut any_missing = false;
    for &(var, purpose) in BUILD_VARS {
        let effective = env::var(var).ok().filter(|v| !v.is_empty());
        let from_config = config_env.get(var);

        match (&effective, from_config) {
            (Some(val), Some(cfg_val)) if val == cfg_val => {
                eprintln!("  {var}={val} (from config)");
            }
            (Some(val), Some(cfg_val)) => {
                // Value exists but differs from config — inherited from parent process
                // or overridden by botty spawn --env flag
                eprintln!("  {var}={val} (effective; config has {cfg_val})");
            }
            (Some(val), None) => {
                // Not in config but set in environment (from botty spawn --env or parent)
                eprintln!("  {var}={val} (from environment, not in config)");
            }
            (None, Some(cfg_val)) => {
                // This shouldn't happen since we just applied config env, but
                // log it for completeness
                eprintln!("  {var}=<unset> (config has {cfg_val}, failed to apply?)");
                any_missing = true;
            }
            (None, None) => {
                eprintln!("  {var}=<unset> — warning: {purpose}");
                any_missing = true;
            }
        }
    }

    if any_missing {
        eprintln!(
            "  ⚠ Some build env vars are unset. Consider adding them to .edict.toml [env]"
        );
        eprintln!(
            "    to prevent OOM from unthrottled parallel builds in multi-agent setups."
        );
    }

    eprintln!("--- end build env diagnostic ---");
}

/// Load config from .edict.toml (or legacy .botbox.toml/.botbox.json) using the canonical priority
/// (root TOML > ws/default TOML > root JSON > ws/default JSON).
fn load_config(root: &Path) -> anyhow::Result<Config> {
    let (config_path, _config_dir) = crate::config::find_config_in_project(root)?;
    Config::load(&config_path)
}

/// Run an agent with rate limit fallback across the model pool.
///
/// Tries each model in the pool sequentially. If a model returns a rate limit error (429),
/// logs a warning and tries the next model. Returns error only when all models are exhausted
/// or a non-rate-limit error occurs.
fn run_agent_with_fallback(
    prompt: &str,
    model_pool: &[String],
    timeout: u64,
) -> anyhow::Result<String> {
    for (i, model) in model_pool.iter().enumerate() {
        if model_pool.len() > 1 {
            eprintln!("Trying model {}/{}: {}", i + 1, model_pool.len(), model);
        }
        match try_run_agent(prompt, model, timeout) {
            Ok(output) => {
                if is_rate_limit_output(&output) {
                    eprintln!(
                        "Rate limited on {} (detected in output), trying next model...",
                        model
                    );
                    crate::telemetry::metrics::counter(
                        "edict.worker.rate_limit_retries_total",
                        1,
                        &[("model", model)],
                    );
                    continue;
                }
                // Empty or near-empty output means the model hung/crashed without
                // producing useful work (e.g., Pi killed a hung Gemini process).
                // Try the next model if available.
                if output.trim().is_empty() && i + 1 < model_pool.len() {
                    eprintln!(
                        "Empty output from {} (process likely hung), trying next model...",
                        model
                    );
                    continue;
                }
                return Ok(output);
            }
            Err(e) => {
                let err_str = format!("{e:#}");
                if (is_rate_limit_error(&err_str) || err_str.contains("exited with code"))
                    && i + 1 < model_pool.len()
                {
                    eprintln!(
                        "Failed on {} ({}), trying next model...",
                        model,
                        err_str.lines().next().unwrap_or("error")
                    );
                    continue;
                }
                return Err(e);
            }
        }
    }
    anyhow::bail!(
        "All {} models in pool exhausted (rate limited)",
        model_pool.len()
    )
}

/// Check if output text indicates a rate limit error.
fn is_rate_limit_output(output: &str) -> bool {
    let lower = output.to_lowercase();
    lower.contains("429")
        && (lower.contains("rate limit")
            || lower.contains("rate_limit")
            || lower.contains("quota")
            || lower.contains("exhausted your capacity")
            || lower.contains("resource_exhausted"))
}

/// Check if an error message indicates a rate limit error.
fn is_rate_limit_error(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("429")
        || lower.contains("rate limit")
        || lower.contains("rate_limit")
        || lower.contains("quota")
        || lower.contains("resource_exhausted")
}

/// Run an agent via `edict run agent` (Pi by default).
///
/// Supports `provider/model:thinking` syntax for thinking levels.
/// Echoes output to stderr for visibility in botty while capturing stdout for parsing.
fn try_run_agent(prompt: &str, model: &str, timeout: u64) -> anyhow::Result<String> {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let timeout_string = timeout.to_string();
    let mut args = vec!["run", "agent", prompt, "-t", &timeout_string];

    // Pass the full model string (e.g. "anthropic/claude-sonnet-4-6:medium") — Pi handles :suffix natively
    if !model.is_empty() {
        args.push("-m");
        args.push(model);
    }

    let mut child = Command::new("edict")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("spawning edict run agent")?;

    let stdout = child.stdout.take().context("capturing stdout")?;
    let reader = BufReader::new(stdout);

    let mut output = String::new();
    for line in reader.lines() {
        let line = line.context("reading line from edict run agent")?;
        // Echo to stderr for visibility in botty
        eprintln!("{}", line);
        output.push_str(&line);
        output.push('\n');
    }

    let status = child.wait().context("waiting for edict run agent")?;
    if status.success() {
        Ok(output)
    } else {
        let code = status.code().unwrap_or(-1);
        anyhow::bail!("edict run agent exited with code {code}")
    }
}

/// Parse completion signal from Claude output.
fn parse_completion_signal(output: &str) -> LoopStatus {
    if output.contains("<promise>COMPLETE</promise>") {
        LoopStatus::Complete
    } else if output.contains("<promise>BLOCKED</promise>") {
        LoopStatus::Blocked
    } else {
        LoopStatus::Unknown
    }
}

/// Register cleanup handlers for SIGINT/SIGTERM.
fn register_cleanup_handlers(agent: &str, project: &str) {
    let agent = agent.to_string();
    let project = project.to_string();

    ctrlc::set_handler(move || {
        eprintln!("Received interrupt signal, cleaning up...");
        let _ = cleanup(&agent, &project);
        process::exit(0);
    })
    .expect("Error setting Ctrl-C handler");
}

/// Cleanup: release claims, clear status.
fn cleanup(agent: &str, project: &str) -> anyhow::Result<()> {
    eprintln!("Cleaning up...");

    // All subprocess spawns below use .new_process_group() so they run in their
    // own process group and survive the SIGTERM that triggered this cleanup
    // (botty kill sends SIGTERM to the parent's process group, which would
    // otherwise kill these children before they complete).

    // Reset orphaned doing bones
    let result = Tool::new("bn")
        .args(&["list", "--state", "doing", "--assignee", agent, "--json"])
        .in_workspace("default")?
        .new_process_group()
        .run();

    if let Ok(output) = result
        && let Ok(bones) = output.parse_json::<Vec<serde_json::Value>>()
    {
        for bone in bones {
            if let Some(id) = bone.get("id").and_then(|v| v.as_str()) {
                // bn doesn't have an "undo" command — just add a comment noting the orphan
                let _ = Tool::new("bn")
                    .args(&[
                        "bone",
                        "comment",
                        "add",
                        id,
                        &format!("Worker {agent} exited without completing. Needs reassignment."),
                    ])
                    .in_workspace("default")?
                    .new_process_group()
                    .run();
                eprintln!("Noted orphaned bone {id}");
            }
        }
    }

    // Sign off on bus
    let _ = Tool::new("bus")
        .args(&[
            "send",
            "--agent",
            agent,
            project,
            &format!("Agent {agent} signing off."),
            "-L",
            "agent-idle",
        ])
        .new_process_group()
        .run();

    // Clear status
    let _ = Tool::new("bus")
        .args(&["statuses", "clear", "--agent", agent])
        .new_process_group()
        .run();

    // Release agent claim
    let _ = Tool::new("bus")
        .args(&[
            "claims",
            "release",
            "--agent",
            agent,
            &format!("agent://{agent}"),
        ])
        .new_process_group()
        .run();

    // Release all claims
    let _ = Tool::new("bus")
        .args(&["claims", "release", "--agent", agent, "--all"])
        .new_process_group()
        .run();

    // bn is event-sourced — no sync step needed

    eprintln!("Cleanup complete for {agent}.");
    Ok(())
}

/// Run the worker loop.
pub fn run_worker_loop(
    project_root: Option<PathBuf>,
    agent: Option<String>,
    model: Option<String>,
) -> anyhow::Result<()> {
    let worker = WorkerLoop::new(project_root, agent, model)?;

    // Announce startup on bus (survives botty log eviction)
    let bone_info = worker
        .dispatched_bone
        .as_deref()
        .unwrap_or("(triage)");
    let ws_info = worker
        .dispatched_workspace
        .as_deref()
        .unwrap_or("(none)");
    let _ = Tool::new("bus")
        .args(&[
            "send",
            "--agent",
            &worker.agent,
            &worker.project,
            &format!("Worker started: {bone_info} in ws/{ws_info}"),
            "-L",
            "worker-lifecycle",
        ])
        .run();

    let status = worker.run_once();

    // Announce exit on bus regardless of outcome
    let exit_msg = match &status {
        Ok(LoopStatus::Complete) => format!("Worker exited OK: {bone_info} COMPLETE"),
        Ok(LoopStatus::Blocked) => format!("Worker exited OK: {bone_info} BLOCKED"),
        Ok(LoopStatus::Unknown) => format!("Worker exited: {bone_info} (no completion signal)"),
        Err(e) => format!("Worker exited ERROR: {bone_info} — {e}"),
    };
    let _ = Tool::new("bus")
        .args(&[
            "send",
            "--agent",
            &worker.agent,
            &worker.project,
            &exit_msg,
            "-L",
            "worker-lifecycle",
        ])
        .run();

    match status? {
        LoopStatus::Complete => {
            eprintln!("Worker loop completed successfully");
            Ok(())
        }
        LoopStatus::Blocked => {
            eprintln!("Worker loop blocked");
            Ok(())
        }
        LoopStatus::Unknown => {
            eprintln!("Warning: completion signal not found in output");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_completion_signal_complete() {
        let output = "some text\n<promise>COMPLETE</promise>\nmore text";
        assert_eq!(parse_completion_signal(output), LoopStatus::Complete);
    }

    #[test]
    fn parse_completion_signal_blocked() {
        let output = "error occurred\n<promise>BLOCKED</promise>";
        assert_eq!(parse_completion_signal(output), LoopStatus::Blocked);
    }

    #[test]
    fn parse_completion_signal_missing() {
        let output = "no signal here";
        assert_eq!(parse_completion_signal(output), LoopStatus::Unknown);
    }

    #[test]
    fn build_prompt_contains_agent_identity() {
        unsafe {
            std::env::set_var("EDICT_BONE", "");
            std::env::set_var("EDICT_WORKSPACE", "");
        }

        let worker = WorkerLoop {
            project_root: PathBuf::from("/test"),
            agent: "test-worker".to_string(),
            project: "testproject".to_string(),
            model_pool: vec!["haiku".to_string()],
            timeout: 900,
            review_enabled: true,
            critical_approvers: vec![],
            dispatched_bone: None,
            dispatched_workspace: None,
            dispatched_mission: None,
            dispatched_siblings: None,
            dispatched_mission_outcome: None,
            dispatched_file_hints: None,
        };

        let prompt = worker.build_prompt();
        assert!(prompt.contains("test-worker"));
        assert!(prompt.contains("testproject"));
        assert!(prompt.contains("RESUME CHECK"));
        assert!(prompt.contains("INBOX"));
        assert!(prompt.contains("TRIAGE"));
    }

    #[test]
    fn build_prompt_dispatched_fast_path() {
        unsafe {
            std::env::set_var("EDICT_BONE", "bd-test");
            std::env::set_var("EDICT_WORKSPACE", "test-ws");
        }

        let worker = WorkerLoop {
            project_root: PathBuf::from("/test"),
            agent: "test-worker".to_string(),
            project: "testproject".to_string(),
            model_pool: vec!["haiku".to_string()],
            timeout: 900,
            review_enabled: true,
            critical_approvers: vec![],
            dispatched_bone: Some("bd-test".to_string()),
            dispatched_workspace: Some("test-ws".to_string()),
            dispatched_mission: None,
            dispatched_siblings: None,
            dispatched_mission_outcome: None,
            dispatched_file_hints: None,
        };

        let prompt = worker.build_prompt();
        assert!(prompt.contains("DISPATCHED WORKER — FAST PATH"));
        assert!(prompt.contains("Pre-assigned bone: bd-test"));
        assert!(prompt.contains("Pre-assigned workspace: test-ws"));
        assert!(prompt.contains("Skip steps 0 (RESUME CHECK), 1 (INBOX), and 2 (TRIAGE)"));
    }

    #[test]
    fn build_prompt_contains_all_protocol_commands() {
        unsafe {
            std::env::set_var("EDICT_BONE", "");
            std::env::set_var("EDICT_WORKSPACE", "");
        }

        let worker = WorkerLoop {
            project_root: PathBuf::from("/test"),
            agent: "test-worker".to_string(),
            project: "testproject".to_string(),
            model_pool: vec!["haiku".to_string()],
            timeout: 900,
            review_enabled: true,
            critical_approvers: vec![],
            dispatched_bone: None,
            dispatched_workspace: None,
            dispatched_mission: None,
            dispatched_siblings: None,
            dispatched_mission_outcome: None,
            dispatched_file_hints: None,
        };

        let prompt = worker.build_prompt();

        // All 5 protocol commands must be referenced in the worker prompt
        assert!(
            prompt.contains("edict protocol resume"),
            "worker prompt must reference 'edict protocol resume'"
        );
        assert!(
            prompt.contains("edict protocol start"),
            "worker prompt must reference 'edict protocol start'"
        );
        assert!(
            prompt.contains("edict protocol review"),
            "worker prompt must reference 'edict protocol review'"
        );
        assert!(
            prompt.contains("edict protocol finish"),
            "worker prompt must reference 'edict protocol finish'"
        );
        assert!(
            prompt.contains("edict protocol cleanup"),
            "worker prompt must reference 'edict protocol cleanup'"
        );
    }

    #[test]
    fn build_prompt_review_disabled() {
        unsafe {
            std::env::set_var("EDICT_BONE", "");
            std::env::set_var("EDICT_WORKSPACE", "");
        }

        let worker = WorkerLoop {
            project_root: PathBuf::from("/test"),
            agent: "test-worker".to_string(),
            project: "testproject".to_string(),
            model_pool: vec!["haiku".to_string()],
            timeout: 900,
            review_enabled: false,
            critical_approvers: vec![],
            dispatched_bone: None,
            dispatched_workspace: None,
            dispatched_mission: None,
            dispatched_siblings: None,
            dispatched_mission_outcome: None,
            dispatched_file_hints: None,
        };

        let prompt = worker.build_prompt();
        assert!(prompt.contains("REVIEW is disabled"));
        assert!(prompt.contains("Skip code review"));
        assert!(prompt.contains("REVIEW=false"));
    }

    #[test]
    fn build_prompt_contains_protocol_fallback_wording() {
        unsafe {
            std::env::set_var("EDICT_BONE", "");
            std::env::set_var("EDICT_WORKSPACE", "");
        }

        let worker = WorkerLoop {
            project_root: PathBuf::from("/test"),
            agent: "test-worker".to_string(),
            project: "testproject".to_string(),
            model_pool: vec!["haiku".to_string()],
            timeout: 900,
            review_enabled: true,
            critical_approvers: vec![],
            dispatched_bone: None,
            dispatched_workspace: None,
            dispatched_mission: None,
            dispatched_siblings: None,
            dispatched_mission_outcome: None,
            dispatched_file_hints: None,
        };

        let prompt = worker.build_prompt();

        // Verify fallback wording is present for protocol transitions
        // This prevents silent regressions where protocol fallback guidance is removed
        assert!(
            prompt.contains("If it fails (exit 1 = command unavailable), fall back"),
            "worker prompt must contain protocol fallback wording for unavailable commands"
        );

        // Verify transition hooks are explicitly documented
        assert!(
            prompt.contains("Try protocol command:"),
            "worker prompt must guide agents to try protocol commands first"
        );

        // Verify at least one complete fallback example path is present
        // (e.g., for resume, start, review, finish, cleanup transitions)
        let fallback_patterns = [
            ("protocol resume", "resume check"),
            ("protocol start", "start"),
            ("protocol review", "review request"),
            ("protocol finish", "finish"),
            ("protocol cleanup", "cleanup"),
        ];

        for (protocol_cmd, step_name) in fallback_patterns.iter() {
            assert!(
                prompt.contains(protocol_cmd),
                "worker prompt must reference '{}' in {} step",
                protocol_cmd,
                step_name
            );
        }
    }

    #[test]
    fn rate_limit_detection_output() {
        assert!(is_rate_limit_output("Error 429: rate limit exceeded"));
        assert!(is_rate_limit_output("HTTP 429 - quota exceeded"));
        assert!(is_rate_limit_output("429 resource_exhausted"));
        assert!(is_rate_limit_output(
            "Got 429: You have exhausted your capacity"
        ));
        assert!(!is_rate_limit_output("Everything is fine"));
        assert!(!is_rate_limit_output("Error 500: server error"));
        // 429 alone without rate limit keywords should not match
        assert!(!is_rate_limit_output("429"));
    }

    #[test]
    fn rate_limit_detection_error() {
        assert!(is_rate_limit_error("429 Too Many Requests"));
        assert!(is_rate_limit_error("rate limit exceeded"));
        assert!(is_rate_limit_error("quota exhausted"));
        assert!(is_rate_limit_error("resource_exhausted"));
        assert!(!is_rate_limit_error("normal error"));
        assert!(!is_rate_limit_error("exit code 1"));
    }

    #[test]
    fn build_env_diagnostic_does_not_panic() {
        // The diagnostic function should handle all combinations of
        // set/unset vars without panicking. It writes to stderr only.
        let mut config_env = std::collections::HashMap::new();
        config_env.insert("CARGO_BUILD_JOBS".to_string(), "2".to_string());
        // RUSTC_WRAPPER not in config, SCCACHE_DIR not in config

        // Should not panic regardless of env state
        emit_build_env_diagnostic(&config_env);
    }

    #[test]
    fn build_env_diagnostic_with_empty_config() {
        let config_env = std::collections::HashMap::new();
        // All vars unset + empty config = should emit warnings without panic
        emit_build_env_diagnostic(&config_env);
    }
}
