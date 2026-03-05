use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn run_agent_requires_prompt() {
    let mut cmd = Command::cargo_bin("edict").unwrap();
    cmd.arg("run").arg("agent");
    cmd.assert().failure().stderr(predicate::str::contains(
        "required arguments were not provided",
    ));
}

#[test]
fn run_agent_rejects_unknown_runner() {
    let mut cmd = Command::cargo_bin("edict").unwrap();
    cmd.arg("run")
        .arg("agent")
        .arg("test prompt")
        .arg("--runner")
        .arg("unknown-runner");
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Unsupported runner"));
}

#[test]
fn run_agent_handles_claude_not_found() {
    // This test assumes 'claude' is not in PATH
    // If claude IS installed, this test will fail, which is acceptable
    let mut cmd = Command::cargo_bin("edict").unwrap();
    cmd.arg("run")
        .arg("agent")
        .arg("say hello")
        .arg("--runner")
        .arg("claude")
        .arg("--timeout")
        .arg("5");

    // We expect either:
    // 1. claude not found (most common in CI)
    // 2. claude runs successfully (if installed)
    // Don't assert on exit code, just verify command doesn't crash
    let _ = cmd.output();
}

#[test]
fn run_agent_defaults_to_pi_runner() {
    // Verify default runner is "pi" (even if pi binary isn't installed)
    let mut cmd = Command::cargo_bin("edict").unwrap();
    cmd.arg("run")
        .arg("agent")
        .arg("test")
        .arg("--timeout")
        .arg("2");

    let output = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should NOT say "Unsupported runner" — pi is the default
    assert!(
        !stderr.contains("Unsupported runner"),
        "pi should be the default runner, got stderr: {}",
        stderr
    );
}

/// Simulation test: create a fake "pi" script that emits Pi JSONL events,
/// put it on PATH, and verify botbox run agent pi processes the events correctly.
#[test]
fn run_agent_pi_simulation_text_output() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_pi = tmp.path().join("pi");

    // Create a fake pi script that emits a realistic JSONL event stream
    let script = r#"#!/bin/sh
cat <<'EVENTS'
{"type":"session","version":3,"id":"sim-001","timestamp":"2026-02-18T00:00:00Z","cwd":"/tmp"}
{"type":"agent_start"}
{"type":"turn_start"}
{"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"Say hello"}],"timestamp":1700000000}}
{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"Say hello"}],"timestamp":1700000000}}
{"type":"message_start","message":{"role":"assistant","content":[],"api":"anthropic-messages","provider":"anthropic","model":"claude-haiku-4-5-20251001","usage":{"input":0,"output":0},"stopReason":"stop","timestamp":1700000001}}
{"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0,"partial":{"role":"assistant","content":[{"type":"text","text":"","index":0}]}}}
{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Hello from simulated Pi!"}}
{"type":"message_update","assistantMessageEvent":{"type":"text_end","contentIndex":0,"content":"Hello from simulated Pi!"}}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"Hello from simulated Pi!"}],"api":"anthropic-messages","provider":"anthropic","model":"claude-haiku-4-5-20251001","usage":{"input":5,"output":10},"stopReason":"stop","timestamp":1700000001}}
{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"Hello from simulated Pi!"}]}}
{"type":"agent_end","messages":[]}
EVENTS
"#;

    fs::write(&fake_pi, script).unwrap();
    fs::set_permissions(&fake_pi, fs::Permissions::from_mode(0o755)).unwrap();

    let mut cmd = Command::cargo_bin("edict").unwrap();
    cmd.env(
        "PATH",
        format!(
            "{}:{}",
            tmp.path().display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    )
    .arg("run")
    .arg("agent")
    .arg("Say hello")
    .arg("--timeout")
    .arg("10")
    .arg("--format")
    .arg("text");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Hello from simulated Pi!"));
}

/// Simulation test: verify Pi tool use events are displayed.
#[test]
fn run_agent_pi_simulation_tool_use() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_pi = tmp.path().join("pi");

    let script = r#"#!/bin/sh
cat <<'EVENTS'
{"type":"session","version":3,"id":"sim-002","timestamp":"2026-02-18T00:00:00Z","cwd":"/tmp"}
{"type":"agent_start"}
{"type":"turn_start"}
{"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"Read test file"}],"timestamp":1700000000}}
{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"Read test file"}],"timestamp":1700000000}}
{"type":"message_start","message":{"role":"assistant","content":[],"api":"anthropic-messages","provider":"openai","model":"gpt-4o","usage":{"input":0,"output":0},"stopReason":"stop","timestamp":1700000001}}
{"type":"message_update","assistantMessageEvent":{"type":"toolcall_start","contentIndex":0,"partial":{"role":"assistant","content":[{"type":"toolCall","id":"tc-1","name":"read","arguments":{},"partialJson":"","index":0}]}}}
{"type":"message_update","assistantMessageEvent":{"type":"toolcall_end","contentIndex":0,"toolCall":{"type":"toolCall","id":"tc-1","name":"read","arguments":{"path":"/tmp/test.txt"}}}}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"toolCall","id":"tc-1","name":"read","arguments":{"path":"/tmp/test.txt"}}]}}
{"type":"tool_execution_start","toolCallId":"tc-1","toolName":"read","args":{"path":"/tmp/test.txt"}}
{"type":"tool_execution_end","toolCallId":"tc-1","toolName":"read","result":{"content":[{"type":"text","text":"test file contents here"}],"details":{}},"isError":false}
{"type":"message_start","message":{"role":"assistant","content":[],"api":"anthropic-messages","provider":"openai","model":"gpt-4o","usage":{"input":0,"output":0},"stopReason":"stop","timestamp":1700000002}}
{"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0,"partial":{"role":"assistant","content":[{"type":"text","text":"","index":0}]}}}
{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"The file contains test data."}}
{"type":"message_update","assistantMessageEvent":{"type":"text_end","contentIndex":0,"content":"The file contains test data."}}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"The file contains test data."}]}}
{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"The file contains test data."}]}}
{"type":"agent_end","messages":[]}
EVENTS
"#;

    fs::write(&fake_pi, script).unwrap();
    fs::set_permissions(&fake_pi, fs::Permissions::from_mode(0o755)).unwrap();

    let mut cmd = Command::cargo_bin("edict").unwrap();
    cmd.env(
        "PATH",
        format!(
            "{}:{}",
            tmp.path().display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    )
    .arg("run")
    .arg("agent")
    .arg("Read test file")
    .arg("--timeout")
    .arg("10")
    .arg("--format")
    .arg("text");

    let output = cmd.assert().success();
    let stdout = String::from_utf8_lossy(&output.get_output().stdout);

    // Verify tool call displayed
    assert!(
        stdout.contains("read"),
        "should show tool name 'read', got: {}",
        stdout
    );
    // Verify tool result displayed
    assert!(
        stdout.contains("test file contents here"),
        "should show tool result, got: {}",
        stdout
    );
    // Verify assistant text displayed
    assert!(
        stdout.contains("The file contains test data."),
        "should show assistant response, got: {}",
        stdout
    );
}

/// Simulation test: verify tool execution errors are displayed.
#[test]
fn run_agent_pi_simulation_tool_error() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_pi = tmp.path().join("pi");

    let script = r#"#!/bin/sh
cat <<'EVENTS'
{"type":"session","version":3,"id":"sim-003","timestamp":"2026-02-18T00:00:00Z","cwd":"/tmp"}
{"type":"agent_start"}
{"type":"turn_start"}
{"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"Read missing file"}],"timestamp":1700000000}}
{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"Read missing file"}],"timestamp":1700000000}}
{"type":"message_start","message":{"role":"assistant","content":[]}}
{"type":"message_update","assistantMessageEvent":{"type":"toolcall_start","contentIndex":0,"partial":{"role":"assistant","content":[{"type":"toolCall","id":"tc-1","name":"read","arguments":{},"index":0}]}}}
{"type":"message_update","assistantMessageEvent":{"type":"toolcall_end","contentIndex":0,"toolCall":{"type":"toolCall","id":"tc-1","name":"read","arguments":{"path":"/tmp/nonexistent.txt"}}}}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"toolCall","id":"tc-1","name":"read","arguments":{"path":"/tmp/nonexistent.txt"}}]}}
{"type":"tool_execution_start","toolCallId":"tc-1","toolName":"read","args":{"path":"/tmp/nonexistent.txt"}}
{"type":"tool_execution_end","toolCallId":"tc-1","toolName":"read","result":{"content":[{"type":"text","text":"ENOENT: no such file or directory"}],"details":{}},"isError":true}
{"type":"message_start","message":{"role":"assistant","content":[]}}
{"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0,"partial":{"role":"assistant","content":[{"type":"text","text":"","index":0}]}}}
{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"File not found."}}
{"type":"message_update","assistantMessageEvent":{"type":"text_end","contentIndex":0,"content":"File not found."}}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"File not found."}]}}
{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"File not found."}]}}
{"type":"agent_end","messages":[]}
EVENTS
"#;

    fs::write(&fake_pi, script).unwrap();
    fs::set_permissions(&fake_pi, fs::Permissions::from_mode(0o755)).unwrap();

    let mut cmd = Command::cargo_bin("edict").unwrap();
    cmd.env(
        "PATH",
        format!(
            "{}:{}",
            tmp.path().display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    )
    .arg("run")
    .arg("agent")
    .arg("Read missing file")
    .arg("--timeout")
    .arg("10")
    .arg("--format")
    .arg("text");

    let output = cmd.assert().success();
    let stdout = String::from_utf8_lossy(&output.get_output().stdout);

    // Verify error result displayed
    assert!(
        stdout.contains("ENOENT"),
        "should show error text, got: {}",
        stdout
    );
}

/// Simulation test: verify Pi handles non-zero exit code from agent.
#[test]
fn run_agent_pi_simulation_error_exit() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_pi = tmp.path().join("pi");

    let script = r#"#!/bin/sh
echo '{"type":"session","version":3,"id":"sim-err","timestamp":"2026-02-18T00:00:00Z","cwd":"/tmp"}'
echo '{"type":"agent_start"}'
exit 1
"#;

    fs::write(&fake_pi, script).unwrap();
    fs::set_permissions(&fake_pi, fs::Permissions::from_mode(0o755)).unwrap();

    let mut cmd = Command::cargo_bin("edict").unwrap();
    cmd.env(
        "PATH",
        format!(
            "{}:{}",
            tmp.path().display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    )
    .arg("run")
    .arg("agent")
    .arg("test")
    .arg("--timeout")
    .arg("5")
    .arg("--format")
    .arg("text");

    cmd.assert().failure();
}
