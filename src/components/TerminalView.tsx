import { useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SerializeAddon } from "@xterm/addon-serialize";
import { attachSmartCopy } from "./terminalCopyHelper";
import { FASTAF_FILE_PATH_MIME, shellQuotePath } from "./file-explorer/dragPath";
import {
  DEFAULT_SHIFT_ENTER_NEWLINE,
  matchesTerminalNewline,
  normalizeShiftEnterNewline,
  TERMINAL_NEWLINE_SEQUENCE,
} from "../shortcuts";
import type { TerminalFontSize, FontFamily, ThemeVariant } from "../types";
import {
  themeFor,
  minimumContrastRatioFor,
  initTerminal,
  loadWebglAddon,
  safeFit,
  createSmartWriter,
  attachMacWebKitTerminalGuard,
  attachTerminalScrollbarAutoHide,
  applyTerminalFontSize,
  applyTerminalFontFamily,
  applyDomCharSizeOverride,
  refreshTerminalDisplay,
} from "./terminalShared";
import { attachLinuxIMEFix, attachMacWebKitShiftInputFix } from "./terminalInputFix";
import "@xterm/xterm/css/xterm.css";

function themeForAgentTerminal(variant: ThemeVariant, container: HTMLElement) {
  const theme = themeFor(variant);
  const background = window.getComputedStyle(container).getPropertyValue("--bg-panel").trim();
  return background ? { ...theme, background } : theme;
}

function applyAgentTerminalTheme(
  term: Terminal,
  variant: ThemeVariant,
  container: HTMLElement,
): void {
  term.options.theme = themeForAgentTerminal(variant, container);
  term.options.minimumContrastRatio = minimumContrastRatioFor(variant);
}

interface TerminalViewProps {
  onInput: (data: string) => void;
  onResize: (cols: number, rows: number) => void;
  onRegisterTerminal: (
    writeFn: ((data: string, callback?: () => void) => void) | null,
  ) => number;
  onReady?: (generation: number) => void;
  themeVariant: ThemeVariant;
  terminalFontSize: TerminalFontSize;
  monoFontFamily: FontFamily;
  isActive?: boolean;
  initialData?: string;
  initialSnapshot?: string;
  onSnapshot?: (snapshot: string) => void;
}

export function TerminalView({
  onInput,
  onResize,
  onRegisterTerminal,
  onReady,
  themeVariant,
  terminalFontSize,
  monoFontFamily,
  isActive = true,
  initialData,
  initialSnapshot,
  onSnapshot,
}: TerminalViewProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const onInputRef = useRef(onInput);
  const onResizeRef = useRef(onResize);
  const onRegisterRef = useRef(onRegisterTerminal);
  const onReadyRef = useRef(onReady);
  const onSnapshotRef = useRef(onSnapshot);
  const lastSizeRef = useRef<{ cols: number; rows: number } | null>(null);
  const shiftEnterNewlineRef = useRef<boolean>(DEFAULT_SHIFT_ENTER_NEWLINE);
  onReadyRef.current = onReady;
  onSnapshotRef.current = onSnapshot;

  // Keep refs current on every render
  onInputRef.current = onInput;
  onResizeRef.current = onResize;
  onRegisterRef.current = onRegisterTerminal;

  // Only call back when cols/rows actually change; otherwise it triggers resize_pty → SIGWINCH →
  // a fullscreen repaint of the downstream TUI (Claude Code / Codex), causing a redundant redraw on every switch-back.
  const notifyResize = useCallback((cols: number, rows: number) => {
    const last = lastSizeRef.current;
    if (last && last.cols === cols && last.rows === rows) return;
    lastSizeRef.current = { cols, rows };
    onResizeRef.current(cols, rows);
  }, []);

  useEffect(() => {
    if (!containerRef.current) return;
    const container = containerRef.current;

    const { term, fitAddon, whenFontsReady } = initTerminal(
      themeVariant,
      1000,
      terminalFontSize,
      monoFontFamily,
    );
    applyAgentTerminalTheme(term, themeVariant, container);
    terminalRef.current = term;
    fitAddonRef.current = fitAddon;
    let disposed = false;

    const serializeAddon = new SerializeAddon();
    term.loadAddon(serializeAddon);
    term.open(container);
    // Must attach after term.open(): _charSizeService is only instantiated at open time.
    const disposeCharSizeOverride = applyDomCharSizeOverride(term);
    const disposeScrollbarAutoHide = attachTerminalScrollbarAutoHide(term, container);
    const disposeInputFix = attachMacWebKitShiftInputFix(term);
    const webglHandle = loadWebglAddon(term);

    const size = safeFit(fitAddon, term, container);
    if (size) notifyResize(size.cols, size.rows);

    // The real cell width may change once the font is ready, so fit once more to keep cols/rows in step.
    whenFontsReady.then(() => {
      if (disposed) return;
      const s = safeFit(fitAddon, term, container);
      if (s) notifyResize(s.cols, s.rows);
    });

    const focusTerminal = () => {
      window.requestAnimationFrame(() => {
        term.focus();
      });
    };

    const writer = createSmartWriter(term);
    const disposeMacWebKitGuard = attachMacWebKitTerminalGuard({ term, container, writer });

    const terminalGeneration = onRegisterRef.current(writer.write);

    const completeRestore = () => {
      onReadyRef.current?.(terminalGeneration);
      focusTerminal();
    };

    window.requestAnimationFrame(() => {
      const s = safeFit(fitAddon, term, container);
      if (s) notifyResize(s.cols, s.rows);
      if (initialSnapshot) {
        term.write(initialSnapshot, () => {
          if (initialData) {
            term.write(initialData, completeRestore);
            return;
          }
          completeRestore();
        });
        return;
      }
      if (initialData) {
        term.write(initialData, completeRestore);
        return;
      }
      completeRestore();
    });

    const disposeSmartCopy = attachSmartCopy(term, {
      matchesNewline: (e) => matchesTerminalNewline(e, shiftEnterNewlineRef.current),
      onNewline: () => onInputRef.current(TERMINAL_NEWLINE_SEQUENCE),
    });
    const linuxIME = attachLinuxIMEFix(term, (data) => onInputRef.current(data));
    const disposeOnData = { dispose: () => linuxIME.dispose() };

    const handlePointerDown = (e: PointerEvent) => {
      if (e.button === 0) {
        focusTerminal();
      }
    };
    const handleVisibilityChange = () => {
      if (document.visibilityState !== "visible") return;
      window.requestAnimationFrame(() => {
        const s = safeFit(fitAddon, term, container);
        if (s) notifyResize(s.cols, s.rows);
        refreshTerminalDisplay(term);
        term.focus();
      });
    };

    container.addEventListener("pointerdown", handlePointerDown as EventListener);
    document.addEventListener("visibilitychange", handleVisibilityChange);

    let resizeTimer: ReturnType<typeof setTimeout> | null = null;
    const resizeObserver = new ResizeObserver(() => {
      if (resizeTimer) clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => {
        const s = safeFit(fitAddon, term, container);
        if (s) notifyResize(s.cols, s.rows);
      }, 50);
    });
    resizeObserver.observe(container);

    return () => {
      disposed = true;
      try {
        const snapshot = serializeAddon.serialize();
        if (snapshot) onSnapshotRef.current?.(snapshot);
      } catch {
        /* ignore */
      }
      onRegisterRef.current(null);
      fitAddonRef.current = null;
      disposeCharSizeOverride();
      webglHandle.dispose();
      disposeScrollbarAutoHide();
      disposeMacWebKitGuard();
      disposeInputFix();
      disposeSmartCopy();
      disposeOnData.dispose();
      if (resizeTimer) clearTimeout(resizeTimer);
      resizeObserver.disconnect();
      container.removeEventListener("pointerdown", handlePointerDown as EventListener);
      document.removeEventListener("visibilitychange", handleVisibilityChange);
      terminalRef.current = null;
      term.dispose();
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // Keep the configured "insert newline" combo in sync with app settings.
  // Mirrors NewTaskView: load once, then react to the global settings event.
  useEffect(() => {
    function loadNewlineShortcut() {
      invoke<{ terminal_shift_enter_newline?: unknown }>("load_app_settings")
        .then((settings) => {
          shiftEnterNewlineRef.current = normalizeShiftEnterNewline(
            settings.terminal_shift_enter_newline,
          );
        })
        .catch(() => {
          shiftEnterNewlineRef.current = DEFAULT_SHIFT_ENTER_NEWLINE;
        });
    }
    loadNewlineShortcut();
    window.addEventListener("fastaf:app-settings-changed", loadNewlineShortcut);
    return () => window.removeEventListener("fastaf:app-settings-changed", loadNewlineShortcut);
  }, []);

  useEffect(() => {
    if (!isActive) return;
    window.requestAnimationFrame(() => {
      if (!fitAddonRef.current || !terminalRef.current || !containerRef.current) return;
      const s = safeFit(fitAddonRef.current, terminalRef.current, containerRef.current);
      if (s) notifyResize(s.cols, s.rows);
      refreshTerminalDisplay(terminalRef.current);
      terminalRef.current.focus();
    });
  }, [isActive, notifyResize]);

  useEffect(() => {
    if (terminalRef.current) {
      terminalRef.current.options.cursorBlink = isActive;
    }
  }, [isActive]);

  useEffect(() => {
    if (!terminalRef.current || !containerRef.current) return;
    applyAgentTerminalTheme(terminalRef.current, themeVariant, containerRef.current);
    // After a theme/contrast change, xterm's computed final foreground color changes, but the WebGL
    // atlas still caches glyph textures in the old color; without a refresh you'd see color and glyph mismatches.
    refreshTerminalDisplay(terminalRef.current);
  }, [themeVariant]);

  useEffect(() => {
    if (!terminalRef.current || !fitAddonRef.current || !containerRef.current) return;
    const size = applyTerminalFontSize(
      terminalRef.current,
      fitAddonRef.current,
      terminalFontSize,
      containerRef.current,
    );
    if (size) notifyResize(size.cols, size.rows);
  }, [terminalFontSize, notifyResize]);

  useEffect(() => {
    if (!terminalRef.current || !fitAddonRef.current || !containerRef.current) return;
    const result = applyTerminalFontFamily(
      terminalRef.current,
      fitAddonRef.current,
      monoFontFamily,
      containerRef.current,
    );
    if (!result) return;
    if (result.immediate) notifyResize(result.immediate.cols, result.immediate.rows);
    let cancelled = false;
    result.whenSettled.then((s) => {
      if (cancelled || !s) return;
      notifyResize(s.cols, s.rows);
    });
    return () => {
      cancelled = true;
    };
  }, [monoFontFamily, notifyResize]);

  return (
    <div
      ref={containerRef}
      className="fastaf-xterm-host"
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
        // Insert the quoted path with a trailing space (VS Code behaviour); no
        // newline — the user completes the command themselves.
        onInputRef.current(shellQuotePath(p) + " ");
        terminalRef.current?.focus();
      }}
      style={{
        width: "100%",
        height: "100%",
        overflow: "hidden",
        cursor: "text",
        background: "inherit",
      }}
    />
  );
}
