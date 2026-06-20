export interface Project {
  id: string;
  name: string;
  path: string;
  branch?: string;
  lastOpenedAt: number;
  /** When true, not shown in the left pinned rail; accessible only from the home page or the "Expand All" drawer. Default = pinned. */
  hiddenFromRail?: boolean;
}

export type AgentType = "claude" | "codex";
export type ThemeMode = "system" | "dark" | "light" | "eyecare" | "midnight";
export type ThemeVariant = "dark" | "light" | "eyecare" | "midnight";
export type PermissionMode = "ask" | "auto_edit" | "full_access";
export type TaskDisplayWindow = 3 | 7 | 15 | 30 | "all";

export const TASK_DISPLAY_WINDOW_VALUES = [3, 7, 15, 30, "all"] as const;
export const DEFAULT_TASK_DISPLAY_WINDOW: TaskDisplayWindow = 3;

export function normalizeTaskDisplayWindow(value: unknown): TaskDisplayWindow {
  if (value === "all") return "all";
  const parsed = typeof value === "number" ? value : Number(value);
  return TASK_DISPLAY_WINDOW_VALUES.includes(parsed as TaskDisplayWindow)
    ? (parsed as TaskDisplayWindow)
    : DEFAULT_TASK_DISPLAY_WINDOW;
}

export type TerminalFontSize = number;

export const TERMINAL_FONT_SIZE_MIN = 10;
export const TERMINAL_FONT_SIZE_MAX = 20;
export const TERMINAL_FONT_SIZE_STEP = 1;
export const DEFAULT_TERMINAL_FONT_SIZE: TerminalFontSize = 12;

export function clampTerminalFontSize(value: number): TerminalFontSize {
  if (!Number.isFinite(value)) return DEFAULT_TERMINAL_FONT_SIZE;
  const snapped = Math.round(value / TERMINAL_FONT_SIZE_STEP) * TERMINAL_FONT_SIZE_STEP;
  return Math.min(TERMINAL_FONT_SIZE_MAX, Math.max(TERMINAL_FONT_SIZE_MIN, snapped));
}

export type FontFamily = string;
export const DEFAULT_UI_FONT: FontFamily =
  '"SF Pro Display", "IBM Plex Sans", "PingFang SC", "Noto Sans SC", sans-serif';
export const DEFAULT_MONO_FONT: FontFamily =
  '"JetBrains Mono", "Fira Code", ui-monospace, monospace';

export type TaskStatus =
  | "todo"
  | "pending"
  | "running"
  | "input_required"
  | "detached"
  | "interrupted"
  | "done"
  | "failed"
  | "cancelled";

export interface Task {
  id: string;
  projectId: string;
  name?: string;
  prompt: string;
  /**
   * "shell" = pure terminal task, launches a login shell directly, without injecting Claude/Codex.
   * "agent" = traditional AI agent task. Defaults to "shell" (new tasks open a terminal by default).
   */
  kind?: "shell" | "agent";
  agent: AgentType;
  permissionMode: PermissionMode;
  status: TaskStatus;
  createdAt: number;
  attentionRequestedAt?: number;
  starred?: boolean;
  failureReason?: string;
  codexSessionId?: string;
  codexSessionPath?: string;
  claudeSessionId?: string;
  claudeSessionPath?: string;
  worktreePath?: string;
  worktreeBranch?: string;
  baseBranch?: string;
  /** Set true after the worktree is merged or discarded: branch/path kept for auditing, but resume / merge / discard are disabled */
  worktreeDiscarded?: boolean;
  /** Cumulative added lines relative to the baseBranch merge-base, computed when the task completes (worktree tasks only) */
  additions?: number;
  /** Cumulative deleted lines relative to the baseBranch merge-base, computed when the task completes (worktree tasks only) */
  deletions?: number;
}

export const PERM_LABELS: Record<PermissionMode, string> = {
  ask: "Ask Permission",
  auto_edit: "Auto-edit",
  full_access: "Full Access",
};

export function permissionModeLabel(
  mode: PermissionMode,
  agent?: AgentType,
  askLabel = PERM_LABELS.ask,
): string {
  if (agent === "codex" && mode === "auto_edit") {
    return "Auto Mode";
  }
  if (mode === "ask") return askLabel;
  return PERM_LABELS[mode];
}

export const STATUS_LABEL: Record<TaskStatus, string> = {
  todo: "Todo",
  pending: "Pending",
  running: "Running...",
  input_required: "Needs confirmation",
  detached: "Terminal disconnected",
  interrupted: "Interrupted",
  done: "Done",
  failed: "Failed",
  cancelled: "Cancelled",
};

export function isActiveTaskStatus(status: TaskStatus): boolean {
  return (
    status === "pending" ||
    status === "running" ||
    status === "input_required" ||
    status === "detached"
  );
}


export interface UsageWindow {
  usedPercent: number;
  remainingPercent: number;
  resetAt?: number | null;
}

export interface ClaudeUsageData {
  fiveHour?: UsageWindow | null;
  sevenDay?: UsageWindow | null;
}

export interface CodexUsageData {
  email?: string | null;
  planType?: string | null;
  primary?: UsageWindow | null;
  secondary?: UsageWindow | null;
}

export type UsageSource<T> =
  | { status: "available"; data: T }
  | { status: "unavailable"; reason: string };

export interface UsageSnapshot {
  claude: UsageSource<ClaudeUsageData>;
  codex: UsageSource<CodexUsageData>;
  fetchedAt: number;
}

// ── Skill Hub ────────────────────────────────────────────────────────────────

export interface SkillHubConfig {
  hubProjectId?: string;
  hubPath?: string;
  createdAt?: number;
}

export interface Skill {
  /** SKILL directory name (authoritative identifier) */
  name: string;
  /** The frontmatter `name` field, which may differ from the directory name */
  displayName?: string;
  /** The parsed description, which may contain newlines */
  description?: string;
  /** Absolute path of the skill directory */
  path: string;
  /** Error description when frontmatter parsing fails */
  hasError?: string;
}

export type SkillInstallationHealth = "ok" | "broken" | "diverged";

export interface SkillInstallation {
  skillName: string;
  projectId: string;
  agent: AgentType;
  installedAt: number;
  linkPath: string;
  targetPath: string;
  health?: SkillInstallationHealth;
}

export type SkillInstallStrategy = "detect" | "skip" | "overwrite" | "cancel";

export interface SkillConflictInfo {
  existingKind: "directory" | "file" | "symlink";
  existingTarget?: string;
  linkPath: string;
}

export interface SkillInstallResult {
  ok: boolean;
  conflict?: SkillConflictInfo;
  alreadyInstalled?: boolean;
  skipped?: boolean;
  cancelled?: boolean;
  installation?: SkillInstallation;
}

export interface SkillDeleteResult {
  ok: boolean;
  removedLinks: number;
}

export interface SetSkillHubResult {
  config: SkillHubConfig;
  project: Project;
  createdNewProject: boolean;
  /** The authoritative projects list after the backend writes it */
  projects: Project[];
}
