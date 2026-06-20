import { useState, useEffect, memo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Trash2, Star, Play, GitBranch, TerminalSquare } from "lucide-react";
import type { Task } from "../../types";
import { StatusIcon } from "../StatusIcon";
import { useI18n } from "../../i18n";
import s from "../../styles";
import claudeLogo from "../../assets/claude.svg";
import chatgptLogo from "../../assets/chatgpt.svg";

function statusLabelKey(status: Task["status"]): string {
  switch (status) {
    case "todo":
      return "status.todo";
    case "pending":
      return "status.pending";
    case "running":
      return "status.running";
    case "input_required":
      return "status.inputRequired";
    case "detached":
      return "status.detached";
    case "interrupted":
      return "status.interrupted";
    case "done":
      return "status.done";
    case "failed":
      return "status.failed";
    case "cancelled":
      return "status.cancelled";
  }
}

export const TaskListItem = memo(
  function TaskListItem({
    task,
    selected,
    onClick,
    onDelete,
    onToggleStar,
    onRunTodo,
    onRename,
  }: {
    task: Task;
    selected: boolean;
    onClick: () => void;
    onDelete: () => void;
    onToggleStar: () => void;
    onRunTodo?: () => void;
    onRename?: (name: string) => void;
  }) {
    const { t } = useI18n();
    const [hov, setHov] = useState(false);
    const [editing, setEditing] = useState(false);
    const [editValue, setEditValue] = useState("");
    const displayTitle = task.name ?? task.prompt;

    // git ahead/behind (↑ahead / ↓behind): worktree tasks only, fetched once on mount and cached, no polling.
    const [aheadBehind, setAheadBehind] = useState<{ ahead: number; behind: number } | null>(null);
    const wtPath = task.worktreePath;
    const baseBranch = task.baseBranch;
    const canAheadBehind = !!wtPath && !!baseBranch && !task.worktreeDiscarded;
    useEffect(() => {
      if (!canAheadBehind) return;
      let cancelled = false;
      invoke<{ ahead: number; behind: number }>("branch_ahead_behind", {
        worktreePath: wtPath,
        baseBranch,
      })
        .then((r) => {
          if (!cancelled) setAheadBehind(r);
        })
        .catch(() => {});
      return () => {
        cancelled = true;
      };
    }, [canAheadBehind, wtPath, baseBranch]);

    const startRename = () => {
      if (!onRename) return;
      setEditValue(task.name ?? "");
      setEditing(true);
    };
    const commitRename = () => {
      onRename?.(editValue.trim());
      setEditing(false);
    };
    return (
      <div
        style={{
          ...s.taskCard,
          position: "relative",
          background: selected ? "var(--bg-selected)" : hov ? "var(--bg-hover)" : "transparent",
        }}
        onMouseEnter={() => setHov(true)}
        onMouseLeave={() => setHov(false)}
        onClick={onClick}
      >
        <div style={{ flexShrink: 0, marginTop: 1 }}>
          <StatusIcon status={task.status} />
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          {editing ? (
            <input
              autoFocus
              value={editValue}
              placeholder={(task.prompt || displayTitle).slice(0, 60)}
              onClick={(e) => e.stopPropagation()}
              onChange={(e) => setEditValue(e.target.value)}
              onBlur={commitRename}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  commitRename();
                } else if (e.key === "Escape") {
                  setEditing(false);
                }
              }}
              style={{
                ...s.taskCardTitle,
                width: "100%",
                background: "var(--bg-input)",
                border: "1px solid var(--border-strong)",
                borderRadius: 4,
                padding: "1px 4px",
                outline: "none",
                color: "var(--text-primary)",
              }}
            />
          ) : (
            <div
              style={s.taskCardTitle}
              title={onRename ? "Double-click to rename" : undefined}
              onDoubleClick={(e) => {
                e.stopPropagation();
                startRename();
              }}
            >
              {displayTitle.slice(0, 70)}
              {displayTitle.length > 70 ? "…" : ""}
            </div>
          )}
          <div style={s.taskCardSub}>
            {t(statusLabelKey(task.status))}
            {aheadBehind && (aheadBehind.ahead > 0 || aheadBehind.behind > 0) && (
              <span style={{ display: "inline-flex", gap: 6, marginLeft: 6, fontVariantNumeric: "tabular-nums" }}>
                {aheadBehind.ahead > 0 && (
                  <span style={{ color: "var(--text-muted)" }}>↑{aheadBehind.ahead}</span>
                )}
                {aheadBehind.behind > 0 && (
                  <span style={{ color: "var(--text-muted)" }}>↓{aheadBehind.behind}</span>
                )}
              </span>
            )}
            {task.status === "done" &&
              task.worktreePath &&
              task.baseBranch &&
              task.additions !== undefined &&
              task.deletions !== undefined && (
                <span style={s.taskDiffStats}>
                  <span style={s.taskDiffAdditions}>+{task.additions}</span>
                  <span style={s.taskDiffDeletions}>−{task.deletions}</span>
                </span>
              )}
          </div>
        </div>
        {(task.kind ?? "shell") === "shell" ? (
          <span
            title="Terminal"
            style={{
              position: "absolute",
              right: 16,
              top: 11,
              opacity: hov ? 0 : 1,
              color: "var(--text-hint)",
              pointerEvents: "none",
              transition: "opacity 0.12s ease",
              zIndex: 1,
              display: "flex",
            }}
          >
            <TerminalSquare size={14} strokeWidth={2} />
          </span>
        ) : (
          <img
            src={task.agent === "claude" ? claudeLogo : chatgptLogo}
            title={task.agent === "claude" ? "Claude Code" : "Codex"}
            style={{
              ...s.agentBadge,
              position: "absolute",
              right: 16,
              top: 11,
              opacity: hov ? 0 : 1,
              filter: task.agent === "codex" ? "var(--agent-badge-filter)" : "none",
              pointerEvents: "none",
              transition: "opacity 0.12s ease",
              zIndex: 1,
            }}
          />
        )}
        {task.worktreePath && task.worktreeBranch && (
          <span
            title={t("task.worktreeBadge", { branch: task.worktreeBranch })}
            style={{ ...s.worktreeBadge, opacity: hov ? 0 : 1 }}
          >
            <GitBranch size={11} strokeWidth={2.2} />
          </span>
        )}
        <button
          type="button"
          aria-label={task.starred ? t("task.unstar") : t("task.star")}
          title={task.starred ? t("task.unstar") : t("task.star")}
          style={{
            ...s.taskStarBtn,
            opacity: task.starred ? 1 : hov ? 0.7 : 0,
            pointerEvents: task.starred || hov ? "auto" : "none",
            color: task.starred ? "var(--star-fg)" : "var(--text-hint)",
          }}
          onClick={(e) => {
            e.stopPropagation();
            onToggleStar();
          }}
        >
          <Star size={12} strokeWidth={2.2} fill={task.starred ? "currentColor" : "none"} />
        </button>
        {onRunTodo && (
          <button
            type="button"
            aria-label={t("task.runNow")}
            title={t("task.runNow")}
            style={{ ...s.taskPlayBtn, opacity: hov ? 1 : 0.5 }}
            onClick={(e) => {
              e.stopPropagation();
              onRunTodo();
            }}
          >
            <Play size={11} strokeWidth={2} fill="currentColor" />
          </button>
        )}
        <button
          type="button"
          aria-label={t("task.deleteTask")}
          title={t("task.deleteTask")}
          style={{
            ...s.taskDeleteBtn,
            opacity: hov ? 1 : 0,
            pointerEvents: hov ? "auto" : "none",
          }}
          onClick={(e) => {
            e.stopPropagation();
            onDelete();
          }}
        >
          <Trash2 size={12} strokeWidth={2.2} />
        </button>
      </div>
    );
  },
  (prev, next) =>
    prev.task === next.task &&
    prev.selected === next.selected &&
    (prev.onRunTodo !== undefined) === (next.onRunTodo !== undefined),
);
