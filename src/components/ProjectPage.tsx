import { useMemo, useState, useCallback, useEffect, useRef } from "react";
import type {
  Project,
  Task,
  AgentType,
  PermissionMode,
  TaskStatus,
  ThemeMode,
  ThemeVariant,
  TerminalFontSize,
  TaskDisplayWindow,
  FontFamily,
} from "../types";
import { ProjectSidebar } from "./ProjectSidebar";
import { NewTaskView, type NewTaskDraft } from "./NewTaskView";
import { RunningView } from "./RunningView";
import { FileExplorer } from "./FileExplorer";
import { FileSearchDialog } from "./file-explorer/SearchPanel";
import { FileViewer } from "./FileViewer";
import { GitChanges } from "./GitChanges";
import { GitHistory } from "./GitHistory";
import { GitDiffViewer } from "./GitDiffViewer";
import { SettingsDialog } from "./SettingsDialog";
import { RightToolbar } from "./RightToolbar";
import { TodoTaskView } from "./TodoTaskView";
import { ShellTerminalPanel, type ShellTerminalPanelHandle } from "./ShellTerminalPanel";
import { ErrorBoundary } from "./ErrorBoundary";
import { useProjectPanels } from "../hooks/useProjectPanels";
import { useI18n } from "../i18n";
import { APP_PLATFORM } from "../platform";
import {
  isToggleTerminalShortcut,
  isOpenSearchShortcut,
  isToggleSidebarShortcut,
  isQuickOpenShortcut,
  isCommandPaletteShortcut,
} from "../shortcuts";
import { CommandPalette, type PaletteCommand } from "./CommandPalette";
import s from "../styles";

export function ProjectPage({
  project,
  visible = true,
  allProjects = [],
  otherProjects = [],
  tasks,
  getTaskRestoreState,
  taskRunCounts,
  selectedTaskId,
  isNewTask,
  onNewTask,
  onSelectTask,
  onDeleteTask,
  onToggleTaskStar,
  onRenameTask,
  onGenerateTaskName,
  onSubmitTask,
  onRunTodoTask,
  onUpdateTodo,
  onCancelTask,
  onResumeTask,
  onMergeWorktree,
  onDiscardWorktree,
  onReconnectTask,
  onMarkTaskDone,
  onInput,
  onResize,
  onRegisterTerminal,
  onTerminalReady,
  onSnapshot,
  onBack,
  onSwitchProject,
  expandedProjectIds,
  onToggleProjectExpanded,
  onOpenTaskInProject,
  onNewTerminalInProject,
  onNewTaskInProject,
  onRenameProject,
  onRemoveProject,
  onOpen,
  themeVariant,
  themeMode,
  systemPrefersDark,
  onThemeModeChange,
  onToggleTheme,
  terminalFontSize,
  onTerminalFontSizeChange,
  taskDisplayWindow,
  onTaskDisplayWindowChange,
  attentionBadge,
  onAttentionBadgeChange,
  filesPanelDefaultOpen,
  onFilesPanelDefaultOpenChange,
  uiFontFamily,
  onUiFontFamilyChange,
  monoFontFamily,
  onMonoFontFamilyChange,
  hubMode = false,
  onExitSkillHub,
}: {
  project: Project;
  visible?: boolean;
  allProjects?: Project[];
  otherProjects?: Project[];
  tasks: Task[];
  getTaskRestoreState: (taskId: string) => { initialData?: string; initialSnapshot?: string };
  taskRunCounts: Record<string, number>;
  selectedTaskId: string | null;
  isNewTask: boolean;
  onNewTask: () => void;
  onSelectTask: (id: string) => void;
  onDeleteTask: (id: string) => void;
  onDeleteAllTasks: () => void;
  onToggleTaskStar: (id: string) => void;
  onRenameTask: (id: string, name: string) => void;
  onGenerateTaskName: (id: string) => Promise<void>;
  onSubmitTask: (t: {
    kind: "shell" | "agent";
    prompt: string;
    agent: AgentType;
    permissionMode: PermissionMode;
    images: string[];
    texts: string[];
    immediate: boolean;
    launchMode: "local" | "worktree";
    baseBranch: string;
  }) => void;
  onRunTodoTask: (task: Task) => void;
  onUpdateTodo: (
    taskId: string,
    updates: { prompt: string; agent: AgentType; permissionMode: PermissionMode },
  ) => void;
  onCancelTask: (id: string) => void;
  onResumeTask: (id: string) => void;
  onMergeWorktree: (id: string) => Promise<void>;
  onDiscardWorktree: (id: string) => Promise<void>;
  onReconnectTask: (id: string) => void;
  onMarkTaskDone: (id: string) => void;
  onInput: (taskId: string, data: string) => void;
  onResize: (taskId: string, cols: number, rows: number) => void;
  onRegisterTerminal: (
    taskId: string,
    writeFn: ((data: string, callback?: () => void) => void) | null,
  ) => number;
  onTerminalReady: (taskId: string, generation: number) => void;
  onSnapshot: (taskId: string, snapshot: string) => void;
  onBack: () => void;
  onSwitchProject: (project: Project) => void;
  expandedProjectIds: Set<string>;
  onToggleProjectExpanded: (projectId: string) => void;
  onOpenTaskInProject: (projectId: string, taskId: string) => void;
  onNewTerminalInProject: (projectId: string) => void;
  onNewTaskInProject: (projectId: string) => void;
  onRenameProject: (projectId: string, name: string) => void;
  onRemoveProject: (projectId: string) => void;
  onOpen: (anchor?: { x: number; y: number }) => void;
  themeVariant: ThemeVariant;
  themeMode: ThemeMode;
  systemPrefersDark: boolean;
  onThemeModeChange: (mode: ThemeMode) => void;
  onToggleTheme: () => void;
  terminalFontSize: TerminalFontSize;
  onTerminalFontSizeChange: (size: TerminalFontSize) => void;
  taskDisplayWindow: TaskDisplayWindow;
  onTaskDisplayWindowChange: (window: TaskDisplayWindow) => void;
  attentionBadge: boolean;
  onAttentionBadgeChange: (enabled: boolean) => void;
  filesPanelDefaultOpen: boolean;
  onFilesPanelDefaultOpenChange: (enabled: boolean) => void;
  uiFontFamily: FontFamily;
  onUiFontFamilyChange: (family: FontFamily) => void;
  monoFontFamily: FontFamily;
  onMonoFontFamilyChange: (family: FontFamily) => void;
  hubMode?: boolean;
  onExitSkillHub?: () => void;
}) {
  const { t } = useI18n();
  const {
    rightPanel,
    openFiles,
    activeFilePath,
    openDiff,
    rightPanelWidth,
    terminalHeight,
    setOpenDiff,
    openRightPanel,
    handleTogglePanel,
    handleFileSelect,
    handleFileTabSelect,
    handleFileTabClose,
    handleCloseOtherFileTabs,
    handleCloseTabsToRight,
    handleCloseTabsToLeft,
    handleCloseAllFileTabs,
    handleDiffFileSelect,
    handleCommitSelect,
    handleCommitFileClick,
    clearFileAndDiff,
    handleRightResizeStart,
    handleTerminalResizeStart,
  } = useProjectPanels(filesPanelDefaultOpen);

  const [showShellTerminal, setShowShellTerminal] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [showFileSearch, setShowFileSearch] = useState(false);
  const [fileSearchMode, setFileSearchMode] = useState<"files" | "content">("files");
  const [showCommandPalette, setShowCommandPalette] = useState(false);
  const [taskPanelCollapsed, setTaskPanelCollapsed] = useState(false);
  const [mountedTaskIds, setMountedTaskIds] = useState<Set<string>>(() => new Set());
  const shellRef = useRef<ShellTerminalPanelHandle>(null);
  const pendingCmdRef = useRef<string | null>(null);
  const prevHadDiffRef = useRef(false);
  const newTaskDraftRef = useRef<NewTaskDraft | null>(null);
  const handleCacheNewTaskDraft = useCallback((draft: NewTaskDraft | null) => {
    newTaskDraftRef.current = draft;
  }, []);

  // VS Code-style global shortcuts (only the currently active project responds):
  //   Ctrl/Cmd+`         toggle the bottom terminal
  //   Cmd/Ctrl+Shift+F   open "search in files"
  //   Cmd/Ctrl+B         toggle the right-side file panel
  //   Cmd/Ctrl+P         quick-open file
  //   Cmd/Ctrl+Shift+P   command palette
  // Zoom (Cmd/Ctrl +/-/0) is handled natively by WKWebView (tauri.conf zoomHotkeysEnabled); no JS needed.
  useEffect(() => {
    if (!visible) return;
    function onKeyDown(e: KeyboardEvent) {
      if (isToggleTerminalShortcut(e)) {
        e.preventDefault();
        setShowShellTerminal((v) => !v);
      } else if (isCommandPaletteShortcut(e, APP_PLATFORM)) {
        e.preventDefault();
        setShowCommandPalette((v) => !v);
      } else if (isOpenSearchShortcut(e, APP_PLATFORM)) {
        e.preventDefault();
        setFileSearchMode("content");
        setShowFileSearch(true);
      } else if (isQuickOpenShortcut(e, APP_PLATFORM)) {
        e.preventDefault();
        setFileSearchMode("files");
        setShowFileSearch(true);
      } else if (isToggleSidebarShortcut(e, APP_PLATFORM)) {
        e.preventDefault();
        handleTogglePanel("files");
      }
    }
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [visible, handleTogglePanel]);

  const projectTasks = useMemo(
    () => tasks.filter((t) => t.projectId === project.id),
    [tasks, project.id],
  );
  const selectedTask = projectTasks.find((t) => t.id === selectedTaskId) ?? null;
  // cwd for GitChanges/GitHistory: worktree tasks use the worktree path, otherwise the main repo.
  // The main repo's git status can't see uncommitted changes inside the worktree, so we must switch to the worktree cwd to view / stage / commit them.
  const gitContextPath =
    selectedTask?.worktreePath && !selectedTask.worktreeDiscarded
      ? selectedTask.worktreePath
      : project.path;

  const handleSearchFileSelect = useCallback(
    (path: string, name: string) => {
      handleFileSelect(path, name);
      openRightPanel("files");
    },
    [handleFileSelect, openRightPanel],
  );

  // Only mount the xterm instance for the currently selected task; other tasks are serialized via snapshot and unmounted.
  // This keeps only 1 WebGL context alive at a time, avoiding GPU memory accumulation after long runs.
  useEffect(() => {
    if (selectedTaskId && !isNewTask) {
      setMountedTaskIds((prev) => {
        if (prev.size === 1 && prev.has(selectedTaskId)) return prev;
        return new Set([selectedTaskId]);
      });
    }
  }, [selectedTaskId, isNewTask]);

  // When the diff viewer opens/closes, auto-link the task panel's collapsed state, but sync only once
  // at the moment of crossing "no diff → diff" or "diff → no diff". The user manually expanding/collapsing
  // midway, and switching between different diff files (openDiff reference changes but stays truthy),
  // won't be overridden.
  useEffect(() => {
    const hasDiff = Boolean(openDiff);
    if (hasDiff !== prevHadDiffRef.current) {
      setTaskPanelCollapsed(hasDiff);
      prevHadDiffRef.current = hasDiff;
    }
  }, [openDiff]);

  const handleSelectTask = useCallback(
    (id: string) => {
      clearFileAndDiff();
      onSelectTask(id);
    },
    [onSelectTask, clearFileAndDiff],
  );

  const handleRunMakeTarget = useCallback(
    (target: string) => {
      const cmd = `make ${target}\n`;
      if (showShellTerminal && shellRef.current) {
        shellRef.current.sendCommand(cmd);
      } else {
        pendingCmdRef.current = cmd;
        setShowShellTerminal(true);
      }
    },
    [showShellTerminal],
  );

  const handleShellReady = useCallback(() => {
    if (pendingCmdRef.current) {
      shellRef.current?.sendCommand(pendingCmdRef.current);
      pendingCmdRef.current = null;
    }
  }, []);

  const handleNewTask = useCallback(() => {
    clearFileAndDiff();
    onNewTask();
  }, [onNewTask, clearFileAndDiff]);

  const collapseTaskPanelForNewDiff = useCallback(() => {
    if (!openDiff) {
      setTaskPanelCollapsed(true);
    }
  }, [openDiff]);

  const handleDiffFileSelectWithCollapse = useCallback(
    (filePath: string, staged: boolean, label: string) => {
      collapseTaskPanelForNewDiff();
      handleDiffFileSelect(filePath, staged, label);
    },
    [collapseTaskPanelForNewDiff, handleDiffFileSelect],
  );

  const handleCommitSelectWithCollapse = useCallback(
    (hash: string, message: string) => {
      collapseTaskPanelForNewDiff();
      handleCommitSelect(hash, message);
    },
    [collapseTaskPanelForNewDiff, handleCommitSelect],
  );

  const handleCommitFileClickWithCollapse = useCallback(
    (hash: string, filePath: string, label: string) => {
      collapseTaskPanelForNewDiff();
      handleCommitFileClick(hash, filePath, label);
    },
    [collapseTaskPanelForNewDiff, handleCommitFileClick],
  );

  const currentTaskCreatedAt = selectedTask?.createdAt ?? null;

  // Command palette entries (Cmd/Ctrl+Shift+P). The hint is display-only; the actual trigger still goes through each global shortcut.
  const paletteCommands = useMemo<PaletteCommand[]>(() => {
    const mod = APP_PLATFORM === "macos" ? "⌘" : "Ctrl";
    return [
      { id: "new-agent", title: "New Agent Task", keywords: "claude codex run", run: handleNewTask },
      {
        id: "new-terminal",
        title: "New Terminal",
        keywords: "shell",
        run: () => onNewTerminalInProject(project.id),
      },
      {
        id: "toggle-terminal",
        title: "Toggle Terminal Panel",
        hint: `${mod}\``,
        run: () => setShowShellTerminal((v) => !v),
      },
      {
        id: "toggle-files-panel",
        title: "Toggle Files Panel",
        hint: `${mod}B`,
        keywords: "explorer right",
        run: () => handleTogglePanel("files"),
      },
      {
        id: "toggle-sidebar",
        title: "Toggle Sidebar",
        keywords: "left collapse",
        run: () => setTaskPanelCollapsed((v) => !v),
      },
      {
        id: "go-to-file",
        title: "Go to File…",
        hint: `${mod}P`,
        keywords: "quick open find",
        run: () => {
          setFileSearchMode("files");
          setShowFileSearch(true);
        },
      },
      {
        id: "search-in-files",
        title: "Search in Files…",
        hint: `${mod}⇧F`,
        keywords: "grep content",
        run: () => {
          setFileSearchMode("content");
          setShowFileSearch(true);
        },
      },
      { id: "toggle-theme", title: "Toggle Light/Dark Theme", run: onToggleTheme },
      {
        id: "open-settings",
        title: "Open Settings",
        keywords: "preferences config",
        run: () => setShowSettings(true),
      },
    ];
  }, [handleNewTask, onNewTerminalInProject, project.id, onToggleTheme, handleTogglePanel]);

  return (
    <div
      style={{
        ...s.projectBody,
        position: "absolute",
        inset: 0,
        // Inactive projects use display:none rather than visibility:hidden — visibility:hidden
        // still keeps elements in the layout tree, and macOS WKWebView's NSTextInputClient,
        // during a Chinese IME drag-select, scans all RenderText (including emoji/img in inactive
        // project subtrees), triggering a hit-test storm. display:none removes the entire subtree
        // from the layout tree, so the storm's scope is limited to the currently visible project.
        // The xterm buffer still updates in sync under display:none, and on switch-back the
        // ResizeObserver-triggered fit loses no data.
        display: visible ? "flex" : "none",
        pointerEvents: visible ? "auto" : "none",
        zIndex: visible ? 1 : 0,
      }}
    >
      <ProjectSidebar
        projects={hubMode ? [project] : allProjects}
        activeProjectId={project.id}
        allTasks={tasks}
        selectedTaskId={selectedTaskId}
        isNewTask={isNewTask}
        expandedProjectIds={expandedProjectIds}
        onToggleExpanded={onToggleProjectExpanded}
        onSwitchProject={onSwitchProject}
        onSelectActiveTask={handleSelectTask}
        onOpenTaskInProject={onOpenTaskInProject}
        onNewTerminal={onNewTerminalInProject}
        onNewAgent={(pid) => (pid === project.id ? handleNewTask() : onNewTaskInProject(pid))}
        onRenameProject={onRenameProject}
        onRemoveProject={onRemoveProject}
        onDeleteTask={onDeleteTask}
        onToggleTaskStar={onToggleTaskStar}
        onRenameTask={onRenameTask}
        onRunTodo={onRunTodoTask}
        onOpen={onOpen}
        onBack={hubMode ? (onExitSkillHub ?? onBack) : onBack}
        backTitle={hubMode ? t("skill.taskView.back") : undefined}
        singleProjectMode={hubMode}
        taskDisplayWindow={taskDisplayWindow}
        themeVariant={themeVariant}
        themeMode={themeMode}
        systemPrefersDark={systemPrefersDark}
        onThemeModeChange={onThemeModeChange}
        onToggleTheme={onToggleTheme}
        terminalFontSize={terminalFontSize}
        onTerminalFontSizeChange={onTerminalFontSizeChange}
        onTaskDisplayWindowChange={onTaskDisplayWindowChange}
        attentionBadge={attentionBadge}
        onAttentionBadgeChange={onAttentionBadgeChange}
        filesPanelDefaultOpen={filesPanelDefaultOpen}
        onFilesPanelDefaultOpenChange={onFilesPanelDefaultOpenChange}
        uiFontFamily={uiFontFamily}
        onUiFontFamilyChange={onUiFontFamilyChange}
        monoFontFamily={monoFontFamily}
        onMonoFontFamilyChange={onMonoFontFamilyChange}
        collapsed={taskPanelCollapsed}
        onToggleCollapsed={() => setTaskPanelCollapsed((v) => !v)}
      />
      <div style={{ ...s.mainContent, flexDirection: "column" }}>
        <div
          style={{
            flex: 1,
            display: "flex",
            flexDirection: "column",
            overflow: "hidden",
            minHeight: 0,
            position: "relative",
          }}
        >
          {/* Foreground: file viewer, diff, or new-task composer */}
          <ErrorBoundary
            label="Main content area"
            fallback={(error, reset) => (
              <div style={s.errorBoundaryWrap}>
                <div style={s.errorBoundaryIcon}>⚠</div>
                <div style={s.errorBoundaryTitle}>Content area failed to render</div>
                <div style={s.errorBoundaryMessage}>{error.message || "Unknown error"}</div>
                <div style={s.errorBoundaryActions}>
                  <button onClick={reset} style={s.errorBoundaryBtn}>
                    Retry
                  </button>
                  <button
                    onClick={() => {
                      clearFileAndDiff();
                      reset();
                    }}
                    style={s.errorBoundaryBtn}
                  >
                    Back to task view
                  </button>
                </div>
              </div>
            )}
          >
            {openDiff ? (
              openDiff.kind === "file" ? (
                <GitDiffViewer
                  projectPath={gitContextPath}
                  mode="file"
                  filePath={openDiff.filePath}
                  staged={openDiff.staged}
                  title={openDiff.label}
                  onClose={() => setOpenDiff(null)}
                />
              ) : openDiff.kind === "commit-file" ? (
                <GitDiffViewer
                  projectPath={gitContextPath}
                  mode="commit-file"
                  commitHash={openDiff.hash}
                  filePath={openDiff.filePath}
                  title={openDiff.label}
                  onClose={() => setOpenDiff(null)}
                />
              ) : (
                <GitDiffViewer
                  projectPath={gitContextPath}
                  mode="commit"
                  commitHash={openDiff.hash}
                  title={openDiff.message}
                  onClose={() => setOpenDiff(null)}
                />
              )
            ) : openFiles.length > 0 ? (
              <FileViewer
                tabs={openFiles}
                activeFilePath={activeFilePath}
                projectPath={project.path}
                onSelectTab={handleFileTabSelect}
                onCloseTab={handleFileTabClose}
                onCloseOtherTabs={handleCloseOtherFileTabs}
                onCloseTabsToRight={handleCloseTabsToRight}
                onCloseTabsToLeft={handleCloseTabsToLeft}
                onCloseAllTabs={handleCloseAllFileTabs}
                themeVariant={themeVariant}
                onRunMakeTarget={handleRunMakeTarget}
              />
            ) : isNewTask || !selectedTask ? (
              <NewTaskView
                project={project}
                otherProjects={otherProjects}
                onSubmit={onSubmitTask}
                initialDraft={newTaskDraftRef.current}
                onCacheDraft={handleCacheNewTaskDraft}
              />
            ) : selectedTask.status === ("todo" as TaskStatus) ? (
              <TodoTaskView
                task={selectedTask}
                onRunTodo={onRunTodoTask}
                onUpdateTodo={onUpdateTodo}
              />
            ) : null}
          </ErrorBoundary>

          {/* Background terminals */}
          {projectTasks
            .filter((t) => mountedTaskIds.has(t.id))
            .map((task) => {
              const isVisible =
                openFiles.length === 0 &&
                !openDiff &&
                !isNewTask &&
                !!selectedTask &&
                task.id === selectedTaskId &&
                task.status !== "todo";
              // Pure terminal workspace: use the same ShellTerminalPanel as the right side (with its own new/multi-terminal bar).
              if ((task.kind ?? "shell") === "shell") {
                return (
                  <div
                    key={task.id}
                    style={{
                      position: "absolute",
                      inset: 0,
                      visibility: visible && isVisible ? "visible" : "hidden",
                      pointerEvents: visible && isVisible ? "auto" : "none",
                      zIndex: visible && isVisible ? 1 : 0,
                    }}
                  >
                    <ShellTerminalPanel
                      fill
                      projectPath={project.path}
                      projectId={task.id}
                      isActive={visible && isVisible}
                      onClose={() => onDeleteTask(task.id)}
                      themeVariant={themeVariant}
                      terminalFontSize={terminalFontSize}
                      monoFontFamily={monoFontFamily}
                    />
                  </div>
                );
              }
              return (
                <RunningView
                  key={task.id}
                  task={task}
                  projectPath={project.path}
                  runCount={taskRunCounts[task.id] ?? 0}
                  visible={visible && isVisible}
                  projectActive={visible}
                  onCancel={() => onCancelTask(task.id)}
                  onResume={() => onResumeTask(task.id)}
                  onMergeWorktree={() => onMergeWorktree(task.id)}
                  onDiscardWorktree={() => onDiscardWorktree(task.id)}
                  onReconnect={() => onReconnectTask(task.id)}
                  onMarkDone={() => onMarkTaskDone(task.id)}
                  onInput={(data) => onInput(task.id, data)}
                  onResize={(cols, rows) => onResize(task.id, cols, rows)}
                  onRegisterTerminal={(fn) => onRegisterTerminal(task.id, fn)}
                  onTerminalReady={(generation) => onTerminalReady(task.id, generation)}
                  onSnapshot={(snapshot) => onSnapshot(task.id, snapshot)}
                  getRestoreState={() => getTaskRestoreState(task.id)}
                  onRename={(name) => onRenameTask(task.id, name)}
                  onGenerateName={() => onGenerateTaskName(task.id)}
                  themeVariant={themeVariant}
                  terminalFontSize={terminalFontSize}
                  monoFontFamily={monoFontFamily}
                />
              );
            })}
        </div>
        {showShellTerminal && (
          <ShellTerminalPanel
            ref={shellRef}
            projectPath={project.path}
            projectId={project.id}
            isActive={visible}
            onClose={() => setShowShellTerminal(false)}
            themeVariant={themeVariant}
            terminalFontSize={terminalFontSize}
            monoFontFamily={monoFontFamily}
            onReady={handleShellReady}
            height={terminalHeight}
            onResizeStart={handleTerminalResizeStart}
          />
        )}
      </div>

      {rightPanel && (
        <div style={{ position: "relative", display: "flex", flexShrink: 0 }}>
          <div
            onMouseDown={handleRightResizeStart}
            style={{
              position: "absolute",
              left: 0,
              top: 0,
              bottom: 0,
              width: 5,
              cursor: "col-resize",
              zIndex: 10,
            }}
          />
          {rightPanel === "files" && (
            <ErrorBoundary label="File browser">
              <FileExplorer
                projectPath={project.path}
                projectName={project.name}
                onFileSelect={handleFileSelect}
                active={visible}
                width={rightPanelWidth}
              />
            </ErrorBoundary>
          )}
          {rightPanel === "git-changes" && (
            <ErrorBoundary label="Git changes">
              <GitChanges
                projectPath={gitContextPath}
                currentTaskCreatedAt={currentTaskCreatedAt}
                onFileSelect={handleDiffFileSelectWithCollapse}
                width={rightPanelWidth}
              />
            </ErrorBoundary>
          )}
          {rightPanel === "git-history" && (
            <ErrorBoundary label="Git history">
              <GitHistory
                projectPath={gitContextPath}
                onCommitSelect={handleCommitSelectWithCollapse}
                onFileClick={handleCommitFileClickWithCollapse}
                width={rightPanelWidth}
              />
            </ErrorBoundary>
          )}
        </div>
      )}

      <RightToolbar
        activePanel={rightPanel}
        onToggle={handleTogglePanel}
        terminalActive={showShellTerminal}
        onToggleTerminal={() => setShowShellTerminal((v) => !v)}
        onOpenSearch={() => setShowFileSearch(true)}
        onOpenSettings={() => setShowSettings(true)}
      />

      {showFileSearch && (
        <FileSearchDialog
          projectPath={project.path}
          onFileSelect={handleSearchFileSelect}
          onClose={() => setShowFileSearch(false)}
          initialMode={fileSearchMode}
        />
      )}

      {showCommandPalette && (
        <CommandPalette
          commands={paletteCommands}
          onClose={() => setShowCommandPalette(false)}
        />
      )}

      {showSettings && (
        <SettingsDialog projectPath={project.path} onClose={() => setShowSettings(false)} />
      )}
    </div>
  );
}
