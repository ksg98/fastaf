import type React from "react";
import { Wordmark } from "./Wordmark";
import { useMemo, useState, useRef, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  ChevronRight,
  ChevronLeft,
  Plus,
  Sparkles,
  PanelLeftClose,
  PanelLeftOpen,
  Moon,
  Sun,
  FolderPlus,
  Pencil,
  FolderOpen,
  Trash2,
  ArrowUpDown,
  Check,
  Layers,
} from "lucide-react";
import type {
  Project,
  Task,
  ThemeMode,
  ThemeVariant,
  TerminalFontSize,
  TaskDisplayWindow,
  FontFamily,
} from "../types";
import { ProjectAvatar } from "./ProjectAvatar";
import { SidebarFooterActions } from "./SidebarFooterActions";
import { TaskListItem } from "./task-panel/TaskListItem";
import { useI18n } from "../i18n";
import s from "../styles";

const DEFAULT_WIDTH = 260;
const MIN_WIDTH = 200;
const MAX_WIDTH = 480;

// ── Workspaces: named groups of projects (frontend-only, localStorage) ────────
interface Workspace {
  id: string;
  name: string;
  projectIds: string[];
}
const WS_EVENT = "fastaf:workspaces-changed";
function loadWorkspaces(): Workspace[] {
  try {
    const v = JSON.parse(localStorage.getItem("fastaf:workspaces") || "[]");
    return Array.isArray(v) ? v : [];
  } catch {
    return [];
  }
}
function loadActiveWorkspace(): string | null {
  return localStorage.getItem("fastaf:activeWorkspace") || null;
}
// Write and broadcast so other (mounted but hidden) sidebar instances stay in sync. Listeners only reload and do not re-broadcast, to avoid loops.
function persistWorkspaces(workspaces: Workspace[]) {
  localStorage.setItem("fastaf:workspaces", JSON.stringify(workspaces));
  window.dispatchEvent(new Event(WS_EVENT));
}
function persistActiveWorkspace(id: string | null) {
  if (id) localStorage.setItem("fastaf:activeWorkspace", id);
  else localStorage.removeItem("fastaf:activeWorkspace");
  window.dispatchEvent(new Event(WS_EVENT));
}

function hasAttention(tasks: Task[]): boolean {
  return tasks.some(
    (t) =>
      t.status === "input_required" || t.status === "detached" || t.status === "interrupted",
  );
}

/**
 * Unified sidebar (Superset style): merges the project switcher bar and the task list into a single column.
 * Each project is a collapsible group; when expanded, it nests that project's sessions/tasks beneath it.
 */
export function ProjectSidebar({
  projects,
  activeProjectId,
  allTasks,
  selectedTaskId,
  isNewTask,
  expandedProjectIds,
  onToggleExpanded,
  onSwitchProject,
  onSelectActiveTask,
  onOpenTaskInProject,
  onNewTerminal,
  onToggleSplit,
  onNewAgent,
  onDeleteTask,
  onToggleTaskStar,
  onRenameTask,
  onRunTodo,
  onRenameProject,
  onRemoveProject,
  onOpen,
  onBack,
  backTitle,
  singleProjectMode = false,
  taskDisplayWindow,
  themeVariant,
  themeMode,
  systemPrefersDark,
  onThemeModeChange,
  onToggleTheme,
  terminalFontSize,
  onTerminalFontSizeChange,
  onTaskDisplayWindowChange,
  attentionBadge,
  onAttentionBadgeChange,
  filesPanelDefaultOpen,
  onFilesPanelDefaultOpenChange,
  uiFontFamily,
  onUiFontFamilyChange,
  monoFontFamily,
  onMonoFontFamilyChange,
  collapsed = false,
  onToggleCollapsed,
}: {
  projects: Project[];
  activeProjectId: string;
  allTasks: Task[];
  selectedTaskId: string | null;
  isNewTask: boolean;
  expandedProjectIds: Set<string>;
  onToggleExpanded: (projectId: string) => void;
  onSwitchProject: (project: Project) => void;
  onSelectActiveTask: (taskId: string) => void;
  onOpenTaskInProject: (projectId: string, taskId: string) => void;
  onNewTerminal: (projectId: string) => void;
  onToggleSplit: () => void;
  onNewAgent: (projectId: string) => void;
  onDeleteTask: (id: string) => void;
  onToggleTaskStar: (id: string) => void;
  onRenameTask: (id: string, name: string) => void;
  onRunTodo: (task: Task) => void;
  onRenameProject: (projectId: string, name: string) => void;
  onRemoveProject: (projectId: string) => void;
  onOpen: (anchor?: { x: number; y: number }) => void;
  onBack: () => void;
  backTitle?: string;
  singleProjectMode?: boolean;
  taskDisplayWindow: TaskDisplayWindow;
  themeVariant: ThemeVariant;
  themeMode: ThemeMode;
  systemPrefersDark: boolean;
  onThemeModeChange: (mode: ThemeMode) => void;
  onToggleTheme: () => void;
  terminalFontSize: TerminalFontSize;
  onTerminalFontSizeChange: (size: TerminalFontSize) => void;
  onTaskDisplayWindowChange: (window: TaskDisplayWindow) => void;
  attentionBadge: boolean;
  onAttentionBadgeChange: (enabled: boolean) => void;
  filesPanelDefaultOpen: boolean;
  onFilesPanelDefaultOpenChange: (enabled: boolean) => void;
  uiFontFamily: FontFamily;
  onUiFontFamilyChange: (family: FontFamily) => void;
  monoFontFamily: FontFamily;
  onMonoFontFamilyChange: (family: FontFamily) => void;
  collapsed?: boolean;
  onToggleCollapsed?: () => void;
}) {
  const { t } = useI18n();
  const isDark = themeVariant === "dark" || themeVariant === "midnight";

  // Sessions per project, in reverse creation-time order.
  const tasksByProject = useMemo(() => {
    const map = new Map<string, Task[]>();
    for (const p of projects) map.set(p.id, []);
    for (const task of allTasks) {
      const arr = map.get(task.projectId);
      if (arr) arr.push(task);
    }
    for (const arr of map.values()) arr.sort((a, b) => b.createdAt - a.createdAt);
    return map;
  }, [projects, allTasks]);

  // ── Resizable width (persisted) ────────────────────────────────────────────
  const [width, setWidth] = useState<number>(() => {
    const saved = Number(localStorage.getItem("fastaf:sidebarWidth"));
    return Number.isFinite(saved) && saved >= MIN_WIDTH && saved <= MAX_WIDTH
      ? saved
      : DEFAULT_WIDTH;
  });
  const resizingRef = useRef(false);
  const startResize = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    resizingRef.current = true;
    const startX = e.clientX;
    const startW = (e.currentTarget.parentElement as HTMLElement).offsetWidth;
    const onMove = (ev: MouseEvent) => {
      if (!resizingRef.current) return;
      const next = Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, startW + (ev.clientX - startX)));
      setWidth(next);
    };
    const onUp = () => {
      resizingRef.current = false;
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
    };
    document.body.style.cursor = "col-resize";
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }, []);
  useEffect(() => {
    localStorage.setItem("fastaf:sidebarWidth", String(width));
  }, [width]);

  // ── Sort mode + manual (drag) order ────────────────────────────────────────
  const [sortMode, setSortMode] = useState<"manual" | "name" | "recent">(
    () => (localStorage.getItem("fastaf:projectSort") as "manual" | "name" | "recent") || "manual",
  );
  const [sortMenu, setSortMenu] = useState<{ x: number; y: number } | null>(null);
  const [manualOrder, setManualOrder] = useState<string[]>(() => {
    try {
      return JSON.parse(localStorage.getItem("fastaf:projectOrder") || "[]");
    } catch {
      return [];
    }
  });
  const [dragId, setDragId] = useState<string | null>(null);

  useEffect(() => {
    localStorage.setItem("fastaf:projectSort", sortMode);
  }, [sortMode]);
  useEffect(() => {
    localStorage.setItem("fastaf:projectOrder", JSON.stringify(manualOrder));
  }, [manualOrder]);

  // ── Workspaces ─────────────────────────────────────────────────────────────
  const [workspaces, setWorkspaces] = useState<Workspace[]>(loadWorkspaces);
  const [activeWorkspaceId, setActiveWorkspaceId] = useState<string | null>(loadActiveWorkspace);
  const [wsMenu, setWsMenu] = useState<{ x: number; y: number } | null>(null);
  const [renamingWsId, setRenamingWsId] = useState<string | null>(null);
  const [wsRenameValue, setWsRenameValue] = useState("");

  // Sync other sidebar instances (reload only, no re-broadcast).
  useEffect(() => {
    const reload = () => {
      setWorkspaces(loadWorkspaces());
      setActiveWorkspaceId(loadActiveWorkspace());
    };
    window.addEventListener(WS_EVENT, reload);
    return () => window.removeEventListener(WS_EVENT, reload);
  }, []);

  const activeWorkspace = workspaces.find((w) => w.id === activeWorkspaceId) ?? null;

  const selectWorkspace = useCallback((id: string | null) => {
    setActiveWorkspaceId(id);
    persistActiveWorkspace(id);
  }, []);
  const createWorkspace = useCallback(
    (firstProjectId?: string) => {
      const id = `ws-${Date.now()}`;
      const name = `Workspace ${workspaces.length + 1}`;
      const next = [...workspaces, { id, name, projectIds: firstProjectId ? [firstProjectId] : [] }];
      setWorkspaces(next);
      persistWorkspaces(next);
      selectWorkspace(id);
      return id;
    },
    [workspaces, selectWorkspace],
  );
  const addProjectToWorkspace = useCallback(
    (wsId: string, projectId: string) => {
      const next = workspaces.map((w) =>
        w.id === wsId && !w.projectIds.includes(projectId)
          ? { ...w, projectIds: [...w.projectIds, projectId] }
          : w,
      );
      setWorkspaces(next);
      persistWorkspaces(next);
    },
    [workspaces],
  );
  const removeProjectFromWorkspace = useCallback(
    (wsId: string, projectId: string) => {
      const next = workspaces.map((w) =>
        w.id === wsId ? { ...w, projectIds: w.projectIds.filter((id) => id !== projectId) } : w,
      );
      setWorkspaces(next);
      persistWorkspaces(next);
    },
    [workspaces],
  );
  const deleteWorkspace = useCallback(
    (wsId: string) => {
      const next = workspaces.filter((w) => w.id !== wsId);
      setWorkspaces(next);
      persistWorkspaces(next);
      if (activeWorkspaceId === wsId) selectWorkspace(null);
    },
    [workspaces, activeWorkspaceId, selectWorkspace],
  );
  const renameWorkspace = useCallback(
    (wsId: string, name: string) => {
      const trimmed = name.trim();
      if (!trimmed) return;
      const next = workspaces.map((w) => (w.id === wsId ? { ...w, name: trimmed } : w));
      setWorkspaces(next);
      persistWorkspaces(next);
    },
    [workspaces],
  );

  const orderedProjects = useMemo(() => {
    // Workspace filter: when a workspace is selected, only show the projects it contains.
    const base = activeWorkspace
      ? projects.filter((p) => activeWorkspace.projectIds.includes(p.id))
      : projects;
    if (sortMode === "name") {
      return [...base].sort((a, b) =>
        a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
      );
    }
    if (sortMode === "recent") {
      return [...base].sort((a, b) => b.lastOpenedAt - a.lastOpenedAt);
    }
    const pos = new Map(manualOrder.map((id, i) => [id, i]));
    return [...base].sort((a, b) => {
      const ai = pos.has(a.id) ? pos.get(a.id)! : Number.MAX_SAFE_INTEGER;
      const bi = pos.has(b.id) ? pos.get(b.id)! : Number.MAX_SAFE_INTEGER;
      return ai !== bi ? ai - bi : Number(a.id) - Number(b.id);
    });
  }, [projects, sortMode, manualOrder, activeWorkspace]);

  // Reorder live as you drag over a project (more reliable than relying on the drop position under WebKit): move the dragged project to the target position.
  const reorderTo = useCallback(
    (draggedId: string, targetId: string) => {
      if (!draggedId || draggedId === targetId) return;
      const ids = orderedProjects.map((p) => p.id);
      const from = ids.indexOf(draggedId);
      const to = ids.indexOf(targetId);
      if (from < 0 || to < 0 || from === to) return;
      ids.splice(from, 1);
      ids.splice(to, 0, draggedId);
      setManualOrder(ids);
      setSortMode("manual");
    },
    [orderedProjects],
  );
  const endDrag = useCallback(() => {
    setDragId(null);
  }, []);

  // ── Project context menu + inline rename ───────────────────────────────────
  const [ctxMenu, setCtxMenu] = useState<{ projectId: string; x: number; y: number } | null>(null);
  const [renamingProjectId, setRenamingProjectId] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");

  const startRenameProject = useCallback((project: Project) => {
    setCtxMenu(null);
    setRenameValue(project.name);
    setRenamingProjectId(project.id);
  }, []);
  const commitRenameProject = useCallback(
    (projectId: string) => {
      onRenameProject(projectId, renameValue);
      setRenamingProjectId(null);
    },
    [onRenameProject, renameValue],
  );
  const revealProject = useCallback((project: Project) => {
    setCtxMenu(null);
    invoke("open_in_system_file_manager", {
      path: project.path,
      projectPath: project.path,
    }).catch(() => {});
  }, []);

  if (collapsed) {
    return (
      <div style={{ ...s.taskPanel, ...s.taskPanelCollapsed }}>
        <button
          type="button"
          style={s.taskPanelExpandBtn}
          onClick={onToggleCollapsed}
          title={t("task.showTasks")}
          aria-label={t("task.showTasks")}
        >
          <PanelLeftOpen size={16} strokeWidth={2} />
        </button>
        <div style={{ ...s.taskPanelCollapsedBody, gap: 8, overflowY: "auto" }}>
          {orderedProjects.map((p) => {
            const attention = hasAttention(tasksByProject.get(p.id) ?? []);
            return (
              <button
                key={p.id}
                type="button"
                draggable
                onDragStart={(e) => {
                  e.dataTransfer.setData("text/plain", p.id);
                  e.dataTransfer.effectAllowed = "move";
                  setDragId(p.id);
                }}
                onDragEnter={() => {
                  if (dragId) reorderTo(dragId, p.id);
                }}
                onDragOver={(e) => {
                  e.preventDefault();
                  e.dataTransfer.dropEffect = "move";
                }}
                onDrop={(e) => {
                  e.preventDefault();
                  endDrag();
                }}
                onDragEnd={endDrag}
                onClick={() => onSwitchProject(p)}
                title={p.name}
                style={{
                  position: "relative",
                  border:
                    p.id === activeProjectId
                      ? "2px solid var(--accent)"
                      : "2px solid transparent",
                  borderRadius: 10,
                  padding: 0,
                  background: "transparent",
                  cursor: "pointer",
                  lineHeight: 0,
                }}
              >
                <ProjectAvatar name={p.name} size={26} />
                {attention && <span style={collapsedDot} aria-hidden />}
              </button>
            );
          })}
          <button
            type="button"
            style={{ ...s.taskPanelCollapsedNewBtn, color: "var(--text-muted)" }}
            onClick={(e) => onOpen({ x: e.clientX, y: e.clientY })}
            title="Add project"
            aria-label="Add project"
          >
            <Plus size={15} strokeWidth={2.4} />
          </button>
        </div>
        <div style={s.taskPanelCollapsedFooter}>
          <button
            type="button"
            style={s.taskPanelCollapsedSmallBtn}
            onClick={onToggleTheme}
            title={isDark ? t("theme.switchToLight") : t("theme.switchToDark")}
            aria-label={isDark ? t("theme.switchToLight") : t("theme.switchToDark")}
          >
            {isDark ? <Sun size={14} strokeWidth={1.8} /> : <Moon size={14} strokeWidth={1.8} />}
          </button>
        </div>
      </div>
    );
  }

  return (
    <div style={{ ...s.taskPanel, width, position: "relative" }}>
      {/* Drag-to-resize handle (right edge) */}
      <div
        onMouseDown={startResize}
        title="Drag to resize"
        style={{
          position: "absolute",
          top: 0,
          right: -3,
          width: 6,
          height: "100%",
          cursor: "col-resize",
          zIndex: 20,
        }}
      />
      {/* Header */}
      <div style={s.panelHeader}>
        <button style={s.backBtn} onClick={onBack} title={backTitle ?? t("task.switchProject")}>
          <ChevronLeft size={15} strokeWidth={2} />
        </button>
        <Wordmark size={16} style={{ ...s.panelProjectName, fontWeight: undefined }} />
        <button
          type="button"
          style={s.panelCollapseBtn}
          title={`Sort projects (${sortMode})`}
          onClick={(e) => setSortMenu({ x: e.clientX, y: e.clientY })}
        >
          <ArrowUpDown size={14} strokeWidth={2} />
        </button>
        <button
          type="button"
          style={s.panelCollapseBtn}
          onClick={onToggleCollapsed}
          title={t("task.hideTasks")}
        >
          <PanelLeftClose size={15} strokeWidth={2} />
        </button>
      </div>

      {/* Workspace switcher */}
      <button
        type="button"
        onClick={(e) => setWsMenu({ x: e.clientX, y: e.clientY })}
        style={{
          display: "flex",
          alignItems: "center",
          gap: 7,
          margin: "6px 8px 2px",
          padding: "6px 9px",
          borderRadius: 8,
          border: "1px solid var(--border-medium)",
          background: "var(--bg-card)",
          color: "var(--text-primary)",
          cursor: "pointer",
          fontSize: 12.5,
          fontWeight: 600,
        }}
      >
        <Layers size={14} strokeWidth={2} style={{ color: "var(--text-muted)", flexShrink: 0 }} />
        <span style={{ flex: 1, textAlign: "left", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {activeWorkspace ? activeWorkspace.name : "All Projects"}
        </span>
        <ChevronRight size={13} style={{ transform: "rotate(90deg)", color: "var(--text-muted)" }} />
      </button>

      {/* Project groups + nested sessions */}
      <div style={{ flex: 1, overflowY: "auto", minHeight: 0, padding: "4px 0" }}>
        {orderedProjects.map((project) => {
          const projTasks = tasksByProject.get(project.id) ?? [];
          const isActive = project.id === activeProjectId;
          const expanded = singleProjectMode || expandedProjectIds.has(project.id);
          const attention = hasAttention(projTasks);
          return (
            <div key={project.id} style={{ marginBottom: 2 }}>
              {/* Group header */}
              <div
                draggable={renamingProjectId !== project.id}
                onDragStart={(e) => {
                  e.dataTransfer.setData("text/plain", project.id);
                  e.dataTransfer.effectAllowed = "move";
                  setDragId(project.id);
                }}
                onDragEnter={() => {
                  if (dragId) reorderTo(dragId, project.id);
                }}
                onDragOver={(e) => {
                  e.preventDefault();
                  e.dataTransfer.dropEffect = "move";
                }}
                onDrop={(e) => {
                  e.preventDefault();
                  endDrag();
                }}
                onDragEnd={endDrag}
                style={{
                  ...groupHeader,
                  background: isActive ? "var(--bg-selected)" : "transparent",
                  opacity: dragId === project.id ? 0.4 : 1,
                }}
                onClick={() => {
                  if (!isActive) onSwitchProject(project);
                  if (!expanded) onToggleExpanded(project.id);
                }}
                onContextMenu={(e) => {
                  e.preventDefault();
                  setCtxMenu({ projectId: project.id, x: e.clientX, y: e.clientY });
                }}
              >
                <button
                  type="button"
                  style={chevronBtn}
                  onClick={(e) => {
                    e.stopPropagation();
                    onToggleExpanded(project.id);
                  }}
                  aria-label={expanded ? "Collapse" : "Expand"}
                >
                  <ChevronRight
                    size={13}
                    strokeWidth={2.4}
                    style={{
                      transform: expanded ? "rotate(90deg)" : "none",
                      transition: "transform 0.12s ease",
                    }}
                  />
                </button>
                <ProjectAvatar name={project.name} size={20} />
                {renamingProjectId === project.id ? (
                  <input
                    autoFocus
                    value={renameValue}
                    onClick={(e) => e.stopPropagation()}
                    onChange={(e) => setRenameValue(e.target.value)}
                    onBlur={() => commitRenameProject(project.id)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault();
                        commitRenameProject(project.id);
                      } else if (e.key === "Escape") {
                        setRenamingProjectId(null);
                      }
                    }}
                    style={{
                      ...groupName,
                      background: "var(--bg-input)",
                      border: "1px solid var(--border-strong)",
                      borderRadius: 4,
                      padding: "1px 4px",
                      outline: "none",
                    }}
                  />
                ) : (
                  <span
                    style={groupName}
                    title="Double-click to rename"
                    onDoubleClick={(e) => {
                      e.stopPropagation();
                      startRenameProject(project);
                    }}
                  >
                    {project.name}
                  </span>
                )}
                {attention && !isActive && <span style={inlineDot} aria-hidden />}
                <span style={groupCount}>{projTasks.length}</span>
                <button
                  type="button"
                  style={groupAddBtn}
                  title="New agent task"
                  aria-label="New agent task"
                  onClick={(e) => {
                    e.stopPropagation();
                    onNewAgent(project.id);
                  }}
                >
                  <Sparkles size={13} strokeWidth={2.2} />
                </button>
                <button
                  type="button"
                  style={groupAddBtn}
                  title="New terminal"
                  aria-label="New terminal"
                  onClick={(e) => {
                    e.stopPropagation();
                    onNewTerminal(project.id);
                  }}
                >
                  <Plus size={13} strokeWidth={2.4} />
                </button>
              </div>

              {/* Nested sessions */}
              {expanded && (
                <div style={{ paddingLeft: 8 }}>
                  {projTasks.length === 0 ? (
                    <div style={emptyHint}>{t("task.tasks")}: 0</div>
                  ) : (
                    projTasks.map((task) => (
                      <TaskListItem
                        key={task.id}
                        task={task}
                        selected={isActive && !isNewTask && selectedTaskId === task.id}
                        onClick={() =>
                          isActive
                            ? onSelectActiveTask(task.id)
                            : onOpenTaskInProject(project.id, task.id)
                        }
                        onDelete={() => onDeleteTask(task.id)}
                        onToggleStar={() => onToggleTaskStar(task.id)}
                        onRename={(name) => onRenameTask(task.id, name)}
                        onRunTodo={task.status === "todo" ? () => onRunTodo(task) : undefined}
                      />
                    ))
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>

      {/* Add project */}
      {!singleProjectMode && (
        <button
          style={addProjectRow}
          onClick={(e) => onOpen({ x: e.clientX, y: e.clientY })}
        >
          <FolderPlus size={14} strokeWidth={2.2} style={{ flexShrink: 0 }} />
          <span style={{ fontSize: 13, fontWeight: 500 }}>Add project</span>
        </button>
      )}

      <div style={s.taskPanelFooter}>
        <SidebarFooterActions
          themeVariant={themeVariant}
          themeMode={themeMode}
          systemPrefersDark={systemPrefersDark}
          onThemeModeChange={onThemeModeChange}
          onToggleTheme={onToggleTheme}
          onToggleSplit={onToggleSplit}
          terminalFontSize={terminalFontSize}
          onTerminalFontSizeChange={onTerminalFontSizeChange}
          taskDisplayWindow={taskDisplayWindow}
          onTaskDisplayWindowChange={onTaskDisplayWindowChange}
          attentionBadge={attentionBadge}
          onAttentionBadgeChange={onAttentionBadgeChange}
          filesPanelDefaultOpen={filesPanelDefaultOpen}
          onFilesPanelDefaultOpenChange={onFilesPanelDefaultOpenChange}
          uiFontFamily={uiFontFamily}
          onUiFontFamilyChange={onUiFontFamilyChange}
          monoFontFamily={monoFontFamily}
          onMonoFontFamilyChange={onMonoFontFamilyChange}
        />
      </div>

      {/* Project context menu */}
      {ctxMenu &&
        (() => {
          const project = projects.find((p) => p.id === ctxMenu.projectId);
          if (!project) return null;
          const item = (
            label: string,
            icon: React.ReactNode,
            onClick: () => void,
            danger = false,
          ) => (
            <button
              type="button"
              style={{ ...s.fileCtxMenuItem, display: "flex", alignItems: "center", gap: 9, color: danger ? "var(--color-error)" : "var(--text-primary)" }}
              onMouseEnter={(e) => {
                e.currentTarget.style.background = danger ? "var(--color-error)" : "var(--accent)";
                e.currentTarget.style.color = danger ? "#fff" : "var(--fg-on-accent)";
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.background = "transparent";
                e.currentTarget.style.color = danger ? "var(--color-error)" : "var(--text-primary)";
              }}
              onClick={onClick}
            >
              {icon}
              {label}
            </button>
          );
          return (
            <>
              <div style={s.fileCtxBackdrop} onPointerDown={() => setCtxMenu(null)} />
              <div style={{ ...s.fileCtxMenu, left: ctxMenu.x, top: ctxMenu.y, maxHeight: "70vh", overflowY: "auto" }}>
                {item("Rename", <Pencil size={14} />, () => startRenameProject(project))}
                {item("Open in Finder", <FolderOpen size={14} />, () => revealProject(project))}
                <div style={s.fileCtxSeparator} />
                {/* Add to / remove from workspaces */}
                {workspaces.map((w) => {
                  const inWs = w.projectIds.includes(project.id);
                  return item(
                    `${inWs ? "Remove from" : "Add to"} ${w.name}`,
                    <Layers size={14} />,
                    () => {
                      setCtxMenu(null);
                      if (inWs) removeProjectFromWorkspace(w.id, project.id);
                      else addProjectToWorkspace(w.id, project.id);
                    },
                  );
                })}
                {item("New workspace with this…", <Plus size={14} />, () => {
                  setCtxMenu(null);
                  createWorkspace(project.id);
                })}
                <div style={s.fileCtxSeparator} />
                {item(
                  "Remove project",
                  <Trash2 size={14} />,
                  () => {
                    setCtxMenu(null);
                    onRemoveProject(project.id);
                  },
                  true,
                )}
              </div>
            </>
          );
        })()}

      {/* Sort menu */}
      {sortMenu && (
        <>
          <div style={s.fileCtxBackdrop} onPointerDown={() => setSortMenu(null)} />
          <div style={{ ...s.fileCtxMenu, left: sortMenu.x, top: sortMenu.y }}>
            {(
              [
                ["manual", "Manual (drag)"],
                ["name", "Name (A–Z)"],
                ["recent", "Recently opened"],
              ] as const
            ).map(([mode, label]) => (
              <button
                key={mode}
                type="button"
                style={{
                  ...s.fileCtxMenuItem,
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                  color: "var(--text-primary)",
                }}
                onMouseEnter={(e) => {
                  e.currentTarget.style.background = "var(--accent)";
                  e.currentTarget.style.color = "var(--fg-on-accent)";
                }}
                onMouseLeave={(e) => {
                  e.currentTarget.style.background = "transparent";
                  e.currentTarget.style.color = "var(--text-primary)";
                }}
                onClick={() => {
                  setSortMode(mode);
                  setSortMenu(null);
                }}
              >
                <span style={{ width: 14, display: "inline-flex" }}>
                  {sortMode === mode && <Check size={13} />}
                </span>
                {label}
              </button>
            ))}
          </div>
        </>
      )}

      {/* Workspace switcher menu */}
      {wsMenu && (
        <>
          <div style={s.fileCtxBackdrop} onPointerDown={() => setWsMenu(null)} />
          <div style={{ ...s.fileCtxMenu, left: wsMenu.x, top: wsMenu.y, minWidth: 220, maxHeight: "70vh", overflowY: "auto" }}>
            <button
              type="button"
              style={{ ...s.fileCtxMenuItem, display: "flex", alignItems: "center", gap: 8, color: "var(--text-primary)" }}
              onMouseEnter={(e) => {
                e.currentTarget.style.background = "var(--accent)";
                e.currentTarget.style.color = "var(--fg-on-accent)";
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.background = "transparent";
                e.currentTarget.style.color = "var(--text-primary)";
              }}
              onClick={() => {
                selectWorkspace(null);
                setWsMenu(null);
              }}
            >
              <span style={{ width: 14, display: "inline-flex" }}>
                {!activeWorkspaceId && <Check size={13} />}
              </span>
              All Projects
            </button>
            {workspaces.length > 0 && <div style={s.fileCtxSeparator} />}
            {workspaces.map((w) => (
              <div
                key={w.id}
                // Drag a project onto a workspace row to add it to that workspace.
                onDragOver={(e) => {
                  if (dragId) {
                    e.preventDefault();
                    e.dataTransfer.dropEffect = "copy";
                  }
                }}
                onDrop={(e) => {
                  if (dragId) {
                    e.preventDefault();
                    addProjectToWorkspace(w.id, dragId);
                    endDrag();
                    setWsMenu(null);
                  }
                }}
                style={{ display: "flex", alignItems: "center" }}
              >
                {renamingWsId === w.id ? (
                  <input
                    autoFocus
                    value={wsRenameValue}
                    onChange={(e) => setWsRenameValue(e.target.value)}
                    onClick={(e) => e.stopPropagation()}
                    onBlur={() => {
                      renameWorkspace(w.id, wsRenameValue);
                      setRenamingWsId(null);
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        renameWorkspace(w.id, wsRenameValue);
                        setRenamingWsId(null);
                      } else if (e.key === "Escape") {
                        setRenamingWsId(null);
                      }
                    }}
                    style={{
                      flex: 1,
                      margin: "2px 6px",
                      fontSize: 13,
                      background: "var(--bg-input)",
                      border: "1px solid var(--border-strong)",
                      borderRadius: 4,
                      padding: "3px 6px",
                      outline: "none",
                      color: "var(--text-primary)",
                    }}
                  />
                ) : (
                  <>
                    <button
                      type="button"
                      style={{ ...s.fileCtxMenuItem, flex: 1, display: "flex", alignItems: "center", gap: 8, color: "var(--text-primary)" }}
                      onMouseEnter={(e) => {
                        e.currentTarget.style.background = "var(--accent)";
                        e.currentTarget.style.color = "var(--fg-on-accent)";
                      }}
                      onMouseLeave={(e) => {
                        e.currentTarget.style.background = "transparent";
                        e.currentTarget.style.color = "var(--text-primary)";
                      }}
                      onDoubleClick={() => {
                        setWsRenameValue(w.name);
                        setRenamingWsId(w.id);
                      }}
                      onClick={() => {
                        selectWorkspace(w.id);
                        setWsMenu(null);
                      }}
                    >
                      <span style={{ width: 14, display: "inline-flex" }}>
                        {activeWorkspaceId === w.id && <Check size={13} />}
                      </span>
                      <span style={{ flex: 1, textAlign: "left", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                        {w.name}
                      </span>
                      <span style={{ color: "var(--text-hint)", fontSize: 11 }}>
                        {w.projectIds.length}
                      </span>
                    </button>
                    <button
                      type="button"
                      title="Delete workspace"
                      onClick={(e) => {
                        e.stopPropagation();
                        deleteWorkspace(w.id);
                      }}
                      style={{
                        background: "none",
                        border: "none",
                        color: "var(--text-hint)",
                        cursor: "pointer",
                        padding: "0 8px",
                        display: "flex",
                        alignItems: "center",
                      }}
                    >
                      <Trash2 size={12} />
                    </button>
                  </>
                )}
              </div>
            ))}
            <div style={s.fileCtxSeparator} />
            <button
              type="button"
              style={{ ...s.fileCtxMenuItem, display: "flex", alignItems: "center", gap: 8, color: "var(--text-primary)" }}
              onMouseEnter={(e) => {
                e.currentTarget.style.background = "var(--accent)";
                e.currentTarget.style.color = "var(--fg-on-accent)";
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.background = "transparent";
                e.currentTarget.style.color = "var(--text-primary)";
              }}
              onClick={() => {
                createWorkspace();
                setWsMenu(null);
              }}
            >
              <Plus size={14} />
              New workspace
            </button>
          </div>
        </>
      )}
    </div>
  );
}

const groupHeader: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 7,
  padding: "5px 10px 5px 6px",
  cursor: "pointer",
  userSelect: "none",
};

const chevronBtn: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  width: 18,
  height: 18,
  border: "none",
  background: "transparent",
  color: "var(--text-muted)",
  cursor: "pointer",
  flexShrink: 0,
  padding: 0,
};

const groupName: React.CSSProperties = {
  flex: 1,
  minWidth: 0,
  fontSize: 13,
  fontWeight: 650,
  color: "var(--text-primary)",
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
};

const groupCount: React.CSSProperties = {
  fontSize: 11,
  fontWeight: 600,
  color: "var(--text-hint)",
  flexShrink: 0,
};

const groupAddBtn: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  width: 20,
  height: 20,
  border: "none",
  borderRadius: 5,
  background: "transparent",
  color: "var(--text-muted)",
  cursor: "pointer",
  flexShrink: 0,
};

const inlineDot: React.CSSProperties = {
  width: 7,
  height: 7,
  borderRadius: "50%",
  background: "var(--color-warning)",
  flexShrink: 0,
};

const collapsedDot: React.CSSProperties = {
  position: "absolute",
  top: -2,
  right: -2,
  width: 8,
  height: 8,
  borderRadius: "50%",
  background: "var(--color-warning)",
  border: "1.5px solid var(--bg-sidebar)",
};

const emptyHint: React.CSSProperties = {
  padding: "4px 12px 8px 18px",
  fontSize: 12,
  color: "var(--text-hint)",
};

const addProjectRow: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  margin: "6px 10px",
  padding: "8px 10px",
  border: "1px dashed var(--border-medium)",
  borderRadius: 8,
  background: "transparent",
  color: "var(--text-secondary)",
  cursor: "pointer",
};
