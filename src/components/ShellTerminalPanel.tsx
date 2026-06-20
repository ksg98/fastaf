import type React from "react";
import { useCallback, useEffect, useRef, useState, forwardRef, useImperativeHandle } from "react";
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
import { Plus, Terminal as TerminalIcon, Trash2, X, Columns2 } from "lucide-react";
import { useI18n } from "../i18n";
import "@xterm/xterm/css/xterm.css";

interface ShellOutputEvent {
  shell_id: string;
  data: string;
}

export interface ShellTerminalPanelHandle {
  sendCommand: (cmd: string) => void;
}

interface ShellTerminalInstanceHandle {
  sendCommand: (cmd: string) => void;
}

interface ShellSession {
  id: string;
  title: string;
}

interface Props {
  projectPath: string;
  projectId: string;
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

function createShellSession(projectId: string, index: number): ShellSession {
  return {
    id: `shell:${projectId}:${index}:${Date.now()}`,
    title: `Terminal ${index}`,
  };
}

const ShellTerminalInstance = forwardRef<ShellTerminalInstanceHandle, {
  shellId: string;
  projectPath: string;
  isActive: boolean;
  themeVariant: ThemeVariant;
  terminalFontSize: TerminalFontSize;
  monoFontFamily: FontFamily;
  onReady?: () => void;
}>(
  function ShellTerminalInstance(
    { shellId, projectPath, isActive, themeVariant, terminalFontSize, monoFontFamily, onReady },
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
      const webglHandle = loadWebglAddon(term);
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
        webglHandle.dispose();
        disposeScrollbarAutoHide();
        disposeMacWebKitGuard();
        disposeInputFix();
        term.dispose();
        invoke("kill_shell", { shellId }).catch(() => {});
      };
    }, [shellId, projectPath]);

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
    const initialShellRef = useRef<ShellSession | null>(null);
    if (!initialShellRef.current) {
      initialShellRef.current = createShellSession(projectId, 1);
    }

    const nextShellIndexRef = useRef(2);
    const shellRefs = useRef<Record<string, ShellTerminalInstanceHandle | null>>({});
    const [shells, setShells] = useState<ShellSession[]>(() => [initialShellRef.current!]);
    const [activeShellId, setActiveShellId] = useState<string | null>(() => initialShellRef.current!.id);
    // The set of terminals currently shown side by side in the main area (at least 1). The user can check which terminals to show simultaneously in the right column.
    const [shownIds, setShownIds] = useState<Set<string>>(
      () => new Set([initialShellRef.current!.id]),
    );
    const [editingShellId, setEditingShellId] = useState<string | null>(null);
    const [editTitle, setEditTitle] = useState("");

    // Single-select a terminal (plain click): show only it.
    const showOnly = useCallback((shellId: string) => {
      setShownIds(new Set([shellId]));
      setActiveShellId(shellId);
    }, []);

    // Toggle whether a terminal participates in the side-by-side display (split). Removing the last one isn't allowed.
    const toggleShown = useCallback((shellId: string) => {
      setShownIds((prev) => {
        const next = new Set(prev);
        if (next.has(shellId)) {
          if (next.size > 1) next.delete(shellId);
        } else {
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

    useImperativeHandle(
      ref,
      () => ({
        sendCommand: (cmd: string) => {
          const currentShellId = activeShellIdRef.current;
          if (!currentShellId) return;
          shellRefs.current[currentShellId]?.sendCommand(cmd);
        },
      }),
      [],
    );

    const handleAddShell = useCallback(() => {
      const nextShell = createShellSession(projectId, nextShellIndexRef.current++);
      setShells((prev) => [...prev, nextShell]);
      setActiveShellId(nextShell.id);
      setShownIds(new Set([nextShell.id]));
    }, [projectId]);

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
          {/* Terminals area. In split mode every shell tiles side-by-side; otherwise
              only the active shell pane has width (instances stay mounted either way). */}
          <div style={{ flex: 1, minWidth: 0, minHeight: 0, display: "flex" }}>
            {shells.map((shell, i) => {
              const shown = shownIds.has(shell.id);
              const multi = shownIds.size > 1;
              return (
                <div
                  key={shell.id}
                  onMouseDown={() => setActiveShellId(shell.id)}
                  style={{
                    flex: shown ? 1 : 0,
                    width: shown ? undefined : 0,
                    minWidth: 0,
                    minHeight: 0,
                    position: "relative",
                    overflow: "hidden",
                    borderLeft: multi && i > 0 ? "1px solid var(--border-dim)" : "none",
                  }}
                >
                  <ShellTerminalInstance
                    ref={(instance) => {
                      shellRefs.current[shell.id] = instance;
                    }}
                    shellId={shell.id}
                    projectPath={projectPath}
                    isActive={isActive && shown}
                    themeVariant={themeVariant}
                    terminalFontSize={terminalFontSize}
                    monoFontFamily={monoFontFamily}
                    onReady={onReady}
                  />
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
              <button
                onClick={handleAddShell}
                title={t("terminal.newTerminal")}
                style={{
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
                }}
              >
                <Plus size={13} />
              </button>
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
                      height: 28,
                      padding: "0 4px 0 8px",
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
                        title="Double-click to rename"
                        onDoubleClick={(e) => {
                          e.stopPropagation();
                          setEditTitle(shell.title);
                          setEditingShellId(shell.id);
                        }}
                        style={{
                          flex: 1,
                          minWidth: 0,
                          fontSize: 11.5,
                          fontWeight: selected ? 600 : 500,
                          whiteSpace: "nowrap",
                          overflow: "hidden",
                          textOverflow: "ellipsis",
                          color: selected ? "var(--text-primary)" : "var(--text-secondary)",
                        }}
                      >
                        {shell.title}
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
