//! Watches the events.jsonl written by the hook script and dispatches events to the frontend.
//!
//! How it works:
//! - A long-lived thread polling `~/.fastaf/events/<task_id>/events.jsonl` every 200ms
//! - Each file maintains a byte offset; only incremental lines are read
//! - After parsing each line of JSON, dispatch by the event field:
//!   * SessionStart → register the session into TaskManager + emit `task-session`
//!   * Notification (Claude) / PermissionRequest (Codex) → `task-status` = input_required
//!   * UserPromptSubmit / PostToolUse → `task-status` = running (clears input_required)
//!   * Stop (Claude & Codex) → `task-status` = input_required (one round of the interactive
//!     REPL is done, waiting for the user; the process does not exit, so the PTY exit monitor
//!     does not fire). Claude cannot rely on Notification as a fallback—its "idle, waiting for
//!     input" Notification fires only after about 60s, which would make the badge appear a minute late.
//!   * SubagentStop → don't emit proactively; let the PTY exit monitor handle the terminal state
//!
//! Event-driven (rather than fixed-interval polling): nearly zero wakeups when idle, near-instant
//! response on writes, cutting out the previous worst-case 200ms polling wait. When watcher
//! initialization fails, fall back to fixed-interval polling (FALLBACK_INTERVAL), and use the same
//! interval as a fallback wakeup to prevent missed events from leaving state stuck.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::session::{ClaudeSessionInfo, CodexSessionInfo};
use crate::TaskManager;

/// The fallback polling interval when the watcher is unavailable (initialization failed); when the
/// watcher is working it's also used as the fallback wakeup interval—even if a file event is missed,
/// it will be rescanned within this interval at worst.
const FALLBACK_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Deserialize)]
struct HookEvent {
    #[serde(default)]
    task_id: String,
    #[serde(default)]
    agent: String,
    #[serde(default)]
    event: String,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    transcript_path: String,
}

pub fn start(app: AppHandle) {
    // Run the polling loop on a dedicated long-lived thread. Can't use tokio::spawn_blocking—
    // the setup() closure runs on the main thread, where there's no Tokio runtime context yet, so it would panic.
    thread::spawn(move || run_loop(app));
}

fn run_loop(app: AppHandle) {
    use notify::{RecursiveMode, Watcher};

    let events_root = match crate::hooks::events_root() {
        Ok(p) => p,
        Err(_) => return,
    };
    // Startup cleanup: if the app was force-killed last time, the events directory may linger; no
    // tasks are running at this moment, so clearing the whole directory is safe. Otherwise, on the
    // next startup the offset starts from 0 and replays old SessionStart events, re-registering
    // sessions and emitting `task-session` for already-finished tasks. Delete then recreate to
    // ensure we start from a clean state.
    let _ = fs::remove_dir_all(&events_root);
    let _ = fs::create_dir_all(&events_root);

    // Recursively watch the entire events root directory: creation/appending of new task
    // subdirectories and their events.jsonl both trigger events, driving an incremental scan.
    // On initialization failure watcher_opt is None and it falls back to fixed-interval polling.
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher_opt = notify::RecommendedWatcher::new(tx, notify::Config::default())
        .ok()
        .and_then(|mut w| {
            w.watch(&events_root, RecursiveMode::Recursive).ok()?;
            Some(w)
        });

    let mut offsets: HashMap<PathBuf, u64> = HashMap::new();

    loop {
        // Wait for filesystem events: event-driven → nearly zero wakeups when idle, near-instant
        // wakeup on writes; the fallback timeout ensures that even if an event is missed it's
        // rescanned within FALLBACK_INTERVAL at worst. If the watcher is unavailable, poll.
        if watcher_opt.is_some() {
            match rx.recv_timeout(FALLBACK_INTERVAL) {
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => watcher_opt = None,
            }
            // Coalesce multiple events from the same batch of writes, to avoid one write triggering multiple scans
            while rx.try_recv().is_ok() {}
        } else {
            thread::sleep(FALLBACK_INTERVAL);
        }

        let Ok(entries) = fs::read_dir(&events_root) else {
            continue;
        };
        let mut seen: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let file = dir.join("events.jsonl");
            if !file.is_file() {
                continue;
            }
            seen.push(file.clone());
            let offset = *offsets.entry(file.clone()).or_insert(0);
            if let Some(new_offset) = read_and_dispatch(&app, &file, offset) {
                offsets.insert(file, new_offset);
            }
        }
        // Clean up offsets for files that have disappeared
        offsets.retain(|path, _| seen.iter().any(|p| p == path));
    }
}

fn read_and_dispatch(app: &AppHandle, path: &PathBuf, offset: u64) -> Option<u64> {
    let mut file = fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    if size <= offset {
        return Some(offset);
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;

    // Only process complete lines (ending with \n); leave partial lines for the next loop
    let mut last_complete_end = 0usize;
    for (idx, ch) in buf.char_indices() {
        if ch == '\n' {
            let line = &buf[last_complete_end..idx];
            last_complete_end = idx + 1;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(ev) = serde_json::from_str::<HookEvent>(line) {
                dispatch(app, &ev);
            }
        }
    }
    Some(offset + last_complete_end as u64)
}

fn dispatch(app: &AppHandle, ev: &HookEvent) {
    if ev.task_id.is_empty() {
        return;
    }
    match ev.event.as_str() {
        "SessionStart" => handle_session_start(app, ev),
        // Claude's Notification and Codex's PermissionRequest both mean "waiting for user input"
        // (Claude tool-approval/question notifications; Codex tool-approval/network-escalation requests).
        "Notification" | "PermissionRequest" => {
            emit_active_status(app, ev, "input_required", ev.event.as_str())
        }
        // Two signals that return to the working state and clear input_required:
        // - UserPromptSubmit: the user submitted the next prompt.
        // - PostToolUse: fires after a tool runs successfully (in ask mode, right after approval).
        //   Tool approval does not trigger UserPromptSubmit, so PostToolUse is required to reset input_required.
        "UserPromptSubmit" | "PostToolUse" => {
            emit_active_status(app, ev, "running", ev.event.as_str())
        }
        // Both Claude and Codex launch as interactive REPLs; after a round ends the process does
        // not exit but stops waiting for the user's next input, so the PTY exit monitor does not
        // fire a terminal state. Here Stop means "this round is done, waiting for the user's next
        // step", mapped to input_required (needs attention).
        // Note: Claude's Stop must be handled here and cannot rely on Notification as a fallback—
        // Claude Code's "idle, waiting for input" Notification fires only after about 60s of idle
        // (measured Stop→Notification is exactly +60s), which would make the badge appear a full
        // minute late. The Notification on tool-approval is the one that fires immediately (which is
        // why ask mode is fast). The child_handles liveness guard in emit_active_status ensures we
        // don't emit by mistake after the process has really exited; true exit is still left to the PTY exit monitor.
        "Stop" => emit_active_status(app, ev, "input_required", ev.event.as_str()),
        // SubagentStop (a subagent finished): the main agent is still working, so don't emit proactively.
        _ => {}
    }
}

fn handle_session_start(app: &AppHandle, ev: &HookEvent) {
    if ev.session_id.is_empty() {
        return;
    }
    let tm = app.state::<TaskManager>();
    let session_path = ev.transcript_path.clone();

    // Skip if already registered with a matching session_id, to avoid duplicate emits
    let already = match ev.agent.as_str() {
        "codex" => tm
            .codex_sessions
            .lock()
            .get(&ev.task_id)
            .map(|info| info.session_id == ev.session_id)
            .unwrap_or(false),
        _ => tm
            .claude_sessions
            .lock()
            .get(&ev.task_id)
            .map(|info| info.session_id == ev.session_id && !info.is_placeholder)
            .unwrap_or(false),
    };
    if already {
        return;
    }

    if ev.agent == "codex" {
        tm.codex_sessions.lock().insert(
            ev.task_id.clone(),
            CodexSessionInfo {
                session_id: ev.session_id.clone(),
                session_path: session_path.clone(),
            },
        );
    } else {
        tm.claude_sessions.lock().insert(
            ev.task_id.clone(),
            ClaudeSessionInfo {
                session_id: ev.session_id.clone(),
                session_path: session_path.clone(),
                is_placeholder: false,
            },
        );
    }
    if !session_path.is_empty() {
        let mut claimed = tm.claimed_session_paths.lock();
        claimed.insert(session_path.clone());
    }

    let _ = app.emit(
        "task-session",
        serde_json::json!({
            "task_id": ev.task_id,
            "session_id": ev.session_id,
            "session_path": session_path,
        }),
    );
}

/// Records the most recent hook-broadcast status for each task. PostToolUse fires frequently, once
/// per tool call, and emitting `running` every time would cause needless frontend setState/re-renders,
/// so we dedupe here.
static LAST_STATUS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn last_status() -> &'static Mutex<HashMap<String, String>> {
    LAST_STATUS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Broadcast only when the task process is still alive (this process holds a child-process handle)
/// and the status has changed since last time, to avoid sending input_required/running to
/// already-exited tasks and to avoid flooding from high-frequency events.
fn emit_active_status(app: &AppHandle, ev: &HookEvent, status: &str, raw_event: &str) {
    let tm = app.state::<TaskManager>();
    if !tm.child_handles.lock().contains_key(&ev.task_id) {
        return;
    }
    {
        let mut last = last_status().lock();
        if last.get(&ev.task_id).map(String::as_str) == Some(status) {
            return;
        }
        last.insert(ev.task_id.clone(), status.to_string());
    }
    // Fire a native desktop notification exactly once per transition into input_required (the
    // dedupe check above guarantees we only get here when the status actually changed, so never on
    // a repeated `running`). This uniformly covers both terminal sessions and wrapped agent tasks.
    if status == "input_required" {
        notify_attention(app, raw_event);
    }
    let _ = app.emit(
        "task-status",
        serde_json::json!({ "task_id": ev.task_id, "status": status }),
    );
}

/// Fire a native macOS banner-with-sound when an agent transitions to "needs attention".
///
/// Mechanism: the banner is shown via `tauri-plugin-notification` (title "FastAF"), and the sound is
/// played separately via `afplay` on a system sound. We deliberately do NOT rely on the plugin's
/// own `.sound(...)` because its macOS sound support routes through the bundle/notification-center
/// sound name and is unreliable for an unsigned/dev build — `afplay` always produces an audible
/// cue. Gated on the main window NOT being focused, so we don't spam the user while they're already
/// watching the app (mirrors how the superset only notifies when you're not looking at the pane).
fn notify_attention(app: &AppHandle, raw_event: &str) {
    // Only notify when the main window is not focused. If we can't determine focus (window missing
    // or query failed), treat it as "not focused" and notify, so we never silently swallow an alert.
    let focused = app
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    if focused {
        return;
    }

    let body = match raw_event {
        // Claude Stop / Codex Stop: a round finished, the agent is idle waiting for review.
        "Stop" => "Finished — ready for review",
        // Notification (Claude) / PermissionRequest (Codex): the agent is asking for input/approval.
        _ => "Waiting for your input",
    };

    // Banner via the notification plugin (cross-platform; harmless no-op visual elsewhere).
    {
        use tauri_plugin_notification::NotificationExt;
        let _ = app
            .notification()
            .builder()
            .title("FastAF")
            .body(body)
            .show();
    }

    // Sound: macOS only. Glass.aiff is the standard "attention" chime and is present on every mac.
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("afplay")
            .arg("/System/Library/Sounds/Glass.aiff")
            .spawn();
    }
}

/// Clean up the corresponding directory after a task reaches its terminal state (called by finalize_task_exit).
pub fn cleanup_task_events(task_id: &str) {
    last_status().lock().remove(task_id);
    if let Ok(dir) = crate::hooks::events_dir_for(task_id) {
        let _ = fs::remove_dir_all(dir);
    }
}
