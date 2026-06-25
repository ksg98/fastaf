import type React from "react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  forwardRef,
  useImperativeHandle,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { attachSmartCopy } from "./terminalCopyHelper";
import { FASTAF_FILE_PATH_MIME, shellQuotePath } from "./file-explorer/dragPath";
import type { TerminalFontSize, FontFamily, ThemeVariant } from "../types";
import {
  applyTerminalTheme,
  initTerminal,
  loadWebglAddon,
  safeFit,
  createSmartWriter,
  themeFor,
  attachMacWebKitTerminalGuard,
  attachTerminalScrollbarAutoHide,
  applyTerminalFontSize,
  applyTerminalFontFamily,
  applyDomCharSizeOverride,
  refreshTerminalDisplay,
} from "./terminalShared";
import { attachLinuxIMEFix, attachMacWebKitShiftInputFix } from "./terminalInputFix";
import {
  Plus,
  Terminal as TerminalIcon,
  Trash2,
  X,
  Columns2,
  Square,
  Grid2x2,
  Grid3x3,
  LayoutGrid,
  Folder,
  Check,
} from "lucide-react";
import * as Popover from "@radix-ui/react-popover";
import { useI18n } from "../i18n";
import "@xterm/xterm/css/xterm.css";

interface ShellOutputEvent {
  shell_id: string;
  data: string;
}

export interface ShellTerminalPanelHandle {
  sendCommand: (cmd: string) => void;
  /** Toggle the terminal area between a single pane and a 2-pane split. */
  toggleSplit: () => void;
}

interface ShellTerminalInstanceHandle {
  sendCommand: (cmd: string) => void;
}

/** Minimal project shape the terminal panel needs to open shells across projects. */
export interface ShellProject {
  id: string;
  name: string;
  path: string;
}

interface ShellSession {
  id: string;
  title: string;
  /** The project this terminal runs in. Terminals can come from different projects. */
  projectId: string;
  projectPath: string;
  projectName: string;
}

interface Props {
  projectPath: string;
  projectId: string;
  /** Display name of the current/default project (used for new terminals opened here). */
  projectName: string;
  /** All projects the user can open a terminal in. Defaults to just the current project. */
  projects?: ShellProject[];
  isActive?: boolean;
  onClose: () => void;
  themeVariant: ThemeVariant;
  terminalFontSize: TerminalFontSize;
  monoFontFamily: FontFamily;
  onReady?: () => void;
  height?: number;
  onResizeStart?: (e: React.MouseEvent) => void;
  /** Fill the parent container (used to treat the whole terminal panel as the main view, rather than a bottom dock). */
  fill?: boolean;
}

/** Max terminals that can tile at once (a 3×3 grid). */
const MAX_GRID_PANES = 9;

/** Preset pane counts offered by the grid selector. */
const GRID_PRESETS: { count: number; Icon: typeof Square }[] = [
  { count: 1, Icon: Square },
  { count: 2, Icon: Columns2 },
  { count: 4, Icon: Grid2x2 },
  { count: 6, Icon: LayoutGrid },
  { count: 9, Icon: Grid3x3 },
];

const GRID_LAYOUT_LS_KEY = "fastaf:terminalGridLayout";

const addShellBtnStyle: React.CSSProperties = {
  width: 20,
  height: 20,
  borderRadius: 4,
  border: "none",
  background: "transparent",
  color: "var(--text-secondary)",
  cursor: "pointer",
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
};

const projectMenuContentStyle: React.CSSProperties = {
  minWidth: 180,
  maxWidth: 280,
  padding: 4,
  background: "var(--bg-panel)",
  border: "1px solid var(--border-medium)",
  borderRadius: 8,
  boxShadow: "var(--shadow-md)",
  zIndex: 50,
};

const projectMenuHeaderStyle: React.CSSProperties = {
  padding: "5px 8px 6px",
  fontSize: 10,
  fontWeight: 700,
  textTransform: "uppercase",
  letterSpacing: 0.5,
  color: "var(--text-hint)",
};

const projectMenuItemStyle: React.CSSProperties = {
  width: "100%",
  display: "flex",
  alignItems: "center",
  gap: 8,
  padding: "6px 8px",
  borderRadius: 6,
  border: "none",
  background: "transparent",
  color: "var(--text-primary)",
  fontSize: 12.5,
  fontWeight: 500,
  cursor: "pointer",
  textAlign: "left",
  outline: "none",
};

/**
 * Map the number of shown terminals to a grid spec. The grid is derived entirely
 * from how many shells are shown, so the preset buttons and the per-terminal
 * "show in split" toggle both feed the same layout.
 *
 * `cols` is the number of column tracks; `spans[i]` is how many tracks the i-th
 * shown pane occupies. Most layouts are uniform (span 1), but odd counts use
 * non-uniform shapes so there are no awkward thin columns:
 *   3 → 2 on top, 1 full-width below      5 → 3 on top, 2 below
 */
function gridSpecFor(count: number): { cols: number; rows: number; spans: number[] } {
  const n = Math.max(1, Math.min(MAX_GRID_PANES, count));
  switch (n) {
    case 1:
      return { cols: 1, rows: 1, spans: [1] };
    case 2:
      return { cols: 2, rows: 1, spans: [1, 1] };
    case 3:
      // 2 on top, 1 spanning the full width below.
      return { cols: 2, rows: 2, spans: [1, 1, 2] };
    case 4:
      return { cols: 2, rows: 2, spans: [1, 1, 1, 1] };
    case 5:
      // 3 on top (each 2 of 6 tracks), 2 below (each 3 of 6 tracks).
      return { cols: 6, rows: 2, spans: [2, 2, 2, 3, 3] };
    case 6:
      return { cols: 3, rows: 2, spans: [1, 1, 1, 1, 1, 1] };
    default:
      // 7, 8, 9 → fill a 3×3 grid left-to-right.
      return { cols: 3, rows: 3, spans: Array(n).fill(1) };
  }
}

/**
 * The next "Terminal N" number for a project: the lowest positive integer not
 * already used by an existing (un-renamed) terminal in that project.
 */
function nextTerminalIndex(shells: ShellSession[], project: ShellProject): number {
  const used = new Set<number>();
  for (const sh of shells) {
    if (sh.projectPath !== project.path) continue;
    const m = /^Terminal (\d+)$/.exec(sh.title);
    if (m) used.add(Number(m[1]));
  }
  let n = 1;
  while (used.has(n)) n++;
  return n;
}

function createShellSession(project: ShellProject, index: number): ShellSession {
  return {
    id: `shell:${project.id}:${index}:${Date.now()}`,
    title: `Terminal ${index}`,
    projectId: project.id,
    projectPath: project.path,
    projectName: project.name,
  };
}

const ShellTerminalInstance = forwardRef<ShellTerminalInstanceHandle, {
  shellId: string;
  projectPath: string;
  isActive: boolean;
  /** Whether this instance is currently tiled in the grid. WebGL is only loaded
   *  for shown panes so a 9-pane grid doesn't exhaust the browser's WebGL context budget. */
  webgl: boolean;
  themeVariant: ThemeVariant;
  terminalFontSize: TerminalFontSize;
  monoFontFamily: FontFamily;
  onReady?: () => void;
}>(
  function ShellTerminalInstance(
    { shellId, projectPath, isActive, webgl, themeVariant, terminalFontSize, monoFontFamily, onReady },
    ref,
  ) {
    const containerRef = useRef<HTMLDivElement>(null);
    const terminalRef = useRef<XTerm | null>(null);
    const fitAddonRef = useRef<FitAddon | null>(null);
    const themeVariantRef = useRef(themeVariant);
    const isActiveRef = useRef(isActive);
    const terminalFontSizeRef = useRef(terminalFontSize);
    const monoFontFamilyRef = useRef(monoFontFamily);
    const onReadyRef = useRef(onReady);
    const lastSizeRef = useRef<{ cols: number; rows: number } | null>(null);
    themeVariantRef.current = themeVariant;
    isActiveRef.current = isActive;
    terminalFontSizeRef.current = terminalFontSize;
    monoFontFamilyRef.current = monoFontFamily;
    onReadyRef.current = onReady;

    useImperativeHandle(
      ref,
      () => ({
        sendCommand: (cmd: string) => {
          invoke("send_input", { taskId: shellId, data: cmd }).catch(console.error);
        },
      }),
      [shellId],
    );

    useEffect(() => {
      if (!containerRef.current) return;
      const container = containerRef.current;
      let cleaned = false;
      let initTimeoutId: number | null = null;
      let readyTimeoutId: number | null = null;

      const { term, fitAddon, whenFontsReady } = initTerminal(
        themeVariantRef.current,
        5000,
        terminalFontSizeRef.current,
        monoFontFamilyRef.current,
      );
      terminalRef.current = term;
      fitAddonRef.current = fitAddon;
      term.open(container);
      // Must attach after term.open(): _charSizeService is only instantiated at open time.
      const disposeCharSizeOverride = applyDomCharSizeOverride(term);
      const disposeScrollbarAutoHide = attachTerminalScrollbarAutoHide(term, container);
      const disposeInputFix = attachMacWebKitShiftInputFix(term);
      const writer = createSmartWriter(term);
      const disposeMacWebKitGuard = attachMacWebKitTerminalGuard({ term, container, writer });

      const fit = () => {
        if (cleaned) return;
        const s = safeFit(fitAddon, term, container);
        if (!s) return;
        const last = lastSizeRef.current;
        if (last && last.cols === s.cols && last.rows === s.rows) return;
        lastSizeRef.current = { cols: s.cols, rows: s.rows };
        invoke("resize_pty", { taskId: shellId, cols: s.cols, rows: s.rows }).catch(() => {});
      };

      // The real cell width may change once the font is ready, so fit once more to keep cols/rows in step.
      whenFontsReady.then(() => {
        if (cleaned) return;
        fit();
      });

      initTimeoutId = window.setTimeout(() => {
        if (cleaned) return;
        fit();
        invoke<void>("open_shell", {
          shellId,
          projectPath,
          cols: term.cols,
          rows: term.rows,
        })
          .then(() => {
            if (cleaned) return;
            readyTimeoutId = window.setTimeout(() => {
              if (!cleaned) {
                onReadyRef.current?.();
              }
            }, 300);
          })
          .catch(console.error);
        if (isActiveRef.current) {
          term.focus();
        }
      }, 50);

      const disposeSmartCopy = attachSmartCopy(term);
      const linuxIME = attachLinuxIMEFix(term, (data) => {
        invoke("send_input", { taskId: shellId, data }).catch(() => {});
      });
      const disposeOnData = { dispose: () => linuxIME.dispose() };

      const resizeObserver = new ResizeObserver(() => {
        setTimeout(() => {
          if (isActiveRef.current) {
            fit();
          }
        }, 50);
      });
      resizeObserver.observe(container);

      const handleVisibilityChange = () => {
        if (document.visibilityState !== "visible" || !terminalRef.current || !isActiveRef.current) return;
        window.requestAnimationFrame(() => {
          fit();
          const t = terminalRef.current;
          if (t) {
            refreshTerminalDisplay(t);
            t.focus();
          }
        });
      };
      document.addEventListener("visibilitychange", handleVisibilityChange);

      let unlisten: (() => void) | null = null;
      listen<ShellOutputEvent>("shell-output", (event) => {
        if (event.payload.shell_id === shellId && terminalRef.current) {
          writer.write(event.payload.data);
        }
      }).then((fn) => {
        if (cleaned) {
          fn();
        } else {
          unlisten = fn;
        }
      });

      return () => {
        cleaned = true;
        if (initTimeoutId !== null) {
          window.clearTimeout(initTimeoutId);
        }
        if (readyTimeoutId !== null) {
          window.clearTimeout(readyTimeoutId);
        }
        unlisten?.();
        disposeSmartCopy();
        disposeOnData.dispose();
        resizeObserver.disconnect();
        document.removeEventListener("visibilitychange", handleVisibilityChange);
        terminalRef.current = null;
        fitAddonRef.current = null;
        disposeCharSizeOverride();
        disposeScrollbarAutoHide();
        disposeMacWebKitGuard();
        disposeInputFix();
        term.dispose();
        invoke("kill_shell", { shellId }).catch(() => {});
      };
    }, [shellId, projectPath]);

    // Load the WebGL renderer only while this pane is tiled. Browsers cap WebGL
    // contexts (~8–16/page), so a full 3×3 grid plus other terminals could exhaust
    // them; non-shown panes fall back to xterm's DOM renderer until shown again.
    useEffect(() => {
      if (!webgl) return;
      const term = terminalRef.current;
      if (!term) return;
      const handle = loadWebglAddon(term);
      return () => handle.dispose();
    }, [webgl]);

    useEffect(() => {
      if (!isActive) return;
      window.requestAnimationFrame(() => {
        if (!fitAddonRef.current || !terminalRef.current || !containerRef.current) return;
        const s = safeFit(fitAddonRef.current, terminalRef.current, containerRef.current);
        if (s) {
          const last = lastSizeRef.current;
          if (!last || last.cols !== s.cols || last.rows !== s.rows) {
            lastSizeRef.current = { cols: s.cols, rows: s.rows };
            invoke("resize_pty", { taskId: shellId, cols: s.cols, rows: s.rows }).catch(() => {});
          }
        }
        refreshTerminalDisplay(terminalRef.current);
        terminalRef.current.focus();
      });
    }, [isActive, shellId]);

    useEffect(() => {
      if (terminalRef.current) {
        applyTerminalTheme(terminalRef.current, themeVariant);
        // After a theme/contrast change, xterm's computed final foreground color changes, but the WebGL
        // atlas still caches glyph textures in the old color; without a refresh you'd see color and glyph mismatches.
        refreshTerminalDisplay(terminalRef.current);
      }
    }, [themeVariant]);

    useEffect(() => {
      if (!terminalRef.current || !fitAddonRef.current || !containerRef.current) return;
      const size = applyTerminalFontSize(
        terminalRef.current,
        fitAddonRef.current,
        terminalFontSize,
        containerRef.current,
      );
      if (!size) return;
      const last = lastSizeRef.current;
      if (last && last.cols === size.cols && last.rows === size.rows) return;
      lastSizeRef.current = { cols: size.cols, rows: size.rows };
      invoke("resize_pty", { taskId: shellId, cols: size.cols, rows: size.rows }).catch(() => {});
    }, [terminalFontSize, shellId]);

    useEffect(() => {
      if (!terminalRef.current || !fitAddonRef.current || !containerRef.current) return;
      const result = applyTerminalFontFamily(
        terminalRef.current,
        fitAddonRef.current,
        monoFontFamily,
        containerRef.current,
      );
      if (!result) return;
      const pushResize = (size: { cols: number; rows: number } | null) => {
        if (!size) return;
        const last = lastSizeRef.current;
        if (last && last.cols === size.cols && last.rows === size.rows) return;
        lastSizeRef.current = { cols: size.cols, rows: size.rows };
        invoke("resize_pty", { taskId: shellId, cols: size.cols, rows: size.rows }).catch(() => {});
      };
      pushResize(result.immediate);
      let cancelled = false;
      result.whenSettled.then((s) => {
        if (cancelled) return;
        pushResize(s);
      });
      return () => {
        cancelled = true;
      };
    }, [monoFontFamily, shellId]);

    return (
      <div
        ref={containerRef}
        className="fastaf-xterm-host fastaf-shell-xterm-host"
        onDragOver={(e) => {
          if (e.dataTransfer.types.includes(FASTAF_FILE_PATH_MIME)) {
            e.preventDefault();
            e.dataTransfer.dropEffect = "copy";
          }
        }}
        onDrop={(e) => {
          const p =
            e.dataTransfer.getData(FASTAF_FILE_PATH_MIME) || e.dataTransfer.getData("text/plain");
          if (!p) return;
          e.preventDefault();
          // Insert the quoted path with a trailing space (VS Code behaviour); the
          // user completes the command and presses Enter themselves.
          invoke("send_input", { taskId: shellId, data: shellQuotePath(p) + " " }).catch(
            console.error,
          );
          terminalRef.current?.focus();
        }}
        style={{
          position: "absolute",
          inset: 0,
          overflow: "hidden",
          padding: "4px 0 16px 6px",
          cursor: "text",
          visibility: isActive ? "visible" : "hidden",
          pointerEvents: isActive ? "auto" : "none",
        }}
      />
    );
  },
);

export const ShellTerminalPanel = forwardRef<ShellTerminalPanelHandle, Props>(
  function ShellTerminalPanel(
    {
      projectPath,
      projectId,
      projectName,
      projects,
      isActive = true,
      onClose,
      themeVariant,
      terminalFontSize,
      monoFontFamily,
      onReady,
      height = 240,
      onResizeStart,
      fill = false,
    },
    ref,
  ) {
    const { t } = useI18n();

    const currentProject: ShellProject = { id: projectId, name: projectName, path: projectPath };

    const initialShellRef = useRef<ShellSession | null>(null);
    if (!initialShellRef.current) {
      initialShellRef.current = createShellSession(
        currentProject,
        nextTerminalIndex([], currentProject),
      );
    }

    const shellRefs = useRef<Record<string, ShellTerminalInstanceHandle | null>>({});
    const [shells, setShells] = useState<ShellSession[]>(() => [initialShellRef.current!]);
    const [activeShellId, setActiveShellId] = useState<string | null>(() => initialShellRef.current!.id);
    // The set of terminals currently shown side by side in the main area (at least 1). The user can check which terminals to show simultaneously in the right column.
    const [shownIds, setShownIds] = useState<Set<string>>(
      () => new Set([initialShellRef.current!.id]),
    );
    const [editingShellId, setEditingShellId] = useState<string | null>(null);
    const [addOpen, setAddOpen] = useState(false);
    const [editTitle, setEditTitle] = useState("");

    // Single-select a terminal (plain click): show only it.
    const showOnly = useCallback((shellId: string) => {
      setShownIds(new Set([shellId]));
      setActiveShellId(shellId);
    }, []);

    // Toggle whether a terminal participates in the grid. Removing the last one
    // isn't allowed, and the grid tiles at most MAX_GRID_PANES at once.
    const toggleShown = useCallback((shellId: string) => {
      setShownIds((prev) => {
        const next = new Set(prev);
        if (next.has(shellId)) {
          if (next.size > 1) next.delete(shellId);
        } else if (next.size < MAX_GRID_PANES) {
          next.add(shellId);
        }
        return next;
      });
      setActiveShellId(shellId);
    }, []);

    const handleRenameShell = useCallback((shellId: string, title: string) => {
      const trimmed = title.trim();
      setShells((prev) =>
        prev.map((sh) => (sh.id === shellId ? { ...sh, title: trimmed || sh.title } : sh)),
      );
    }, []);
    const activeShellIdRef = useRef(activeShellId);
    activeShellIdRef.current = activeShellId;
    // Refs let the imperative handle reach the latest split state / layout fn
    // without rebuilding the handle on every render.
    const shownSizeRef = useRef(shownIds.size);
    shownSizeRef.current = shownIds.size;
    const applyGridLayoutRef = useRef<(count: number) => void>(() => {});

    useImperativeHandle(
      ref,
      () => ({
        sendCommand: (cmd: string) => {
          const currentShellId = activeShellIdRef.current;
          if (!currentShellId) return;
          shellRefs.current[currentShellId]?.sendCommand(cmd);
        },
        toggleSplit: () => {
          applyGridLayoutRef.current(shownSizeRef.current > 1 ? 1 : 2);
        },
      }),
      [],
    );

    const handleAddShell = useCallback(
      (project: ShellProject) => {
        const nextShell = createShellSession(project, nextTerminalIndex(shells, project));
        setShells((prev) => [...prev, nextShell]);
        setActiveShellId(nextShell.id);
        // Grow the grid by adding the new terminal to the tiled set (up to the cap);
        // once full, the terminal is added to the list but not auto-tiled.
        setShownIds((prev) =>
          prev.size < MAX_GRID_PANES ? new Set(prev).add(nextShell.id) : new Set([nextShell.id]),
        );
      },
      [shells],
    );

    // Apply a preset grid: ensure at least `count` terminals exist, then tile the
    // first `count` of them. Backs the header grid selector and is restored on mount.
    const applyGridLayout = useCallback(
      (count: number) => {
        const clamped = Math.max(1, Math.min(MAX_GRID_PANES, count));
        const next = [...shells];
        while (next.length < clamped) {
          next.push(createShellSession(currentProject, nextTerminalIndex(next, currentProject)));
        }
        const shownList = next.slice(0, clamped);
        const shownSet = new Set(shownList.map((sh) => sh.id));
        if (next.length !== shells.length) setShells(next);
        setShownIds(shownSet);
        setActiveShellId((cur) => (cur && shownSet.has(cur) ? cur : shownList[0]!.id));
        try {
          localStorage.setItem(GRID_LAYOUT_LS_KEY, String(clamped));
        } catch {
          /* ignore storage failures */
        }
      },
      // eslint-disable-next-line react-hooks/exhaustive-deps
      [shells, currentProject.id, currentProject.name, currentProject.path],
    );
    applyGridLayoutRef.current = applyGridLayout;

    // Restore the last-used grid preset once, after the initial shell is set up.
    const didRestoreGridRef = useRef(false);
    useEffect(() => {
      if (didRestoreGridRef.current) return;
      didRestoreGridRef.current = true;
      let saved = 1;
      try {
        saved = Number(localStorage.getItem(GRID_LAYOUT_LS_KEY));
      } catch {
        /* ignore storage failures */
      }
      if (Number.isFinite(saved) && saved > 1) {
        applyGridLayout(saved);
      }
    }, [applyGridLayout]);

    const handleCloseShell = useCallback(
      (shellId: string) => {
        const closingIndex = shells.findIndex((shell) => shell.id === shellId);
        if (closingIndex === -1) return;

        const nextShells = shells.filter((shell) => shell.id !== shellId);
        setShells(nextShells);
        delete shellRefs.current[shellId];

        if (nextShells.length === 0) {
          onClose();
          return;
        }

        const fallbackId =
          nextShells[closingIndex]?.id ?? nextShells[closingIndex - 1]?.id ?? nextShells[0]!.id;

        setShownIds((prev) => {
          const next = new Set(prev);
          next.delete(shellId);
          if (next.size === 0) next.add(fallbackId);
          return next;
        });

        if (activeShellId === shellId) {
          setActiveShellId(fallbackId);
        }
      },
      [activeShellId, onClose, shells],
    );

    const gridSpec = gridSpecFor(shownIds.size);
    // Ordered ids of the shown panes, so each can be assigned its column span.
    const shownOrder = shells.filter((sh) => shownIds.has(sh.id)).map((sh) => sh.id);

    // Projects offered in the "add terminal" picker: the current project first, then
    // any others, de-duplicated by path (the canonical project identity — the fill
    // panel passes a task id as projectId, so id-based dedup would double-list it).
    const pickerProjects = useMemo<ShellProject[]>(() => {
      const seen = new Set<string>();
      const list: ShellProject[] = [];
      for (const p of [currentProject, ...(projects ?? [])]) {
        if (p && p.path && !seen.has(p.path)) {
          seen.add(p.path);
          list.push(p);
        }
      }
      return list;
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [currentProject.id, currentProject.name, currentProject.path, projects]);

    // Whether terminals span more than one project (drives showing project labels).
    const multiProject = useMemo(
      () => new Set(shells.map((sh) => sh.projectPath)).size > 1,
      [shells],
    );

    return (
      <div
        style={
          fill
            ? {
                position: "absolute",
                inset: 0,
                display: "flex",
                flexDirection: "column",
                background: themeFor(themeVariant).background,
              }
            : {
                flexShrink: 0,
                height,
                borderTop: "1px solid var(--border-dim)",
                display: "flex",
                flexDirection: "column",
                background: themeFor(themeVariant).background,
              }
        }
      >
        {!fill && onResizeStart && (
          <div
            onMouseDown={onResizeStart}
            style={{
              height: 4,
              flexShrink: 0,
              cursor: "row-resize",
              background: "transparent",
            }}
          />
        )}
        <div
          style={{
            height: 32,
            flexShrink: 0,
            display: "flex",
            alignItems: "center",
            padding: "0 10px 0 14px",
            borderBottom: "1px solid var(--border-dim)",
            background: "var(--bg-sidebar)",
            gap: 8,
          }}
        >
          <span style={{ fontSize: 12, fontWeight: 600, color: "var(--text-primary)", flex: 1 }}>
            {t("terminal.title")}
          </span>
          <span style={{ fontSize: 11, color: "var(--text-muted)" }}>{shells.length}</span>
          <div
            role="group"
            aria-label={t("terminal.splitGrid")}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 1,
              border: "1px solid var(--border-dim)",
              borderRadius: 6,
              padding: 1,
              background: "var(--bg-input)",
            }}
          >
            {GRID_PRESETS.map(({ count, Icon }) => {
              const active = shownIds.size === count;
              return (
                <button
                  key={count}
                  onClick={() => applyGridLayout(count)}
                  title={count === 1 ? t("terminal.gridSingle") : t("terminal.gridPanes", { count })}
                  aria-pressed={active}
                  style={{
                    width: 22,
                    height: 20,
                    borderRadius: 4,
                    border: "none",
                    cursor: "pointer",
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "center",
                    background: active ? "var(--bg-hover)" : "transparent",
                    color: active ? "var(--control-active-fg)" : "var(--text-hint)",
                  }}
                >
                  <Icon size={13} />
                </button>
              );
            })}
          </div>
          <button
            onClick={onClose}
            title={t("terminal.closeTerminals")}
            style={{
              background: "none",
              border: "none",
              cursor: "pointer",
              padding: 3,
              borderRadius: 4,
              display: "flex",
              alignItems: "center",
              color: "var(--text-hint)",
            }}
          >
            <X size={14} />
          </button>
        </div>
        <div style={{ flex: 1, display: "flex", minHeight: 0 }}>
          {/* Terminals area. Shown shells tile in a rows×cols grid derived from how
              many are shown; non-shown instances stay mounted (display:none) so their
              scrollback and PTY survive being toggled out of the grid. */}
          <div
            style={{
              flex: 1,
              minWidth: 0,
              minHeight: 0,
              display: "grid",
              gridTemplateColumns: `repeat(${gridSpec.cols}, minmax(0, 1fr))`,
              gridTemplateRows: `repeat(${gridSpec.rows}, minmax(0, 1fr))`,
              gridAutoFlow: "row dense",
              gap: shownIds.size > 1 ? 1 : 0,
              background: shownIds.size > 1 ? "var(--border-dim)" : undefined,
            }}
          >
            {shells.map((shell) => {
              const shown = shownIds.has(shell.id);
              const active = activeShellId === shell.id;
              const shownIndex = shownOrder.indexOf(shell.id);
              const span = shownIndex >= 0 ? gridSpec.spans[shownIndex] ?? 1 : 1;
              return (
                <div
                  key={shell.id}
                  onMouseDown={() => setActiveShellId(shell.id)}
                  style={{
                    display: shown ? "flex" : "none",
                    flexDirection: "column",
                    gridColumn: `span ${span}`,
                    minWidth: 0,
                    minHeight: 0,
                    position: "relative",
                    overflow: "hidden",
                    background: themeFor(themeVariant).background,
                    outline:
                      active && shownIds.size > 1 ? "1px solid var(--accent)" : "none",
                    outlineOffset: -1,
                  }}
                >
                  {/* Per-pane header: project + terminal name, so it's always clear
                      which project/terminal a pane is. */}
                  <div
                    style={{
                      flexShrink: 0,
                      height: 22,
                      display: "flex",
                      alignItems: "center",
                      gap: 5,
                      padding: "0 8px",
                      background: active ? "var(--bg-hover)" : "var(--bg-sidebar)",
                      borderBottom: "1px solid var(--border-dim)",
                      fontSize: 11,
                      minWidth: 0,
                    }}
                  >
                    <Folder
                      size={11}
                      color="var(--text-hint)"
                      style={{ flexShrink: 0 }}
                    />
                    <span
                      style={{
                        flexShrink: 0,
                        maxWidth: "55%",
                        fontWeight: 600,
                        color: "var(--text-secondary)",
                        whiteSpace: "nowrap",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                      }}
                    >
                      {shell.projectName}
                    </span>
                    <span style={{ color: "var(--text-hint)", flexShrink: 0 }}>·</span>
                    <span
                      style={{
                        minWidth: 0,
                        color: active ? "var(--text-primary)" : "var(--text-secondary)",
                        whiteSpace: "nowrap",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                      }}
                    >
                      {shell.title}
                    </span>
                  </div>
                  <div style={{ flex: 1, position: "relative", minHeight: 0 }}>
                    <ShellTerminalInstance
                      ref={(instance) => {
                        shellRefs.current[shell.id] = instance;
                      }}
                      shellId={shell.id}
                      projectPath={shell.projectPath}
                      isActive={isActive && shown}
                      webgl={shown}
                      themeVariant={themeVariant}
                      terminalFontSize={terminalFontSize}
                      monoFontFamily={monoFontFamily}
                      onReady={onReady}
                    />
                  </div>
                </div>
              );
            })}
          </div>
          <div
            style={{
              width: 104,
              flexShrink: 0,
              borderLeft: "1px solid var(--border-dim)",
              background: "var(--bg-sidebar)",
              display: "flex",
              flexDirection: "column",
              minHeight: 0,
            }}
          >
            <div
              style={{
                height: 28,
                flexShrink: 0,
                display: "flex",
                alignItems: "center",
                justifyContent: "flex-end",
                gap: 2,
                padding: "0 4px",
                borderBottom: "1px solid var(--border-dim)",
              }}
            >
              {pickerProjects.length <= 1 ? (
                <button
                  onClick={() => handleAddShell(currentProject)}
                  title={t("terminal.newTerminal")}
                  style={addShellBtnStyle}
                >
                  <Plus size={13} />
                </button>
              ) : (
                <Popover.Root open={addOpen} onOpenChange={setAddOpen}>
                  <Popover.Trigger asChild>
                    <button
                      title={t("terminal.newTerminal")}
                      aria-label={t("terminal.chooseProject")}
                      style={addShellBtnStyle}
                    >
                      <Plus size={13} />
                    </button>
                  </Popover.Trigger>
                  <Popover.Portal>
                    <Popover.Content
                      align="end"
                      sideOffset={6}
                      style={projectMenuContentStyle}
                    >
                      <div style={projectMenuHeaderStyle}>{t("terminal.chooseProject")}</div>
                      {pickerProjects.map((p) => {
                        const isCurrent = p.path === currentProject.path;
                        return (
                          <button
                            key={p.path}
                            style={projectMenuItemStyle}
                            onClick={() => {
                              handleAddShell(p);
                              setAddOpen(false);
                            }}
                            onMouseEnter={(e) => {
                              e.currentTarget.style.background = "var(--bg-hover)";
                            }}
                            onMouseLeave={(e) => {
                              e.currentTarget.style.background = "transparent";
                            }}
                          >
                            <Folder
                              size={13}
                              color="var(--text-muted)"
                              style={{ flexShrink: 0 }}
                            />
                            <span
                              style={{
                                flex: 1,
                                minWidth: 0,
                                whiteSpace: "nowrap",
                                overflow: "hidden",
                                textOverflow: "ellipsis",
                              }}
                            >
                              {p.name}
                            </span>
                            {isCurrent && (
                              <Check
                                size={13}
                                color="var(--accent)"
                                style={{ flexShrink: 0 }}
                              />
                            )}
                          </button>
                        );
                      })}
                    </Popover.Content>
                  </Popover.Portal>
                </Popover.Root>
              )}
            </div>
            <div style={{ flex: 1, overflowY: "auto", padding: "4px 0" }}>
              {shells.map((shell) => {
                const selected = activeShellId === shell.id;
                const inSplit = shownIds.has(shell.id);
                const multi = shownIds.size > 1;
                return (
                  <div
                    key={shell.id}
                    onClick={() => showOnly(shell.id)}
                    style={{
                      minHeight: 28,
                      padding: "3px 4px 3px 8px",
                      borderLeft: selected
                        ? "2px solid var(--control-active-fg)"
                        : inSplit && multi
                          ? "2px solid var(--accent)"
                          : "2px solid transparent",
                      background:
                        selected || (inSplit && multi) ? "var(--bg-hover)" : "transparent",
                      color: "var(--text-primary)",
                      cursor: "pointer",
                      display: "flex",
                      alignItems: "center",
                      gap: 6,
                    }}
                  >
                    <TerminalIcon
                      size={13}
                      color={selected ? "var(--control-active-fg)" : "var(--text-hint)"}
                    />
                    {editingShellId === shell.id ? (
                      <input
                        autoFocus
                        value={editTitle}
                        onClick={(e) => e.stopPropagation()}
                        onChange={(e) => setEditTitle(e.target.value)}
                        onBlur={() => {
                          handleRenameShell(shell.id, editTitle);
                          setEditingShellId(null);
                        }}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") {
                            handleRenameShell(shell.id, editTitle);
                            setEditingShellId(null);
                          } else if (e.key === "Escape") {
                            setEditingShellId(null);
                          }
                        }}
                        style={{
                          flex: 1,
                          minWidth: 0,
                          fontSize: 11.5,
                          fontWeight: 600,
                          color: "var(--text-primary)",
                          background: "var(--bg-input)",
                          border: "1px solid var(--border-strong)",
                          borderRadius: 4,
                          padding: "1px 4px",
                          outline: "none",
                        }}
                      />
                    ) : (
                      <div
                        title={
                          multiProject
                            ? `${shell.projectName} · ${shell.title} (double-click to rename)`
                            : "Double-click to rename"
                        }
                        onDoubleClick={(e) => {
                          e.stopPropagation();
                          setEditTitle(shell.title);
                          setEditingShellId(shell.id);
                        }}
                        style={{
                          flex: 1,
                          minWidth: 0,
                          display: "flex",
                          flexDirection: "column",
                          justifyContent: "center",
                          gap: 1,
                        }}
                      >
                        {multiProject && (
                          <span
                            style={{
                              fontSize: 9.5,
                              fontWeight: 600,
                              whiteSpace: "nowrap",
                              overflow: "hidden",
                              textOverflow: "ellipsis",
                              color: "var(--text-hint)",
                            }}
                          >
                            {shell.projectName}
                          </span>
                        )}
                        <span
                          style={{
                            fontSize: 11.5,
                            fontWeight: selected ? 600 : 500,
                            whiteSpace: "nowrap",
                            overflow: "hidden",
                            textOverflow: "ellipsis",
                            color: selected ? "var(--text-primary)" : "var(--text-secondary)",
                          }}
                        >
                          {shell.title}
                        </span>
                      </div>
                    )}
                    <button
                      onClick={(event) => {
                        event.stopPropagation();
                        toggleShown(shell.id);
                      }}
                      title={inSplit && multi ? "Remove from split view" : "Show in split view"}
                      aria-pressed={inSplit && multi}
                      style={{
                        background: "none",
                        border: "none",
                        color: inSplit && multi ? "var(--accent)" : "var(--text-hint)",
                        display: "flex",
                        alignItems: "center",
                        justifyContent: "center",
                        padding: 1,
                        borderRadius: 4,
                        cursor: "pointer",
                        flexShrink: 0,
                      }}
                    >
                      <Columns2 size={12} />
                    </button>
                    <button
                      onClick={(event) => {
                        event.stopPropagation();
                        handleCloseShell(shell.id);
                      }}
                      title={t("terminal.closeShell", { title: shell.title })}
                      style={{
                        background: "none",
                        border: "none",
                        color: "var(--text-hint)",
                        display: "flex",
                        alignItems: "center",
                        justifyContent: "center",
                        padding: 1,
                        borderRadius: 4,
                        cursor: "pointer",
                        flexShrink: 0,
                      }}
                    >
                      <Trash2 size={12} />
                    </button>
                  </div>
                );
              })}
            </div>
          </div>
        </div>
      </div>
    );
  },
);
