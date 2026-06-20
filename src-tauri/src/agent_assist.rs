use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};

const NAMING_PROMPT_TEMPLATE: &str = r#"You are a task title generator. Given the original task prompt below and (when available) the session execution summary, produce a single short title for this task.

Rules:
1. The output language MUST match the primary language of the task content. Chinese in -> Chinese out. English in -> English out. For mixed input, follow the dominant language.
2. Strictly 120 characters or fewer.
3. Start with a verb. Describe the core work that was actually performed or is being performed (e.g. "Fix login token expiration", "Refactor PTY read buffer", "Fix login page token expiration").
4. If the session execution summary is present and diverges from the original prompt, follow what was actually done — not what was originally asked.
5. No surrounding quotes, no trailing punctuation, no emoji, no Markdown, no prefixes such as "Task:" or "Title:", no explanations.
6. CRITICAL: Output a single line wrapped exactly in <TITLE> and </TITLE> tags. Example: <TITLE>Fix login token expiration</TITLE>
   Output nothing outside these tags — no extra text, blank lines, code fences, or commentary.

──── Original Task Prompt ────
{prompt}

──── Session Execution Summary ────
{summary}
"#;

const NAMING_FALLBACK_SUMMARY: &str =
    "(No session summary available — generate the title based on the original prompt alone.)";

const NAMING_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_TITLE_CHARS: usize = 120;
const MAX_PROMPT_CHARS: usize = 4000;

fn build_naming_prompt(original_prompt: &str, summary: Option<&str>) -> String {
    let summary_text = summary.unwrap_or(NAMING_FALLBACK_SUMMARY);
    NAMING_PROMPT_TEMPLATE
        .replace("{prompt}", original_prompt)
        .replace("{summary}", summary_text)
}

/// Validate project_path: it must be absolute, canonicalizable, and actually a directory.
/// Avoids launching the agent process with an arbitrary directory as cwd (M-3 fix).
fn validate_project_path_for_naming(project_path: &str) -> Result<(), String> {
    let path = Path::new(project_path);
    if !path.is_absolute() {
        return Err("project_path must be absolute".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("Cannot resolve project_path: {}", e))?;
    if !canonical.is_dir() {
        return Err("project_path is not a directory".into());
    }
    Ok(())
}

async fn read_pipe_to_end<R: AsyncRead + Unpin>(
    mut pipe: R,
    stream_name: &str,
) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    pipe.read_to_end(&mut data)
        .await
        .map_err(|e| format!("Failed to read agent {}: {}", stream_name, e))?;
    Ok(data)
}

/// Asynchronously launch the naming agent subprocess. On timeout, terminate the subprocess via
/// `start_kill()` to avoid blocking the thread and leaving a background agent running (M-2 fix).
async fn run_naming_agent_with_timeout(
    agent: &str,
    project_path: &str,
    prompt: &str,
    timeout_dur: Duration,
) -> Result<Output, String> {
    let launch = crate::app_settings::get_agent_launch_spec(agent);
    let login_env: Vec<(String, String)> = crate::app_settings::get_login_shell_env().to_vec();

    let mut cmd = tokio::process::Command::new(&launch.program);
    crate::subprocess::configure_background_tokio_command(&mut cmd);
    if agent == "codex" {
        cmd.args([
            "exec",
            "--sandbox",
            "read-only",
            "--ephemeral",
            "-c",
            "approval_policy=\"never\"",
            prompt,
        ]);
    } else {
        cmd.args([
            "-p",
            prompt,
            "--output-format",
            "text",
            "--permission-mode",
            "plan",
            "--tools",
            "",
            "--no-session-persistence",
        ]);
    }
    cmd.current_dir(project_path);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    for (key, value) in &login_env {
        cmd.env(key, value);
    }
    for (key, value) in &launch.extra_env {
        cmd.env(key, value);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn {agent}: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture agent stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture agent stderr".to_string())?;

    let stdout_task = tokio::spawn(read_pipe_to_end(stdout, "stdout"));
    let stderr_task = tokio::spawn(read_pipe_to_end(stderr, "stderr"));

    let status = match tokio::time::timeout(timeout_dur, child.wait()).await {
        Ok(result) => result.map_err(|e| format!("Agent wait error: {}", e))?,
        Err(_) => {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            stdout_task.abort();
            stderr_task.abort();
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            return Err(format!("Task name generation timed out ({} s)", timeout_dur.as_secs()));
        }
    };

    let stdout_data = stdout_task
        .await
        .map_err(|e| format!("Agent stdout task failed: {}", e))??;
    let stderr_data = stderr_task
        .await
        .map_err(|e| format!("Agent stderr task failed: {}", e))??;

    Ok(Output {
        status,
        stdout: stdout_data,
        stderr: stderr_data,
    })
}

/// Preferentially extract the title wrapped in `<TITLE>...</TITLE>` from stdout.
/// Takes the last `<TITLE>...</TITLE>` pair, to avoid interference from earlier example or explanatory text in the same output section.
fn extract_titled_answer(stdout: &str) -> Option<String> {
    const OPEN: &str = "<TITLE>";
    const CLOSE: &str = "</TITLE>";
    let close_pos = stdout.rfind(CLOSE)?;
    let prefix = &stdout[..close_pos];
    let open_start = prefix.rfind(OPEN)? + OPEN.len();
    let inner = stdout[open_start..close_pos].trim();
    if inner.is_empty() {
        None
    } else {
        Some(inner.split_whitespace().collect::<Vec<_>>().join(" "))
    }
}

/// In non-interactive mode Codex may first echo the user prompt, and the prompt itself contains a `<TITLE>` example.
/// So only accept the title tag in the last `codex` output section, to avoid treating the prompt example as the answer.
fn extract_codex_titled_answer(stdout: &str) -> Option<String> {
    let mut section_start = None;
    let mut offset = 0;

    for line in stdout.split_inclusive('\n') {
        if line.trim() == "codex" {
            section_start = Some(offset + line.len());
        }
        offset += line.len();
    }

    extract_titled_answer(&stdout[section_start?..])
}

/// Codex's non-interactive-mode stdout contains noise like the banner, user/codex labels, and token counts,
/// so take the last non-noise line of text as the model's actual reply.
fn extract_codex_final_message(stdout: &str) -> String {
    for line in stdout.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if matches!(trimmed, "user" | "codex" | "thinking") {
            continue;
        }
        if trimmed.chars().all(|c| c == '-') {
            continue;
        }
        if trimmed.starts_with("OpenAI Codex") {
            continue;
        }
        if trimmed.starts_with("hook:") {
            continue;
        }
        if trimmed.starts_with("workdir:")
            || trimmed.starts_with("model:")
            || trimmed.starts_with("provider:")
            || trimmed.starts_with("approval:")
            || trimmed.starts_with("sandbox:")
            || trimmed.starts_with("session id:")
            || trimmed.starts_with("reasoning effort:")
            || trimmed.starts_with("reasoning summaries:")
            || trimmed.starts_with("tokens used")
        {
            continue;
        }
        // Skip pure-numeric (including commas) token-count lines, e.g. "16,330"
        if trimmed
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c.is_whitespace())
        {
            continue;
        }
        return trimmed.to_string();
    }
    String::new()
}

fn sanitize_title(raw: &str) -> String {
    let trimmed = raw.trim();
    let trimmed = trimmed
        .strip_prefix("<TITLE>")
        .and_then(|value| value.strip_suffix("</TITLE>"))
        .unwrap_or(trimmed)
        .trim();
    let stripped = trimmed.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\''
                | '`'
                | '\u{201C}'
                | '\u{201D}'
                | '\u{2018}'
                | '\u{2019}'
                | '《'
                | '》'
                | '【'
                | '】'
                | '「'
                | '」'
                | '『'
                | '』'
        )
    });
    let stripped = stripped
        .trim_end_matches(|c: char| matches!(c, '.' | '。' | '!' | '！' | '?' | '？'))
        .trim();
    stripped
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

fn truncate_prompt(prompt: String) -> String {
    if prompt.chars().count() <= MAX_PROMPT_CHARS {
        prompt
    } else {
        prompt.chars().take(MAX_PROMPT_CHARS).collect::<String>() + "…"
    }
}

#[tauri::command]
pub async fn generate_task_name(
    project_path: String,
    agent: String,
    session_path: Option<String>,
    original_prompt: String,
) -> Result<String, String> {
    if !matches!(agent.as_str(), "claude" | "codex") {
        return Err(format!("Unsupported agent: {}", agent));
    }
    let is_codex = agent == "codex";

    // 1. Validate that project_path is legal (M-3)
    let project_for_validation = project_path.clone();
    tokio::task::spawn_blocking(move || validate_project_path_for_naming(&project_for_validation))
        .await
        .map_err(|e| format!("project_path validation thread error: {}", e))??;

    // 2. Session summary extraction is done in spawn_blocking (to avoid blocking Tokio)
    let summary = if let Some(raw_path) = session_path {
        let project_for_summary = project_path.clone();
        tokio::task::spawn_blocking(move || {
            match crate::session::validate_session_path(&raw_path, &project_for_summary, is_codex) {
                Ok(canonical) => {
                    crate::session::extract_session_summary_text(&canonical.to_string_lossy(), 7000)
                }
                Err(e) => {
                    eprintln!("[generate_task_name] session_path validation failed: {}", e);
                    None
                }
            }
        })
        .await
        .map_err(|e| format!("summary thread error: {}", e))?
    } else {
        None
    };

    // 3. Assemble the naming prompt
    let truncated_prompt = truncate_prompt(original_prompt);
    let full_prompt = build_naming_prompt(&truncated_prompt, summary.as_deref());

    // 4. Invoke the agent subprocess (kill-on-timeout)
    let output =
        run_naming_agent_with_timeout(&agent, &project_path, &full_prompt, NAMING_TIMEOUT).await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!("Agent failed: {}{}", stderr, stdout));
    }

    let raw = String::from_utf8_lossy(&output.stdout).into_owned();
    // Codex only accepts the <TITLE> in the last `codex` output section, to avoid hitting the
    // example in the prompt echo; if there's no match, fall back to banner filtering.
    let answer = if is_codex {
        extract_codex_titled_answer(&raw).unwrap_or_else(|| extract_codex_final_message(&raw))
    } else {
        extract_titled_answer(&raw).unwrap_or_else(|| raw.trim().to_string())
    };

    let sanitized = sanitize_title(&answer);
    if sanitized.is_empty() {
        return Err("Agent returned empty response.".to_string());
    }
    Ok(sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_titled_answer_from_wrapped_output() {
        let stdout = "OpenAI Codex v0.128\nuser\n...\ncodex\n<TITLE>Fix login page token expiration</TITLE>\ntokens used\n12,345\n";
        assert_eq!(
            extract_titled_answer(stdout).as_deref(),
            Some("Fix login page token expiration")
        );
    }

    #[test]
    fn extract_titled_takes_last_match_avoiding_prompt_echo() {
        // Simulate the prompt being echoed in Codex stdout (with the prompt's embedded example tag), followed by the model's actual reply
        let stdout = "user\n...e.g.: <TITLE>Fix login page token expiration</TITLE> is an example\nhook: SessionStart\ncodex\n<TITLE>Refactor PTY buffer to 64KB</TITLE>\ntokens used\n";
        assert_eq!(
            extract_titled_answer(stdout).as_deref(),
            Some("Refactor PTY buffer to 64KB")
        );
        assert_eq!(
            extract_codex_titled_answer(stdout).as_deref(),
            Some("Refactor PTY buffer to 64KB")
        );
    }

    #[test]
    fn extract_codex_titled_answer_ignores_prompt_echo_sample() {
        let stdout = "OpenAI Codex v0.128\nuser\nCorrect example: <TITLE>Fix login page token expiration</TITLE>\nhook: SessionStart\ncodex\nRename the task-title generation logic\ntokens used\n1,234\n";
        assert_eq!(
            extract_titled_answer(stdout).as_deref(),
            Some("Fix login page token expiration")
        );
        assert_eq!(extract_codex_titled_answer(stdout), None);
        assert_eq!(
            extract_codex_final_message(stdout),
            "Rename the task-title generation logic"
        );
    }

    #[test]
    fn extract_titled_answer_collapses_internal_whitespace() {
        let stdout = "<TITLE>  Fix\n  login\tbug  </TITLE>";
        assert_eq!(
            extract_titled_answer(stdout).as_deref(),
            Some("Fix login bug")
        );
    }

    #[test]
    fn extract_titled_answer_returns_none_when_tag_missing() {
        assert_eq!(extract_titled_answer("plain output without tags"), None);
        assert_eq!(extract_titled_answer("<TITLE></TITLE>"), None);
        assert_eq!(extract_titled_answer("<TITLE>  </TITLE>"), None);
    }

    #[test]
    fn extracts_codex_final_message_skipping_banner_and_tokens() {
        let stdout = "OpenAI Codex v0.128.0 (research preview)\n--------\nworkdir: /tmp\nmodel: gpt-5.5\nprovider: openai\nsession id: 019e\n--------\nuser\nReply with hi\nhook: SessionStart\nhook: SessionStart Completed\ncodex\nFix the login page token expiration issue\ntokens used\n16,330\n";
        assert_eq!(
            extract_codex_final_message(stdout),
            "Fix the login page token expiration issue"
        );
    }

    #[test]
    fn sanitize_strips_quotes_and_trailing_punct() {
        assert_eq!(
            sanitize_title("\"Fix login token expiration!\""),
            "Fix login token expiration"
        );
        assert_eq!(
            sanitize_title("'Fix login token expiration.'"),
            "Fix login token expiration"
        );
    }

    #[test]
    fn sanitize_strips_wrapping_title_tags() {
        assert_eq!(
            sanitize_title("<TITLE>Get familiar with the project README and AGENTS conventions</TITLE>"),
            "Get familiar with the project README and AGENTS conventions"
        );
    }

    #[test]
    fn sanitize_truncates_to_120_chars() {
        let long = "a".repeat(200);
        assert_eq!(sanitize_title(&long).chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn build_naming_prompt_with_summary() {
        let p = build_naming_prompt("Fix the login bug", Some("[User] login failed [AI] look at auth.ts"));
        assert!(p.contains("Fix the login bug"));
        assert!(p.contains("[User] login failed"));
    }

    #[test]
    fn build_naming_prompt_without_summary_uses_fallback() {
        let p = build_naming_prompt("Write a hello world", None);
        assert!(p.contains(NAMING_FALLBACK_SUMMARY));
    }
}
