use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Validate that project_path is absolute and looks like a real project directory.
fn validate_project_path(project_path: &str) -> Result<(), String> {
    let path = Path::new(project_path);
    if !path.is_absolute() {
        return Err("Project path must be absolute".to_string());
    }
    if !path.exists() {
        return Err("Project path does not exist".to_string());
    }
    // Resolve symlinks / .. and ensure the path didn't escape
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("Cannot resolve project path: {}", e))?;
    if canonical != path {
        // Allow symlinks that resolve to a valid directory, but block obvious traversal
        if !canonical.is_dir() {
            return Err("Project path is not a directory".to_string());
        }
    }
    Ok(())
}

/// Run a git command and return the raw Output.
/// The generic S allows accepting both `&[&str]` and `&[String]`.
fn run_git<S: AsRef<std::ffi::OsStr>>(
    project_path: &str,
    args: &[S],
) -> Result<std::process::Output, String> {
    validate_project_path(project_path)?;

    let mut cmd = std::process::Command::new("git");
    crate::subprocess::configure_background_command(&mut cmd);
    cmd.args(args)
        .current_dir(project_path)
        .output()
        .map_err(|e| e.to_string())
}

async fn read_pipe_to_end<R: AsyncRead + Unpin>(
    mut pipe: R,
    stream_name: &str,
) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    pipe.read_to_end(&mut data)
        .await
        .map_err(|e| format!("Failed to read git {}: {}", stream_name, e))?;
    Ok(data)
}

/// Run a git command with a timeout.
/// On timeout it kills the underlying git child process, avoiding a buildup of background processes
/// and blocked threads.
async fn run_git_with_timeout(
    project_path: String,
    args: Vec<String>,
    timeout: Duration,
) -> Result<Output, String> {
    validate_project_path(&project_path)?;

    let mut cmd = tokio::process::Command::new("git");
    crate::subprocess::configure_background_tokio_command(&mut cmd);
    let mut child = cmd
        .args(&args)
        .current_dir(&project_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture git stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture git stderr".to_string())?;

    let stdout_task = tokio::spawn(read_pipe_to_end(stdout, "stdout"));
    let stderr_task = tokio::spawn(read_pipe_to_end(stderr, "stderr"));

    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(result) => result.map_err(|e| e.to_string())?,
        Err(_) => {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
            stdout_task.abort();
            stderr_task.abort();
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            return Err(format!("Git command timed out ({}s)", timeout.as_secs()));
        }
    };

    let stdout = stdout_task
        .await
        .map_err(|e| format!("Git stdout task failed: {}", e))??;
    let stderr = stderr_task
        .await
        .map_err(|e| format!("Git stderr task failed: {}", e))??;

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Run a git command; if the exit code is non-zero, return stderr as the error.
fn run_git_check<S: AsRef<std::ffi::OsStr>>(project_path: &str, args: &[S]) -> Result<(), String> {
    let output = run_git(project_path, args)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(())
}

fn git_command_error(output: &Output, fallback: &str) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let message = format!("{}{}", stderr, stdout).trim().to_string();
    if message.is_empty() {
        fallback.to_string()
    } else {
        message
    }
}

/// Derive a default directory name from a git remote URL: take the last path segment and strip the
/// `.git` suffix. For example `https://github.com/owner/repo.git` -> `repo`.
fn repo_name_from_url(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let last = trimmed
        .rsplit(|c| c == '/' || c == ':')
        .next()
        .unwrap_or("");
    let name = last.strip_suffix(".git").unwrap_or(last);
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// `git clone <url>` into `parent_dir`, returning the absolute path of the cloned project.
/// `name` is optional, for customizing the target directory name; when not provided it's derived
/// from the URL.
#[tauri::command]
pub async fn git_clone(
    url: String,
    parent_dir: String,
    name: Option<String>,
) -> Result<String, String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("Repository URL must not be empty".to_string());
    }
    // Basic protocol check: only allow common git remote forms, to avoid passing arbitrary strings into the shell.
    let looks_like_remote = url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with("git://")
        || url.starts_with("ssh://")
        || url.starts_with("git@");
    if !looks_like_remote {
        return Err("Unsupported repository URL (use https://, ssh:// or git@…)".to_string());
    }

    let dir_name = name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .or_else(|| repo_name_from_url(&url))
        .ok_or_else(|| "Cannot derive a target folder name from the URL".to_string())?;
    // Prevent directory-name injection (path separators / parent-dir traversal).
    if dir_name.contains('/') || dir_name.contains('\\') || dir_name == ".." || dir_name == "." {
        return Err("Invalid target folder name".to_string());
    }

    let parent = Path::new(&parent_dir);
    if !parent.is_absolute() || !parent.exists() {
        return Err("Destination folder does not exist".to_string());
    }
    let target = parent.join(&dir_name);
    if target.exists() {
        return Err(format!("A folder named \"{}\" already exists here", dir_name));
    }

    // Cloning can be slow (large repos / slow networks), so allow a 10-minute timeout.
    let output = run_git_with_timeout(
        parent_dir.clone(),
        vec!["clone".to_string(), url, dir_name.clone()],
        Duration::from_secs(600),
    )
    .await?;

    if !output.status.success() {
        return Err(git_command_error(&output, "git clone failed"));
    }

    Ok(target.to_string_lossy().into_owned())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AheadBehind {
    pub ahead: u32,
    pub behind: u32,
}

/// Compute how many commits the worktree branch is ahead/behind relative to the base branch
/// (↑ahead / ↓behind). Runs `git rev-list --left-right --count base...HEAD` inside `worktree_path`.
#[tauri::command]
pub fn branch_ahead_behind(
    worktree_path: String,
    base_branch: String,
) -> Result<AheadBehind, String> {
    let base = base_branch.trim();
    if base.is_empty() {
        return Ok(AheadBehind { ahead: 0, behind: 0 });
    }
    let range = format!("{}...HEAD", base);
    let output = run_git(&worktree_path, &["rev-list", "--left-right", "--count", &range])?;
    if !output.status.success() {
        // When base is unreachable (e.g. the branch was deleted), don't treat it as an error; return 0.
        return Ok(AheadBehind { ahead: 0, behind: 0 });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // The output looks like "<behind>\t<ahead>" (left = base-only = behind, right = HEAD-only = ahead).
    let mut parts = text.split_whitespace();
    let behind = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let ahead = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    Ok(AheadBehind { ahead, behind })
}

fn validate_git_relative_path(relative_path: &str) -> Result<(), String> {
    if relative_path.is_empty() {
        return Err("File path must not be empty".to_string());
    }

    let path = Path::new(relative_path);
    if path.is_absolute() {
        return Err("File path must be relative".to_string());
    }

    for component in path.components() {
        match component {
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err("File path must stay inside the git worktree".to_string());
            }
            _ => {}
        }
    }

    Ok(())
}

fn unique_git_file_paths(file_paths: Vec<String>) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let mut paths = Vec::new();

    for file_path in file_paths {
        validate_git_relative_path(&file_path)?;
        if seen.insert(file_path.clone()) {
            paths.push(file_path);
        }
    }

    Ok(paths)
}

fn git_path_args(base_args: &[&str], file_paths: Vec<String>) -> Result<Vec<String>, String> {
    let paths = unique_git_file_paths(file_paths)?;
    if paths.is_empty() {
        return Ok(Vec::new());
    }

    let mut args: Vec<String> = base_args.iter().map(|arg| (*arg).to_string()).collect();
    args.push("--".to_string());
    args.extend(paths);
    Ok(args)
}

fn git_worktree_root(project_path: &str) -> Result<PathBuf, String> {
    let output = run_git(project_path, &["rev-parse", "--show-toplevel"])?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        return Err("Cannot resolve git worktree root".to_string());
    }

    let root = PathBuf::from(root)
        .canonicalize()
        .map_err(|e| format!("Cannot resolve git worktree root: {}", e))?;
    let project = Path::new(project_path)
        .canonicalize()
        .map_err(|e| format!("Cannot resolve project path: {}", e))?;

    if !project.starts_with(&root) {
        return Err("Git worktree root does not contain project path".to_string());
    }

    Ok(root)
}

fn path_to_string(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(|path| path.to_string())
        .ok_or_else(|| "Path contains invalid UTF-8".to_string())
}

fn git_has_head(worktree_root: &str) -> Result<bool, String> {
    let output = run_git(worktree_root, &["rev-parse", "--verify", "HEAD"])?;
    Ok(output.status.success())
}

const PROTECTED_FIRST_SEGMENTS: &[&str] = &[".git", ".fastaf"];

fn is_protected_project_relative_path(relative_path: &str) -> bool {
    Path::new(relative_path)
        .components()
        .find_map(|component| match component {
            std::path::Component::Normal(name) => name.to_str().map(|name| {
                PROTECTED_FIRST_SEGMENTS
                    .iter()
                    .any(|protected| name.eq_ignore_ascii_case(protected))
            }),
            _ => None,
        })
        .unwrap_or(false)
}

fn apply_login_shell_env(cmd: &mut Command) {
    for (key, value) in crate::app_settings::get_login_shell_env() {
        cmd.env(key, value);
    }
}

fn run_agent_commit_message_command(
    agent: &str,
    project_path: &str,
    prompt: &str,
) -> Result<Output, String> {
    let launch = crate::app_settings::get_agent_launch_spec(agent);
    let mut cmd = Command::new(&launch.program);
    crate::subprocess::configure_background_command(&mut cmd);
    if agent == "codex" {
        cmd.args(["exec", prompt]);
    } else {
        cmd.args(["-p", prompt, "--output-format", "text"]);
    }
    cmd.current_dir(project_path);
    cmd.stdin(Stdio::null());
    apply_login_shell_env(&mut cmd);
    for (key, value) in &launch.extra_env {
        cmd.env(key, value);
    }
    cmd.output()
        .map_err(|e| format!("Failed to run {agent}: {e}"))
}

fn create_empty_temp_file() -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!("fastaf-empty-{}.tmp", uuid::Uuid::new_v4()));
    std::fs::File::create(&path)
        .map_err(|e| format!("Failed to create temporary file for git diff: {e}"))?;
    Ok(path)
}

// ── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn generate_commit_message(project_path: String) -> Result<String, String> {
    // 1. Get staged diff
    let diff_output = run_git(&project_path, &["diff", "--staged"])?;
    let diff = String::from_utf8_lossy(&diff_output.stdout).into_owned();
    if diff.trim().is_empty() {
        return Err("No staged changes to generate a commit message for.".to_string());
    }

    // Truncate diff if too large to avoid CLI arg limits
    let diff = if diff.len() > 50_000 {
        format!("{}...(diff truncated)", &diff[..50_000])
    } else {
        diff
    };

    // 2. Read project config for prompt and default agent
    let config = crate::config::read_project_config(project_path.clone())?;
    let commit_prompt = config.git.commit_prompt;
    let timeout_secs = config.git.commit_message_timeout_secs.clamp(1, 120);
    let agent = config.agent.default;

    // 3. Build full prompt
    let full_prompt = format!(
        "{}\n\nGit diff:\n```diff\n{}\n```\n\nOutput only the commit message, nothing else.",
        commit_prompt, diff
    );

    // 4. Run agent in non-interactive exec mode with configurable timeout
    let output = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        tokio::task::spawn_blocking(move || {
            run_agent_commit_message_command(&agent, &project_path, &full_prompt)
        }),
    )
    .await
    .map_err(|_| format!("Generating commit message timed out ({}s)", timeout_secs))?
    .map_err(|e| format!("Commit message generation thread error: {}", e))??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!("Agent failed: {}{}", stderr, stdout));
    }

    let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if result.is_empty() {
        return Err("Agent returned empty response.".to_string());
    }
    Ok(result)
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct GitFileChange {
    path: String,
    status: String,
    staged: bool,
}

fn parse_porcelain_z_status(stdout: &[u8]) -> Vec<GitFileChange> {
    let mut changes = Vec::new();
    let mut entries = stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty());

    while let Some(entry) = entries.next() {
        if entry.len() < 4 || entry[2] != b' ' {
            continue;
        }

        let x = entry[0] as char;
        let y = entry[1] as char;
        let display_path = String::from_utf8_lossy(&entry[3..]).into_owned();

        if x == 'R' || x == 'C' {
            let _ = entries.next();
        }

        if x == '?' && y == '?' {
            changes.push(GitFileChange {
                path: display_path,
                status: "?".to_string(),
                staged: false,
            });
        } else {
            if x != ' ' && x != '?' {
                changes.push(GitFileChange {
                    path: display_path.clone(),
                    status: x.to_string(),
                    staged: true,
                });
            }
            if y != ' ' && y != '?' {
                changes.push(GitFileChange {
                    path: display_path,
                    status: y.to_string(),
                    staged: false,
                });
            }
        }
    }

    changes
}

#[tauri::command]
pub async fn git_status(project_path: String) -> Result<Vec<GitFileChange>, String> {
    let args = vec![
        "-c".to_string(),
        "core.quotePath=false".to_string(),
        "status".to_string(),
        "--porcelain=v1".to_string(),
        "-z".to_string(),
        "--untracked-files=all".to_string(),
    ];

    let output = run_git_with_timeout(project_path, args, Duration::from_secs(5)).await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let message = format!("{}{}", stderr, stdout).trim().to_string();

        return Err(if message.is_empty() {
            "Failed to get git status".to_string()
        } else {
            message
        });
    }

    Ok(parse_porcelain_z_status(&output.stdout))
}

#[derive(serde::Serialize, Clone)]
pub(crate) struct GitCommit {
    hash: String,
    short_hash: String,
    author: String,
    date: String,
    message: String,
    refs: Vec<String>,
}

#[derive(serde::Serialize)]
pub(crate) struct GitBranchInfo {
    name: String,
    current: bool,
    remote: Option<String>,
}

#[tauri::command]
pub async fn git_list_branches(project_path: String) -> Result<Vec<GitBranchInfo>, String> {
    let output = run_git_with_timeout(
        project_path,
        vec!["branch".to_string(), "-a".to_string()],
        Duration::from_secs(5),
    )
    .await?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut branches = Vec::new();
    for line in stdout.lines() {
        if line.len() < 2 {
            continue;
        }
        let current = line.starts_with("* ");
        let raw = line[2..].trim();
        // Skip HEAD pointer lines like "remotes/origin/HEAD -> origin/main"
        if raw.contains(" -> ") {
            continue;
        }
        if let Some(without_remotes) = raw.strip_prefix("remotes/") {
            // "origin/main" -> remote = "origin", name = "origin/main"
            let name = without_remotes.to_string();
            let remote = name.split('/').next().map(|s| s.to_string());
            branches.push(GitBranchInfo {
                name,
                current,
                remote,
            });
        } else if !raw.is_empty() {
            branches.push(GitBranchInfo {
                name: raw.to_string(),
                current,
                remote: None,
            });
        }
    }
    Ok(branches)
}

#[tauri::command]
pub async fn git_checkout_branch(
    project_path: String,
    branch_name: String,
    is_remote: bool,
) -> Result<(), String> {
    let args: Vec<String> = if is_remote {
        // "origin/main" -> local name "main", track remote
        let local_name = branch_name
            .split_once('/')
            .map(|(_, n)| n.to_string())
            .unwrap_or_else(|| branch_name.clone());
        vec![
            "checkout".into(),
            "-b".into(),
            local_name,
            "--track".into(),
            format!("remotes/{}", branch_name),
        ]
    } else {
        vec!["checkout".into(), branch_name.clone()]
    };
    run_git_check(&project_path, &args)
}

#[tauri::command]
pub async fn git_create_branch(
    project_path: String,
    branch_name: String,
    from_branch: String,
    checkout: bool,
) -> Result<(), String> {
    let args: &[&str] = if checkout {
        &["checkout", "-b", &branch_name, &from_branch]
    } else {
        &["branch", &branch_name, &from_branch]
    };
    run_git_check(&project_path, args)
}

#[tauri::command]
pub async fn git_log(
    project_path: String,
    limit: u32,
    search: Option<String>,
    branch: Option<String>,
) -> Result<Vec<GitCommit>, String> {
    let limit_str = limit.to_string();
    let format = "COMMIT:%H%nSHORT:%h%nAUTHOR:%an%nDATE:%ar%nSUBJECT:%s%nREFS:%D%nEND_RECORD";
    let mut args: Vec<String> = vec![
        "log".into(),
        format!("--format={}", format),
        "-n".into(),
        limit_str,
    ];
    if let Some(ref s) = search {
        if !s.is_empty() {
            args.push(format!("--grep={}", s));
        }
    }
    if let Some(ref b) = branch {
        if !b.is_empty() {
            args.push(b.clone());
        }
    }

    let output = run_git_with_timeout(project_path, args, Duration::from_secs(10)).await?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut commits = Vec::new();
    let mut hash = String::new();
    let mut short_hash = String::new();
    let mut author = String::new();
    let mut date = String::new();
    let mut message = String::new();
    let mut refs: Vec<String> = Vec::new();

    for line in stdout.lines() {
        if let Some(v) = line.strip_prefix("COMMIT:") {
            hash = v.to_string();
        } else if let Some(v) = line.strip_prefix("SHORT:") {
            short_hash = v.to_string();
        } else if let Some(v) = line.strip_prefix("AUTHOR:") {
            author = v.to_string();
        } else if let Some(v) = line.strip_prefix("DATE:") {
            date = v.to_string();
        } else if let Some(v) = line.strip_prefix("SUBJECT:") {
            message = v.to_string();
        } else if let Some(v) = line.strip_prefix("REFS:") {
            refs = v
                .split(", ")
                .filter(|s| !s.is_empty())
                .map(|s| s.trim().to_string())
                .collect();
        } else if line == "END_RECORD" && !hash.is_empty() {
            commits.push(GitCommit {
                hash: hash.clone(),
                short_hash: short_hash.clone(),
                author: author.clone(),
                date: date.clone(),
                message: message.clone(),
                refs: refs.clone(),
            });
            hash.clear();
            short_hash.clear();
            author.clear();
            date.clear();
            message.clear();
            refs.clear();
        }
    }
    Ok(commits)
}

#[derive(serde::Serialize)]
pub(crate) struct GitCommitFile {
    path: String,
    status: String,
    additions: i32,
    deletions: i32,
}

#[derive(serde::Serialize)]
pub(crate) struct GitCommitDetail {
    hash: String,
    short_hash: String,
    author: String,
    date: String,
    message: String,
    files: Vec<GitCommitFile>,
    total_additions: i32,
    total_deletions: i32,
}

#[tauri::command]
pub async fn git_commit_detail(
    project_path: String,
    commit_hash: String,
) -> Result<GitCommitDetail, String> {
    let info_out = run_git(
        &project_path,
        &[
            "show",
            "--no-patch",
            "--format=HASH:%H%nSHORT:%h%nAUTHOR:%an%nDATE:%ar%nSUBJECT:%s",
            &commit_hash,
        ],
    )?;

    let info_str = String::from_utf8_lossy(&info_out.stdout).into_owned();
    let mut hash = String::new();
    let mut short_hash = String::new();
    let mut author = String::new();
    let mut date = String::new();
    let mut message = String::new();
    for line in info_str.lines() {
        if let Some(v) = line.strip_prefix("HASH:") {
            hash = v.to_string();
        } else if let Some(v) = line.strip_prefix("SHORT:") {
            short_hash = v.to_string();
        } else if let Some(v) = line.strip_prefix("AUTHOR:") {
            author = v.to_string();
        } else if let Some(v) = line.strip_prefix("DATE:") {
            date = v.to_string();
        } else if let Some(v) = line.strip_prefix("SUBJECT:") {
            message = v.to_string();
        }
    }

    let ns_out = run_git(
        &project_path,
        &[
            "diff-tree",
            "--no-commit-id",
            "-r",
            "--name-status",
            &commit_hash,
        ],
    )?;

    let mut file_statuses: HashMap<String, String> = HashMap::new();
    for line in String::from_utf8_lossy(&ns_out.stdout).lines() {
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        match parts.as_slice() {
            [st, path] => {
                file_statuses.insert(
                    path.to_string(),
                    if st.starts_with('R') {
                        "R".to_string()
                    } else {
                        st.to_string()
                    },
                );
            }
            [st, _old, new_path] => {
                file_statuses.insert(
                    new_path.to_string(),
                    if st.starts_with('R') {
                        "R".to_string()
                    } else {
                        st.to_string()
                    },
                );
            }
            _ => {}
        }
    }

    let num_out = run_git(
        &project_path,
        &[
            "diff-tree",
            "--no-commit-id",
            "-r",
            "--numstat",
            &commit_hash,
        ],
    )?;

    let mut files = Vec::new();
    let mut total_additions = 0i32;
    let mut total_deletions = 0i32;

    for line in String::from_utf8_lossy(&num_out.stdout).lines() {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() == 3 {
            let additions: i32 = parts[0].parse().unwrap_or(0);
            let deletions: i32 = parts[1].parse().unwrap_or(0);
            let path = parts[2].to_string();
            total_additions += additions;
            total_deletions += deletions;
            let status = file_statuses
                .get(&path)
                .cloned()
                .unwrap_or_else(|| "M".to_string());
            files.push(GitCommitFile {
                path,
                status,
                additions,
                deletions,
            });
        }
    }

    Ok(GitCommitDetail {
        hash,
        short_hash,
        author,
        date,
        message,
        files,
        total_additions,
        total_deletions,
    })
}

#[tauri::command]
pub async fn git_show_diff(project_path: String, commit_hash: String) -> Result<String, String> {
    let args = vec!["show".to_string(), "--format=".to_string(), commit_hash];
    let output = run_git_with_timeout(project_path, args, Duration::from_secs(10)).await?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let raw = output.stdout;
    let limit = 500 * 1024;
    Ok(String::from_utf8_lossy(if raw.len() > limit {
        &raw[..limit]
    } else {
        &raw
    })
    .into_owned())
}

#[tauri::command]
pub async fn git_file_diff(
    project_path: String,
    file_path: String,
    staged: bool,
) -> Result<String, String> {
    let mut args = vec!["diff".to_string()];
    if staged {
        args.push("--cached".to_string());
    }
    args.push("--".to_string());
    args.push(file_path.clone());

    let output = run_git_with_timeout(project_path.clone(), args, Duration::from_secs(10)).await?;
    let raw = output.stdout;

    // For untracked files, git diff returns nothing — fall back to --no-index diff
    if raw.is_empty() && !staged {
        let abs_path = std::path::Path::new(&project_path).join(&file_path);
        let abs_path_str = abs_path.to_string_lossy().into_owned();
        let empty_file = create_empty_temp_file()?;
        let fallback_args = vec![
            "diff".to_string(),
            "--no-index".to_string(),
            empty_file.to_string_lossy().into_owned(),
            abs_path_str,
        ];
        let fallback =
            run_git_with_timeout(project_path, fallback_args, Duration::from_secs(10)).await;
        let _ = std::fs::remove_file(&empty_file);
        let fallback = fallback?;
        let fallback_raw = fallback.stdout;
        let limit = 200 * 1024;
        return Ok(String::from_utf8_lossy(if fallback_raw.len() > limit {
            &fallback_raw[..limit]
        } else {
            &fallback_raw
        })
        .into_owned());
    }

    let limit = 200 * 1024;
    Ok(String::from_utf8_lossy(if raw.len() > limit {
        &raw[..limit]
    } else {
        &raw
    })
    .into_owned())
}

#[tauri::command]
pub async fn git_stage(project_path: String, file_path: String) -> Result<(), String> {
    run_git_check(&project_path, &["add", "--", &file_path])
}

#[tauri::command]
pub async fn git_unstage(project_path: String, file_path: String) -> Result<(), String> {
    if git_has_head(&project_path)? {
        run_git_check(&project_path, &["restore", "--staged", "--", &file_path])
    } else {
        // Before the first commit there is no HEAD, so use `git reset` to unstage instead.
        run_git_check(&project_path, &["reset", "--", &file_path])
    }
}

#[tauri::command]
pub async fn git_stage_files(project_path: String, file_paths: Vec<String>) -> Result<(), String> {
    let args = git_path_args(&["add"], file_paths)?;
    if args.is_empty() {
        return Ok(());
    }

    let output = run_git_with_timeout(project_path, args, Duration::from_secs(10)).await?;
    if !output.status.success() {
        return Err(git_command_error(&output, "Failed to stage files"));
    }
    Ok(())
}

#[tauri::command]
pub async fn git_unstage_files(
    project_path: String,
    file_paths: Vec<String>,
) -> Result<(), String> {
    // Before the first commit there is no HEAD, so `git restore --staged` would fail; fall back to
    // `git reset`, which doesn't depend on HEAD.
    // Detect this with the async run_git_with_timeout (not the synchronous git_has_head) to avoid
    // blocking the Tokio runtime.
    let head_check = run_git_with_timeout(
        project_path.clone(),
        vec![
            "rev-parse".to_string(),
            "--verify".to_string(),
            "HEAD".to_string(),
        ],
        Duration::from_secs(5),
    )
    .await?;
    let base: &[&str] = if head_check.status.success() {
        &["restore", "--staged"]
    } else {
        &["reset"]
    };

    let args = git_path_args(base, file_paths)?;
    if args.is_empty() {
        return Ok(());
    }

    let output = run_git_with_timeout(project_path, args, Duration::from_secs(10)).await?;
    if !output.status.success() {
        return Err(git_command_error(&output, "Failed to unstage files"));
    }
    Ok(())
}

#[tauri::command]
pub async fn git_stage_all(project_path: String) -> Result<(), String> {
    run_git_check(&project_path, &["add", "-A"])
}

#[tauri::command]
pub async fn git_unstage_all(project_path: String) -> Result<(), String> {
    run_git_check(&project_path, &["restore", "--staged", "."])
}

#[tauri::command]
pub async fn git_commit(project_path: String, message: String) -> Result<(), String> {
    run_git_check(&project_path, &["commit", "-m", &message])
}

fn untracked_files_under_directory<'a>(
    directory_path: &str,
    untracked_files: &'a [String],
) -> Vec<&'a str> {
    let directory = Path::new(directory_path);
    untracked_files
        .iter()
        .map(String::as_str)
        .filter(|path| {
            let path = Path::new(path);
            path != directory && path.starts_with(directory)
        })
        .collect()
}

fn is_listed_untracked_file(relative_path: &str, untracked_files: &[String]) -> bool {
    let relative_path = Path::new(relative_path);
    untracked_files
        .iter()
        .any(|path| Path::new(path) == relative_path)
}

fn is_protected_worktree_relative_path(
    worktree_root: &Path,
    project_path: &str,
    relative_path: &str,
) -> bool {
    if is_protected_project_relative_path(relative_path) {
        return true;
    }

    let rel = Path::new(relative_path);
    if rel.is_absolute() {
        return false;
    }

    let canonical_project = match Path::new(project_path).canonicalize() {
        Ok(path) => path,
        Err(_) => return false,
    };
    let target = worktree_root.join(rel);
    let Some(file_name) = target.file_name() else {
        return false;
    };
    let Some(parent) = target.parent() else {
        return false;
    };
    let Ok(canonical_parent) = parent.canonicalize() else {
        return false;
    };
    let resolved = canonical_parent.join(file_name);

    resolved
        .strip_prefix(&canonical_project)
        .ok()
        .map(|rel_from_project| {
            is_protected_project_relative_path(&rel_from_project.to_string_lossy())
        })
        .unwrap_or(false)
}

/// Move a worktree-relative path to the system trash. Canonicalize only the parent directory so
/// symlinks at the leaf are deleted as themselves rather than followed to their target. Reject
/// absolute or `..`-escaping relative paths defensively even though `git status` should never emit them.
fn trash_worktree_relative_path(
    worktree_root: &Path,
    project_path: &str,
    relative_path: &str,
) -> Result<(), String> {
    let rel = Path::new(relative_path);
    if rel.is_absolute() {
        return Err("Untracked path must be relative".to_string());
    }

    let target = worktree_root.join(rel);
    let file_name = target
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "Invalid file name".to_string())?
        .to_string();
    let parent = target
        .parent()
        .ok_or_else(|| "Cannot resolve parent directory".to_string())?;

    let canonical_parent = parent
        .canonicalize()
        .map_err(|e| format!("Cannot resolve parent directory: {}", e))?;
    let canonical_root = worktree_root
        .canonicalize()
        .map_err(|e| format!("Cannot resolve git worktree root: {}", e))?;
    let canonical_project = Path::new(project_path)
        .canonicalize()
        .map_err(|e| format!("Cannot resolve project path: {}", e))?;

    if !canonical_parent.starts_with(&canonical_root) {
        return Err("Path is outside the git worktree".to_string());
    }

    let resolved = canonical_parent.join(&file_name);
    if resolved == canonical_root {
        return Err("Refusing to delete project root".to_string());
    }
    if resolved.symlink_metadata().is_err() {
        return Err("Path does not exist".to_string());
    }
    if is_protected_project_relative_path(relative_path) {
        return Err("Refusing to delete protected project metadata".to_string());
    }
    if let Ok(rel_from_project) = resolved.strip_prefix(&canonical_project) {
        let rel_from_project = rel_from_project.to_string_lossy();
        if is_protected_project_relative_path(&rel_from_project) {
            return Err("Refusing to delete protected project metadata".to_string());
        }
    }

    trash::delete(&resolved).map_err(|e| e.to_string())
}

fn discard_untracked_path(
    project_path: &str,
    worktree_root: &Path,
    relative_path: &str,
    untracked_files: &[String],
) -> Result<(), String> {
    let rel = Path::new(relative_path);
    if rel.is_absolute() {
        return Err("Untracked path must be relative".to_string());
    }
    if is_protected_worktree_relative_path(worktree_root, project_path, relative_path) {
        return Err("Refusing to delete protected project metadata".to_string());
    }

    let target = worktree_root.join(rel);
    let metadata = target
        .symlink_metadata()
        .map_err(|_| "Path does not exist".to_string())?;

    if metadata.file_type().is_dir() {
        for rel in untracked_files_under_directory(relative_path, untracked_files) {
            if is_protected_worktree_relative_path(worktree_root, project_path, rel) {
                continue;
            }
            trash_worktree_relative_path(worktree_root, project_path, rel)?;
        }
        return Ok(());
    }

    if !is_listed_untracked_file(relative_path, untracked_files) {
        return Err("Path is not an untracked file".to_string());
    }

    trash_worktree_relative_path(worktree_root, project_path, relative_path)
}

fn discard_untracked_file(
    project_path: &str,
    worktree_root: &Path,
    relative_path: &str,
) -> Result<(), String> {
    let worktree_root = worktree_root
        .canonicalize()
        .map_err(|e| format!("Cannot resolve git worktree root: {}", e))?;
    let worktree_root_string = path_to_string(&worktree_root)?;
    let untracked_files = list_untracked_files(&worktree_root_string)?;

    discard_untracked_path(
        project_path,
        &worktree_root,
        relative_path,
        &untracked_files,
    )
}

fn list_untracked_files(project_path: &str) -> Result<Vec<String>, String> {
    let output = run_git(
        project_path,
        &[
            "-c",
            "core.quotePath=false",
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(output
        .stdout
        .split(|b| *b == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| String::from_utf8_lossy(entry).into_owned())
        .collect())
}

/// Discard a single file's pending changes.
///
/// - Untracked files: moved to the system trash.
/// - Tracked unstaged changes: `git restore -- <file>` resets the worktree to the index, leaving
///   any staged half intact (so MM files don't lose their staged portion).
///
/// We deliberately don't expose a "discard staged" path here — staged-only files have no per-row
/// discard button in the UI (matching VSCode), and "Discard All" handles the staged side via
/// `git_discard_all` which correctly undoes renames too.
#[tauri::command]
pub async fn git_discard_file(
    project_path: String,
    file_path: String,
    untracked: bool,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        validate_project_path(&project_path)?;
        let worktree_root = git_worktree_root(&project_path)?;
        let worktree_root_string = path_to_string(&worktree_root)?;
        if untracked {
            discard_untracked_file(&project_path, &worktree_root, &file_path)
        } else {
            run_git_check(&worktree_root_string, &["restore", "--", &file_path])
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn git_discard_files(
    project_path: String,
    file_paths: Vec<String>,
    untracked: bool,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        validate_project_path(&project_path)?;
        let file_paths = unique_git_file_paths(file_paths)?;
        if file_paths.is_empty() {
            return Ok(());
        }

        let worktree_root = git_worktree_root(&project_path)?;
        let worktree_root_string = path_to_string(&worktree_root)?;
        if untracked {
            let untracked_files = list_untracked_files(&worktree_root_string)?;
            for file_path in file_paths {
                discard_untracked_path(
                    &project_path,
                    &worktree_root,
                    &file_path,
                    &untracked_files,
                )?;
            }
            return Ok(());
        }

        let mut args = vec!["restore".to_string(), "--".to_string()];
        args.extend(file_paths);
        run_git_check(&worktree_root_string, &args)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn git_discard_all(project_path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        validate_project_path(&project_path)?;
        let worktree_root = git_worktree_root(&project_path)?;
        let worktree_root_string = path_to_string(&worktree_root)?;
        // Reset every tracked file (staged + worktree) back to HEAD.
        // Staged-only adds become untracked after this; they are cleaned in the second pass.
        if git_has_head(&worktree_root_string)? {
            run_git_check(
                &worktree_root_string,
                &["restore", "--source=HEAD", "--staged", "--worktree", "."],
            )?;
        } else {
            run_git_check(
                &worktree_root_string,
                &["rm", "-r", "--cached", "--ignore-unmatch", "--", "."],
            )?;
        }

        for rel in list_untracked_files(&worktree_root_string)? {
            if is_protected_worktree_relative_path(&worktree_root, &project_path, &rel) {
                continue;
            }
            trash_worktree_relative_path(&worktree_root, &project_path, &rel)?;
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn git_show_file_diff(
    project_path: String,
    commit_hash: String,
    file_path: String,
) -> Result<String, String> {
    let output = run_git(
        &project_path,
        &["show", "--format=", &commit_hash, "--", &file_path],
    )?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let raw = output.stdout;
    let limit = 500 * 1024;
    Ok(String::from_utf8_lossy(if raw.len() > limit {
        &raw[..limit]
    } else {
        &raw
    })
    .into_owned())
}

#[tauri::command]
pub async fn git_push(project_path: String, branch: Option<String>) -> Result<String, String> {
    let mut args = vec!["push".to_string()];
    if let Some(ref b) = branch.filter(|s| !s.is_empty()) {
        args.push("origin".to_string());
        args.push(b.clone());
    }
    let output = run_git(&project_path, &args)?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Err(combined);
    }
    Ok(combined.trim().to_string())
}

#[tauri::command]
pub async fn git_pull(project_path: String) -> Result<String, String> {
    let output = run_git(&project_path, &["pull"])?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Err(combined);
    }
    Ok(combined.trim().to_string())
}

#[derive(serde::Serialize)]
pub(crate) struct GitRemoteCounts {
    ahead: i32,
    behind: i32,
    branch: String,
}

#[tauri::command]
pub async fn git_remote_counts(
    project_path: String,
    branch: Option<String>,
) -> Result<GitRemoteCounts, String> {
    let branch = if let Some(b) = branch.filter(|s| !s.is_empty()) {
        b
    } else {
        let branch_out = run_git(&project_path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        String::from_utf8_lossy(&branch_out.stdout)
            .trim()
            .to_string()
    };

    let rev_str = format!("{}...@{{u}}", branch);
    let rev_out = run_git(
        &project_path,
        &["rev-list", "--count", "--left-right", &rev_str],
    );

    let (ahead, behind) = match rev_out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let parts: Vec<&str> = s.split_whitespace().collect();
            if parts.len() == 2 {
                (parts[0].parse().unwrap_or(0), parts[1].parse().unwrap_or(0))
            } else {
                (0, 0)
            }
        }
        _ => (0, 0),
    };

    Ok(GitRemoteCounts {
        ahead,
        behind,
        branch,
    })
}

// ── Task worktree management ─────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub(crate) struct WorktreeCreated {
    #[serde(rename = "worktreePath")]
    worktree_path: String,
    #[serde(rename = "worktreeBranch")]
    worktree_branch: String,
    #[serde(rename = "baseBranch")]
    base_branch: String,
}

fn task_worktree_branch_name(task_id: &str) -> String {
    let short = if task_id.len() > 6 {
        &task_id[task_id.len() - 6..]
    } else {
        task_id
    };
    format!("fastaf/task-{}", short)
}

/// Validate that the worktree path must lie under `<project>/.fastaf/worktrees/`,
/// preventing remove_task_worktree from being passed an arbitrary path.
fn ensure_path_under_worktrees_root(project_path: &str, worktree_path: &str) -> Result<(), String> {
    let project = Path::new(project_path)
        .canonicalize()
        .map_err(|e| format!("Cannot resolve project path: {}", e))?;
    let expected_root = project.join(".fastaf").join("worktrees");
    let target = Path::new(worktree_path)
        .canonicalize()
        .map_err(|e| format!("Cannot resolve worktree path: {}", e))?;
    if !target.starts_with(&expected_root) {
        return Err("Worktree path is outside .fastaf/worktrees".to_string());
    }
    Ok(())
}

#[tauri::command]
pub async fn create_task_worktree(
    project_path: String,
    task_id: String,
    base_branch: String,
) -> Result<WorktreeCreated, String> {
    validate_project_path(&project_path)?;
    if task_id.trim().is_empty() {
        return Err("Task id is required".to_string());
    }
    if base_branch.trim().is_empty() {
        return Err("Base branch is required".to_string());
    }

    tokio::task::spawn_blocking(move || -> Result<WorktreeCreated, String> {
        let worktrees_dir = Path::new(&project_path).join(".fastaf").join("worktrees");
        std::fs::create_dir_all(&worktrees_dir)
            .map_err(|e| format!("Failed to create worktrees dir: {}", e))?;

        let worktree_path = worktrees_dir.join(&task_id);
        if worktree_path.exists() {
            return Err(format!(
                "Worktree path already exists: {}",
                worktree_path.display()
            ));
        }

        let wt_path_str = path_to_string(&worktree_path)?;
        let branch = task_worktree_branch_name(&task_id);

        let output = run_git(
            &project_path,
            &["worktree", "add", &wt_path_str, "-b", &branch, &base_branch],
        )?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }

        Ok(WorktreeCreated {
            worktree_path: wt_path_str,
            worktree_branch: branch,
            base_branch,
        })
    })
    .await
    .map_err(|e| format!("Worktree task panicked: {}", e))?
}

#[tauri::command]
pub async fn merge_task_worktree(
    project_path: String,
    worktree_path: String,
    branch: String,
    base_branch: String,
) -> Result<String, String> {
    validate_project_path(&project_path)?;
    ensure_path_under_worktrees_root(&project_path, &worktree_path)?;
    if branch.trim().is_empty() || base_branch.trim().is_empty() {
        return Err("Branch and base branch are required".to_string());
    }

    tokio::task::spawn_blocking(move || -> Result<String, String> {
        // 0) If the worktree itself has uncommitted changes → refuse to merge, to avoid losing work
        let wt_status = run_git(&worktree_path, &["status", "--porcelain"])?;
        if !wt_status.status.success() {
            return Err(String::from_utf8_lossy(&wt_status.stderr)
                .trim()
                .to_string());
        }
        if !wt_status.stdout.is_empty() {
            return Err(
                "Worktree has uncommitted changes; commit or stash them before merging".into(),
            );
        }

        // Get the main repo's current HEAD: if HEAD == base, merge directly; otherwise use a fetch
        // fast-forward (without switching away from HEAD).
        let head_out = run_git(&project_path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        if !head_out.status.success() {
            return Err(String::from_utf8_lossy(&head_out.stderr).trim().to_string());
        }
        let original_branch = String::from_utf8_lossy(&head_out.stdout).trim().to_string();

        if original_branch == base_branch {
            // The main repo is on base, so merge directly (keep the merge commit so history stays traceable)
            let merge_out = run_git(&project_path, &["merge", "--no-ff", &branch])?;
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&merge_out.stdout),
                String::from_utf8_lossy(&merge_out.stderr)
            );
            if !merge_out.status.success() {
                return Err(format!(
                    "Merge failed (main repo on '{}'; please resolve manually): {}",
                    base_branch, combined
                ));
            }
            return Ok(combined.trim().to_string());
        }

        // The main repo isn't on base: use `git fetch . <src>:<dst>` to fast-forward the worktree
        // branch onto the base ref without touching the main repo's HEAD.
        // git fetch only allows fast-forward updates by default (a `+` prefix forces it), which
        // conveniently prevents accidentally overwriting commits on base.
        let refspec = format!("{}:{}", branch, base_branch);
        let ff_out = run_git(&project_path, &["fetch", ".", &refspec])?;
        if !ff_out.status.success() {
            let err = String::from_utf8_lossy(&ff_out.stderr);
            return Err(format!(
                "Cannot fast-forward '{}' (worktree may have diverged from base). \
                 Pull base into the worktree and retry, or merge manually. Detail: {}",
                base_branch,
                err.trim()
            ));
        }
        Ok(format!("Fast-forwarded '{}' to '{}'", base_branch, branch))
    })
    .await
    .map_err(|e| format!("Merge task panicked: {}", e))?
}

#[tauri::command]
pub async fn remove_task_worktree(
    project_path: String,
    worktree_path: String,
    branch: String,
) -> Result<(), String> {
    validate_project_path(&project_path)?;
    ensure_path_under_worktrees_root(&project_path, &worktree_path)?;

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        // worktree remove --force can remove a worktree with uncommitted changes and also cleans up metadata.
        let remove_out = run_git(
            &project_path,
            &["worktree", "remove", "--force", &worktree_path],
        )?;
        if !remove_out.status.success() {
            return Err(String::from_utf8_lossy(&remove_out.stderr)
                .trim()
                .to_string());
        }

        if !branch.trim().is_empty() {
            // -D allows deleting an unmerged branch (discard semantics). It also succeeds for merged branches.
            let branch_out = run_git(&project_path, &["branch", "-D", &branch])?;
            if !branch_out.status.success() {
                return Err(String::from_utf8_lossy(&branch_out.stderr)
                    .trim()
                    .to_string());
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("Remove worktree task panicked: {}", e))?
}

#[derive(serde::Serialize)]
pub(crate) struct WorktreeDiffStats {
    pub additions: i32,
    pub deletions: i32,
}

/// Compute the +/− line counts of the worktree (including uncommitted changes + untracked files)
/// relative to the merge-base of `base_branch` and HEAD.
/// Use the merge-base rather than base_branch itself, to avoid attributing changes committed by
/// others to this task after the main repo's base advances.
#[tauri::command]
pub async fn worktree_diff_stats(
    project_path: String,
    worktree_path: String,
    base_branch: String,
) -> Result<WorktreeDiffStats, String> {
    if base_branch.trim().is_empty() {
        return Err("Base branch is required".to_string());
    }

    tokio::task::spawn_blocking(move || -> Result<WorktreeDiffStats, String> {
        // Path validation includes synchronous canonicalize, so it must stay inside spawn_blocking to avoid blocking the Tokio runtime.
        validate_project_path(&project_path)?;
        ensure_path_under_worktrees_root(&project_path, &worktree_path)?;

        // 1) Tracked changes (both staged and unstaged): working tree vs merge-base
        let mb_out = run_git(&worktree_path, &["merge-base", &base_branch, "HEAD"])?;
        if !mb_out.status.success() {
            return Err(String::from_utf8_lossy(&mb_out.stderr).trim().to_string());
        }
        let merge_base = String::from_utf8_lossy(&mb_out.stdout).trim().to_string();

        let mut additions = 0i32;
        let mut deletions = 0i32;

        if !merge_base.is_empty() {
            let num_out = run_git(&worktree_path, &["diff", "--numstat", &merge_base])?;
            if !num_out.status.success() {
                return Err(String::from_utf8_lossy(&num_out.stderr).trim().to_string());
            }
            accumulate_numstat(&num_out.stdout, &mut additions, &mut deletions);
        }

        // 2) Untracked files: git diff won't list them, so compare each one against an empty file with --no-index
        let untracked = list_untracked_files(&worktree_path)?;
        if !untracked.is_empty() {
            let empty_file = create_empty_temp_file()?;
            let empty_path = empty_file.to_string_lossy().into_owned();
            for rel in &untracked {
                let abs = Path::new(&worktree_path).join(rel);
                let abs_str = abs.to_string_lossy().into_owned();
                // git diff --no-index returns exit code 1 when files differ, so status can't be used to judge success/failure
                let no_index = run_git(
                    &worktree_path,
                    &["diff", "--no-index", "--numstat", &empty_path, &abs_str],
                )?;
                accumulate_numstat(&no_index.stdout, &mut additions, &mut deletions);
            }
            let _ = std::fs::remove_file(&empty_file);
        }

        Ok(WorktreeDiffStats {
            additions,
            deletions,
        })
    })
    .await
    .map_err(|e| format!("Diff stats task panicked: {}", e))?
}

/// Parse `git diff --numstat` output and accumulate the +/− line counts.
/// numstat outputs `-\t-\t<path>` for binary files; when parsing fails, treat it as 0 and skip.
fn accumulate_numstat(stdout: &[u8], additions: &mut i32, deletions: &mut i32) {
    for line in String::from_utf8_lossy(stdout).lines() {
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() != 3 {
            continue;
        }
        *additions += parts[0].parse::<i32>().unwrap_or(0);
        *deletions += parts[1].parse::<i32>().unwrap_or(0);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        git_has_head, git_worktree_root, is_protected_project_relative_path, list_untracked_files,
        parse_porcelain_z_status, path_to_string, run_git_check, untracked_files_under_directory,
        GitFileChange,
    };
    use std::{fs, path::PathBuf, process::Command};

    struct TempRepo {
        path: PathBuf,
    }

    impl TempRepo {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("fastaf-git-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            let output = Command::new("git").arg("init").arg(&path).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            Self { path }
        }

        fn path_string(&self) -> String {
            path_to_string(&self.path.canonicalize().unwrap()).unwrap()
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn parses_untracked_path_with_spaces_without_quotes() {
        let changes = parse_porcelain_z_status(b"?? te st2.txt\0");

        assert_eq!(
            changes,
            vec![GitFileChange {
                path: "te st2.txt".to_string(),
                status: "?".to_string(),
                staged: false,
            }]
        );
    }

    #[test]
    fn parses_staged_and_unstaged_changes_for_same_path() {
        let changes = parse_porcelain_z_status(b"MM src/file name.ts\0");

        assert_eq!(
            changes,
            vec![
                GitFileChange {
                    path: "src/file name.ts".to_string(),
                    status: "M".to_string(),
                    staged: true,
                },
                GitFileChange {
                    path: "src/file name.ts".to_string(),
                    status: "M".to_string(),
                    staged: false,
                },
            ]
        );
    }

    #[test]
    fn parses_rename_destination_and_skips_source_path() {
        let changes = parse_porcelain_z_status(b"R  new name.txt\0old name.txt\0");

        assert_eq!(
            changes,
            vec![GitFileChange {
                path: "new name.txt".to_string(),
                status: "R".to_string(),
                staged: true,
            }]
        );
    }

    #[test]
    fn detects_protected_project_metadata_paths() {
        assert!(is_protected_project_relative_path(".fastaf/config.toml"));
        assert!(is_protected_project_relative_path("./.git/index"));
        assert!(is_protected_project_relative_path(
            ".FastAF/attachments/file.png"
        ));
        assert!(!is_protected_project_relative_path(
            "src/.fastaf/config.toml"
        ));
        assert!(!is_protected_project_relative_path(".gitignore"));
        assert!(!is_protected_project_relative_path("src/git.rs"));
    }

    #[test]
    fn lists_only_untracked_files_under_requested_directory() {
        let untracked_files = vec![
            "dir/file.txt".to_string(),
            "dir/nested/other.txt".to_string(),
            "dir2/file.txt".to_string(),
            "other.txt".to_string(),
        ];

        assert_eq!(
            untracked_files_under_directory("dir/", &untracked_files),
            vec!["dir/file.txt", "dir/nested/other.txt"]
        );
    }

    #[test]
    fn resolves_worktree_root_for_nested_project_paths() {
        let repo = TempRepo::new();
        let nested_project = repo.path.join("nested/project");
        fs::create_dir_all(&nested_project).unwrap();

        let resolved = git_worktree_root(nested_project.to_str().unwrap()).unwrap();

        assert_eq!(resolved, repo.path.canonicalize().unwrap());
    }

    #[test]
    fn unborn_repository_can_prepare_staged_files_for_untracked_cleanup() {
        let repo = TempRepo::new();
        let repo_path = repo.path_string();
        fs::write(repo.path.join("new-file.txt"), "content").unwrap();

        assert!(!git_has_head(&repo_path).unwrap());
        run_git_check(&repo_path, &["add", "new-file.txt"]).unwrap();
        run_git_check(
            &repo_path,
            &["rm", "-r", "--cached", "--ignore-unmatch", "--", "."],
        )
        .unwrap();

        assert_eq!(
            list_untracked_files(&repo_path).unwrap(),
            vec!["new-file.txt".to_string()]
        );
    }
}
