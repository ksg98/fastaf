import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import { IS_MAC_WEBKIT } from "../platform";
import type { ThemeVariant } from "../types";
// Explicit contract for accessing xterm private fields — see the header of xterm-private.d.ts.
import type { XTermWithPrivates } from "./xterm-private";

// xterm 6's self-drawn scrollbar width is reused from overviewRuler.width; FitAddon uses
// it to compute the available column count, so it must match the scrollbar gutter width in App.css.
const XTERM_SCROLLBAR_WIDTH = 12;

// ── Theme ────────────────────────────────────────────────────────────────────

export const DARK_THEME = {
  background: "#1e2230",
  foreground: "#cdd6f4",
  cursor: "#cdd6f4",
  selectionBackground: "#45475a",
  black: "#484f58",
  red: "#ff7b72",
  green: "#3fb950",
  yellow: "#d29922",
  blue: "#58a6ff",
  magenta: "#d2a8ff",
  cyan: "#39c5cf",
  white: "#b1bac4",
  brightBlack: "#6e7681",
  brightRed: "#ffa198",
  brightGreen: "#56d364",
  brightYellow: "#e3b341",
  brightBlue: "#79c0ff",
  brightMagenta: "#f0a1ff",
  brightCyan: "#56d4dd",
  brightWhite: "#f0f6fc",
};

export const LIGHT_THEME = {
  background: "#ffffff",
  foreground: "#24292f",
  cursor: "#24292f",
  selectionBackground: "#b3d7ff",
  black: "#24292f",
  red: "#cf222e",
  green: "#116329",
  yellow: "#9a6700",
  blue: "#0550ae",
  magenta: "#8250df",
  cyan: "#1b7c83",
  white: "#6e7781",
  brightBlack: "#57606a",
  brightRed: "#a40e26",
  brightGreen: "#1a7f37",
  brightYellow: "#633c01",
  brightBlue: "#0969da",
  brightMagenta: "#6639ba",
  brightCyan: "#3192aa",
  brightWhite: "#8c959f",
};

// Midnight dark: same syntax palette as DARK_THEME, but a neutral near-black
// background (#1A1B1D) to match the `html.midnight` --bg-panel surface.
export const MIDNIGHT_THEME = {
  ...DARK_THEME,
  background: "#1a1b1d",
};

// Solarized Light–inspired warm palette to match the eyecare CSS tokens.
export const EYECARE_THEME = {
  background: "#fdf6e3",
  foreground: "#586e75",
  cursor: "#586e75",
  selectionBackground: "#eee8d5",
  black: "#073642",
  red: "#dc322f",
  green: "#859900",
  yellow: "#b58900",
  blue: "#268bd2",
  magenta: "#d33682",
  cyan: "#2aa198",
  white: "#93a1a1",
  brightBlack: "#657b83",
  brightRed: "#cb4b16",
  brightGreen: "#586e75",
  brightYellow: "#657b83",
  brightBlue: "#839496",
  brightMagenta: "#6c71c4",
  brightCyan: "#93a1a1",
  brightWhite: "#fdf6e3",
};

export function themeFor(variant: ThemeVariant) {
  if (variant === "dark") return DARK_THEME;
  if (variant === "midnight") return MIDNIGHT_THEME;
  if (variant === "eyecare") return EYECARE_THEME;
  return LIGHT_THEME;
}

export function minimumContrastRatioFor(variant: ThemeVariant): number {
  // Dark-family variants (dark / midnight) ship a hand-tuned palette already
  // readable on their backgrounds, so we skip xterm's auto contrast lift to
  // preserve the original ANSI hues. Light-family variants (light / eyecare)
  // pair light surfaces with high-saturation ANSI defaults that fall below
  // WCAG AA — there we let xterm bump foregrounds until they hit 4.5:1.
  return variant === "dark" || variant === "midnight" ? 1 : 4.5;
}

export function applyTerminalTheme(term: Terminal, variant: ThemeVariant): void {
  term.options.theme = themeFor(variant);
  term.options.minimumContrastRatio = minimumContrastRatioFor(variant);
}

// ── Watermark flow control ───────────────────────────────────────────────────

const HIGH_WATER = 128 * 1024; // 128 KB: stop writing when exceeded
const LOW_WATER  =  16 * 1024; //  16 KB: resume writing

export interface SmartWriter {
  write: (data: string, callback?: () => void) => void;
  drainPending: () => void;
  setSelectionPaused: (paused: boolean) => void;
}

interface TerminalSelectionGuardOptions {
  term: Terminal;
  container: HTMLElement;
  writer?: Pick<SmartWriter, "setSelectionPaused">;
}

function setMacWebKitTextareaAttrs(term: Terminal): void {
  if (!term.textarea) return;
  term.textarea.setAttribute("autocomplete", "off");
  term.textarea.setAttribute("autocorrect", "off");
  term.textarea.setAttribute("autocapitalize", "off");
  term.textarea.setAttribute("spellcheck", "false");
  // Hint to WKWebView that no candidate-bar UI is needed, skipping the
  // wordRangeFromPosition → ICU cluster analysis on the EditorState::stringForCandidateRequest
  // path — this path runs once per frame on willCommitMainFrameData (even when the textarea is
  // already blurred), and it's a separate entry point the spellcheck=false trio doesn't cover.
  term.textarea.setAttribute("inputmode", "none");
}

// During an xterm selection drag, macOS WKWebView continuously queries the
// NSTextInputClient for characterIndexForPoint, triggering LocalFrame::rangeForPoint → ICU
// cluster analysis and saturating the main thread.
//
// Fix: set the textarea to disabled during the drag — with no focusable text input for
// NSTextInputContext to query, the hit-test storm is cut off at the source. On release we
// re-enable and refocus, so normal character / IME input works as usual. Community precedent:
// xterm.js Discussion #5227.
//
// History:
// - We once used `inert` to mark sibling subtrees outside the terminal as non-hittable (an
//   attempt to block the NSTextInput hit-test traversal). A 2026-05-25 sample proved `inert`
//   only changes interaction semantics, not the presence of RenderText in the layout tree, so
//   the hit-test still traverses it; removed.
// - We once used textarea.blur(). A 2026-05-27 user A/B test showed Pinyin stuttered while
//   English didn't, confirming the IME path is the real cause; after blur the textarea is still
//   focusable (a RAF / internal callback may steal focus back), so switching to `disabled` —
//   a hard disable — is more thorough.
// - We once layered on user-select:none suppression + window.getSelection().removeAllRanges() +
//   a TERMINAL_SELECTION_ACTIVE_EVENT broadcast to RunningView/useUsageSnapshot to pause IPC
//   polling. After the 2026-05-27 `disabled` upgrade, Pinyin no longer stuttered in testing, so
//   all the side defenses were removed.
export function attachMacWebKitTerminalGuard({
  term,
  container,
  writer,
}: TerminalSelectionGuardOptions): () => void {
  if (!IS_MAC_WEBKIT) return () => {};

  setMacWebKitTextareaAttrs(term);

  let pointerSelecting = false;
  let terminalHasSelection = term.hasSelection();

  // Use `disabled` to cut off the IME host during a selection drag:
  // - blur: the textarea is still focusable; a later RAF / internal callback may steal focus
  //   back, letting the IME query again
  // - disabled: hard-disables receiving focus / input, so the IME can 100% never initiate an
  //   NSTextInputClient query
  // Reference: xterm.js Discussion #5227 (community battle-tested).
  const disableTextarea = () => {
    if (term.textarea && !term.textarea.disabled) {
      term.textarea.disabled = true;
    }
  };

  const enableTextarea = () => {
    if (term.textarea && term.textarea.disabled) {
      term.textarea.disabled = false;
    }
  };

  const refocusTextarea = () => {
    if (term.textarea) {
      term.textarea.focus({ preventScroll: true });
    }
  };

  const syncSelectionGuard = () => {
    if (pointerSelecting) disableTextarea();
    else enableTextarea();
  };

  const handlePointerDown = (e: PointerEvent) => {
    if (e.button !== 0) return;
    pointerSelecting = true;
    writer?.setSelectionPaused(true);
    syncSelectionGuard();
  };

  const handlePointerUp = (e: PointerEvent) => {
    if (e.button !== 0) return;
    // document-level listener: must first confirm this is a terminal-initiated drag-select flow,
    // otherwise we'd steal focus away from some other input field.
    if (!pointerSelecting) return;
    pointerSelecting = false;
    writer?.setSelectionPaused(false);
    terminalHasSelection = term.hasSelection();
    syncSelectionGuard();
    refocusTextarea();
  };

  const handlePointerCancel = () => {
    if (!pointerSelecting) return;
    pointerSelecting = false;
    writer?.setSelectionPaused(false);
    terminalHasSelection = term.hasSelection();
    syncSelectionGuard();
    refocusTextarea();
  };

  const handleDocumentPointerDown = (e: PointerEvent) => {
    const target = e.target;
    if (!terminalHasSelection || (target instanceof Node && container.contains(target))) return;
    pointerSelecting = false;
    terminalHasSelection = false;
    writer?.setSelectionPaused(false);
    term.clearSelection();
    syncSelectionGuard();
    // The user clicked outside the terminal, so focus is meant to go there — don't grab it back to the textarea.
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key !== "Escape" || !terminalHasSelection) return;
    pointerSelecting = false;
    terminalHasSelection = false;
    writer?.setSelectionPaused(false);
    term.clearSelection();
    syncSelectionGuard();
    refocusTextarea();
  };

  const selectionDisposable = term.onSelectionChange(() => {
    terminalHasSelection = term.hasSelection();
    syncSelectionGuard();
  });

  container.addEventListener("pointerdown", handlePointerDown);
  document.addEventListener("pointerup", handlePointerUp);
  document.addEventListener("pointercancel", handlePointerCancel);
  document.addEventListener("pointerdown", handleDocumentPointerDown, true);
  document.addEventListener("keydown", handleKeyDown, true);

  return () => {
    selectionDisposable.dispose();
    container.removeEventListener("pointerdown", handlePointerDown);
    document.removeEventListener("pointerup", handlePointerUp);
    document.removeEventListener("pointercancel", handlePointerCancel);
    document.removeEventListener("pointerdown", handleDocumentPointerDown, true);
    document.removeEventListener("keydown", handleKeyDown, true);
    // Safety net: if we're still mid-selection-drag at teardown, re-enable the textarea so the next input isn't lost.
    enableTextarea();
    writer?.setSelectionPaused(false);
  };
}

/**
 * Create a watermark-based flow-controlled writer.
 *
 * - Pauses writing when the xterm write queue accumulates beyond HIGH_WATER
 * - Resumes when it drops below LOW_WATER
 * - selectionPaused pauses writing during a mouse selection (optional use)
 */
export function createSmartWriter(term: Terminal): SmartWriter {
  const state = {
    pendingChunks: [] as Array<{ data: string; callback?: () => void }>,
    watermark: 0,
    paused: false,
    selectionPaused: false,
  };

  function flushOne(data: string, callback?: () => void) {
    state.watermark += data.length;
    term.write(data, () => {
      state.watermark -= data.length;
      callback?.();
      if (state.paused && state.watermark < LOW_WATER) {
        state.paused = false;
        drainPending();
      }
    });
  }

  function drainPending() {
    while (state.pendingChunks.length > 0 && !state.paused && !state.selectionPaused) {
      const next = state.pendingChunks.shift()!;
      if (state.watermark >= HIGH_WATER) {
        state.pendingChunks.unshift(next);
        state.paused = true;
        break;
      }
      flushOne(next.data, next.callback);
    }
  }

  function write(data: string, callback?: () => void) {
    if (state.paused || state.selectionPaused || state.watermark >= HIGH_WATER) {
      if (state.watermark >= HIGH_WATER) state.paused = true;
      state.pendingChunks.push({ data, callback });
      return;
    }
    flushOne(data, callback);
  }

  function setSelectionPaused(paused: boolean) {
    state.selectionPaused = paused;
    if (!paused) drainPending();
  }

  return { write, drainPending, setSelectionPaused };
}

// ── xterm initialization ─────────────────────────────────────────────────────

export interface InitTerminalResult {
  term: Terminal;
  fitAddon: FitAddon;
  /** Resolves once the font is ready (with a 1s timeout fallback); never rejects. On ready it has
   *  already toggled fontFamily to make xterm re-measure the cell; callers should safeFit once more after. */
  whenFontsReady: Promise<void>;
}

const fontReadyCache = new Set<string>();
const FONT_READY_TIMEOUT_MS = 1000;
const TEXTURE_ATLAS_REFRESH_DELAYS_MS = [0, 50, 250, 1000, 2500, 5000] as const;

function primaryFontFamily(fontFamily: string): string | null {
  const first = fontFamily.split(",")[0]?.trim().replace(/^["']|["']$/g, "");
  if (!first) return null;
  if (first === "monospace" || first === "serif" || first === "sans-serif" || first === "system-ui") {
    return null;
  }
  return first;
}

function waitForFontReady(fontFamily: string, fontSize: number): Promise<void> {
  const key = `${fontFamily}|${fontSize}`;
  if (fontReadyCache.has(key)) return Promise.resolve();

  const fonts = typeof document !== "undefined" ? document.fonts : undefined;
  if (!fonts) {
    fontReadyCache.add(key);
    return Promise.resolve();
  }

  const primary = primaryFontFamily(fontFamily);
  const spec = primary ? `${fontSize}px "${primary}"` : null;

  // fastaf only uses system fonts, so fonts.load won't trigger a network download — it only
  // rejects when the spec string fails to parse (a developer concatenation bug); warn so it's easy to spot.
  const load = spec
    ? fonts.load(spec).catch((err) => {
        console.warn(`[terminal] invalid font spec "${spec}"`, err);
      })
    : Promise.resolve();

  const ready = load.then(() => fonts.ready).then(() => {});

  return new Promise<void>((resolve) => {
    let settled = false;
    const finish = () => {
      if (settled) return;
      settled = true;
      fontReadyCache.add(key);
      resolve();
    };
    ready.then(finish).catch(finish);
    setTimeout(finish, FONT_READY_TIMEOUT_MS);
  });
}

function whenFontEventuallyReady(fontFamily: string, fontSize: number): Promise<void> {
  const fonts = typeof document !== "undefined" ? document.fonts : undefined;
  if (!fonts) return Promise.resolve();

  const primary = primaryFontFamily(fontFamily);
  const spec = primary ? `${fontSize}px "${primary}"` : null;
  const load = spec
    ? fonts.load(spec).catch((err) => {
        console.warn(`[terminal] invalid font spec "${spec}"`, err);
      })
    : Promise.resolve();
  return load.then(() => fonts.ready).then(() => {});
}

const DOM_MEASURE_REPEAT = 32;
const domCellWidthCache = new Map<string, number>();

function isFontLoaded(fontFamily: string, fontSize: number): boolean {
  const primary = primaryFontFamily(fontFamily);
  if (!primary) return true; // Generic keywords (monospace, etc.) are always ready.
  const fonts = typeof document !== "undefined" ? document.fonts : undefined;
  if (!fonts) return true;
  try {
    return fonts.check(`${fontSize}px "${primary}"`);
  } catch {
    return true;
  }
}

function measureCellWidthInDOM(fontFamily: string, fontSize: number): number | null {
  if (typeof document === "undefined" || !document.body) return null;
  const key = `${fontFamily}|${fontSize}`;
  const cached = domCellWidthCache.get(key);
  if (cached !== undefined) return cached;

  const probe = document.createElement("span");
  probe.classList.add("xterm-char-measure-element");
  probe.setAttribute("aria-hidden", "true");
  probe.style.whiteSpace = "pre";
  probe.style.fontKerning = "none";
  probe.style.fontFamily = fontFamily;
  probe.style.fontSize = `${fontSize}px`;
  // Match xterm's DomMeasureStrategy: 32 W's to average out layout rounding error.
  probe.textContent = "W".repeat(DOM_MEASURE_REPEAT);
  document.body.appendChild(probe);
  try {
    const width = probe.offsetWidth / DOM_MEASURE_REPEAT;
    if (!Number.isFinite(width) || width <= 0) return null;
    // If the font isn't ready we measured the fallback width, which can't be cached; we re-measure once it's ready.
    if (isFontLoaded(fontFamily, fontSize)) {
      domCellWidthCache.set(key, width);
    }
    return width;
  } finally {
    probe.remove();
  }
}

/**
 * Only correct the `width` in xterm's measurement result; don't directly write the current
 * value of `_charSizeService`.
 *
 * WKWebView/OffscreenCanvas measureText for CJK Nerd Fonts may measure half-width characters as
 * fullwidth. Here we override the strategy's returned width with the DOM width, letting xterm's
 * own measure() keep handling writing width/height, firing onCharSizeChange, and the renderer
 * update. We keep xterm's original height result to avoid distorting every cell on screen when DOM
 * height semantics stack on top of xterm's lineHeight.
 */
export function applyDomCharSizeOverride(term: Terminal): () => void {
  const core = (term as XTermWithPrivates)._core;
  const charSizeService = core?._charSizeService;
  const strategy = charSizeService?._measureStrategy;
  if (!charSizeService || !strategy || typeof strategy.measure !== "function") {
    console.warn("[terminal] xterm char size strategy inaccessible; skip DOM width override");
    return () => {};
  }

  const original = strategy.measure.bind(strategy);
  let active = true;
  let warnedMismatch = false;

  strategy.measure = () => {
    const result = original();
    if (!active || result.width <= 0 || result.height <= 0) return result;

    const fontFamily = term.options.fontFamily;
    const fontSize = term.options.fontSize;
    if (typeof fontFamily !== "string" || typeof fontSize !== "number") return result;

    const domWidth = measureCellWidthInDOM(fontFamily, fontSize);
    if (domWidth === null || Math.abs(result.width - domWidth) < 0.5) return result;

    if (!warnedMismatch) {
      warnedMismatch = true;
      console.warn(
        `[terminal] xterm measured cell width=${result.width.toFixed(2)}, DOM width=${domWidth.toFixed(2)}; using DOM width`,
      );
    }
    return { width: domWidth, height: result.height };
  };

  try {
    charSizeService.measure();
  } catch {
    /* Ignore when term isn't fully ready; a font/size change will trigger measure again */
  }

  return () => {
    active = false;
    strategy.measure = original;
  };
}

// xterm's OptionsService dirty-checks and skips an identical fontFamily value, so toggle around it.
function refreshCharSizeAfterFontReady(term: Terminal, fontFamily: string): void {
  try {
    if (term.options.fontFamily !== fontFamily) return;
    term.options.fontFamily = `${fontFamily}, monospace`;
    term.options.fontFamily = fontFamily;
  } catch {
    /* Normal race where term is already disposed */
  }
}

export function initTerminal(
  variant: ThemeVariant,
  scrollback = 1000,
  fontSize = 12,
  fontFamily = "monospace",
): InitTerminalResult {
  const term = new Terminal({
    convertEol: false,
    scrollback,
    cursorBlink: true,
    fontFamily,
    fontSize,
    theme: themeFor(variant),
    minimumContrastRatio: minimumContrastRatioFor(variant),
    allowProposedApi: true,
    overviewRuler: { width: XTERM_SCROLLBAR_WIDTH },
    // When a running TUI (Claude Code / Codex) enables mouse reporting, xterm by default treats a
    // drag as a mouse event, forwards it to the program, and cancels the local selection, leaving
    // macOS users "unable to box-select while running". With this on, holding ⌥ Option while
    // dragging forces a local selection (the iTerm2 / Terminal.app standard convention).
    macOptionClickForcesSelection: true,
  });

  const fitAddon = new FitAddon();
  const unicode11Addon = new Unicode11Addon();
  term.loadAddon(fitAddon);
  term.loadAddon(unicode11Addon);
  term.unicode.activeVersion = "11";

  const whenFontsReady = waitForFontReady(fontFamily, fontSize).then(() => {
    refreshCharSizeAfterFontReady(term, fontFamily);
  });

  return { term, fitAddon, whenFontsReady };
}

export function attachTerminalScrollbarAutoHide(term: Terminal, container: HTMLElement): () => void {
  const ownerWindow = container.ownerDocument.defaultView ?? window;
  let scrollHideTimer: number | null = null;

  const clearScrollHideTimer = () => {
    if (scrollHideTimer === null) return;
    ownerWindow.clearTimeout(scrollHideTimer);
    scrollHideTimer = null;
  };

  const hideAfterScroll = () => {
    clearScrollHideTimer();
    scrollHideTimer = ownerWindow.setTimeout(() => {
      container.classList.remove("fastaf-xterm-scrolling");
      scrollHideTimer = null;
    }, 700);
  };

  const handleScroll = () => {
    container.classList.add("fastaf-xterm-scrolling");
    hideAfterScroll();
  };

  const scrollDisposable = term.onScroll(handleScroll);

  return () => {
    clearScrollHideTimer();
    container.classList.remove("fastaf-xterm-scrolling");
    scrollDisposable.dispose();
  };
}

export interface WebglAddonHandle {
  /** Release the WebGL addon. Safe to call even before lazy loading completes; marks disposed to block a later load. */
  dispose: () => void;
}

interface TextureAtlasRefreshState {
  generation: number;
  frameIds: number[];
  timerIds: number[];
}

const textureAtlasRefreshState = new WeakMap<Terminal, TextureAtlasRefreshState>();

function getTerminalOwnerWindow(term: Terminal): Window {
  return term.element?.ownerDocument.defaultView ?? window;
}

function getTextureAtlasRefreshState(term: Terminal): TextureAtlasRefreshState {
  let state = textureAtlasRefreshState.get(term);
  if (!state) {
    state = { generation: 0, frameIds: [], timerIds: [] };
    textureAtlasRefreshState.set(term, state);
  }
  return state;
}

function cancelScheduledTextureAtlasRefresh(term: Terminal): TextureAtlasRefreshState {
  const ownerWindow = getTerminalOwnerWindow(term);
  const state = getTextureAtlasRefreshState(term);
  for (const frameId of state.frameIds) {
    ownerWindow.cancelAnimationFrame(frameId);
  }
  for (const timerId of state.timerIds) {
    ownerWindow.clearTimeout(timerId);
  }
  state.frameIds = [];
  state.timerIds = [];
  return state;
}

/**
 * After a font or size change, drop the WebGL atlas so glyphs re-rasterize at the new size.
 * When there's no WebGL (`clearTextureAtlas` missing or throwing), silently ignore.
 */
function refreshTextureAtlas(term: Terminal): void {
  try {
    term.clearTextureAtlas();
    if (term.rows > 0) {
      term.refresh(0, term.rows - 1);
    }
  } catch {
    /* DOM renderer has no atlas / term already disposed */
  }
}

function scheduleTextureAtlasRefresh(term: Terminal): void {
  const ownerWindow = getTerminalOwnerWindow(term);
  const state = cancelScheduledTextureAtlasRefresh(term);
  const generation = state.generation + 1;
  state.generation = generation;

  const firstFrame = ownerWindow.requestAnimationFrame(() => {
    if (state.generation !== generation || !term.element) return;
    const secondFrame = ownerWindow.requestAnimationFrame(() => {
      if (state.generation !== generation || !term.element) return;
      for (const delay of TEXTURE_ATLAS_REFRESH_DELAYS_MS) {
        const timerId = ownerWindow.setTimeout(() => {
          if (state.generation !== generation || !term.element) return;
          refreshTextureAtlas(term);
        }, delay);
        state.timerIds.push(timerId);
      }
    });
    state.frameIds.push(secondFrame);
  });
  state.frameIds.push(firstFrame);
}

/**
 * For the `display:none → visible again` path: while the xterm WebGL canvas is removed from the
 * layout tree, the atlas/render cache can enter a bad state (visible garbage when switching back
 * to a project, fixed by a resize). Wait one frame for layout to settle, then clear the cache once.
 *
 * Don't reuse scheduleTextureAtlasRefresh — its 6 delayed nodes are a fallback for async font
 * loading; when switching back the font is long since ready, so running 6 times only makes the
 * user see 6 flickers.
 */
export function refreshTerminalDisplay(term: Terminal): void {
  const ownerWindow = getTerminalOwnerWindow(term);
  const state = cancelScheduledTextureAtlasRefresh(term);
  const generation = state.generation + 1;
  state.generation = generation;
  const frameId = ownerWindow.requestAnimationFrame(() => {
    if (state.generation !== generation || !term.element) return;
    refreshTextureAtlas(term);
  });
  state.frameIds.push(frameId);
}

/**
 * Asynchronously load the WebGL addon: wait until the font is ready before constructing it, to
 * avoid the atlas first prefilling with the fallback font. On failure, silently degrade to the
 * xterm DOM renderer.
 *
 * Why we must wait for the font to be ready: the WebGL renderer uses a glyph atlas to cache
 * rasterization results, and whatever font is used on the first fill is what's used thereafter — if
 * the atlas is filled with a not-yet-loaded fallback font, even after cell sizes are computed
 * correctly later, the rendered characters keep the fallback shapes.
 *
 * Measured conclusions on "whether to turn WebGL off" (recording8/9/10 comparison):
 * - WebGL's cost: occasional 100–400 ms composite spikes when dragging a large selection (GPU geometry upload)
 * - DOM renderer's cost: sustained moderate jank under high-frequency mousemove (mouse moving over the
 *   terminal area) + high-speed text output (each mousemove triggers reflow/composite on several row DOM
 *   nodes; rec10 measured a 511ms single frame under 1233 mousemove/2.7s)
 * - FastAF's day-to-day is mostly "mouse activity over the terminal area", with long selection drags
 *   relatively rare, so WebGL's "occasional spikes" are more acceptable than DOM's "sustained small jank".
 *
 * Don't turn this off again to "avoid occasional jank" — see timeline rec10.
 *
 * Must be called after `term.open()` — term.element is only attached at open time.
 */
export function loadWebglAddon(term: Terminal): WebglAddonHandle {
  let disposed = false;
  let addon: WebglAddon | null = null;

  const fontFamily = typeof term.options.fontFamily === "string" ? term.options.fontFamily : "monospace";
  const fontSize = typeof term.options.fontSize === "number" ? term.options.fontSize : 12;

  void waitForFontReady(fontFamily, fontSize).finally(() => {
    if (disposed || !term.element) return;
    refreshCharSizeAfterFontReady(term, fontFamily);
    try {
      addon = new WebglAddon();
      addon.onContextLoss(() => {
        console.warn("[terminal] WebGL context lost; falling back to xterm DOM renderer");
        addon?.dispose();
        addon = null;
      });
      term.loadAddon(addon);
      scheduleTextureAtlasRefresh(term);
      void whenFontEventuallyReady(fontFamily, fontSize).then(() => {
        if (!disposed && term.element) {
          refreshCharSizeAfterFontReady(term, fontFamily);
          scheduleTextureAtlasRefresh(term);
        }
      });
    } catch (err) {
      console.warn("[terminal] WebGL addon unavailable; using xterm DOM renderer", err);
      /* Degrade when WebGL is unsupported; functionality is unaffected */
    }
  });

  return {
    dispose: () => {
      disposed = true;
      cancelScheduledTextureAtlasRefresh(term);
      addon?.dispose();
      addon = null;
    },
  };
}

/**
 * Safely run fitAddon.fit() and return { cols, rows }; return null on failure / when the container
 * is invisible.
 *
 * If a container is passed, two defenses run (known pitfalls from xterm.js issues #3029 / #4338 / #4841):
 * 1. Either rect width or height is 0 → the container is inside a display:none subtree; skip. With
 *    multiple projects mounted this is the normal state (inactive ProjectPage is display:none).
 * 2. proposeDimensions returns a non-finite value or cols/rows < 2 → a degenerate scenario; skip.
 *
 * Why we must intercept: on a 0-size container FitAddon doesn't return NaN, but degrades to
 * `Math.max(MINIMUM_COLS, Math.floor(0 / cell))` = MINIMUM_COLS (2); if let through → caller
 * notifyResize → resize_pty → SIGWINCH → a TUI like Claude Code / Codex reflows to cols=2,
 * permanently shattering the buffer into one character per line. VS Code's equivalent guard in
 * _resize() is `if (isNaN(cols) || isNaN(rows)) return`, but this NaN path doesn't exist in
 * xterm.js, so we must intercept at the rect layer first.
 */
export function safeFit(
  fitAddon: FitAddon,
  term: Terminal,
  container?: HTMLElement,
): { cols: number; rows: number } | null {
  if (container) {
    const rect = container.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return null;
  }
  try {
    const dims = fitAddon.proposeDimensions();
    if (!dims || !Number.isFinite(dims.cols) || !Number.isFinite(dims.rows)) return null;
    if (dims.cols < 2 || dims.rows < 2) return null;
    fitAddon.fit();
    return { cols: term.cols, rows: term.rows };
  } catch {
    return null;
  }
}

/**
 * Update the terminal font size and re-fit, returning the new { cols, rows } or null.
 */
export function applyTerminalFontSize(
  term: Terminal,
  fitAddon: FitAddon,
  fontSize: number,
  container?: HTMLElement,
): { cols: number; rows: number } | null {
  if (term.options.fontSize === fontSize) return null;
  term.options.fontSize = fontSize;
  const result = safeFit(fitAddon, term, container);
  scheduleTextureAtlasRefresh(term);
  return result;
}

export interface FontFamilyApplyResult {
  /** The synchronous fit result. When the new font isn't loaded yet this is the fallback font's size, shown to the user first. */
  immediate: { cols: number; rows: number } | null;
  /** The result of re-measuring and fitting once the font is ready. CJK monospace fonts need this step on first load to correct cols/rows. */
  whenSettled: Promise<{ cols: number; rows: number } | null>;
}

export function applyTerminalFontFamily(
  term: Terminal,
  fitAddon: FitAddon,
  fontFamily: string,
  container?: HTMLElement,
): FontFamilyApplyResult | null {
  if (term.options.fontFamily === fontFamily) return null;
  term.options.fontFamily = fontFamily;
  const fontSize = typeof term.options.fontSize === "number" ? term.options.fontSize : 12;
  const immediate = safeFit(fitAddon, term, container);
  const whenSettled = waitForFontReady(fontFamily, fontSize).then(() => {
    refreshCharSizeAfterFontReady(term, fontFamily);
    scheduleTextureAtlasRefresh(term);
    return safeFit(fitAddon, term, container);
  });
  return { immediate, whenSettled };
}
