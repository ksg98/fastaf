use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Emitter, Manager};

use crate::TaskManager;

#[derive(Clone)]
pub(crate) struct CodexSessionInfo {
    pub(crate) session_id: String,
    pub(crate) session_path: String,
}

#[derive(Clone)]
pub(crate) struct ClaudeSessionInfo {
    pub(crate) session_id: String,
    pub(crate) session_path: String,
    /// true means a placeholder entry pre-injected by lazy attach (the jsonl hasn't been written to
    /// disk yet); both `is_task_active` and `finalize_task_exit::had_agent_session` should skip it.
    pub(crate) is_placeholder: bool,
}

// ── Shared helpers ────────────────────────────────────────────────────────────

pub(crate) fn emit_task_status(app: &AppHandle, task_id: &str, status: &str) {
    let _ = app.emit(
        "task-status",
        serde_json::json!({ "task_id": task_id, "status": status }),
    );
}

fn emit_active_task_status(app: &AppHandle, task_id: &str, status: &str) {
    if is_task_active(app, task_id) {
        emit_task_status(app, task_id, status);
    }
}

pub(crate) fn is_task_active(app: &AppHandle, task_id: &str) -> bool {
    let tm = app.state::<TaskManager>();
    if tm.child_handles.lock().contains_key(task_id) {
        return true;
    }

    let has_codex_session = tm
        .codex_sessions
        .lock()
        .get(task_id)
        .map(|info| !info.session_id.is_empty() && !info.session_path.is_empty())
        .unwrap_or(false);

    if has_codex_session {
        return true;
    }

    let has_claude_session = tm
        .claude_sessions
        .lock()
        .get(task_id)
        .map(|info| {
            !info.session_id.is_empty() && !info.session_path.is_empty() && !info.is_placeholder
        })
        .unwrap_or(false);

    has_claude_session
}

fn claim_session_path(app: &AppHandle, path: &str) -> bool {
    let tm = app.state::<TaskManager>();
    let mut claimed = tm.claimed_session_paths.lock();
    if claimed.contains(path) {
        return false;
    }
    claimed.insert(path.to_string());
    true
}

fn read_session_lines_since(
    session_path: &Path,
    offset: &mut u64,
    partial: &mut String,
) -> Result<Vec<String>, std::io::Error> {
    let mut file = File::open(session_path)?;
    file.seek(SeekFrom::Start(*offset))?;

    let mut chunk = String::new();
    file.read_to_string(&mut chunk)?;
    *offset += chunk.as_bytes().len() as u64;

    if chunk.is_empty() {
        return Ok(Vec::new());
    }

    partial.push_str(&chunk);
    let complete_len = if partial.ends_with('\n') {
        partial.len()
    } else {
        partial.rfind('\n').map(|idx| idx + 1).unwrap_or(0)
    };

    if complete_len == 0 {
        return Ok(Vec::new());
    }

    let completed = partial[..complete_len].to_string();
    let remaining = partial[complete_len..].to_string();
    *partial = remaining;

    Ok(completed.lines().map(|line| line.to_string()).collect())
}

fn session_modified_at(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

// ── Codex session watcher ─────────────────────────────────────────────────────

fn codex_sessions_roots(project_path: &str) -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from(project_path).join(".codex").join("sessions")];
    if let Some(home) = crate::platform::home_dir() {
        let home_root = home.join(".codex").join("sessions");
        if !roots.iter().any(|root| root == &home_root) {
            roots.push(home_root);
        }
    }
    roots
}

fn collect_session_files_from_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in roots {
        collect_session_files(root, &mut files);
    }
    files
}

fn collect_session_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_session_files(&path, out);
            continue;
        }

        let is_rollout_jsonl = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
            .unwrap_or(false);

        if is_rollout_jsonl {
            out.push(path);
        }
    }
}

fn watch_codex_session(
    app: AppHandle,
    task_id: String,
    session_path: PathBuf,
    project_path: PathBuf,
) {
    use notify::{RecursiveMode, Watcher};

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher_opt = notify::RecommendedWatcher::new(tx, notify::Config::default())
        .ok()
        .and_then(|mut w| {
            w.watch(&session_path, RecursiveMode::NonRecursive).ok()?;
            Some(w)
        });

    let mut offset = 0u64;
    let mut partial = String::new();
    let mut waiting_for_user = false;
    let mut pending_confirmation_calls = HashSet::new();
    let mut awaiting_user_reply = false;

    while is_task_active(&app, &task_id) {
        if let Ok(lines) = read_session_lines_since(&session_path, &mut offset, &mut partial) {
            for line in lines {
                process_codex_session_line(
                    &app,
                    &task_id,
                    &line,
                    &project_path,
                    &mut waiting_for_user,
                    &mut pending_confirmation_calls,
                    &mut awaiting_user_reply,
                );
            }
        }

        if watcher_opt.is_some() {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => watcher_opt = None,
            }
        } else {
            thread::sleep(Duration::from_millis(400));
        }
    }
}

fn process_codex_session_line(
    app: &AppHandle,
    task_id: &str,
    line: &str,
    project_path: &Path,
    waiting_for_user: &mut bool,
    pending_confirmation_calls: &mut HashSet<String>,
    awaiting_user_reply: &mut bool,
) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };

    let event_type = value.get("type").and_then(serde_json::Value::as_str);
    let payload = value.get("payload");

    match event_type {
        Some("response_item") => {
            let payload_type = payload
                .and_then(|item| item.get("type"))
                .and_then(serde_json::Value::as_str);

            match payload_type {
                Some("function_call") => {
                    let name = payload
                        .and_then(|item| item.get("name"))
                        .and_then(serde_json::Value::as_str);
                    let call_id = payload
                        .and_then(|item| item.get("call_id"))
                        .and_then(serde_json::Value::as_str);
                    let arguments = payload
                        .and_then(|item| item.get("arguments"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");

                    if name == Some("request_user_input") {
                        *awaiting_user_reply = true;
                    } else if name
                        .map(|tool| tool_call_requires_confirmation(tool, arguments, project_path))
                        .unwrap_or(false)
                    {
                        if let Some(call_id) = call_id {
                            pending_confirmation_calls.insert(call_id.to_string());
                        } else {
                            *awaiting_user_reply = true;
                        }
                    }
                    sync_waiting_for_user(
                        app,
                        task_id,
                        waiting_for_user,
                        pending_confirmation_calls,
                        *awaiting_user_reply,
                    );
                }
                Some("function_call_output") => {
                    if let Some(call_id) = payload
                        .and_then(|item| item.get("call_id"))
                        .and_then(serde_json::Value::as_str)
                    {
                        pending_confirmation_calls.remove(call_id);
                    }

                    let output = payload
                        .and_then(|item| item.get("output"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    if output.starts_with("aborted by user after") {
                        *awaiting_user_reply = true;
                    }
                    sync_waiting_for_user(
                        app,
                        task_id,
                        waiting_for_user,
                        pending_confirmation_calls,
                        *awaiting_user_reply,
                    );
                }
                Some("custom_tool_call") => {
                    let status = payload
                        .and_then(|item| item.get("status"))
                        .and_then(serde_json::Value::as_str);
                    let call_id = payload
                        .and_then(|item| item.get("call_id"))
                        .and_then(serde_json::Value::as_str);

                    if matches!(status, Some("completed") | Some("failed")) {
                        if let Some(call_id) = call_id {
                            pending_confirmation_calls.remove(call_id);
                        }
                        sync_waiting_for_user(
                            app,
                            task_id,
                            waiting_for_user,
                            pending_confirmation_calls,
                            *awaiting_user_reply,
                        );
                    }
                }
                Some("message") => {
                    let role = payload
                        .and_then(|item| item.get("role"))
                        .and_then(serde_json::Value::as_str);
                    if role == Some("user") {
                        *awaiting_user_reply = false;
                    } else if role == Some("assistant")
                        && assistant_message_requests_user_input(payload)
                    {
                        *awaiting_user_reply = true;
                    }
                    sync_waiting_for_user(
                        app,
                        task_id,
                        waiting_for_user,
                        pending_confirmation_calls,
                        *awaiting_user_reply,
                    );
                }
                _ => {}
            }
        }
        Some("event_msg") => {
            let payload_type = payload
                .and_then(|item| item.get("type"))
                .and_then(serde_json::Value::as_str);
            if payload_type == Some("user_message") {
                *awaiting_user_reply = false;
                sync_waiting_for_user(
                    app,
                    task_id,
                    waiting_for_user,
                    pending_confirmation_calls,
                    *awaiting_user_reply,
                );
            }
        }
        _ => {}
    }
}

fn sync_waiting_for_user(
    app: &AppHandle,
    task_id: &str,
    waiting_for_user: &mut bool,
    pending_confirmation_calls: &HashSet<String>,
    awaiting_user_reply: bool,
) {
    let next_waiting = awaiting_user_reply || !pending_confirmation_calls.is_empty();
    if *waiting_for_user == next_waiting {
        return;
    }

    *waiting_for_user = next_waiting;
    emit_active_task_status(
        app,
        task_id,
        if next_waiting {
            "input_required"
        } else {
            "running"
        },
    );
}

// ── Permission checks ─────────────────────────────────────────────────────────

fn tool_call_requires_confirmation(name: &str, arguments: &str, project_path: &Path) -> bool {
    match name {
        "exec_command" => exec_command_requires_confirmation(arguments),
        "apply_patch" => apply_patch_requires_confirmation(arguments, project_path),
        _ => false,
    }
}

fn exec_command_requires_confirmation(arguments: &str) -> bool {
    let Ok(args) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return false;
    };

    if args
        .get("sandbox_permissions")
        .and_then(serde_json::Value::as_str)
        == Some("require_escalated")
    {
        return true;
    }

    let Some(cmd) = args.get("cmd").and_then(serde_json::Value::as_str) else {
        return false;
    };

    !looks_like_read_only_command(cmd)
}

fn looks_like_read_only_command(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    if trimmed.is_empty() || contains_shell_redirection(trimmed) {
        return false;
    }

    trimmed
        .split(|c| matches!(c, ';' | '|' | '&' | '\n'))
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .all(is_read_only_segment)
}

fn contains_shell_redirection(cmd: &str) -> bool {
    cmd.contains(" >")
        || cmd.contains(">>")
        || cmd.contains("<<")
        || cmd.contains(" 2>")
        || cmd.starts_with('>')
        || cmd.contains("| tee")
}

fn is_read_only_segment(segment: &str) -> bool {
    let tokens: Vec<&str> = segment.split_whitespace().collect();
    let Some(command) = tokens.first().copied() else {
        return true;
    };

    match command {
        "pwd" | "ls" | "rg" | "grep" | "cat" | "head" | "tail" | "wc" | "stat" | "which"
        | "type" | "uname" | "date" | "ps" | "env" | "printenv" | "echo" | "printf"
        | "Get-Location" | "Get-ChildItem" | "Get-Content" | "Select-String" | "Get-Process"
        | "Get-Date" | "Get-Command" | "Test-Path" | "Resolve-Path" | "Where-Object"
        | "Measure-Object" | "Sort-Object" | "Select-Object" => true,
        "sed" => {
            tokens.iter().any(|token| *token == "-n")
                && !tokens.iter().any(|token| token.starts_with("-i"))
        }
        "find" => !tokens
            .iter()
            .any(|token| matches!(*token, "-delete" | "-exec" | "-ok")),
        "git.exe" => matches!(
            tokens.get(1).copied(),
            Some("status")
                | Some("diff")
                | Some("show")
                | Some("log")
                | Some("branch")
                | Some("rev-parse")
                | Some("remote")
        ),
        "git" => matches!(
            tokens.get(1).copied(),
            Some("status")
                | Some("diff")
                | Some("show")
                | Some("log")
                | Some("branch")
                | Some("rev-parse")
                | Some("remote")
        ),
        _ => false,
    }
}

fn apply_patch_requires_confirmation(arguments: &str, project_path: &Path) -> bool {
    arguments.lines().any(|line| {
        extract_patch_path(line)
            .map(|path| patch_target_requires_confirmation(path, project_path))
            .unwrap_or(false)
    })
}

fn extract_patch_path(line: &str) -> Option<&str> {
    line.strip_prefix("*** Add File: ")
        .or_else(|| line.strip_prefix("*** Update File: "))
        .or_else(|| line.strip_prefix("*** Delete File: "))
        .or_else(|| line.strip_prefix("*** Move to: "))
        .map(str::trim)
}

fn patch_target_requires_confirmation(path: &str, project_path: &Path) -> bool {
    let target = Path::new(path);
    if !target.is_absolute() {
        return false;
    }

    let temp_dir = std::env::temp_dir();
    !target.starts_with(project_path) && !target.starts_with(&temp_dir)
}

fn assistant_message_requests_user_input(payload: Option<&serde_json::Value>) -> bool {
    let Some(payload) = payload else {
        return false;
    };

    let phase = payload.get("phase").and_then(serde_json::Value::as_str);
    if !matches!(phase, Some("final") | Some("final_answer")) {
        return false;
    }

    let Some(content) = payload.get("content").and_then(serde_json::Value::as_array) else {
        return false;
    };

    let text = content
        .iter()
        .filter_map(|item| item.get("text").and_then(serde_json::Value::as_str))
        .collect::<String>();
    let text = text.trim();

    text.ends_with('?') || text.ends_with('？')
}

// ── Claude Code session watcher ───────────────────────────────────────────────

fn claude_sessions_dir_for_project(project_path: &str) -> Option<PathBuf> {
    let home = crate::platform::home_dir()?;
    let encoded: String = project_path
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    Some(home.join(".claude").join("projects").join(encoded))
}

fn watch_claude_session(app: AppHandle, task_id: String, session_path: PathBuf) {
    use notify::{RecursiveMode, Watcher};

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher_opt = notify::RecommendedWatcher::new(tx, notify::Config::default())
        .ok()
        .and_then(|mut w| {
            w.watch(&session_path, RecursiveMode::NonRecursive).ok()?;
            Some(w)
        });

    let mut offset = 0u64;
    let mut partial = String::new();
    let mut waiting_for_user = false;

    while is_task_active(&app, &task_id) {
        if let Ok(lines) = read_session_lines_since(&session_path, &mut offset, &mut partial) {
            for line in lines {
                process_claude_session_line(&app, &task_id, &line, &mut waiting_for_user);
            }
        }

        if watcher_opt.is_some() {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => watcher_opt = None,
            }
        } else {
            thread::sleep(Duration::from_millis(400));
        }
    }
}

fn process_claude_session_line(
    app: &AppHandle,
    task_id: &str,
    line: &str,
    waiting_for_user: &mut bool,
) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };

    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("assistant") => {
            // stop_reason == "tool_use" is a clear signal that Claude has paused, waiting for the user to approve or reject a tool call
            let stop_reason = value
                .get("message")
                .and_then(|m| m.get("stop_reason"))
                .and_then(serde_json::Value::as_str);

            if stop_reason == Some("tool_use") && !*waiting_for_user {
                *waiting_for_user = true;
                emit_active_task_status(app, task_id, "input_required");
            }
        }
        Some("user") => {
            // A tool_result entry means the user has acted (approved or rejected)
            let has_tool_result = value
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(serde_json::Value::as_array)
                .map(|content| {
                    content.iter().any(|item| {
                        item.get("type").and_then(serde_json::Value::as_str) == Some("tool_result")
                    })
                })
                .unwrap_or(false);

            if has_tool_result && *waiting_for_user {
                *waiting_for_user = false;
                emit_active_task_status(app, task_id, "running");
            }
        }
        _ => {}
    }
}

// ── Session messages (for conversation view) ──────────────────────────────────

#[derive(serde::Serialize, Clone)]
pub(crate) struct SessionMessage {
    role: String,
    content: Vec<SessionContent>,
}

#[derive(serde::Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum SessionContent {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    Thinking {
        thinking: String,
    },
}

#[tauri::command]
pub async fn read_session_messages(session_path: String) -> Result<Vec<SessionMessage>, String> {
    let content = std::fs::read_to_string(&session_path).map_err(|e| e.to_string())?;
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    if is_codex_format(&lines) {
        Ok(parse_codex_session(&lines))
    } else {
        Ok(parse_claude_session(&lines))
    }
}

fn is_codex_format(lines: &[&str]) -> bool {
    for line in lines.iter().take(10) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            match val.get("type").and_then(|v| v.as_str()) {
                Some("session_meta") | Some("event_msg") => return true,
                _ => {}
            }
        }
    }
    false
}

fn parse_claude_session(lines: &[&str]) -> Vec<SessionMessage> {
    let mut messages = Vec::new();

    for line in lines {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let msg_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let Some(message) = val.get("message") else {
            continue;
        };

        match msg_type {
            "user" => {
                let parts = claude_user_content(message.get("content"));
                if !parts.is_empty() {
                    messages.push(SessionMessage {
                        role: "user".to_string(),
                        content: parts,
                    });
                }
            }
            "assistant" => {
                let parts = message
                    .get("content")
                    .and_then(|c| c.as_array())
                    .map(|arr| claude_assistant_blocks(arr))
                    .unwrap_or_default();
                if !parts.is_empty() {
                    messages.push(SessionMessage {
                        role: "assistant".to_string(),
                        content: parts,
                    });
                }
            }
            _ => {}
        }
    }

    messages
}

fn claude_user_content(content: Option<&serde_json::Value>) -> Vec<SessionContent> {
    match content {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => {
            vec![SessionContent::Text { text: s.clone() }]
        }
        Some(serde_json::Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                    let text = b.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    if !text.trim().is_empty() {
                        return Some(SessionContent::Text {
                            text: text.to_string(),
                        });
                    }
                }
                None
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn claude_assistant_blocks(blocks: &[serde_json::Value]) -> Vec<SessionContent> {
    let mut parts = Vec::new();
    for block in blocks {
        match block.get("type").and_then(|v| v.as_str()) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    if !text.trim().is_empty() {
                        parts.push(SessionContent::Text {
                            text: text.to_string(),
                        });
                    }
                }
            }
            Some("tool_use") => {
                let id = block
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let input = block
                    .get("input")
                    .and_then(|v| serde_json::to_string_pretty(v).ok())
                    .unwrap_or_default();
                parts.push(SessionContent::ToolUse { id, name, input });
            }
            Some("thinking") => {
                if let Some(thinking) = block.get("thinking").and_then(|v| v.as_str()) {
                    if !thinking.trim().is_empty() {
                        parts.push(SessionContent::Thinking {
                            thinking: thinking.to_string(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    parts
}

fn parse_codex_session(lines: &[&str]) -> Vec<SessionMessage> {
    let mut messages: Vec<SessionMessage> = Vec::new();

    for line in lines {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let event_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let payload = val.get("payload");

        match event_type {
            "event_msg" => {
                let payload_type = payload
                    .and_then(|p| p.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if payload_type == "user_message" {
                    let text = payload
                        .and_then(|p| p.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !text.trim().is_empty() {
                        messages.push(SessionMessage {
                            role: "user".to_string(),
                            content: vec![SessionContent::Text {
                                text: text.to_string(),
                            }],
                        });
                    }
                }
            }
            "response_item" => {
                let payload_type = payload
                    .and_then(|p| p.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                match payload_type {
                    "message" => {
                        let role = payload
                            .and_then(|p| p.get("role"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if role != "assistant" {
                            continue;
                        }
                        let parts: Vec<SessionContent> = payload
                            .and_then(|p| p.get("content"))
                            .and_then(|v| v.as_array())
                            .map(|blocks| {
                                blocks
                                    .iter()
                                    .filter_map(|b| {
                                        let t = b.get("type").and_then(|v| v.as_str())?;
                                        if matches!(t, "output_text" | "text") {
                                            let text = b.get("text").and_then(|v| v.as_str())?;
                                            if !text.trim().is_empty() {
                                                return Some(SessionContent::Text {
                                                    text: text.to_string(),
                                                });
                                            }
                                        }
                                        None
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        if !parts.is_empty() {
                            if messages.last().map(|m| m.role.as_str()) == Some("assistant") {
                                messages.last_mut().unwrap().content.extend(parts);
                            } else {
                                messages.push(SessionMessage {
                                    role: "assistant".to_string(),
                                    content: parts,
                                });
                            }
                        }
                    }
                    "function_call" => {
                        let call_id = payload
                            .and_then(|p| p.get("call_id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = payload
                            .and_then(|p| p.get("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let raw = payload
                            .and_then(|p| p.get("arguments"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("{}");
                        let input = serde_json::from_str::<serde_json::Value>(raw)
                            .ok()
                            .and_then(|v| serde_json::to_string_pretty(&v).ok())
                            .unwrap_or_else(|| raw.to_string());
                        let part = SessionContent::ToolUse {
                            id: call_id,
                            name,
                            input,
                        };
                        if messages.last().map(|m| m.role.as_str()) == Some("assistant") {
                            messages.last_mut().unwrap().content.push(part);
                        } else {
                            messages.push(SessionMessage {
                                role: "assistant".to_string(),
                                content: vec![part],
                            });
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    messages
}

// ── Session summary extraction (reused by task naming and other context-aware features) ────────

/// The maximum session file size summary extraction is allowed to read. Beyond this it returns None
/// directly, to avoid loading a 100MB+ long session entirely into memory.
const MAX_SESSION_BYTES_FOR_SUMMARY: u64 = 50 * 1024 * 1024;

/// The maximum number of lines summary extraction is allowed to process. Beyond this it only keeps
/// `MAX_SESSION_LINES_FOR_SUMMARY / 2` lines from the head and tail each, discarding the whole
/// middle, to avoid a 50MB file × multi-MB JSON lines blowing up peak memory during parsing.
const MAX_SESSION_LINES_FOR_SUMMARY: usize = 20_000;

/// Validate that the session_path passed in from the frontend is legitimate:
/// - must be an absolute path and the file must exist
/// - after canonicalize it must lie within a session root allowed for that agent
///   (Claude: `~/.claude/projects/<encoded-project>/`;
///    Codex: `<project_path>/.codex/sessions/` or `~/.codex/sessions/`)
///
/// This gate blocks path traversal — no arbitrary `*.jsonl` file can be read.
pub(crate) fn validate_session_path(
    session_path: &str,
    project_path: &str,
    is_codex: bool,
) -> Result<PathBuf, String> {
    let path = Path::new(session_path);
    if !path.is_absolute() {
        return Err("Session path must be absolute".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("Cannot resolve session path: {}", e))?;
    if !canonical.is_file() {
        return Err("Session path is not a regular file".into());
    }

    let allowed_roots: Vec<PathBuf> = if is_codex {
        codex_sessions_roots(project_path)
            .into_iter()
            .filter_map(|p| p.canonicalize().ok())
            .collect()
    } else {
        claude_sessions_dir_for_project(project_path)
            .and_then(|p| p.canonicalize().ok())
            .into_iter()
            .collect()
    };

    if allowed_roots.is_empty() {
        return Err("No allowed session roots are available for this agent".into());
    }
    if allowed_roots
        .iter()
        .any(|root| canonical.starts_with(root))
    {
        Ok(canonical)
    } else {
        Err(format!(
            "Session path is outside allowed session roots: {}",
            canonical.display()
        ))
    }
}

/// Read and parse the session JSONL, producing a compact plain-text summary for an LLM to process further.
/// When it exceeds `budget_bytes`, trim by "head + omitted middle + tail".
/// When the file exceeds `MAX_SESSION_BYTES_FOR_SUMMARY`, return `None`, and the caller falls back to prompt-only mode.
pub(crate) fn extract_session_summary_text(
    session_path: &str,
    budget_bytes: usize,
) -> Option<String> {
    let metadata = std::fs::metadata(session_path).ok()?;
    if metadata.len() > MAX_SESSION_BYTES_FOR_SUMMARY {
        return None;
    }

    // Stream with BufReader; hard-cap the line count, discarding the middle section once exceeded (keeping only half at each of the head and tail).
    use std::io::BufRead;
    let file = File::open(session_path).ok()?;
    let reader = BufReader::new(file);
    let mut head: Vec<String> = Vec::new();
    let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let half = MAX_SESSION_LINES_FOR_SUMMARY / 2;
    for line in reader
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
    {
        if head.len() < half {
            head.push(line);
        } else {
            tail.push_back(line);
            if tail.len() > half {
                tail.pop_front();
            }
        }
    }
    head.extend(tail);
    let lines = head;
    let line_refs: Vec<&str> = lines.iter().map(String::as_str).collect();

    let messages = if is_codex_format(&line_refs) {
        parse_codex_session(&line_refs)
    } else {
        parse_claude_session(&line_refs)
    };

    let formatted: Vec<String> = messages
        .iter()
        .filter_map(format_message_for_summary)
        .collect();
    if formatted.is_empty() {
        return None;
    }

    let total: usize = formatted.iter().map(|s| s.len() + 1).sum();
    if total <= budget_bytes {
        return Some(formatted.join("\n"));
    }

    // Head + tail slices
    let half = budget_bytes / 2;
    let mut head_msgs: Vec<&str> = Vec::new();
    let mut head_size = 0usize;
    for msg in &formatted {
        if head_size + msg.len() + 1 > half {
            break;
        }
        head_size += msg.len() + 1;
        head_msgs.push(msg.as_str());
    }

    let mut tail_msgs: Vec<&str> = Vec::new();
    let mut tail_size = 0usize;
    let head_count = head_msgs.len();
    for msg in formatted.iter().rev() {
        if tail_msgs.len() + head_count >= formatted.len() {
            break;
        }
        if tail_size + msg.len() + 1 > half {
            break;
        }
        tail_size += msg.len() + 1;
        tail_msgs.push(msg.as_str());
    }
    tail_msgs.reverse();

    let omitted = formatted.len() - head_count - tail_msgs.len();
    let head_text = head_msgs.join("\n");
    let tail_text = tail_msgs.join("\n");
    if omitted == 0 {
        Some(format!("{}\n{}", head_text, tail_text))
    } else {
        Some(format!(
            "{}\n... [{} messages omitted] ...\n{}",
            head_text, omitted, tail_text
        ))
    }
}

/// Keep only the plain-text blocks of user / assistant. Both tool_use and thinking are discarded:
/// - tool_use: a long task can rack up tens to hundreds of Read/Bash calls, which easily blows the
///             budget and pushes the conversation text that actually carries signal out of the
///             tail trimming window.
/// - thinking: not "actual output"; the model talking to itself is worthless for naming.
/// - tool_result: upstream parse_codex_session / parse_claude_session no longer emit it.
fn format_message_for_summary(msg: &SessionMessage) -> Option<String> {
    let role = match msg.role.as_str() {
        "user" => "[user]",
        "assistant" => "[assistant]",
        _ => return None,
    };

    let mut parts: Vec<String> = Vec::new();
    for block in &msg.content {
        if let SessionContent::Text { text } = block {
            let cleaned = truncate_summary_chars(text, 400);
            if !cleaned.is_empty() {
                parts.push(cleaned);
            }
        }
    }

    if parts.is_empty() {
        return None;
    }
    Some(format!("{} {}", role, parts.join(" ")))
}

fn truncate_summary_chars(s: &str, max_chars: usize) -> String {
    let collapsed: String = s
        .replace(['\r', '\n'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let trimmed = collapsed.trim();
    let count = trimmed.chars().count();
    if count <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars).collect();
    out.push('…');
    out
}

// ── Session file utilities ────────────────────────────────────────────────────

/// Strip ANSI escape sequences so we can do plain-text matching.
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some(&'[') => {
                    chars.next(); // consume '['
                                  // consume until a byte that terminates a CSI sequence (ASCII letter)
                    while let Some(&c2) = chars.peek() {
                        chars.next();
                        if c2.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                _ => {
                    chars.next(); // skip the char after bare ESC
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn is_uuid_like(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts[0].len() == 8
        && parts[1].len() == 4
        && parts[2].len() == 4
        && parts[3].len() == 4
        && parts[4].len() == 12
        && parts
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn find_claude_session_file(session_id: &str, project_path: &str) -> Option<PathBuf> {
    let sessions_dir = claude_sessions_dir_for_project(project_path)?;

    // Fast path: UUID session IDs map directly to filenames.
    if is_uuid_like(session_id) {
        let file = sessions_dir.join(format!("{}.jsonl", session_id));
        return if file.exists() { Some(file) } else { None };
    }

    // Slow path: human-readable slug — scan file contents for a matching
    // `custom-title` or `agent-name` record written by the model.
    let entries = std::fs::read_dir(&sessions_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if slug_matches_session_file(&path, session_id) {
            return Some(path);
        }
    }
    None
}

/// Returns true if `path` is a Claude session JSONL that contains a
/// `custom-title` or `agent-name` record matching `slug`.
fn slug_matches_session_file(path: &Path, slug: &str) -> bool {
    use std::io::BufRead;
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let type_str = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if matches!(type_str, "custom-title" | "agent-name") {
            let name = v
                .get("customTitle")
                .or_else(|| v.get("agentName"))
                .and_then(|n| n.as_str())
                .unwrap_or("");
            if name == slug {
                return true;
            }
        }
    }
    false
}

fn find_codex_session_file(session_id: &str, project_path: &str) -> Option<PathBuf> {
    let suffix = format!("-{}.jsonl", session_id);
    let files = collect_session_files_from_roots(&codex_sessions_roots(project_path));
    files
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(&suffix))
                .unwrap_or(false)
        })
        .max_by_key(|p| session_modified_at(p))
}

/// Read the working directory a session was created in, by scanning its first lines for the `cwd`
/// field (Claude: top-level `cwd`; Codex: `session_meta.payload.cwd`).
fn read_session_cwd(path: &Path) -> Option<String> {
    use std::io::BufRead;
    let file = File::open(path).ok()?;
    for line in BufReader::new(file).lines().map_while(Result::ok).take(50) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        // Claude: cwd is a top-level field on every entry.
        if let Some(cwd) = value.get("cwd").and_then(|v| v.as_str()) {
            if !cwd.is_empty() {
                return Some(cwd.to_string());
            }
        }
        // Codex: cwd lives under the session_meta payload.
        if value.get("type").and_then(|v| v.as_str()) == Some("session_meta") {
            if let Some(cwd) = value
                .get("payload")
                .and_then(|p| p.get("cwd"))
                .and_then(|v| v.as_str())
            {
                if !cwd.is_empty() {
                    return Some(cwd.to_string());
                }
            }
        }
    }
    None
}

/// Resolve the directory a session actually belongs to, searching globally by session id rather than
/// assuming it lives under the current project. Used so resume runs the agent in the directory that
/// owns the chat — otherwise `claude --resume` / `codex resume` can't find a session stored elsewhere
/// (e.g. one created earlier under a parent folder) and the agent never starts.
pub(crate) fn resolve_session_cwd(session_id: &str, is_codex: bool) -> Option<String> {
    let home = crate::platform::home_dir()?;
    if is_codex {
        let suffix = format!("-{}.jsonl", session_id);
        let mut files = Vec::new();
        collect_session_files(&home.join(".codex").join("sessions"), &mut files);
        let file = files
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(&suffix))
                    .unwrap_or(false)
            })
            .max_by_key(|p| session_modified_at(p))?;
        read_session_cwd(&file)
    } else {
        let projects = home.join(".claude").join("projects");
        let entries = std::fs::read_dir(&projects).ok()?;
        for entry in entries.flatten() {
            let candidate = entry.path().join(format!("{}.jsonl", session_id));
            if candidate.is_file() {
                return read_session_cwd(&candidate);
            }
        }
        None
    }
}

// ── /status-based session discovery ──────────────────────────────────────────

/// Extract the Session ID from Claude Code's `/status` output.
/// Example output: "Session ID: 1aee0948-e0f2-4ad1-b710-ba236fab378a"
fn extract_claude_status_session_id(output: &str) -> Option<String> {
    let clean = strip_ansi(output);
    // Use find() instead of line-by-line matching because Claude Code renders /status
    // using cursor-positioning escape sequences, which collapse multiple lines into one
    // after ANSI stripping (no \r\n between positioned text fragments).
    let pos = clean.find("Session ID:")?;
    let after = clean[pos + "Session ID:".len()..].trim_start();
    let id: String = after
        .chars()
        .take_while(|c| c.is_ascii_hexdigit() || *c == '-')
        .collect();
    if is_uuid_like(&id) { Some(id) } else { None }
}

/// Extract the Session ID from Codex's `/status` output.
/// Example output: "│  Session:                     019d247a-2a83-76f3-b5c6-e4a59955af3f  │"
///
/// Codex renders /status using cursor-positioning escape sequences, which collapse
/// multiple lines into one after ANSI stripping (same issue as Claude Code).
/// Use find() instead of line-by-line matching to handle both cases.
fn extract_codex_status_session_id(output: &str) -> Option<String> {
    let clean = strip_ansi(output);
    // First filter out the box-drawing border characters, then use find() to search for the
    // "Session:" keyword, avoiding the issue where cursor-positioning sequences collapse multiple
    // lines into one and lines() can no longer match
    let stripped: String = clean
        .chars()
        .filter(|c| !matches!(*c, '│' | '╭' | '╰' | '─' | '╮' | '╯' | '├' | '┤'))
        .collect();
    let pos = stripped.find("Session:")?;
    let after = stripped[pos + "Session:".len()..].trim_start();
    let id: String = after
        .chars()
        .take_while(|c| c.is_ascii_hexdigit() || *c == '-')
        .collect();
    if is_uuid_like(&id) { Some(id) } else { None }
}

/// Poll for up to 5 seconds until the session file appears.
fn wait_for_session_file(session_id: &str, project_path: &str, is_codex: bool) -> Option<PathBuf> {
    for _ in 0..50 {
        let path = if is_codex {
            find_codex_session_file(session_id, project_path)
        } else {
            find_claude_session_file(session_id, project_path)
        };
        if path.is_some() {
            return path;
        }
        thread::sleep(Duration::from_millis(100));
    }
    None
}

/// After the Session ID is confirmed, register the session info and start watching the file.
pub(crate) fn register_and_watch_session(
    app: &AppHandle,
    task_id: &str,
    session_id: &str,
    project_path: &str,
    is_codex: bool,
) {
    let path = match wait_for_session_file(session_id, project_path, is_codex) {
        Some(p) => p,
        None => return,
    };
    let path_string = path.to_string_lossy().into_owned();

    if !claim_session_path(app, &path_string) {
        return;
    }

    if is_codex {
        let tm = app.state::<TaskManager>();
        tm.codex_sessions.lock().insert(
            task_id.to_string(),
            CodexSessionInfo {
                session_id: session_id.to_string(),
                session_path: path_string.clone(),
            },
        );
    } else {
        let tm = app.state::<TaskManager>();
        tm.claude_sessions.lock().insert(
            task_id.to_string(),
            ClaudeSessionInfo {
                session_id: session_id.to_string(),
                session_path: path_string.clone(),
                is_placeholder: false,
            },
        );
    }

    let _ = app.emit(
        "task-session",
        serde_json::json!({
            "task_id": task_id,
            "session_id": session_id,
            "session_path": path_string
        }),
    );

    let app_clone = app.clone();
    let tid = task_id.to_string();
    if is_codex {
        let pp = PathBuf::from(project_path);
        thread::spawn(move || watch_codex_session(app_clone, tid, path, pp));
    } else {
        thread::spawn(move || watch_claude_session(app_clone, tid, path));
    }
}

/// Watch the PTY output stream and obtain the Session ID from the `/status` response.
/// Send `/status` 1.5 seconds after Claude starts; for Codex, wait 1 more second after the first
/// output is received, to avoid querying too early before the session is created.
fn should_send_status_command(
    status_sent: bool,
    is_codex: bool,
    start_elapsed: Duration,
    first_output_elapsed: Option<Duration>,
) -> bool {
    if status_sent {
        return false;
    }

    if is_codex {
        first_output_elapsed
            .map(|elapsed| elapsed >= Duration::from_secs(1))
            .unwrap_or(false)
            // Fallback: if Codex has no output for a long time, don't wait forever either
            || start_elapsed >= Duration::from_secs(8)
    } else {
        start_elapsed >= Duration::from_millis(1500)
    }
}

fn send_status_command(app: &AppHandle, task_id: &str, is_codex: bool) {
    if is_codex {
        // Codex has an autocomplete menu, so type /status first to trigger the menu, then send \r
        // after a delay to select and execute; release the lock between the two writes to avoid
        // holding it for a long time
        {
            let tm = app.state::<TaskManager>();
            let mut writers = tm.pty_writers.lock();
            if let Some(writer) = writers.get_mut(task_id) {
                let _ = writer.write_all(b"/status");
                let _ = writer.flush();
            }
        }
        thread::sleep(Duration::from_millis(100));
        {
            let tm = app.state::<TaskManager>();
            let mut writers = tm.pty_writers.lock();
            if let Some(writer) = writers.get_mut(task_id) {
                let _ = writer.write_all(b"\r");
                let _ = writer.flush();
            }
        }
    } else {
        let tm = app.state::<TaskManager>();
        let mut writers = tm.pty_writers.lock();
        if let Some(writer) = writers.get_mut(task_id) {
            let _ = writer.write_all(b"/status\r");
            let _ = writer.flush();
        }
    }
}

/// Dedicated to empty-prompt Claude startup: immediately register and broadcast the session id
/// using the pre-set UUID, then attach the watcher in the background once the real jsonl file appears.
/// Runs for at most 2 minutes or until the task ends, to avoid leaving the thread hanging indefinitely.
///
/// The injected `claude_sessions` entry is marked `is_placeholder: true`, which both `is_task_active`
/// and `finalize_task_exit::had_agent_session` skip; once the file appears it's upgraded to real,
/// and any exit path cleans up the placeholder entry and the claimed path.
fn spawn_claude_lazy_session_attach(
    app: AppHandle,
    task_id: String,
    session_id: String,
    project_path: String,
) {
    thread::spawn(move || {
        let Some(sessions_dir) = claude_sessions_dir_for_project(&project_path) else {
            return;
        };
        let expected = sessions_dir.join(format!("{}.jsonl", session_id));
        let path_string = expected.to_string_lossy().into_owned();

        if !claim_session_path(&app, &path_string) {
            return;
        }

        {
            let tm = app.state::<TaskManager>();
            tm.claude_sessions.lock().insert(
                task_id.clone(),
                ClaudeSessionInfo {
                    session_id: session_id.clone(),
                    session_path: path_string.clone(),
                    is_placeholder: true,
                },
            );
        }

        let _ = app.emit(
            "task-session",
            serde_json::json!({
                "task_id": task_id,
                "session_id": session_id,
                "session_path": path_string,
            }),
        );

        // Wait in the background for the file to actually appear (500ms × 240 = 2 minutes, or until the task ends).
        let mut attached = false;
        for _ in 0..240 {
            // child_handles is the only reliable signal for whether the process is alive; we can't
            // use is_task_active here, because the placeholder entry we just injected is skipped by
            // it (by design), so checking whether the process exists directly is more accurate.
            let alive = {
                let tm = app.state::<TaskManager>();
                let handles = tm.child_handles.lock();
                handles.contains_key(&task_id)
            };
            if !alive {
                break;
            }
            if expected.exists() {
                // Upgrade to real: flip placeholder to false so is_task_active / had_agent_session
                // recognize it as a valid session again.
                {
                    let tm = app.state::<TaskManager>();
                    let mut sessions = tm.claude_sessions.lock();
                    if let Some(info) = sessions.get_mut(&task_id) {
                        info.is_placeholder = false;
                    }
                }
                attached = true;
                watch_claude_session(app.clone(), task_id.clone(), expected.clone());
                break;
            }
            thread::sleep(Duration::from_millis(500));
        }

        // File appeared → watch_claude_session has taken over, and the claude_sessions entry is left
        // for finalize_task_exit to clean up when the task exits.
        if attached {
            return;
        }

        // Otherwise (timeout / task exit): clean up the placeholder entry and claimed path to avoid leaks.
        let tm = app.state::<TaskManager>();
        let removed = tm.claude_sessions.lock().remove(&task_id);
        if let Some(info) = removed {
            if info.is_placeholder {
                tm.claimed_session_paths.lock().remove(&info.session_path);
            } else {
                // Rare case: placeholder was flipped to false externally but attached is still false
                // (e.g. watch failed to start). Restore it back into sessions to avoid swallowing a real entry.
                tm.claude_sessions.lock().insert(task_id.clone(), info);
            }
        }
    });
}

/// Watch the PTY output stream and obtain the Session ID from the `/status` response.
/// Send `/status` 1.5 seconds after Claude starts; for Codex, wait 1 more second after the first
/// output is received, to avoid querying too early before the session is created.
///
/// When `pre_session_id` is `Some` (Claude >= 2.1.87), skip `/status` discovery and register the
/// session file directly using the pre-set session id. If the file doesn't appear within the
/// timeout, automatically fall back to the `/status` flow.
pub(crate) fn spawn_status_session_watcher(
    app: AppHandle,
    task_id: String,
    project_path: String,
    is_codex: bool,
    rx: mpsc::Receiver<String>,
    pre_session_id: Option<String>,
    prompt_empty: bool,
) {
    // ── Claude empty-prompt fast path: session id is known, wait for the file lazily ──
    // On empty-prompt startup Claude enters the REPL and only writes the session file to disk once
    // the user actually sends the first message, so the standard wait_for_session_file path would
    // always time out; here we broadcast immediately using the pre-generated UUID and then wait
    // indefinitely in the background for the file to appear before attaching the watcher.
    if let Some(ref sid) = pre_session_id {
        if !is_codex && prompt_empty {
            spawn_claude_lazy_session_attach(app, task_id, sid.clone(), project_path);
            return;
        }
    }

    // ── Claude >= 2.1.87 fast path: pre-set session id, don't send /status ──
    if let Some(ref sid) = pre_session_id {
        if !is_codex {
            let app2 = app.clone();
            let tid2 = task_id.clone();
            let pp2 = project_path.clone();
            let sid2 = sid.clone();
            thread::spawn(move || {
                // Wait for Claude to create the session file, up to 10 seconds
                register_and_watch_session(&app2, &tid2, &sid2, &pp2, false);

                // If register_and_watch_session couldn't find the file (its internal
                // wait_for_session_file timed out), check whether it already registered successfully;
                // if not, fall back to the old /status flow.
                let registered = {
                    let tm = app2.state::<TaskManager>();
                    let sessions = tm.claude_sessions.lock();
                    sessions
                        .get(&tid2)
                        .map(|info| !info.session_path.is_empty())
                        .unwrap_or(false)
                };
                if registered {
                    return; // Success; rx will still be dropped but that doesn't affect pty_reader
                }

                // Fallback: start the old /status flow
                run_status_session_watcher(app2, tid2, pp2, false, rx);
            });
            return;
        }
    }

    // ── Original path: Codex or Claude < 2.1.87 ──
    thread::spawn(move || {
        run_status_session_watcher(app, task_id, project_path, is_codex, rx);
    });
}

/// The old /status polling flow: Codex always takes this path, and so does Claude < 2.1.87.
fn run_status_session_watcher(
    app: AppHandle,
    task_id: String,
    project_path: String,
    is_codex: bool,
    rx: mpsc::Receiver<String>,
) {
        let start_time = Instant::now();
        let mut status_sent = false;
        let mut status_sent_at: Option<Instant> = None;
        let mut status_send_count: u32 = 0;
        let mut first_output_at = None;
        let mut accumulated = String::new();
        // A separate buffer after sending /status, to keep heavy output from pushing the /status response out of the trimming window
        let mut status_response_buf = String::new();
        let mut collecting_response = false;

        loop {
            if !is_task_active(&app, &task_id) {
                break;
            }

            let should_send_status = should_send_status_command(
                status_sent,
                is_codex,
                start_time.elapsed(),
                first_output_at.map(|instant: Instant| instant.elapsed()),
            );

            // First send or retry: if already sent but no Session ID was extracted within 3 seconds, send again.
            // Before the session is created, Codex's /status has no Session field, so a retry is needed after the task truly starts.
            // Retry at most 5 times (including the first send), to avoid continually disrupting the PTY input stream for a task that can't be parsed for a long time.
            let should_retry = status_sent
                && status_send_count < 5
                && status_sent_at
                    .map(|t| t.elapsed() >= Duration::from_secs(3))
                    .unwrap_or(false);

            if should_send_status || should_retry {
                status_sent = true;
                status_send_count += 1;
                status_sent_at = Some(Instant::now());
                collecting_response = true;
                status_response_buf.clear();
                send_status_command(&app, &task_id, is_codex);
            }

            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(chunk) => {
                    if is_codex && first_output_at.is_none() {
                        first_output_at = Some(Instant::now());
                    }
                    accumulated.push_str(&chunk);
                    // Cap the buffer size to prevent excessive memory usage
                    if accumulated.len() > 65536 {
                        let trim = accumulated.len() - 32768;
                        accumulated.drain(..trim);
                    }

                    // After sending /status, additionally collect the response into a separate
                    // buffer (up to 8KB), so trimming the main buffer doesn't drop the Session ID
                    if collecting_response {
                        status_response_buf.push_str(&chunk);
                        if status_response_buf.len() > 8192 {
                            collecting_response = false;
                        }
                    }

                    let session_id = if is_codex {
                        extract_codex_status_session_id(&status_response_buf)
                            .or_else(|| extract_codex_status_session_id(&accumulated))
                    } else {
                        extract_claude_status_session_id(&status_response_buf)
                            .or_else(|| extract_claude_status_session_id(&accumulated))
                    };

                    if let Some(sid) = session_id {
                        register_and_watch_session(&app, &task_id, &sid, &project_path, is_codex);
                        // Claude Code's /status is shown as a full-screen panel, so send ESC to close it;
                        // Codex has no such panel, so nothing to do
                        if !is_codex {
                            let tm = app.state::<TaskManager>();
                            let mut writers = tm.pty_writers.lock();
                            if let Some(writer) = writers.get_mut(&task_id) {
                                let _ = writer.write_all(b"\x1b");
                                let _ = writer.flush();
                            }
                        }
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
}

/// For use by `resume_task`: look up the session file by the known session_id and start watching.
pub(crate) fn spawn_resume_session_watcher(
    app: AppHandle,
    task_id: String,
    project_path: String,
    session_id: String,
    is_codex: bool,
) {
    thread::spawn(move || {
        register_and_watch_session(&app, &task_id, &session_id, &project_path, is_codex);
    });
}

// ── Markdown export ──────────────────────────────────────────────────────────

/// The maximum session file size export is allowed to process (200MB). Beyond this it's rejected,
/// to avoid blowing up the process with a single read_to_string. This limit is more lenient than
/// summary's 50MB, because export is a one-off operation that the user triggers deliberately.
const MAX_SESSION_BYTES_FOR_EXPORT: u64 = 200 * 1024 * 1024;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportTaskMeta {
    pub name: Option<String>,
    pub prompt: String,
    pub agent: String,
    pub created_at: i64,
    pub session_id: Option<String>,
    pub worktree_branch: Option<String>,
    pub base_branch: Option<String>,
    pub additions: Option<i64>,
    pub deletions: Option<i64>,
    pub failure_reason: Option<String>,
}

#[tauri::command]
pub async fn export_session_markdown(
    session_path: String,
    project_path: String,
    is_codex: bool,
    output_path: String,
    task_meta: ExportTaskMeta,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        export_session_markdown_inner(
            &session_path,
            &project_path,
            is_codex,
            &output_path,
            &task_meta,
        )
    })
    .await
    .map_err(|e| format!("Join error: {}", e))?
}

fn export_session_markdown_inner(
    session_path: &str,
    project_path: &str,
    is_codex: bool,
    output_path: &str,
    meta: &ExportTaskMeta,
) -> Result<(), String> {
    let canonical = validate_session_path(session_path, project_path, is_codex)?;
    let canonical_out = validate_export_output_path(output_path)?;

    let metadata = fs::metadata(&canonical)
        .map_err(|e| format!("Cannot read session metadata: {}", e))?;
    if metadata.len() > MAX_SESSION_BYTES_FOR_EXPORT {
        return Err(format!(
            "Session file is too large to export ({} MB > {} MB limit)",
            metadata.len() / 1024 / 1024,
            MAX_SESSION_BYTES_FOR_EXPORT / 1024 / 1024
        ));
    }

    // Read the JSONL line by line: avoid the temporary double-holding of `read_to_string` +
    // `lines().collect()` (the whole String + the slice Vec<&str>). True streaming parsing would
    // require rewriting parse_*_session (which consume &[&str]), and the payoff doesn't justify the
    // complexity.
    let session_file = File::open(&canonical)
        .map_err(|e| format!("Cannot open session file: {}", e))?;
    let mut lines: Vec<String> = Vec::new();
    for line in BufReader::new(session_file).lines() {
        let line = line.map_err(|e| format!("Cannot read session file: {}", e))?;
        if !line.trim().is_empty() {
            lines.push(line);
        }
    }
    let line_refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let messages = if is_codex_format(&line_refs) {
        parse_codex_session(&line_refs)
    } else {
        parse_claude_session(&line_refs)
    };

    // Write directly to the BufWriter, avoiding building a whole Markdown String first and then writing it.
    let out_file = File::create(&canonical_out)
        .map_err(|e| format!("Cannot create markdown file: {}", e))?;
    let mut writer = BufWriter::new(out_file);
    write_export_markdown(&mut writer, meta, &messages)
        .map_err(|e| format!("Cannot write markdown file: {}", e))?;
    writer
        .flush()
        .map_err(|e| format!("Cannot flush markdown file: {}", e))?;
    Ok(())
}

/// Validate the export target path passed in from the frontend via IPC.
///
/// Even though the UI goes through the Tauri save dialog, a malicious frontend can bypass the dialog
/// and invoke this command directly, so the backend must do defensive validation (see AGENTS.md:
/// "Tauri commands that accept path parameters must validate path legitimacy"). Rules:
/// - must be an absolute path
/// - must end with `.md` (matching the save dialog's filter)
/// - the parent directory must exist and be canonicalizable (to prevent symlink-chain bypass)
fn validate_export_output_path(output_path: &str) -> Result<PathBuf, String> {
    let path = Path::new(output_path);
    if !path.is_absolute() {
        return Err("Output path must be absolute".into());
    }
    let has_md_ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("md"))
        .unwrap_or(false);
    if !has_md_ext {
        return Err("Output path must end with .md".into());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| "Output path has no parent directory".to_string())?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|e| format!("Cannot resolve output directory: {}", e))?;
    if !canonical_parent.is_dir() {
        return Err("Output directory does not exist".into());
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| "Output path has no file name".to_string())?;
    Ok(canonical_parent.join(file_name))
}

fn write_export_markdown<W: Write>(
    out: &mut W,
    meta: &ExportTaskMeta,
    messages: &[SessionMessage],
) -> std::io::Result<()> {
    // Title — name or prompt may contain newlines/tabs, so sanitize them into a single line first to avoid distorting the structure below.
    let title_raw = meta
        .name
        .as_deref()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or(&meta.prompt);
    writeln!(out, "# {}\n", sanitize_md_inline(title_raw))?;

    // Metadata
    writeln!(out, "## Metadata\n")?;
    writeln!(out, "- **Agent**: {}", sanitize_md_inline(&meta.agent))?;
    writeln!(out, "- **Created**: {}", format_timestamp_ms(meta.created_at))?;
    if let Some(sid) = &meta.session_id {
        if !sid.is_empty() {
            writeln!(out, "- **Session ID**: `{}`", sanitize_md_code_span(sid))?;
        }
    }
    if let (Some(branch), Some(base)) = (&meta.worktree_branch, &meta.base_branch) {
        writeln!(
            out,
            "- **Branch**: `{}` → `{}`",
            sanitize_md_code_span(branch),
            sanitize_md_code_span(base)
        )?;
    }
    if let (Some(add), Some(del)) = (meta.additions, meta.deletions) {
        writeln!(out, "- **Diff**: +{} / −{}", add, del)?;
    }
    if let Some(reason) = &meta.failure_reason {
        if !reason.is_empty() {
            writeln!(
                out,
                "- **Failure reason**: {}",
                sanitize_md_inline(reason)
            )?;
        }
    }
    writeln!(out)?;

    // Prompt
    writeln!(out, "## Prompt\n")?;
    if meta.prompt.trim().is_empty() {
        writeln!(out, "> _(empty)_")?;
    } else {
        for line in meta.prompt.lines() {
            writeln!(out, "> {}", line)?;
        }
    }
    writeln!(out)?;

    // Conversation — export only the plain text of user / assistant, discarding tool_use and thinking.
    // If a message has no text blocks left after filtering, skip it along with its role heading, to avoid empty sections.
    writeln!(out, "## Conversation\n")?;
    let mut current_role: Option<&str> = None;
    for msg in messages {
        let texts: Vec<&str> = msg
            .content
            .iter()
            .filter_map(|c| match c {
                SessionContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        if texts.is_empty() {
            continue;
        }
        if current_role != Some(msg.role.as_str()) {
            current_role = Some(msg.role.as_str());
            match msg.role.as_str() {
                "user" => writeln!(out, "### User\n")?,
                "assistant" => writeln!(out, "### Assistant\n")?,
                other => {
                    // If the parser adds new roles in the future, output them literally rather than panicking.
                    writeln!(out, "### {}\n", sanitize_md_inline(other))?;
                    continue;
                }
            }
        }
        for text in texts {
            out.write_all(text.as_bytes())?;
            if !text.ends_with('\n') {
                out.write_all(b"\n")?;
            }
            out.write_all(b"\n")?;
        }
    }
    Ok(())
}

/// Collapse a multiline / control-character metadata value into a single line: all whitespace and
/// control characters fold into one space, with leading/trailing trim.
/// Used for Markdown titles, list items, and other "must be single-line" positions.
fn sanitize_md_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.trim().chars() {
        if c.is_whitespace() || c.is_control() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out
}

/// Escaping for an inline code span `…`: first collapse to a single line (code spans don't allow
/// newlines), then replace backticks with single quotes, otherwise a backtick inside `…` would
/// close the span early and tear apart the Markdown that follows.
fn sanitize_md_code_span(s: &str) -> String {
    sanitize_md_inline(s).replace('`', "'")
}

fn format_timestamp_ms(ms: i64) -> String {
    use chrono::{TimeZone, Utc};
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| ms.to_string())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_claude_status_session_id_from_status_output() {
        // Simple \r\n separated output
        let output = "\x1b[0m\r\n  Version: 2.1.81\r\n  Session ID: 1aee0948-e0f2-4ad1-b710-ba236fab378a\r\n  cwd: /workspace\r\n\x1b[0m";
        assert_eq!(
            extract_claude_status_session_id(output),
            Some("1aee0948-e0f2-4ad1-b710-ba236fab378a".to_string())
        );
    }

    #[test]
    fn extract_claude_status_session_id_cursor_positioned() {
        // Claude Code renders /status using cursor-positioning sequences; after ANSI
        // stripping the text collapses onto one line with no \r\n separators.
        let output = "\x1b[1;1H  Version: 2.1.83\x1b[2;1H  Session ID: 9d5533cd-af1e-48d5-99d3-a9e61b2a5250\x1b[3;1H  cwd: /workspace";
        assert_eq!(
            extract_claude_status_session_id(output),
            Some("9d5533cd-af1e-48d5-99d3-a9e61b2a5250".to_string())
        );
    }

    #[test]
    fn extract_claude_status_session_id_returns_none_when_absent() {
        assert_eq!(extract_claude_status_session_id("no session info here"), None);
    }

    #[test]
    fn extract_codex_status_session_id_from_status_output() {
        let output = "\r\n│  Session:                     019d247a-2a83-76f3-b5c6-e4a59955af3f                                │\r\n";
        assert_eq!(
            extract_codex_status_session_id(output),
            Some("019d247a-2a83-76f3-b5c6-e4a59955af3f".to_string())
        );
    }

    #[test]
    fn extract_codex_status_session_id_with_ansi() {
        let output = "\x1b[0m\r\n\u{2502}  Session:                     019d0a3e-3cf7-7513-b7de-e3e9bc6c7f4d  \u{2502}\r\n\x1b[0m";
        assert_eq!(
            extract_codex_status_session_id(output),
            Some("019d0a3e-3cf7-7513-b7de-e3e9bc6c7f4d".to_string())
        );
    }

    #[test]
    fn extract_codex_status_session_id_cursor_positioned() {
        // Codex renders /status using cursor-positioning sequences; after ANSI stripping
        // all content collapses onto one line with no \r\n separators — same as Claude Code.
        let output = "\x1b[1;1H  OpenAI Codex (v0.116.0)\x1b[3;1H  Session:                     019d28df-14c0-7d03-8209-07dd4ae22cd1\x1b[4;1H  Context window:  100% left";
        assert_eq!(
            extract_codex_status_session_id(output),
            Some("019d28df-14c0-7d03-8209-07dd4ae22cd1".to_string())
        );
    }

    #[test]
    fn extract_codex_status_session_id_returns_none_when_absent() {
        assert_eq!(extract_codex_status_session_id("no session info here"), None);
    }

    #[test]
    fn codex_status_waits_for_first_output_then_one_second() {
        assert!(!should_send_status_command(
            false,
            true,
            Duration::from_secs(2),
            None,
        ));
        assert!(!should_send_status_command(
            false,
            true,
            Duration::from_millis(2200),
            Some(Duration::from_millis(900)),
        ));
        assert!(should_send_status_command(
            false,
            true,
            Duration::from_millis(2200),
            Some(Duration::from_secs(1)),
        ));
    }

    #[test]
    fn codex_status_has_global_timeout_fallback() {
        assert!(should_send_status_command(
            false,
            true,
            Duration::from_secs(8),
            None,
        ));
    }

    #[test]
    fn claude_status_keeps_original_delay() {
        assert!(!should_send_status_command(
            false,
            false,
            Duration::from_millis(1499),
            None,
        ));
        assert!(should_send_status_command(
            false,
            false,
            Duration::from_millis(1500),
            None,
        ));
    }

    #[test]
    fn read_only_command_detection_is_conservative() {
        assert!(looks_like_read_only_command("pwd && rg -n session src"));
        assert!(looks_like_read_only_command(
            "sed -n '1,120p' src-tauri/src/lib.rs"
        ));
        assert!(!looks_like_read_only_command(
            "cargo test --manifest-path src-tauri/Cargo.toml"
        ));
        assert!(!looks_like_read_only_command("echo hello > out.txt"));
    }

    #[test]
    fn powershell_read_only_commands_are_treated_as_safe() {
        assert!(looks_like_read_only_command(
            "Get-ChildItem -Force | Select-String -Pattern session"
        ));
        assert!(looks_like_read_only_command(
            "Get-Content README.md | Select-Object -First 20"
        ));
        assert!(looks_like_read_only_command("git.exe status --short"));
    }

    #[test]
    fn exec_command_confirmation_detection_matches_escalation_and_write_commands() {
        assert!(exec_command_requires_confirmation(
            r#"{"cmd":"rg -n session src","sandbox_permissions":"require_escalated"}"#
        ));
        assert!(exec_command_requires_confirmation(
            r#"{"cmd":"cargo test --manifest-path src-tauri/Cargo.toml --lib"}"#
        ));
        assert!(!exec_command_requires_confirmation(
            r#"{"cmd":"git status --short"}"#
        ));
    }

    #[test]
    fn apply_patch_confirmation_detection_only_flags_external_absolute_paths() {
        let project_root = Path::new("/repo");

        assert!(!apply_patch_requires_confirmation(
            "*** Begin Patch\n*** Update File: src/main.rs\n*** End Patch",
            project_root,
        ));
        assert!(!apply_patch_requires_confirmation(
            "*** Begin Patch\n*** Update File: /repo/src/main.rs\n*** End Patch",
            project_root,
        ));
        assert!(apply_patch_requires_confirmation(
            "*** Begin Patch\n*** Update File: /tmp/outside.rs\n*** End Patch",
            project_root,
        ));
    }

    #[test]
    fn final_assistant_question_is_treated_as_input_required() {
        let payload = serde_json::json!({
            "role": "assistant",
            "phase": "final_answer",
            "content": [
                { "type": "output_text", "text": "Shall I keep going with this approach？" }
            ]
        });

        assert!(assistant_message_requests_user_input(Some(&payload)));
    }

    fn sample_meta() -> ExportTaskMeta {
        ExportTaskMeta {
            name: Some("Demo task".into()),
            prompt: "do the thing".into(),
            agent: "claude".into(),
            created_at: 1_715_990_400_000, // 2024-05-18T00:00:00Z
            session_id: Some("abc-123".into()),
            worktree_branch: None,
            base_branch: None,
            additions: None,
            deletions: None,
            failure_reason: None,
        }
    }

    /// Test helper: collect the streaming output into a Vec<u8> and convert to String, to make asserting on the content easier.
    fn render_to_string(meta: &ExportTaskMeta, msgs: &[SessionMessage]) -> String {
        let mut buf: Vec<u8> = Vec::new();
        write_export_markdown(&mut buf, meta, msgs).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn export_markdown_includes_metadata_and_prompt() {
        let md = render_to_string(&sample_meta(), &[]);
        assert!(md.starts_with("# Demo task\n\n"), "title missing: {}", md);
        assert!(md.contains("- **Agent**: claude"));
        assert!(md.contains("- **Session ID**: `abc-123`"));
        assert!(md.contains("> do the thing"));
    }

    #[test]
    fn export_markdown_drops_tool_use_and_thinking_blocks() {
        let messages = vec![
            SessionMessage {
                role: "assistant".into(),
                content: vec![
                    SessionContent::Thinking {
                        thinking: "let me reason".into(),
                    },
                    SessionContent::Text {
                        text: "first turn".into(),
                    },
                ],
            },
            SessionMessage {
                role: "assistant".into(),
                content: vec![SessionContent::ToolUse {
                    id: "t1".into(),
                    name: "Bash".into(),
                    input: "{\"cmd\":\"ls\"}".into(),
                }],
            },
            SessionMessage {
                role: "assistant".into(),
                content: vec![SessionContent::Text {
                    text: "second turn".into(),
                }],
            },
        ];
        let md = render_to_string(&sample_meta(), &messages);
        // Consecutive assistant text should merge under the same heading; tool-only messages are discarded entirely
        assert_eq!(md.matches("### Assistant").count(), 1, "{}", md);
        assert!(!md.contains("👤"));
        assert!(!md.contains("🤖"));
        assert!(md.contains("first turn"));
        assert!(md.contains("second turn"));
        assert!(!md.contains("🔧"));
        assert!(!md.contains("Bash"));
        assert!(!md.contains("Thinking"));
        assert!(!md.contains("let me reason"));
    }

    #[test]
    fn export_markdown_falls_back_to_prompt_when_name_missing() {
        let mut meta = sample_meta();
        meta.name = None;
        meta.prompt = "fix the login bug".into();
        let md = render_to_string(&meta, &[]);
        assert!(
            md.starts_with("# fix the login bug\n\n"),
            "title fallback wrong: {}",
            md
        );
    }

    #[test]
    fn export_markdown_sanitizes_metadata_with_newlines_and_backticks() {
        let mut meta = sample_meta();
        meta.name = Some("multi\nline\ttitle".into());
        meta.session_id = Some("abc`evil`123".into());
        meta.worktree_branch = Some("feat/`branch".into());
        meta.base_branch = Some("main".into());
        meta.failure_reason = Some("first line\nsecond line".into());
        let md = render_to_string(&meta, &[]);

        // The title is collapsed to a single line; newlines/tabs must not distort structure beyond the # heading
        assert!(
            md.starts_with("# multi line title\n\n"),
            "title not collapsed: {}",
            md
        );
        // session_id / branch are inside inline code spans, so backticks must be replaced
        assert!(md.contains("- **Session ID**: `abc'evil'123`"), "{}", md);
        assert!(md.contains("- **Branch**: `feat/'branch` → `main`"), "{}", md);
        // The newline in the failure reason is folded into a single space, not breaking the list item
        assert!(md.contains("- **Failure reason**: first line second line"), "{}", md);
    }

    #[test]
    fn validate_export_output_path_rejects_relative_and_non_md() {
        assert!(validate_export_output_path("relative/path.md").is_err());
        assert!(validate_export_output_path("/tmp/notamd.txt").is_err());
    }

    #[test]
    fn validate_export_output_path_rejects_missing_parent() {
        // A parent directory extremely unlikely to exist
        assert!(validate_export_output_path(
            "/nonexistent-9c3a/__fastaf_export_test__/out.md"
        )
        .is_err());
    }

    #[test]
    fn validate_export_output_path_accepts_md_under_existing_dir() {
        let dir = std::env::temp_dir();
        let candidate = dir.join("fastaf-validate-output.md");
        // The file itself need not exist; only the parent directory needs to exist.
        let canonical = validate_export_output_path(candidate.to_str().unwrap())
            .expect("temp dir export path should validate");
        assert!(canonical.is_absolute());
        assert_eq!(canonical.extension().and_then(|e| e.to_str()), Some("md"));
    }
}
