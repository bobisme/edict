use std::cell::RefCell;
use std::io::{BufRead, BufReader, IsTerminal};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use serde_json::Value;

use crate::error::ExitError;

/// Output format: pretty (ANSI colors) or text (plain)
#[derive(Debug, Clone, Copy, PartialEq)]
enum OutputFormat {
    Pretty,
    Text,
}

impl OutputFormat {
    fn detect(explicit: Option<&str>) -> Self {
        if let Some(fmt) = explicit {
            return match fmt {
                "pretty" => OutputFormat::Pretty,
                "text" => OutputFormat::Text,
                _ => OutputFormat::Text,
            };
        }

        if let Ok(env) = std::env::var("FORMAT") {
            if env == "pretty" {
                return OutputFormat::Pretty;
            } else if env == "text" {
                return OutputFormat::Text;
            }
        }

        // TTY detection: check stdout.is_terminal() OR presence of TERM env var
        // The TERM check handles cases where we're in a PTY (like botty spawn)
        // but stdout appears as a pipe due to stream processing
        if std::io::stdout().is_terminal() {
            OutputFormat::Pretty
        } else if let Ok(term) = std::env::var("TERM") {
            // If TERM is set and not "dumb", treat as a terminal
            if !term.is_empty() && term != "dumb" {
                OutputFormat::Pretty
            } else {
                OutputFormat::Text
            }
        } else {
            OutputFormat::Text
        }
    }
}

/// ANSI codes for pretty output
struct Style {
    bold: &'static str,
    bright: &'static str,
    bold_bright: &'static str,
    dim: &'static str,
    reset: &'static str,
    green: &'static str,
    red: &'static str,
    cyan: &'static str,
    yellow: &'static str,
    bullet: &'static str,
    tool_arrow: &'static str,
    checkmark: &'static str,
}

const PRETTY_STYLE: Style = Style {
    bold: "\x1b[1m",
    bright: "\x1b[97m",
    bold_bright: "\x1b[1;97m",
    dim: "\x1b[2m",
    reset: "\x1b[0m",
    green: "\x1b[32m",
    red: "\x1b[31m",
    cyan: "\x1b[36m",
    yellow: "\x1b[33m",
    bullet: "\u{2022}",
    tool_arrow: "\u{25b6}",
    checkmark: "\u{2713}",
};

const TEXT_STYLE: Style = Style {
    bold: "",
    bright: "",
    bold_bright: "",
    dim: "",
    reset: "",
    green: "",
    red: "",
    cyan: "",
    yellow: "",
    bullet: "-",
    tool_arrow: ">",
    checkmark: "+",
};

/// Run an agent (pi or claude) with stream output parsing.
pub fn run_agent(
    runner: &str,
    prompt: &str,
    model: Option<&str>,
    timeout_secs: u64,
    format: Option<&str>,
    skip_permissions: bool,
) -> anyhow::Result<()> {
    let format = OutputFormat::detect(format);
    let style = match format {
        OutputFormat::Pretty => &PRETTY_STYLE,
        OutputFormat::Text => &TEXT_STYLE,
    };

    let (mut child, tool_name) = match runner {
        "claude" => (spawn_claude(prompt, model, skip_permissions)?, "claude"),
        "pi" => (spawn_pi(prompt, model)?, "pi"),
        _ => {
            return Err(anyhow!(
                "Unsupported runner: {}. Supported: 'pi', 'claude'.",
                runner
            ));
        }
    };

    // Spawn threads to read stdout and stderr
    let stdout = child.stdout.take().context("failed to capture stdout")?;
    let stderr = child.stderr.take().context("failed to capture stderr")?;

    let (stdout_tx, stdout_rx) = channel();
    let (stderr_tx, stderr_rx) = channel();

    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().flatten() {
            let _ = stdout_tx.send(line);
        }
    });

    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines().flatten() {
            let _ = stderr_tx.send(line);
        }
    });

    let event_handler: &dyn Fn(&Value, &Style) -> bool = match runner {
        "pi" => &handle_pi_event,
        _ => &handle_claude_event,
    };

    // Process output
    let result = process_output(
        &mut child,
        stdout_rx,
        stderr_rx,
        style,
        Duration::from_secs(timeout_secs),
        tool_name,
        event_handler,
    );

    // Clean up
    let _ = child.kill();
    let _ = child.wait();

    result
}

/// Spawn Claude Code with stream-JSON output.
fn spawn_claude(
    prompt: &str,
    model: Option<&str>,
    skip_permissions: bool,
) -> anyhow::Result<Child> {
    let mut args = vec!["--verbose", "--output-format", "stream-json"];

    if skip_permissions {
        args.push("--dangerously-skip-permissions");
        args.push("--allow-dangerously-skip-permissions");
    }

    let model_arg;
    if let Some(m) = model {
        model_arg = m.to_string();
        args.push("--model");
        args.push(&model_arg);
    }

    args.push("-p");
    args.push(prompt);

    Command::new("claude")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| -> anyhow::Error {
            if e.kind() == std::io::ErrorKind::NotFound {
                ExitError::ToolNotFound {
                    tool: "claude".to_string(),
                }
                .into()
            } else {
                anyhow::Error::new(e).context("spawning claude")
            }
        })
}

fn home_dir() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/root"))
}

/// Spawn Pi agent with JSON mode output.
///
/// Pi is a multi-provider agent harness supporting Anthropic, OpenAI, Google, etc.
/// Model format: "provider/model-id" (e.g. "openai/gpt-4o", "google/gemini-2.5-pro")
/// or just "model-id" with --provider flag.
fn spawn_pi(prompt: &str, model: Option<&str>) -> anyhow::Result<Child> {
    // Disable all auto-discovered extensions (e.g. lsp-pi which spawns rust-analyzer)
    // then explicitly re-enable only the botbox hooks extension.
    let botbox_ext = home_dir().join(".pi/agent/extensions/botbox-hooks.ts");
    let botbox_ext_str;
    let mut args = vec![
        "--print",
        "--no-extensions",
        "--no-skills",
        "--no-prompt-templates",
        "--no-themes",
        "--mode",
        "json",
        "--no-session",
    ];
    if botbox_ext.exists() {
        botbox_ext_str = botbox_ext.to_string_lossy().into_owned();
        args.push("--extension");
        args.push(&botbox_ext_str);
    }

    // Model can be "provider/model" or "provider/model:thinking" — Pi handles the :suffix natively
    let model_arg;
    if let Some(m) = model {
        model_arg = m.to_string();
        args.push("--model");
        args.push(&model_arg);
    }

    // Pi uses positional arg for prompt, not -p (which is --print boolean flag)
    args.push(prompt);

    Command::new("pi")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| -> anyhow::Error {
            if e.kind() == std::io::ErrorKind::NotFound {
                ExitError::ToolNotFound {
                    tool: "pi".to_string(),
                }
                .into()
            } else {
                anyhow::Error::new(e).context("spawning pi")
            }
        })
}

/// Process stdout/stderr from a spawned agent process.
///
/// The `event_handler` callback processes each JSON event and returns true
/// when a "completion" event is received (signaling the agent is done).
fn process_output(
    child: &mut Child,
    stdout_rx: Receiver<String>,
    stderr_rx: Receiver<String>,
    style: &Style,
    timeout: Duration,
    tool_name: &str,
    event_handler: &dyn Fn(&Value, &Style) -> bool,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let mut result_received = false;
    let mut result_time: Option<Instant> = None;
    let mut detected_error: Option<String> = None;

    loop {
        // Check timeout
        let elapsed = start.elapsed();
        if elapsed >= timeout && !result_received {
            return Err(ExitError::Timeout {
                tool: tool_name.to_string(),
                timeout_secs: timeout.as_secs(),
            }
            .into());
        }

        // Check if we should kill after result
        if let Some(result_instant) = result_time
            && result_instant.elapsed() >= Duration::from_secs(2)
        {
            // Kill hung process
            eprintln!("Warning: Process hung after completion, killing...");
            return Ok(());
        }

        // Check if process exited
        match child.try_wait() {
            Ok(Some(status)) => {
                // Drain remaining stdout before returning
                while let Ok(line) = stdout_rx.try_recv() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Ok(event) = serde_json::from_str::<Value>(&line) {
                        if event_handler(&event, style) {
                            result_received = true;
                        }
                    }
                }

                if result_received || status.success() {
                    return Ok(());
                } else {
                    let code = status.code().unwrap_or(-1);
                    let error_msg = if let Some(err) = detected_error {
                        format!("{} (exit code {})", err, code)
                    } else {
                        format!("Agent exited with code {}", code)
                    };
                    return Err(ExitError::ToolFailed {
                        tool: tool_name.to_string(),
                        code,
                        message: error_msg,
                    }
                    .into());
                }
            }
            Ok(None) => {
                // Still running
            }
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!("waiting for {tool_name}")));
            }
        }

        // Process stdout
        while let Ok(line) = stdout_rx.try_recv() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(event) = serde_json::from_str::<Value>(&line) {
                if event_handler(&event, style) {
                    result_received = true;
                    result_time = Some(Instant::now());
                }
            }
        }

        // Process stderr
        while let Ok(line) = stderr_rx.try_recv() {
            if let Some(err) = detect_api_error(&line) {
                detected_error = Some(err.clone());
                eprintln!("\n{}FATAL:{} {}", style.yellow, style.reset, err);
            } else if line.contains("Error") || line.contains("error") {
                eprintln!("{}", line);
            }
        }

        // Small sleep to avoid busy loop
        thread::sleep(Duration::from_millis(10));
    }
}

// --- Claude event handlers ---

/// Handle a Claude stream-JSON event. Returns true if this is a completion event.
fn handle_claude_event(event: &Value, style: &Style) -> bool {
    match event.get("type").and_then(|t| t.as_str()) {
        Some("text") => print_claude_text_event(event, style),
        Some("assistant") => print_claude_assistant_event(event, style),
        Some("user") => print_claude_user_event(event, style),
        Some("result") => return true,
        _ => {}
    }
    false
}

fn print_claude_text_event(event: &Value, style: &Style) {
    if let Some(text) = event.get("text").and_then(|t| t.as_str()) {
        if text.trim().is_empty() {
            return;
        }
        let first_line = text.lines().next().unwrap_or("");
        let truncated = if first_line.len() > 120 {
            format!("{}...", truncate_safe(first_line, 120))
        } else {
            first_line.to_string()
        };
        if !truncated.trim().is_empty() {
            println!(
                "{}{} {}{}",
                style.bright, style.bullet, truncated, style.reset
            );
        }
    }
}

fn print_claude_assistant_event(event: &Value, style: &Style) {
    if let Some(content) = event
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        for item in content {
            if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    let formatted = format_markdown(text, style);
                    println!("\n{}{}{}", style.bright, formatted, style.reset);
                }
            } else if item.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                && let Some(tool_name) = item.get("name").and_then(|n| n.as_str())
            {
                let input = item.get("input").unwrap_or(&Value::Null);
                println!(
                    "\n{} {}{}{}",
                    style.tool_arrow, style.bold_bright, tool_name, style.reset
                );
                print_tool_args(tool_name, input, style);
            }
        }
    }
}

fn print_claude_user_event(event: &Value, style: &Style) {
    if let Some(content) = event
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        for item in content {
            if item.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                let content_val = item.get("content");
                let content_str = match content_val {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => serde_json::to_string(other).unwrap_or_default(),
                    None => String::new(),
                };
                let truncated = content_str.replace('\n', " ");
                let truncated = if truncated.len() > 100 {
                    format!("{}...", truncate_safe(&truncated, 100))
                } else {
                    truncated
                };
                println!(
                    "  {}{}{} {}{}{}",
                    style.green, style.checkmark, style.reset, style.dim, truncated, style.reset
                );
            }
        }
    }
}

// --- Pi event handlers ---

// Thread-local buffer for accumulating Pi text deltas in pretty mode.
// In pretty mode, we buffer the full text block and render it as markdown
// when the text_end event arrives, rather than streaming char-by-char.
thread_local! {
    static PI_TEXT_BUFFER: RefCell<String> = RefCell::new(String::new());
}

/// Handle a Pi JSON mode event. Returns true if this is a completion event.
///
/// Pi JSONL event types:
/// - session: session metadata
/// - agent_start/agent_end: session lifecycle
/// - turn_start/turn_end: turn lifecycle
/// - message_start/message_end: message boundaries
/// - message_update: streaming content (text_delta, toolcall_start/end, thinking_*)
/// - tool_execution_start/end: tool execution with results
fn handle_pi_event(event: &Value, style: &Style) -> bool {
    let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match event_type {
        "message_update" => {
            if let Some(ae) = event.get("assistantMessageEvent") {
                let ae_type = ae.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match ae_type {
                    "text_delta" => print_pi_text_delta(ae, style),
                    "text_end" => print_pi_text_end(ae, style),
                    "toolcall_start" => print_pi_toolcall_start(ae, style),
                    "toolcall_end" => print_pi_toolcall_end(ae, style),
                    "thinking_start" => print_pi_thinking_start(style),
                    "thinking_delta" => print_pi_thinking_delta(ae, style),
                    "thinking_end" => print_pi_thinking_end(style),
                    _ => {} // text_start, toolcall_delta
                }
            }
        }
        "tool_execution_end" => {
            print_pi_tool_result(event, style);
        }
        "agent_end" => return true,
        _ => {} // session, agent_start, turn_start/end, message_start/end, tool_execution_start
    }

    false
}

fn print_pi_text_delta(ae: &Value, style: &Style) {
    if let Some(delta) = ae.get("delta").and_then(|d| d.as_str()) {
        if delta.is_empty() {
            return;
        }
        if !style.bold.is_empty() {
            // Pretty mode: buffer text for markdown rendering at text_end
            PI_TEXT_BUFFER.with(|buf| buf.borrow_mut().push_str(delta));
        } else {
            // Text mode: stream inline as before
            print!("{}", delta);
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
    }
}

fn print_pi_text_end(_ae: &Value, style: &Style) {
    if !style.bold.is_empty() {
        // Pretty mode: render buffered text as markdown
        PI_TEXT_BUFFER.with(|buf| {
            let mut text = buf.borrow_mut();
            if !text.trim().is_empty() {
                let skin = termimad::MadSkin::default_dark();
                // Print a blank line before the rendered block for visual separation
                println!();
                skin.print_text(&text);
            }
            text.clear();
        });
    } else {
        // Text mode: just finish the line
        println!();
    }
}

fn print_pi_toolcall_start(ae: &Value, style: &Style) {
    // Extract tool name from the partial content
    if let Some(content) = ae
        .get("partial")
        .and_then(|p| p.get("content"))
        .and_then(|c| c.as_array())
    {
        for item in content {
            if item.get("type").and_then(|t| t.as_str()) == Some("toolCall") {
                if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                    println!(
                        "\n{} {}{}{}",
                        style.tool_arrow, style.bold_bright, name, style.reset
                    );
                }
            }
        }
    }
}

fn print_pi_toolcall_end(ae: &Value, style: &Style) {
    if let Some(tc) = ae.get("toolCall") {
        let name = tc.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if let Some(args) = tc.get("arguments") {
            print_tool_args(name, args, style);
        }
    }
}

/// Format tool call arguments based on tool type.
/// - bash: show the full command
/// - edit: show file path and a unified diff of old_string → new_string
/// - read: show the file path with offset/limit
/// - write: show the file path
/// - other tools: truncated JSON (default)
fn print_tool_args(name: &str, args: &Value, style: &Style) {
    match name {
        "bash" | "Bash" => {
            if let Some(cmd) = args.get("command").and_then(|c| c.as_str()) {
                println!("  {}$ {}{}", style.dim, cmd, style.reset);
            }
        }
        "edit" | "Edit" => {
            let path = args
                .get("file_path")
                .or_else(|| args.get("path"))
                .and_then(|p| p.as_str());
            if let Some(path) = path {
                let short = path.rsplit('/').next().unwrap_or(path);
                println!("  {}{}{}", style.dim, short, style.reset);
            }
            let old = args
                .get("old_string")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            let new = args
                .get("new_string")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            if !old.is_empty() || !new.is_empty() {
                print_inline_diff(old, new, style);
            }
        }
        "read" | "Read" => {
            let path = args
                .get("file_path")
                .or_else(|| args.get("path"))
                .and_then(|p| p.as_str());
            if let Some(path) = path {
                let short = path.rsplit('/').next().unwrap_or(path);
                let mut extra = Vec::new();
                if let Some(off) = args.get("offset").and_then(|o| o.as_u64()) {
                    extra.push(format!(":{off}"));
                }
                if let Some(lim) = args.get("limit").and_then(|l| l.as_u64()) {
                    extra.push(format!("+{lim}"));
                }
                println!(
                    "  {}{}{}{}",
                    style.dim,
                    short,
                    extra.join(""),
                    style.reset
                );
            }
        }
        "write" | "Write" => {
            let path = args
                .get("file_path")
                .or_else(|| args.get("path"))
                .and_then(|p| p.as_str());
            if let Some(path) = path {
                let short = path.rsplit('/').next().unwrap_or(path);
                println!("  {}{}{}", style.dim, short, style.reset);
            }
        }
        _ => {
            let args_str = serde_json::to_string(args).unwrap_or_default();
            let truncated = if args_str.len() > 80 {
                format!("{}...", truncate_safe(&args_str, 80))
            } else {
                args_str
            };
            println!("  {}{}{}", style.dim, truncated, style.reset);
        }
    }
}

/// Print a compact inline diff of old → new text.
/// Shows removed lines in red with - prefix and added lines in green with + prefix.
/// Limits output to MAX_DIFF_LINES lines total.
fn print_inline_diff(old: &str, new: &str, style: &Style) {
    const MAX_DIFF_LINES: usize = 12;
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let mut output_lines = Vec::new();

    for line in &old_lines {
        output_lines.push(format!(
            "  {}-{} {}{}",
            style.red, style.reset, line, style.reset
        ));
    }
    for line in &new_lines {
        output_lines.push(format!(
            "  {}+{} {}{}",
            style.green, style.reset, line, style.reset
        ));
    }

    let total = output_lines.len();
    if total <= MAX_DIFF_LINES {
        for line in &output_lines {
            println!("{line}");
        }
    } else {
        let head = MAX_DIFF_LINES / 2;
        let tail = MAX_DIFF_LINES - head - 1;
        for line in &output_lines[..head] {
            println!("{line}");
        }
        println!(
            "  {}... {} more lines ...{}",
            style.dim,
            total - head - tail,
            style.reset
        );
        for line in &output_lines[total - tail..] {
            println!("{line}");
        }
    }
}

fn print_pi_tool_result(event: &Value, style: &Style) {
    let tool_name = event
        .get("toolName")
        .and_then(|n| n.as_str())
        .unwrap_or("?");
    let is_error = event
        .get("isError")
        .and_then(|e| e.as_bool())
        .unwrap_or(false);

    // Extract result text
    let result_text = event
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");

    let truncated = result_text.replace('\n', " ");
    let truncated = if truncated.len() > 100 {
        format!("{}...", truncate_safe(&truncated, 100))
    } else {
        truncated
    };

    if is_error {
        println!(
            "  {}x{} {}: {}{}{}",
            style.yellow, style.reset, tool_name, style.dim, truncated, style.reset
        );
    } else {
        println!(
            "  {}{}{} {}: {}{}{}",
            style.green, style.checkmark, style.reset, tool_name, style.dim, truncated, style.reset
        );
    }
}

fn print_pi_thinking_start(style: &Style) {
    print!("{}  [thinking] {}", style.dim, style.reset);
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

fn print_pi_thinking_delta(ae: &Value, style: &Style) {
    if let Some(delta) = ae.get("delta").and_then(|d| d.as_str()) {
        if delta.is_empty() {
            return;
        }
        // Show a dot per chunk to indicate thinking progress without dumping full reasoning
        print!("{}.", style.dim);
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }
}

fn print_pi_thinking_end(style: &Style) {
    println!("{}", style.reset);
}

// --- Shared utilities ---

/// Truncate a string at a valid UTF-8 char boundary.
fn truncate_safe(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn detect_api_error(stderr: &str) -> Option<String> {
    if stderr.contains("API Error: 5") || stderr.contains("500") {
        Some("API Error: Server error (5xx)".to_string())
    } else if stderr.contains("rate limit")
        || stderr.contains("Rate limit")
        || stderr.contains("429")
    {
        Some("API Error: Rate limit exceeded".to_string())
    } else if stderr.contains("overloaded") || stderr.contains("503") {
        Some("API Error: Service overloaded".to_string())
    } else {
        None
    }
}

fn re_code_block() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"```(\w+)?\n([\s\S]*?)```").unwrap())
}

fn re_inline_code() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"`([^`]+)`").unwrap())
}

fn re_bold() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\*\*([^*]+)\*\*").unwrap())
}

fn re_headers() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(?m)^#{1,3}\s+(.+)$").unwrap())
}

fn format_markdown(text: &str, style: &Style) -> String {
    if style.bold.is_empty() {
        // Text mode: strip markdown
        let mut result = text.to_string();
        result = re_code_block()
            .replace_all(&result, |caps: &regex::Captures| {
                format!("\n{}\n", caps.get(2).map_or("", |m| m.as_str()).trim())
            })
            .to_string();
        result = re_inline_code().replace_all(&result, "$1").to_string();
        result = re_bold().replace_all(&result, "$1").to_string();
        result = re_headers().replace_all(&result, "$1").to_string();
        result
    } else {
        // Pretty mode: ANSI colors
        let mut result = text.to_string();
        result = re_code_block()
            .replace_all(&result, |caps: &regex::Captures| {
                format!(
                    "\n{}{}\n{}",
                    style.dim,
                    caps.get(2).map_or("", |m| m.as_str()).trim(),
                    style.reset
                )
            })
            .to_string();
        result = re_inline_code()
            .replace_all(&result, &format!("{}$1{}", style.cyan, style.reset))
            .to_string();
        result = re_bold()
            .replace_all(&result, &format!("{}$1{}", style.bold, style.reset))
            .to_string();
        result = re_headers()
            .replace_all(
                &result,
                &format!("{}{} $1{}", style.bold, style.yellow, style.reset),
            )
            .to_string();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_format_explicit() {
        assert_eq!(OutputFormat::detect(Some("pretty")), OutputFormat::Pretty);
        assert_eq!(OutputFormat::detect(Some("text")), OutputFormat::Text);
    }

    #[test]
    fn detect_format_via_term_env() {
        // Save current TERM value
        let original_term = std::env::var("TERM").ok();

        // Test with TERM=xterm-256color (should enable pretty mode)
        unsafe {
            std::env::set_var("TERM", "xterm-256color");
        }
        // Note: This test might still return Text if stdout is not a TTY,
        // but the TERM check is a fallback that happens after the TTY check fails
        let _format = OutputFormat::detect(None);

        // Test with TERM=dumb (should disable colors)
        unsafe {
            std::env::set_var("TERM", "dumb");
        }
        let format_dumb = OutputFormat::detect(None);
        // dumb terminal should give us Text mode (unless stdout is a real TTY)

        // Test with empty TERM
        unsafe {
            std::env::set_var("TERM", "");
        }
        let format_empty = OutputFormat::detect(None);

        // Restore original TERM
        unsafe {
            match original_term {
                Some(term) => std::env::set_var("TERM", term),
                None => std::env::remove_var("TERM"),
            }
        }

        // Verify dumb and empty both give Text when not in a TTY
        // (can't easily test the Pretty case without a real TTY)
        if !std::io::stdout().is_terminal() {
            assert_eq!(format_dumb, OutputFormat::Text);
            assert_eq!(format_empty, OutputFormat::Text);
        }
    }

    #[test]
    fn detect_api_errors() {
        assert!(detect_api_error("API Error: 500").is_some());
        assert!(detect_api_error("rate limit exceeded").is_some());
        assert!(detect_api_error("service overloaded 503").is_some());
        assert!(detect_api_error("some other error").is_none());
    }

    #[test]
    fn format_markdown_text_mode() {
        let input = "**bold** `code` ```rust\nlet x = 1;\n```\n## Header";
        let output = format_markdown(input, &TEXT_STYLE);
        assert!(!output.contains("**"));
        assert!(!output.contains("`"));
        assert!(!output.contains("```"));
        // Headers on their own line should have the ## removed
        assert!(!output.contains("## Header"));
        assert!(output.contains("Header"));
    }

    #[test]
    fn format_markdown_pretty_mode() {
        let input = "`code`";
        let output = format_markdown(input, &PRETTY_STYLE);
        assert!(output.contains("\x1b[36m")); // cyan
        assert!(output.contains("\x1b[0m")); // reset
    }

    #[test]
    fn unsupported_runner_error() {
        let result = run_agent("foobar", "test", None, 10, Some("text"), false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unsupported runner"));
        assert!(err.contains("foobar"));
    }

    // --- Claude event handler tests ---

    #[test]
    fn claude_result_event_is_completion() {
        let event: Value = serde_json::from_str(r#"{"type":"result"}"#).unwrap();
        assert!(handle_claude_event(&event, &TEXT_STYLE));
    }

    #[test]
    fn claude_text_event_is_not_completion() {
        let event: Value = serde_json::from_str(r#"{"type":"text","text":"hello"}"#).unwrap();
        assert!(!handle_claude_event(&event, &TEXT_STYLE));
    }

    // --- Pi event handler tests ---

    #[test]
    fn pi_agent_end_is_completion() {
        let event: Value = serde_json::from_str(r#"{"type":"agent_end","messages":[]}"#).unwrap();
        assert!(handle_pi_event(&event, &TEXT_STYLE));
    }

    #[test]
    fn pi_text_delta_is_not_completion() {
        let event: Value = serde_json::from_str(
            r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"hello","contentIndex":1}}"#,
        )
        .unwrap();
        assert!(!handle_pi_event(&event, &TEXT_STYLE));
    }

    #[test]
    fn pi_session_event_is_not_completion() {
        let event: Value = serde_json::from_str(
            r#"{"type":"session","version":3,"id":"test-id","timestamp":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert!(!handle_pi_event(&event, &TEXT_STYLE));
    }

    #[test]
    fn pi_toolcall_end_event_parsed() {
        let event: Value = serde_json::from_str(
            r#"{"type":"message_update","assistantMessageEvent":{"type":"toolcall_end","contentIndex":0,"toolCall":{"type":"toolCall","id":"tc-1","name":"read","arguments":{"path":"/tmp/test.txt"}}}}"#,
        )
        .unwrap();
        // Should not panic and should not be completion
        assert!(!handle_pi_event(&event, &TEXT_STYLE));
    }

    #[test]
    fn pi_tool_execution_end_parsed() {
        let event: Value = serde_json::from_str(
            r#"{"type":"tool_execution_end","toolCallId":"tc-1","toolName":"read","result":{"content":[{"type":"text","text":"file contents"}],"details":{}},"isError":false}"#,
        )
        .unwrap();
        assert!(!handle_pi_event(&event, &TEXT_STYLE));
    }

    #[test]
    fn pi_tool_execution_error_parsed() {
        let event: Value = serde_json::from_str(
            r#"{"type":"tool_execution_end","toolCallId":"tc-1","toolName":"read","result":{"content":[{"type":"text","text":"ENOENT: no such file"}],"details":{}},"isError":true}"#,
        )
        .unwrap();
        assert!(!handle_pi_event(&event, &TEXT_STYLE));
    }
}
