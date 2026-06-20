//! Hook injection and uninstallation.
//!
//! Design:
//! - Shared mjs script `~/.fastaf/hooks/fastaf-hook.mjs`
//! - Claude: parse `~/.claude/settings.json` and append, into each event's array,
//!   an object carrying a `_fastaf_managed: "1"` field. Claude ignores unknown
//!   fields, so we rely on this marker for idempotent upgrades and precise
//!   uninstallation.
//! - Codex: in `~/.codex/config.toml`, replace the whole region wrapped by the
//!   `# >>> fastaf-managed-begin >>>` / `# <<< fastaf-managed-end <<<` comments.
//!   User content outside that region is preserved verbatim via string slicing.
//! - The hook script is guarded by the FASTAF_TASK_ID + FASTAF_EVENT_DIR environment
//!   variables; when the user runs the agent manually the hook exits 0 immediately
//!   with no side effects.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::storage::atomic_write;

/// Minimum agent version required for the hook path to be trustworthy.
/// The Codex threshold is 0.131.0: that release added
/// `--dangerously-bypass-hook-trust`; below it, injected hooks get skipped by the
/// trust model or error when the flag is assembled, so we fall back to the polling watcher.
const CODEX_HOOK_MIN_VERSION: &str = "0.131.0";
const CLAUDE_HOOK_MIN_VERSION: &str = "2.1.87";

const HOOK_SCRIPT: &str = include_str!("fastaf-hook.mjs");

const FASTAF_MARKER_FIELD: &str = "_fastaf_managed";

const CODEX_BEGIN: &str = "# >>> fastaf-managed-begin (do not edit; managed by FastAF) >>>";
const CODEX_END: &str = "# <<< fastaf-managed-end <<<";

const CLAUDE_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "Notification",
    // PostToolUse: fires after a tool runs successfully (in ask mode, after the
    // user approves), used to reset input_required back to running — UserPromptSubmit
    // does not fire on tool approval.
    "PostToolUse",
    "Stop",
    "SubagentStop",
];

const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PermissionRequest",
    // See the PostToolUse note on CLAUDE_EVENTS; Codex also supports it since 0.124.
    "PostToolUse",
    "Stop",
    "SubagentStop",
];

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct HookInstallStatus {
    pub node_path: String,
    pub script_path: String,
    pub claude_installed: bool,
    pub codex_installed: bool,
    /// Description of an error that occurred during install (shown to the user, optional)
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub error: String,
}

// ── Path helpers ────────────────────────────────────────────────────────────

fn home_dir() -> Result<PathBuf, String> {
    crate::platform::home_dir().ok_or_else(|| "Cannot find home directory".to_string())
}

pub fn hooks_dir() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".fastaf").join("hooks"))
}

pub fn script_path() -> Result<PathBuf, String> {
    Ok(hooks_dir()?.join("fastaf-hook.mjs"))
}

pub fn events_root() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".fastaf").join("events"))
}

pub fn events_dir_for(task_id: &str) -> Result<PathBuf, String> {
    Ok(events_root()?.join(task_id))
}

fn claude_settings_path() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".claude").join("settings.json"))
}

fn codex_config_path() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".codex").join("config.toml"))
}

// ── Node detection ──────────────────────────────────────────────────────────

/// Detect an available node interpreter path; returns None on failure.
pub fn detect_node() -> Option<String> {
    let raw = crate::platform::detect_path("node");
    if raw.is_empty() {
        return None;
    }
    // Resolve via realpath to bypass shims like nvm/asdf — only needed on Unix.
    // On Windows, fs::canonicalize produces a verbatim path with a `\\?\` prefix,
    // which cmd.exe does not recognize (independent of OS version, true on Win10+ too),
    // causing the hook command to fail to start; and Windows uses nvm-windows rather than
    // symlink shims, so there's no need for this anyway — hence we just use the plain path
    // returned by detect_path.
    #[cfg(unix)]
    {
        if let Ok(real) = fs::canonicalize(&raw) {
            return Some(real.to_string_lossy().into_owned());
        }
    }
    Some(raw)
}

// ── Script writing ──────────────────────────────────────────────────────────

pub fn write_hook_script() -> Result<PathBuf, String> {
    let dir = hooks_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("create {}: {}", dir.display(), e))?;
    let path = script_path()?;
    atomic_write(&path, HOOK_SCRIPT)?;
    Ok(path)
}

// ── Claude (command-line --settings) ─────────────────────────────────────────

/// Path to FastAF's own Claude hooks config file (~/.fastaf/hooks/claude-settings.json).
/// Claude tasks pass it in at launch via `--settings <this path>`, never modifying the
/// user's ~/.claude/settings.json. The config is static (node + script path), written
/// once and reused.
pub fn fastaf_claude_settings_path() -> Result<PathBuf, String> {
    Ok(hooks_dir()?.join("claude-settings.json"))
}

/// Build a cross-shell-safe hook invocation command string, shared by Claude / Codex.
///
/// The form is fixed as `node "<script>"`: **the bare command name `node` is the first token**;
/// cmd.exe / PowerShell / Git Bash / sh all parse it as "invoke node on PATH"; the script path
/// is wrapped in double quotes to accommodate spaces.
///
/// **Do not** revert to a quoted full node path (`"C:\…\node.exe" "<script>"`): there the first
/// token is a quoted string, and PowerShell (when Claude has no Git Bash, and the fallback shell
/// of some Codex versions) treats it as a string literal, raising `UnexpectedToken` at the second
/// path. Bare node is also the common form used by community injectors (claude-code-hooks etc.)
/// and the official Claude/Codex examples. node must be on PATH—`detect_node()` already probes the
/// login shell's PATH, and the agent process plus its spawned hook subshells all inherit the same PATH.
fn hook_command(script: &str) -> String {
    format!("node \"{}\"", script)
}

/// Build a Claude settings value containing only FastAF hooks. Sets only `hooks` (array-typed,
/// Claude merges across sources + dedupes by command), with no scalar keys, so it never
/// overrides user config.
fn build_claude_settings_value(_node_path: &str, script: &str) -> Value {
    let entry = serde_json::json!({
        "hooks": [{ "type": "command", "command": hook_command(script) }],
    });
    let mut hooks = Map::new();
    for event in CLAUDE_EVENTS {
        hooks.insert((*event).to_string(), Value::Array(vec![entry.clone()]));
    }
    serde_json::json!({ "hooks": Value::Object(hooks) })
}

/// Write FastAF's own Claude settings file. Serialized with serde_json—backslashes in Windows
/// paths are correctly escaped; and what's passed to Claude is a plain file path, not subject to
/// command-line string escaping, so it's safe cross-platform (including Windows CreateProcess).
fn write_claude_settings(node_path: &str, script: &str) -> Result<PathBuf, String> {
    let dir = hooks_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("create {}: {}", dir.display(), e))?;
    let path = fastaf_claude_settings_path()?;
    let value = build_claude_settings_value(node_path, script);
    let raw = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    atomic_write(&path, &raw)?;
    Ok(path)
}

// ── Claude legacy injection cleanup (migration) ──────────────────────────────
// The current version uses command-line `--settings` and no longer writes the user's
// settings.json; the functions below only clean up the `_fastaf_managed` entries that
// older versions injected into the user's settings.json.

fn is_fastaf_managed(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|obj| obj.get(FASTAF_MARKER_FIELD))
        .and_then(|v| v.as_str())
        .is_some()
}

/// Remove FastAF hooks from a settings JSON object.
fn uninject_claude_value(mut root: Value) -> Value {
    let Some(root_obj) = root.as_object_mut() else {
        return root;
    };
    let Some(hooks) = root_obj.get_mut("hooks").and_then(|v| v.as_object_mut()) else {
        return root;
    };
    // Collect the names of event arrays to clear
    let event_keys: Vec<String> = hooks
        .iter()
        .filter_map(|(k, v)| v.as_array().map(|_| k.clone()))
        .collect();
    for key in event_keys {
        if let Some(arr) = hooks.get_mut(&key).and_then(|v| v.as_array_mut()) {
            arr.retain(|entry| !is_fastaf_managed(entry));
        }
    }
    // Don't delete empty arrays or the hooks object itself; preserve the user's existing structure
    root
}

fn uninject_claude_settings_at(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    if raw.trim().is_empty() {
        return Ok(());
    }
    let root = serde_json::from_str::<Value>(&raw)
        .map_err(|e| format!("parse {}: {}", path.display(), e))?;
    let updated = uninject_claude_value(root);
    let raw = serde_json::to_string_pretty(&updated).map_err(|e| e.to_string())?;
    atomic_write(path, &raw)
}

// ── Claude global settings.json injection ────────────────────────────────────
// Beyond the command-line `--settings` file (used when FastAF launches Claude itself),
// we also inject the hook into the user's global ~/.claude/settings.json so that a
// `claude` the user starts MANUALLY inside a FastAF terminal session also fires the hook
// and reports status. Entries are marked with `_fastaf_managed` so they are removable and
// idempotent; Claude ignores the extra field and dedupes hook execution by `command`, so a
// wrapped task carrying both the `--settings` file and the global entry runs the hook once.

/// One FastAF-managed hook group for a Claude event array (`{_fastaf_managed, hooks:[…]}`).
fn claude_managed_group(script: &str) -> Value {
    let mut group = Map::new();
    group.insert(FASTAF_MARKER_FIELD.to_string(), Value::String("1".into()));
    group.insert(
        "hooks".to_string(),
        serde_json::json!([{ "type": "command", "command": hook_command(script) }]),
    );
    Value::Object(group)
}

fn build_claude_hooks_map(script: &str) -> Map<String, Value> {
    let mut hooks = Map::new();
    for event in CLAUDE_EVENTS {
        hooks.insert(
            (*event).to_string(),
            Value::Array(vec![claude_managed_group(script)]),
        );
    }
    hooks
}

/// Additively inject FastAF hooks into a Claude settings object, preserving the user's own
/// hooks. Idempotent: existing FastAF-managed groups are stripped first, then re-appended.
fn inject_claude_value(root: Value, script: &str) -> Value {
    let mut root = uninject_claude_value(root);
    let Some(root_obj) = root.as_object_mut() else {
        // Corrupt / non-object root: start fresh with just our hooks.
        return serde_json::json!({ "hooks": build_claude_hooks_map(script) });
    };
    let hooks = root_obj
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    if !hooks.is_object() {
        *hooks = Value::Object(Map::new());
    }
    let hooks_obj = hooks.as_object_mut().unwrap();
    for event in CLAUDE_EVENTS {
        let arr = hooks_obj
            .entry((*event).to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(a) = arr.as_array_mut() {
            a.push(claude_managed_group(script));
        } else {
            *arr = Value::Array(vec![claude_managed_group(script)]);
        }
    }
    root
}

/// Inject FastAF hooks into the user's ~/.claude/settings.json (creating it if absent).
fn inject_claude_settings_at(path: &Path, script: &str) -> Result<(), String> {
    let root = if path.exists() {
        let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
        if raw.trim().is_empty() {
            Value::Object(Map::new())
        } else {
            serde_json::from_str::<Value>(&raw)
                .map_err(|e| format!("parse {}: {}", path.display(), e))?
        }
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        Value::Object(Map::new())
    };
    let updated = inject_claude_value(root, script);
    let raw = serde_json::to_string_pretty(&updated).map_err(|e| e.to_string())?;
    atomic_write(path, &raw)
}

// ── Codex (TOML) injection and uninstallation ────────────────────────────────

fn build_codex_block(_node_path: &str, script: &str) -> String {
    let mut out = String::new();
    out.push_str(CODEX_BEGIN);
    out.push('\n');
    for event in CODEX_EVENTS {
        out.push_str(&format!("[[hooks.{}]]\n", event));
        out.push_str(&format!("[[hooks.{}.hooks]]\n", event));
        out.push_str("type = \"command\"\n");
        // Codex's `command` can only be a string (no args array); it runs via `cmd.exe /C`
        // on Windows and via `/bin/sh -lc` on Unix; bare `node "<script>"` works on both
        // sides. toml_quote escapes the inner `"` and path backslashes into valid TOML.
        out.push_str(&format!("command = {}\n", toml_quote(&hook_command(script))));
        out.push('\n');
    }
    out.push_str(CODEX_END);
    out.push('\n');
    out
}

/// Safely convert a string into a TOML basic string literal.
fn toml_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Write (or update) the FastAF block into the given TOML content.
fn inject_codex_text(existing: &str, node_path: &str, script: &str) -> String {
    let block = build_codex_block(node_path, script);
    if let (Some(begin), Some(end)) = (existing.find(CODEX_BEGIN), existing.find(CODEX_END)) {
        if begin < end {
            let end_line_end = existing[end..]
                .find('\n')
                .map(|n| end + n + 1)
                .unwrap_or(existing.len());
            // Compute the part before begin to keep (trim the adjacent newline for cleanliness)
            let before = &existing[..begin];
            let after = &existing[end_line_end..];
            let mut out = String::with_capacity(before.len() + block.len() + after.len());
            out.push_str(before);
            if !before.is_empty() && !before.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&block);
            if !after.is_empty() && !after.starts_with('\n') {
                out.push('\n');
            }
            out.push_str(after);
            return out;
        }
    }

    // No marker; append at the end of the file
    let mut out = String::with_capacity(existing.len() + block.len() + 2);
    out.push_str(existing);
    if !existing.is_empty() && !existing.ends_with('\n') {
        out.push('\n');
    }
    if !existing.is_empty() {
        out.push('\n');
    }
    out.push_str(&block);
    out
}

/// Remove the FastAF block from the TOML content.
fn uninject_codex_text(existing: &str) -> String {
    let (Some(begin), Some(end)) = (existing.find(CODEX_BEGIN), existing.find(CODEX_END)) else {
        return existing.to_string();
    };
    if begin >= end {
        return existing.to_string();
    }
    let end_line_end = existing[end..]
        .find('\n')
        .map(|n| end + n + 1)
        .unwrap_or(existing.len());
    let before = &existing[..begin];
    let after = &existing[end_line_end..];
    let mut out = String::with_capacity(before.len() + after.len());
    out.push_str(before);
    // Skip any extra blank lines at the end of before, to keep the file tidy
    while out.ends_with("\n\n") {
        out.pop();
    }
    if !after.is_empty() {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(after.trim_start_matches('\n'));
        if !out.ends_with('\n') {
            out.push('\n');
        }
    } else if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn inject_codex_config_at(path: &Path, node_path: &str, script: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {}", parent.display(), e))?;
    }
    let existing = if path.exists() {
        fs::read_to_string(path).map_err(|e| e.to_string())?
    } else {
        String::new()
    };
    let updated = inject_codex_text(&existing, node_path, script);
    // Validate that it's valid TOML
    toml::from_str::<toml::Value>(&updated)
        .map_err(|e| format!("FastAF-injected TOML parse error: {}", e))?;
    atomic_write(path, &updated)
}

fn uninject_codex_config_at(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let existing = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let updated = uninject_codex_text(&existing);
    if updated == existing {
        return Ok(());
    }
    atomic_write(path, &updated)
}

// ── Install status cache + trust check ────────────────────────────────────────

/// Cache the status from the most recent install/query, so `usable_for` can read it with
/// zero blocking at task startup (avoids running a `which node` subprocess on every task launch).
static CACHED_STATUS: OnceLock<Mutex<HookInstallStatus>> = OnceLock::new();

fn status_cache() -> &'static Mutex<HookInstallStatus> {
    CACHED_STATUS.get_or_init(|| Mutex::new(HookInstallStatus::default()))
}

/// Write the cached install status (called at startup and after install/uninstall).
pub fn cache_status(status: HookInstallStatus) {
    *status_cache().lock() = status;
}

/// A single agent's hook readiness status (for display on the frontend task-creation / settings page).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HookAgentReadiness {
    pub agent: String,
    pub usable: bool,
    /// "ok" | "no_node" | "not_installed" | "version_too_low"
    pub reason: String,
    pub detected_version: String,
    pub min_version: String,
}

fn readiness_for(agent: &str, status: &HookInstallStatus) -> HookAgentReadiness {
    let (installed, min_version, detected) = if agent == "codex" {
        (
            status.codex_installed,
            CODEX_HOOK_MIN_VERSION,
            crate::app_settings::detect_codex_version().unwrap_or_default(),
        )
    } else {
        (
            status.claude_installed,
            CLAUDE_HOOK_MIN_VERSION,
            crate::app_settings::detect_claude_version().unwrap_or_default(),
        )
    };

    let version_ok = !detected.is_empty()
        && if agent == "codex" {
            crate::app_settings::codex_version_gte(min_version)
        } else {
            crate::app_settings::claude_version_gte(min_version)
        };

    let reason = if status.node_path.is_empty() {
        "no_node"
    } else if !installed {
        "not_installed"
    } else if !version_ok {
        "version_too_low"
    } else {
        "ok"
    };

    HookAgentReadiness {
        agent: agent.to_string(),
        usable: reason == "ok",
        reason: reason.to_string(),
        detected_version: detected,
        min_version: min_version.to_string(),
    }
}

/// Determine whether the given agent's hook path is trustworthy and can replace polling.
/// All three must hold: node available + the hook is installed for that agent + agent version ≥ threshold.
/// If any fails, returns false, and the caller should fall back to the `/status` polling path.
///
/// Version numbers all go through the global cached probe `*_version_gte`, no longer reading the
/// version field from the project-level config.
pub fn usable_for(agent: &str) -> bool {
    let status = status_cache().lock().clone();
    if status.node_path.is_empty() {
        return false;
    }
    if agent == "codex" {
        status.codex_installed && crate::app_settings::codex_version_gte(CODEX_HOOK_MIN_VERSION)
    } else {
        status.claude_installed && crate::app_settings::claude_version_gte(CLAUDE_HOOK_MIN_VERSION)
    }
}

// ── Public entry points ───────────────────────────────────────────────────────

/// One-time install at startup. Failures don't block; it just returns the status.
pub fn ensure_installed() -> HookInstallStatus {
    let mut status = HookInstallStatus::default();
    let Some(node) = detect_node() else {
        status.error = "node not found in PATH".into();
        return status;
    };
    status.node_path = node.clone();

    let script = match write_hook_script() {
        Ok(p) => p.to_string_lossy().into_owned(),
        Err(e) => {
            status.error = format!("write hook script: {}", e);
            return status;
        }
    };
    status.script_path = script.clone();

    // Claude: command-line --settings mode—write the hooks into FastAF's own file and pass it via
    // `--settings <path>` at task launch, never modifying the user's ~/.claude/settings.json.
    match write_claude_settings(&node, &script) {
        Ok(_) => status.claude_installed = true,
        Err(e) => status.error = format!("claude settings: {}", e),
    }
    // Also inject the hook into the user's global ~/.claude/settings.json so a `claude` the user
    // launches MANUALLY inside a terminal session reports status too (additive + idempotent;
    // preserves the user's own hooks). Without this, only FastAF-launched Claude tasks — which pass
    // `--settings` — would fire the hook, leaving terminal sessions with no status.
    if let Ok(p) = claude_settings_path() {
        let _ = inject_claude_settings_at(&p, &script);
    }

    match codex_config_path().and_then(|p| inject_codex_config_at(&p, &node, &script)) {
        Ok(_) => status.codex_installed = true,
        Err(e) => {
            if status.error.is_empty() {
                status.error = format!("codex config: {}", e);
            } else {
                status.error = format!("{}; codex config: {}", status.error, e);
            }
        }
    }

    status
}

/// Uninstall the hooks injected by FastAF (does not delete the script itself).
pub fn uninstall() -> Result<(), String> {
    // Claude: delete FastAF's own settings file, and clean up any injected entries older versions
    // may have left in the user's ~/.claude/settings.json.
    if let Ok(p) = fastaf_claude_settings_path() {
        let _ = fs::remove_file(&p);
    }
    let claude = claude_settings_path()?;
    uninject_claude_settings_at(&claude)?;
    let codex = codex_config_path()?;
    uninject_codex_config_at(&codex)?;
    Ok(())
}

/// Check whether it's currently installed (for UI status display).
pub fn current_status() -> HookInstallStatus {
    let mut status = HookInstallStatus {
        node_path: detect_node().unwrap_or_default(),
        script_path: script_path()
            .ok()
            .filter(|p| p.exists())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        ..Default::default()
    };
    // Claude command-line mode: the presence of FastAF's own settings file means it's ready.
    if let Ok(p) = fastaf_claude_settings_path() {
        status.claude_installed = p.exists();
    }
    if let Ok(p) = codex_config_path() {
        status.codex_installed = codex_config_has_fastaf(&p);
    }
    status
}

fn codex_config_has_fastaf(path: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(path) else {
        return false;
    };
    raw.contains(CODEX_BEGIN) && raw.contains(CODEX_END)
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_hook_status() -> Result<HookInstallStatus, String> {
    tokio::task::spawn_blocking(current_status)
        .await
        .map_err(|e| e.to_string())
}

/// Return the hook readiness status of both the claude and codex agents (node + installed + version).
#[tauri::command]
pub async fn get_hook_readiness() -> Result<Vec<HookAgentReadiness>, String> {
    tokio::task::spawn_blocking(|| {
        let status = current_status();
        // Refresh the cache while we're at it: keep the node/install status that `usable_for`
        // reads at task startup consistent with the live status shown to the user here (covers
        // cases like installing node only after startup).
        cache_status(status.clone());
        vec![
            readiness_for("claude", &status),
            readiness_for("codex", &status),
        ]
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn install_hooks() -> Result<HookInstallStatus, String> {
    tokio::task::spawn_blocking(|| {
        let status = ensure_installed();
        cache_status(status.clone());
        status
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn uninstall_hooks() -> Result<(), String> {
    tokio::task::spawn_blocking(|| {
        let result = uninstall();
        // Refresh the cache after uninstall, so subsequent tasks fall back to the polling path
        cache_status(current_status());
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Claude settings construction (command-line --settings mode) ──────────

    #[test]
    fn claude_settings_value_has_all_events_no_scalar_keys() {
        let v = build_claude_settings_value("/node", "/script.mjs");
        // The top level has only hooks, never scalar keys like model (which would override user config)
        let root = v.as_object().expect("object");
        assert_eq!(root.len(), 1);
        assert!(root.contains_key("hooks"));
        for event in CLAUDE_EVENTS {
            let arr = v["hooks"][event].as_array().expect("array");
            assert_eq!(arr.len(), 1);
            // Bare node + double-quoted script path, cross-shell safe (no full node path).
            let cmd = arr[0]["hooks"][0]["command"].as_str().unwrap();
            assert_eq!(cmd, "node \"/script.mjs\"");
        }
    }

    #[test]
    fn claude_settings_value_escapes_windows_paths() {
        // The command is bare node + double-quoted script path; after serialization the
        // backslashes in the script path must be correctly escaped, ensuring the Windows path
        // is valid JSON.
        let v = build_claude_settings_value(r"C:\node.exe", r"C:\hooks\fastaf-hook.mjs");
        let raw = serde_json::to_string(&v).unwrap();
        assert!(raw.contains(r"C:\\hooks\\fastaf-hook.mjs"));
        // Round-trip parse to recover the original command
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        let cmd = parsed["hooks"]["SessionStart"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(cmd, "node \"C:\\hooks\\fastaf-hook.mjs\"");
    }

    // ── Claude legacy injection cleanup (migration) ───────────────────────────

    #[test]
    fn claude_uninject_removes_fastaf_only() {
        // Simulate settings after legacy injection: user entries + fastaf entries with the marker
        let injected = serde_json::json!({
            "hooks": {
                "Stop": [
                    { "hooks": [{ "type": "command", "command": "user-script.sh" }] },
                    { FASTAF_MARKER_FIELD: "1", "hooks": [{ "type": "command", "command": "fastaf" }] }
                ]
            }
        });
        let restored = uninject_claude_value(injected);
        // The Stop array should contain only the user's entry
        let stop = restored["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1);
        assert!(!is_fastaf_managed(&stop[0]));
    }

    #[test]
    fn claude_uninject_leaves_other_events_alone() {
        let user_only = serde_json::json!({
            "hooks": {
                "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "policy.sh" }] }]
            }
        });
        let restored = uninject_claude_value(user_only.clone());
        assert_eq!(restored, user_only);
    }

    // ── Codex TOML injection ──────────────────────────────────────────────────

    #[test]
    fn codex_inject_into_empty_creates_block() {
        let out = inject_codex_text("", "/node", "/script.mjs");
        assert!(out.contains(CODEX_BEGIN));
        assert!(out.contains(CODEX_END));
        for event in CODEX_EVENTS {
            assert!(
                out.contains(&format!("[[hooks.{}]]", event)),
                "missing event {}",
                event
            );
        }
        // Must be valid TOML
        toml::from_str::<toml::Value>(&out).expect("valid toml");
    }

    #[test]
    fn codex_inject_preserves_user_content() {
        let original = "model = \"o4-mini\"\n[tui]\nnotifications = [\"agent-turn-complete\"]\n";
        let out = inject_codex_text(original, "/node", "/script.mjs");
        // The user's original content should be fully preserved before the marker block
        let begin = out.find(CODEX_BEGIN).unwrap();
        assert!(out[..begin].contains("model = \"o4-mini\""));
        assert!(out[..begin].contains("[tui]"));
        toml::from_str::<toml::Value>(&out).expect("valid toml");
    }

    #[test]
    fn codex_inject_idempotent_upgrade() {
        let v1 = inject_codex_text("", "/oldnode", "/oldscript.mjs");
        let v2 = inject_codex_text(&v1, "/newnode", "/newscript.mjs");
        // There should be only one pair of markers
        assert_eq!(v2.matches(CODEX_BEGIN).count(), 1);
        assert_eq!(v2.matches(CODEX_END).count(), 1);
        // The command is bare node + script path (no full node path); after upgrade only the new script path remains.
        assert!(v2.contains("newscript"));
        assert!(!v2.contains("oldscript"));
    }

    #[test]
    fn codex_inject_preserves_user_hooks_via_toml_merge() {
        // The user defines their own hooks outside the marker block; ensure they're preserved
        let original = "\
[[hooks.Stop]]\n\
[[hooks.Stop.hooks]]\n\
type = \"command\"\n\
command = \"echo user-stop\"\n";
        let out = inject_codex_text(original, "/node", "/script.mjs");
        // The user's hooks.Stop should be preserved in the file (before the marker block)
        let begin = out.find(CODEX_BEGIN).unwrap();
        assert!(out[..begin].contains("echo user-stop"));
        toml::from_str::<toml::Value>(&out).expect("valid toml");
    }

    #[test]
    fn codex_uninject_removes_block_only() {
        let original = "model = \"o4-mini\"\n";
        let injected = inject_codex_text(original, "/node", "/script.mjs");
        let restored = uninject_codex_text(&injected);
        assert!(!restored.contains(CODEX_BEGIN));
        assert!(!restored.contains(CODEX_END));
        assert!(restored.contains("model = \"o4-mini\""));
    }

    #[test]
    fn codex_uninject_no_marker_is_noop() {
        let original = "model = \"o4-mini\"\n[tui]\n";
        assert_eq!(uninject_codex_text(original), original);
    }

    #[test]
    fn toml_quote_escapes_special() {
        assert_eq!(toml_quote("plain"), "\"plain\"");
        assert_eq!(toml_quote("with \"quote\""), "\"with \\\"quote\\\"\"");
        assert_eq!(toml_quote("with\\back"), "\"with\\\\back\"");
    }

    // ── File-level integration ────────────────────────────────────────────────

    #[test]
    fn codex_inject_file_round_trip() {
        let tmp = std::env::temp_dir().join(format!("fastaf-codex-{}.toml", std::process::id()));
        let _ = fs::remove_file(&tmp);

        inject_codex_config_at(&tmp, "/node", "/script.mjs").expect("inject");
        let raw = fs::read_to_string(&tmp).unwrap();
        assert!(raw.contains(CODEX_BEGIN));

        uninject_codex_config_at(&tmp).expect("uninject");
        let raw = fs::read_to_string(&tmp).unwrap();
        assert!(!raw.contains(CODEX_BEGIN));

        let _ = fs::remove_file(&tmp);
    }
}
