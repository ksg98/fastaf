use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::storage::{
    atomic_write, ensure_fastaf_dirs, load_projects, fastaf_dir, save_projects, Project,
};

// ── Data types ───────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillHubConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    /// SKILL directory name (authoritative identifier)
    pub name: String,
    /// The `name` field from frontmatter; may differ from the directory name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Parsed description (newlines preserved)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Absolute path to the skill directory
    pub path: String,
    /// Error description when frontmatter parsing fails
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SkillInstallation {
    pub skill_name: String,
    pub project_id: String,
    pub agent: String,
    pub installed_at: i64,
    pub link_path: String,
    pub target_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<String>, // "ok" | "broken" | "diverged"
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct InstallationsFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    installations: Vec<SkillInstallation>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SetHubResult {
    pub config: SkillHubConfig,
    pub project: Project,
    pub created_new_project: bool,
    /// The full projects list after the backend write; the frontend uses it to replace React state, avoiding race overwrites.
    pub projects: Vec<Project>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConflictInfo {
    /// "directory" | "file" | "symlink"
    pub existing_kind: String,
    /// When the existing path is a symlink, this is the target it points to
    #[serde(skip_serializing_if = "Option::is_none")]
    pub existing_target: Option<String>,
    pub link_path: String,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct InstallResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<ConflictInfo>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub already_installed: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skipped: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cancelled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installation: Option<SkillInstallation>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResult {
    pub ok: bool,
    pub removed_links: usize,
}

// ── Path helpers ─────────────────────────────────────────────────────────────

fn skill_hub_path() -> Result<PathBuf, String> {
    Ok(fastaf_dir()?.join("skill_hub.json"))
}

fn installations_path() -> Result<PathBuf, String> {
    Ok(fastaf_dir()?.join("skill_installations.json"))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn agent_skills_dir(project_path: &Path, agent: &str) -> PathBuf {
    let sub = match agent {
        "codex" => ".codex/skills",
        _ => ".claude/skills",
    };
    project_path.join(sub)
}

/// skill_name must be a single valid directory segment: non-empty, not `.` / `..`, and containing no path separators.
/// This name becomes the final segment of `agent_skills_dir(...).join(&skill_name)`, so it must be strictly constrained.
fn validate_skill_name(skill_name: &str) -> Result<(), String> {
    if skill_name.is_empty() {
        return Err("Skill name cannot be empty".to_string());
    }
    if skill_name == "." || skill_name == ".." {
        return Err(format!("Invalid skill name: {}", skill_name));
    }
    if skill_name.contains('/') || skill_name.contains('\\') || skill_name.contains('\0') {
        return Err(format!(
            "Skill name must not contain path separators: {}",
            skill_name
        ));
    }
    Ok(())
}

fn target_health(target: &Path) -> &'static str {
    if target.exists() {
        "ok"
    } else {
        "broken"
    }
}

// ── Hub config I/O ───────────────────────────────────────────────────────────

fn load_hub_config_internal() -> SkillHubConfig {
    let Ok(path) = skill_hub_path() else {
        return SkillHubConfig::default();
    };
    if !path.exists() {
        return SkillHubConfig::default();
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<SkillHubConfig>(&raw).ok())
        .unwrap_or_default()
}

fn save_hub_config_internal(config: &SkillHubConfig) -> Result<(), String> {
    ensure_fastaf_dirs()?;
    let raw = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    atomic_write(&skill_hub_path()?, &raw)
}

fn load_installations_internal() -> InstallationsFile {
    let Ok(path) = installations_path() else {
        return InstallationsFile::default();
    };
    if !path.exists() {
        return InstallationsFile::default();
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<InstallationsFile>(&raw).ok())
        .unwrap_or_default()
}

fn save_installations_internal(file: &InstallationsFile) -> Result<(), String> {
    ensure_fastaf_dirs()?;
    let raw = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    atomic_write(&installations_path()?, &raw)
}

// ── SKILL.md frontmatter parsing ─────────────────────────────────────────────
// A hand-written parser that only cares about the top-level `name` and `description` in frontmatter.
// Supports: single line (with quotes), literal block (`|`, `|-`, `|+`), folded (`>`, `>-`, `>+`).

fn strip_yaml_quotes(s: &str) -> String {
    let trimmed = s.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if first == last && (first == b'"' || first == b'\'') {
            return trimmed[1..trimmed.len() - 1].to_string();
        }
    }
    trimmed.to_string()
}

/// Parse the multi-line content of a YAML literal block scalar.
/// `lines` is all candidate lines below the block; returns (joined content, lines consumed).
fn parse_block_scalar(lines: &[&str], folded: bool) -> (String, usize) {
    // Determine the base indentation (leading spaces of the first non-empty line)
    let mut base_indent: Option<usize> = None;
    let mut consumed = 0usize;
    let mut collected: Vec<String> = Vec::new();

    for line in lines {
        // Blank line: always belongs to the current block
        if line.trim().is_empty() {
            collected.push(String::new());
            consumed += 1;
            continue;
        }
        let leading = line.chars().take_while(|c| *c == ' ').count();
        // A top-level key always starts at column 0; if a following line has no indentation, the block has ended
        if leading == 0 {
            break;
        }
        let base = *base_indent.get_or_insert(leading);
        if leading < base {
            break;
        }
        collected.push(line[base..].to_string());
        consumed += 1;
    }

    // Strip trailing blank lines at the end of the block (default clip behavior)
    while collected.last().map(|s| s.is_empty()).unwrap_or(false) {
        collected.pop();
    }

    let joined = if folded {
        fold_lines(&collected)
    } else {
        collected.join("\n")
    };
    (joined, consumed)
}

/// YAML folded scalar rules:
/// - adjacent non-empty lines are joined with a space
/// - a single blank line becomes one newline
/// - multiple consecutive blank lines → n-1 newlines
fn fold_lines(lines: &[String]) -> String {
    let mut out = String::new();
    let mut prev_blank = false;
    let mut first = true;
    for line in lines {
        if line.is_empty() {
            if first {
                first = false;
                prev_blank = true;
                continue;
            }
            out.push('\n');
            prev_blank = true;
            continue;
        }
        if !first && !prev_blank {
            out.push(' ');
        }
        out.push_str(line);
        first = false;
        prev_blank = false;
    }
    out
}

#[derive(Default)]
struct ParsedFrontmatter {
    name: Option<String>,
    description: Option<String>,
}

fn parse_frontmatter(content: &str) -> ParsedFrontmatter {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() || lines[0].trim() != "---" {
        return ParsedFrontmatter::default();
    }

    // Locate the closing `---` of the frontmatter
    let mut end = lines.len();
    for (i, line) in lines.iter().enumerate().skip(1) {
        if line.trim() == "---" {
            end = i;
            break;
        }
    }
    let fm = &lines[1..end];

    let mut parsed = ParsedFrontmatter::default();
    let mut i = 0;
    while i < fm.len() {
        let line = fm[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        // A top-level key must start at column 0
        if line.starts_with(|c: char| c.is_whitespace()) {
            i += 1;
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else {
            i += 1;
            continue;
        };
        let key = key.trim();
        let value_part = rest.trim();

        // Detect the block scalar indicator
        let block_marker = value_part.chars().next().filter(|c| *c == '|' || *c == '>');

        if let Some(marker) = block_marker {
            // Skip the chomping modifiers `-` / `+`; this implementation always uses clip behavior
            let folded = marker == '>';
            let (value, consumed) = parse_block_scalar(&fm[i + 1..], folded);
            match key {
                "name" => parsed.name = Some(value),
                "description" => parsed.description = Some(value),
                _ => {}
            }
            i += 1 + consumed;
        } else {
            let value = strip_yaml_quotes(value_part);
            match key {
                "name" => parsed.name = Some(value),
                "description" => parsed.description = Some(value),
                _ => {}
            }
            i += 1;
        }
    }

    parsed
}

// ── Skill scanning ───────────────────────────────────────────────────────────

fn parse_skill_entry(dir_path: &Path, name: &str) -> Skill {
    let skill_md = dir_path.join("SKILL.md");
    let (display_name, description, has_error) = match fs::read_to_string(&skill_md) {
        Ok(content) => {
            let parsed = parse_frontmatter(&content);
            (parsed.name, parsed.description, None)
        }
        Err(e) => (None, None, Some(format!("Failed to read SKILL.md: {}", e))),
    };
    Skill {
        name: name.to_string(),
        display_name,
        description,
        path: dir_path.to_string_lossy().into_owned(),
        has_error,
    }
}

/// Recursively scan directories: a directory containing SKILL.md is treated as a skill, otherwise keep descending into subdirectories.
/// Depth is limited and symlink subdirectories are rejected to avoid blowing the stack on maliciously/accidentally constructed cyclic symlinks.
const MAX_SCAN_DEPTH: usize = 6;

fn collect_skills(dir: &Path, skills: &mut Vec<Skill>, depth: usize) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Use symlink_metadata to avoid following symlinks (prevents stack blowup from cyclic symlinks)
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) if !n.starts_with('.') => n.to_string(),
            _ => continue,
        };
        if path.join("SKILL.md").is_file() {
            skills.push(parse_skill_entry(&path, &name));
        } else {
            collect_skills(&path, skills, depth + 1);
        }
    }
}

fn scan_skills_in(hub_path: &Path) -> Vec<Skill> {
    let mut skills: Vec<Skill> = Vec::new();
    collect_skills(hub_path, &mut skills, 0);
    skills.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    skills
}

// ── Symlink helpers ──────────────────────────────────────────────────────────

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

fn classify_existing(path: &Path) -> Option<(String, Option<String>)> {
    let meta = fs::symlink_metadata(path).ok()?;
    let kind = if meta.file_type().is_symlink() {
        "symlink"
    } else if meta.is_dir() {
        "directory"
    } else {
        "file"
    };
    let target = if meta.file_type().is_symlink() {
        fs::read_link(path)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    } else {
        None
    };
    Some((kind.to_string(), target))
}

/// Remove an existing link_path (symlink / plain directory / file all supported)
fn remove_existing(path: &Path) -> Result<(), String> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    if meta.file_type().is_symlink() || meta.is_file() {
        fs::remove_file(path).map_err(|e| e.to_string())
    } else {
        fs::remove_dir_all(path).map_err(|e| e.to_string())
    }
}

fn symlink_points_to(link_path: &Path, expected_canonical: &Path) -> bool {
    let Ok(target) = fs::read_link(link_path) else {
        return false;
    };
    let resolved = if target.is_absolute() {
        target
    } else {
        link_path
            .parent()
            .map(|parent| parent.join(&target))
            .unwrap_or(target)
    };
    resolved
        .canonicalize()
        .map(|actual| actual == expected_canonical)
        .unwrap_or(false)
}

fn installation_targets_skill(ins: &SkillInstallation, expected_canonical: &Path) -> bool {
    let target = Path::new(&ins.target_path);
    target
        .canonicalize()
        .map(|actual| actual == expected_canonical)
        .unwrap_or_else(|_| target == expected_canonical)
}

fn remove_symlink_if_present(link_path: &Path) -> Result<bool, String> {
    let Ok(meta) = fs::symlink_metadata(link_path) else {
        return Ok(false);
    };
    if !meta.file_type().is_symlink() {
        return Ok(false);
    }
    fs::remove_file(link_path)
        .map_err(|e| format!("Failed to remove symlink {}: {}", link_path.display(), e))?;
    Ok(true)
}

// ── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_skill_hub_config() -> Result<SkillHubConfig, String> {
    tokio::task::spawn_blocking(load_hub_config_internal)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_skill_hub_path(path: String) -> Result<SetHubResult, String> {
    tokio::task::spawn_blocking(move || {
        let raw = path.trim();
        if raw.is_empty() {
            return Err("Hub path cannot be empty".to_string());
        }
        let target = Path::new(raw);
        if !target.is_absolute() {
            return Err("Hub path must be absolute".to_string());
        }
        let canonical = target
            .canonicalize()
            .map_err(|e| format!("Cannot resolve hub path: {}", e))?;
        if !canonical.is_dir() {
            return Err("Hub path is not a directory".to_string());
        }
        let hub_path_str = canonical.to_string_lossy().into_owned();

        let mut projects = load_projects()?;
        let existing = projects
            .iter()
            .find(|p| {
                Path::new(&p.path).canonicalize().ok().as_deref() == Some(canonical.as_path())
            })
            .cloned();

        let (project, created_new_project) = match existing {
            Some(p) => (p, false),
            None => {
                let name = canonical
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("skills_hub")
                    .to_string();
                let new_project = Project {
                    id: now_ms().to_string(),
                    name,
                    path: hub_path_str.clone(),
                    branch: None,
                    last_opened_at: now_ms(),
                    hidden_from_rail: false,
                };
                projects.push(new_project.clone());
                save_projects(projects.clone())?;
                (new_project, true)
            }
        };

        let config = SkillHubConfig {
            hub_project_id: Some(project.id.clone()),
            hub_path: Some(hub_path_str),
            created_at: Some(now_ms()),
        };
        save_hub_config_internal(&config)?;

        Ok(SetHubResult {
            config,
            project,
            created_new_project,
            projects,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn clear_skill_hub() -> Result<(), String> {
    tokio::task::spawn_blocking(|| {
        let cfg = SkillHubConfig::default();
        save_hub_config_internal(&cfg)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn list_skills() -> Result<Vec<Skill>, String> {
    tokio::task::spawn_blocking(|| {
        let cfg = load_hub_config_internal();
        let Some(hub_path) = cfg.hub_path.as_deref() else {
            return Ok(Vec::new());
        };
        Ok(scan_skills_in(Path::new(hub_path)))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn list_skill_installations(
    skill_name: Option<String>,
) -> Result<Vec<SkillInstallation>, String> {
    tokio::task::spawn_blocking(move || {
        let file = load_installations_internal();
        let mut out: Vec<SkillInstallation> = file
            .installations
            .into_iter()
            .filter(|ins| match &skill_name {
                Some(name) => ins.skill_name == *name,
                None => true,
            })
            .collect();

        // Health check: compare using canonicalize to avoid falsely reporting diverged due to trailing `/` or case differences
        for ins in &mut out {
            let link = Path::new(&ins.link_path);
            let target_canonical = Path::new(&ins.target_path).canonicalize();
            ins.health = Some(match fs::symlink_metadata(link) {
                Err(_) => "broken".to_string(),
                Ok(meta) if !meta.file_type().is_symlink() => "diverged".to_string(),
                Ok(_) => match target_canonical {
                    Err(_) => "broken".to_string(),
                    Ok(expected) if symlink_points_to(link, &expected) => "ok".to_string(),
                    Ok(_) => "diverged".to_string(),
                },
            });
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn install_skill(
    skill_name: String,
    skill_path: String,
    project_id: String,
    agent: String,
    strategy: String,
) -> Result<InstallResult, String> {
    tokio::task::spawn_blocking(move || {
        if !matches!(agent.as_str(), "claude" | "codex") {
            return Err(format!("Unsupported agent: {}", agent));
        }
        if !matches!(
            strategy.as_str(),
            "detect" | "skip" | "overwrite" | "cancel"
        ) {
            return Err(format!("Unsupported strategy: {}", strategy));
        }
        validate_skill_name(&skill_name)?;

        // cancel is an explicit no-op
        if strategy == "cancel" {
            return Ok(InstallResult {
                ok: false,
                cancelled: true,
                ..Default::default()
            });
        }

        let skill_dir = Path::new(&skill_path);
        if !skill_dir.is_dir() {
            return Err(format!(
                "Skill '{}' not found at path: {}",
                skill_name, skill_path
            ));
        }
        if !skill_dir.join("SKILL.md").is_file() {
            return Err(format!("Skill '{}' has no SKILL.md", skill_name));
        }
        // The final segment of skill_path must match skill_name, to prevent forged directory names
        if skill_dir.file_name().and_then(|s| s.to_str()) != Some(skill_name.as_str()) {
            return Err(format!(
                "Skill path '{}' does not match skill name '{}'",
                skill_path, skill_name
            ));
        }

        // Verify the skill path must be located inside the configured hub directory
        let cfg = load_hub_config_internal();
        let hub_path = cfg
            .hub_path
            .as_deref()
            .ok_or_else(|| "Skill Hub is not configured".to_string())?;
        let hub_canonical = Path::new(hub_path)
            .canonicalize()
            .map_err(|e| format!("Cannot resolve hub path '{}': {}", hub_path, e))?;
        let skill_canonical = skill_dir
            .canonicalize()
            .map_err(|e| format!("Cannot resolve skill path '{}': {}", skill_path, e))?;
        if !skill_canonical.starts_with(&hub_canonical) {
            return Err(format!(
                "Skill path '{}' is not inside hub '{}'",
                skill_path, hub_path
            ));
        }

        let projects = load_projects()?;
        let project = projects
            .iter()
            .find(|p| p.id == project_id)
            .ok_or_else(|| format!("Project '{}' not found", project_id))?;
        let project_path = Path::new(&project.path);
        if !project_path.is_dir() {
            return Err(format!("Project path does not exist: {}", project.path));
        }

        let skills_root = agent_skills_dir(project_path, &agent);
        fs::create_dir_all(&skills_root)
            .map_err(|e| format!("Failed to create {}: {}", skills_root.display(), e))?;
        let link_path = skills_root.join(&skill_name);

        let target_path_str = skill_canonical.to_string_lossy().into_owned();
        let link_path_str = link_path.to_string_lossy().into_owned();

        if strategy == "skip" {
            return Ok(InstallResult {
                ok: true,
                skipped: true,
                ..Default::default()
            });
        }

        // Shared entry point for detect / overwrite: check the current state of link_path
        let existing = classify_existing(&link_path);

        if let Some((kind, existing_target)) = existing.as_ref() {
            let already_same_symlink =
                kind == "symlink" && symlink_points_to(&link_path, &skill_canonical);

            if already_same_symlink {
                // Idempotent: fill in the installations record
                let installation = upsert_installation(
                    &skill_name,
                    &project_id,
                    &agent,
                    &link_path_str,
                    &target_path_str,
                )?;
                return Ok(InstallResult {
                    ok: true,
                    already_installed: true,
                    installation: Some(installation),
                    ..Default::default()
                });
            }

            if strategy == "detect" {
                return Ok(InstallResult {
                    ok: false,
                    conflict: Some(ConflictInfo {
                        existing_kind: kind.clone(),
                        existing_target: existing_target.clone(),
                        link_path: link_path_str,
                    }),
                    ..Default::default()
                });
            }

            // overwrite
            remove_existing(&link_path)?;
        }

        create_symlink(&skill_canonical, &link_path).map_err(|e| {
            format!(
                "Failed to create symlink {} -> {}: {}",
                link_path.display(),
                skill_canonical.display(),
                e
            )
        })?;

        let installation = upsert_installation(
            &skill_name,
            &project_id,
            &agent,
            &link_path_str,
            &target_path_str,
        )?;

        Ok(InstallResult {
            ok: true,
            installation: Some(installation),
            ..Default::default()
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn uninstall_skill(
    skill_name: String,
    project_id: String,
    agent: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        validate_skill_name(&skill_name)?;
        if !matches!(agent.as_str(), "claude" | "codex") {
            return Err(format!("Unsupported agent: {}", agent));
        }
        let mut file = load_installations_internal();
        let target = file.installations.iter().find(|ins| {
            ins.skill_name == skill_name && ins.project_id == project_id && ins.agent == agent
        });

        let link_path = match target {
            Some(ins) => PathBuf::from(&ins.link_path),
            None => {
                // Even without a record, still try to clean up at the conventional path
                let projects = load_projects()?;
                let project = projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .ok_or_else(|| format!("Project '{}' not found", project_id))?;
                agent_skills_dir(Path::new(&project.path), &agent).join(&skill_name)
            }
        };

        // Only delete if the existing entry is a symlink; keep plain directories to avoid accidentally deleting user content
        if let Ok(meta) = fs::symlink_metadata(&link_path) {
            if meta.file_type().is_symlink() {
                fs::remove_file(&link_path).map_err(|e| e.to_string())?;
            }
        }

        file.installations.retain(|ins| {
            !(ins.skill_name == skill_name && ins.project_id == project_id && ins.agent == agent)
        });
        save_installations_internal(&file)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Called when deleting a project: clears all skill installation records for that project and removes leftover symlinks on a best-effort basis.
/// best-effort: failing to remove a symlink (e.g. the project directory no longer exists) is not treated as an error.
#[tauri::command]
pub async fn cleanup_installations_for_project(project_id: String) -> Result<usize, String> {
    tokio::task::spawn_blocking(move || {
        let mut file = load_installations_internal();
        let original_len = file.installations.len();

        for ins in file.installations.iter().filter(|i| i.project_id == project_id) {
            let link = Path::new(&ins.link_path);
            if let Ok(meta) = fs::symlink_metadata(link) {
                if meta.file_type().is_symlink() {
                    let _ = fs::remove_file(link);
                }
            }
        }

        file.installations.retain(|ins| ins.project_id != project_id);
        let removed = original_len - file.installations.len();
        if removed > 0 {
            save_installations_internal(&file)?;
        }
        Ok(removed)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn delete_skill(skill_name: String, skill_path: String) -> Result<DeleteResult, String> {
    tokio::task::spawn_blocking(move || {
        validate_skill_name(&skill_name)?;
        let skill_dir = Path::new(&skill_path);
        if !skill_dir.is_dir() {
            return Err(format!(
                "Skill '{}' not found at path: {}",
                skill_name, skill_path
            ));
        }
        if !skill_dir.join("SKILL.md").is_file() {
            return Err(format!("Skill '{}' has no SKILL.md", skill_name));
        }
        if skill_dir.file_name().and_then(|s| s.to_str()) != Some(skill_name.as_str()) {
            return Err(format!(
                "Skill path '{}' does not match skill name '{}'",
                skill_path, skill_name
            ));
        }

        let cfg = load_hub_config_internal();
        let hub_path = cfg
            .hub_path
            .as_deref()
            .ok_or_else(|| "Skill Hub is not configured".to_string())?;
        let hub_canonical = Path::new(hub_path)
            .canonicalize()
            .map_err(|e| format!("Cannot resolve hub path: {}", e))?;
        let skill_canonical = skill_dir
            .canonicalize()
            .map_err(|e| format!("Cannot resolve skill path: {}", e))?;
        if !skill_canonical.starts_with(&hub_canonical) {
            return Err(format!(
                "Skill path '{}' is not inside hub '{}'",
                skill_path, hub_path
            ));
        }

        let file = load_installations_internal();
        let mut candidate_links: HashSet<PathBuf> = file
            .installations
            .iter()
            .filter(|ins| {
                ins.skill_name == skill_name && installation_targets_skill(ins, &skill_canonical)
            })
            .map(|ins| PathBuf::from(&ins.link_path))
            .collect();

        for project in load_projects()? {
            let project_path = Path::new(&project.path);
            for agent in ["claude", "codex"] {
                let link = agent_skills_dir(project_path, agent).join(&skill_name);
                if symlink_points_to(&link, &skill_canonical) {
                    candidate_links.insert(link);
                }
            }
        }

        let mut removed_links = 0usize;
        for link_path in candidate_links {
            if remove_symlink_if_present(&link_path)? {
                removed_links += 1;
            }
        }

        fs::remove_dir_all(&skill_canonical)
            .map_err(|e| format!("Failed to delete skill directory: {}", e))?;

        let mut file = file;
        file.installations.retain(|ins| {
            !(ins.skill_name == skill_name && installation_targets_skill(ins, &skill_canonical))
        });
        save_installations_internal(&file)?;

        Ok(DeleteResult {
            ok: true,
            removed_links,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

fn upsert_installation(
    skill_name: &str,
    project_id: &str,
    agent: &str,
    link_path: &str,
    target_path: &str,
) -> Result<SkillInstallation, String> {
    let mut file = load_installations_internal();
    if file.version == 0 {
        file.version = 1;
    }
    let now = now_ms();
    let mut existing_idx: Option<usize> = None;
    for (i, ins) in file.installations.iter().enumerate() {
        if ins.skill_name == skill_name && ins.project_id == project_id && ins.agent == agent {
            existing_idx = Some(i);
            break;
        }
    }
    let health = target_health(Path::new(target_path)).to_string();
    let installation = SkillInstallation {
        skill_name: skill_name.to_string(),
        project_id: project_id.to_string(),
        agent: agent.to_string(),
        installed_at: now,
        link_path: link_path.to_string(),
        target_path: target_path.to_string(),
        health: Some(health),
    };
    match existing_idx {
        Some(idx) => file.installations[idx] = installation.clone(),
        None => file.installations.push(installation.clone()),
    }
    save_installations_internal(&file)?;
    Ok(installation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_line_description() {
        let md = "---\nname: foo\ndescription: hello world\n---\nbody";
        let p = parse_frontmatter(md);
        assert_eq!(p.name.as_deref(), Some("foo"));
        assert_eq!(p.description.as_deref(), Some("hello world"));
    }

    #[test]
    fn parse_literal_block_description() {
        let md = "---\nname: foo\ndescription: |\n  line 1\n  line 2\n  line 3\n---\n";
        let p = parse_frontmatter(md);
        assert_eq!(p.description.as_deref(), Some("line 1\nline 2\nline 3"));
    }

    #[test]
    fn parse_literal_block_with_blank_line() {
        let md = "---\ndescription: |\n  para 1\n\n  para 2\n---\n";
        let p = parse_frontmatter(md);
        assert_eq!(p.description.as_deref(), Some("para 1\n\npara 2"));
    }

    #[test]
    fn parse_folded_block() {
        let md = "---\ndescription: >\n  line 1\n  line 2\n\n  line 3\n---\n";
        let p = parse_frontmatter(md);
        assert_eq!(p.description.as_deref(), Some("line 1 line 2\nline 3"));
    }

    #[test]
    fn parse_quoted_value() {
        let md = "---\nname: \"my-skill\"\n---\n";
        let p = parse_frontmatter(md);
        assert_eq!(p.name.as_deref(), Some("my-skill"));
    }

    #[test]
    fn parse_ignores_other_fields() {
        let md = "---\nname: foo\ndisable-model-invocation: false\ndescription: bar\n---\n";
        let p = parse_frontmatter(md);
        assert_eq!(p.name.as_deref(), Some("foo"));
        assert_eq!(p.description.as_deref(), Some("bar"));
    }
}
