import type { LucideIcon } from "lucide-react";
import type { SendShortcut } from "../../shortcuts";

export type NavKey =
  | "general"
  | "theme"
  | "fonts"
  | "shortcuts"
  | "hooks"
  | "skills"
  | "about"
  | "claude"
  | "codex";

export interface HookInstallStatus {
  node_path: string;
  script_path: string;
  claude_installed: boolean;
  codex_installed: boolean;
  error?: string;
}

export type HookReadinessReason = "ok" | "no_node" | "not_installed" | "version_too_low";

export interface HookAgentReadiness {
  agent: "claude" | "codex";
  usable: boolean;
  reason: HookReadinessReason;
  detectedVersion: string;
  minVersion: string;
}

export interface AppSettings {
  claude_path: string;
  codex_path: string;
  send_shortcut: SendShortcut;
  terminal_shift_enter_newline: boolean;
}

export interface AgentVersions {
  claude_version: string;
  codex_version: string;
}

export type AgentKey = "claude" | "codex";

export type NavSection = "application" | "agents" | "about";

export interface AppSettingsNavItem {
  key: NavKey;
  labelKey: string;
  section: NavSection;
  icon?: LucideIcon;
  /** Overrides the icon stroke color (defaults to var(--text-secondary)) */
  iconColor?: string;
  /** Icon fill color (defaults to "none"; passing a color makes it a solid icon) */
  iconFill?: string;
  logo?: string;
  filePath?: string;
  lang?: string;
  /** When set, clicking this item does not switch panels but opens this external link in the browser */
  url?: string;
}

export const APP_SETTINGS_CHANGED_EVENT = "fastaf:app-settings-changed";
export const SKILL_HUB_CHANGED_EVENT = "fastaf:skill-hub-changed";
export const OPEN_APP_SETTINGS_EVENT = "fastaf:open-app-settings";

/**
 * `SKILL_HUB_CHANGED_EVENT` may carry `detail.projects` (the full list from the backend `set_skill_hub_path`),
 * which App.tsx treats as the authoritative list to replace frontend state, avoiding a race that overwrites the hub project.
 */
export interface SkillHubChangedDetail {
  projects?: unknown;
}
