use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::Arc;

use usage::CodexRpcClient;

mod agent_assist;
mod analytics;
mod app_settings;
mod config;
mod event_watcher;
mod fs;
mod git;
mod hooks;
mod import;
mod platform;
mod pty;
mod session;
mod skills;
mod storage;
mod subprocess;
mod usage;

use session::{ClaudeSessionInfo, CodexSessionInfo};

pub struct TaskManager {
    pub(crate) pty_masters: Mutex<HashMap<String, Box<dyn portable_pty::MasterPty + Send>>>,
    pub(crate) pty_writers: Mutex<HashMap<String, Box<dyn Write + Send>>>,
    pub(crate) child_handles:
        Mutex<HashMap<String, Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>>>>,
    pub(crate) cancelled_tasks: Mutex<HashSet<String>>,
    pub(crate) manually_completed_tasks: Mutex<HashSet<String>>,
    pub(crate) codex_sessions: Mutex<HashMap<String, CodexSessionInfo>>,
    pub(crate) claude_sessions: Mutex<HashMap<String, ClaudeSessionInfo>>,
    pub(crate) claimed_session_paths: Mutex<HashSet<String>>,
    /// Persistent `codex app-server` process reused across `read_usage_snapshot` calls.
    pub(crate) codex_rpc: Arc<Mutex<Option<CodexRpcClient>>>,
}

impl TaskManager {
    /// Atomically remove a task/shell from all PTY maps (masters, writers, children).
    /// Locks are acquired in a fixed order to prevent deadlocks.
    pub(crate) fn remove_pty_handles(&self, id: &str) {
        let mut masters = self.pty_masters.lock();
        let mut writers = self.pty_writers.lock();
        let mut children = self.child_handles.lock();
        masters.remove(id);
        writers.remove(id);
        children.remove(id);
    }
}

/// macOS: collapse the main window to the Dock (hide rather than quit).
///
/// A native fullscreen window occupies its own Space; hiding it directly leaves an empty Space (black screen), so fullscreen must be exited first.
/// But exiting fullscreen is an animated async transition: `is_fullscreen()` is still true until the animation ends, and for a brief moment right
/// after it ends `hide()` is still ignored by the system. So first poll until the exit completes, then hide repeatedly at intervals,
/// so a slightly later call takes effect after the Space has collapsed (a no-op for an already-hidden window).
/// See tauri-apps/tauri#12056 and electron/electron#20263.
#[cfg(target_os = "macos")]
fn hide_window_to_dock(window: tauri::Window) {
    use std::time::Duration;
    if !window.is_fullscreen().unwrap_or(false) {
        let _ = window.hide();
        return;
    }
    let _ = window.set_fullscreen(false);
    std::thread::spawn(move || {
        // Poll until fullscreen exit completes (~5s fallback).
        let mut exited = false;
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(50));
            if !window.is_fullscreen().unwrap_or(false) {
                exited = true;
                break;
            }
        }
        // If still fullscreen (exit failed/timed out), never hide, otherwise it would again leave a black, empty Space.
        if !exited {
            return;
        }
        // After exiting, hide may still be briefly ignored; hide multiple times at intervals to cover the remaining time until the Space collapses.
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(120));
            let _ = window.hide();
        }
    });
}

/// The frontend Cmd+W routes through this command to collapse the window, reusing the same fullscreen-aware hide logic as the close button.
/// Only macOS has real behavior (other platforms never trigger it from the frontend, see App.tsx).
#[tauri::command]
fn hide_main_window(window: tauri::Window) {
    #[cfg(target_os = "macos")]
    hide_window_to_dock(window);
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Warm up the login shell environment in the background to avoid blocking when the first task starts
            std::thread::spawn(|| {
                crate::app_settings::get_login_shell_path();
            });
            // Install hook scripts and user-level config injection (failure does not block startup; the frontend can query status).
            // The result is cached so the hook trust checks in run_task/resume_task can read it with zero blocking.
            std::thread::spawn(|| {
                crate::hooks::cache_status(crate::hooks::ensure_installed());
            });
            // Start the hook event file watcher
            crate::event_watcher::start(app.handle().clone());
            Ok(())
        })
        .manage(TaskManager {
            pty_masters: Mutex::new(HashMap::new()),
            pty_writers: Mutex::new(HashMap::new()),
            child_handles: Mutex::new(HashMap::new()),
            cancelled_tasks: Mutex::new(HashSet::new()),
            manually_completed_tasks: Mutex::new(HashSet::new()),
            codex_sessions: Mutex::new(HashMap::new()),
            claude_sessions: Mutex::new(HashMap::new()),
            claimed_session_paths: Mutex::new(HashSet::new()),
            codex_rpc: Arc::new(Mutex::new(None)),
        })
        .on_window_event(|window, event| {
            // macOS: clicking the close button (red light) hides the window rather than quitting, consistent with Cmd+W behavior;
            // clicking the Dock icon brings it back (see the Reopen handling below).
            // Other platforms have no tray/Dock entry to bring it back, so keep the default quit behavior to avoid losing the window after hiding.
            #[cfg(target_os = "macos")]
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                hide_window_to_dock(window.clone());
                api.prevent_close();
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (window, event);
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            hide_main_window,
            pty::run_task,
            pty::open_task_shell,
            pty::resume_task,
            pty::cancel_task,
            pty::complete_task,
            pty::get_active_task_ids,
            pty::reset_task_process,
            pty::send_input,
            pty::resize_pty,
            pty::open_shell,
            pty::kill_shell,
            fs::read_dir_entries,
            fs::open_in_system_file_manager,
            fs::read_file_content,
            fs::read_image_preview,
            fs::write_file_content,
            fs::create_file,
            fs::create_directory,
            fs::delete_path,
            fs::list_project_files,
            fs::search_project_files,
            fs::search_project_contents,
            git::generate_commit_message,
            agent_assist::generate_task_name,
            git::git_clone,
            git::branch_ahead_behind,
            git::git_status,
            git::git_list_branches,
            git::git_create_branch,
            git::git_checkout_branch,
            git::git_log,
            git::git_commit_detail,
            git::git_show_diff,
            git::git_show_file_diff,
            git::git_file_diff,
            git::git_stage,
            git::git_unstage,
            git::git_stage_files,
            git::git_unstage_files,
            git::git_stage_all,
            git::git_unstage_all,
            git::git_commit,
            git::git_discard_file,
            git::git_discard_files,
            git::git_discard_all,
            git::git_push,
            git::git_pull,
            git::git_remote_counts,
            git::create_task_worktree,
            git::merge_task_worktree,
            git::remove_task_worktree,
            git::worktree_diff_stats,
            analytics::read_session_metrics,
            session::read_session_messages,
            session::export_session_markdown,
            config::init_project_config,
            config::read_project_config,
            config::write_project_config,
            config::get_agent_config_file_path,
            config::read_agent_config_file,
            config::write_agent_config_file,
            storage::load_projects,
            storage::save_projects,
            storage::load_project_tasks,
            storage::save_project_tasks,
            import::discover_importable_projects,
            app_settings::load_app_settings,
            app_settings::save_app_settings,
            app_settings::save_agent_paths,
            app_settings::save_send_shortcut,
            app_settings::save_shift_enter_newline,
            app_settings::detect_agent_paths,
            app_settings::detect_agent_versions_for_settings,
            app_settings::get_system_fonts,
            usage::read_usage_snapshot,
            hooks::get_hook_status,
            hooks::get_hook_readiness,
            hooks::install_hooks,
            hooks::uninstall_hooks,
            skills::get_skill_hub_config,
            skills::set_skill_hub_path,
            skills::clear_skill_hub,
            skills::list_skills,
            skills::list_skill_installations,
            skills::install_skill,
            skills::uninstall_skill,
            skills::cleanup_installations_for_project,
            skills::delete_skill,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_app_handle, _event| {
            // macOS: after the window has been hidden by Cmd+W, clicking the Dock icon triggers Reopen,
            // at which point there is no visible window, so the main window must be manually shown again and focused.
            #[cfg(target_os = "macos")]
            {
                use tauri::Manager;
                if let tauri::RunEvent::Reopen { .. } = _event {
                    if let Some(window) = _app_handle.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            }
        });
}
