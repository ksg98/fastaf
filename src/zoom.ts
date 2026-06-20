import { getCurrentWebview } from "@tauri-apps/api/webview";

// VS Code-style page zoom. Uses WKWebView's native setPageZoom (exposed via Tauri's setZoom),
// rather than the polyfill injected by zoomHotkeysEnabled — that polyfill is unreliable detecting the "-" key on macOS.
// Here we deterministically handle Cmd/Ctrl +/-/0 in JS; both text and the xterm canvas re-rasterize crisply.

const STORAGE_KEY = "fastaf:zoom";
const MIN_ZOOM = 0.5;
const MAX_ZOOM = 3.0;
const STEP = 0.1;
const DEFAULT_ZOOM = 1.0;

function clamp(level: number): number {
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, Math.round(level * 100) / 100));
}

export function getStoredZoom(): number {
  const raw = localStorage.getItem(STORAGE_KEY);
  const parsed = raw == null ? NaN : Number(raw);
  return Number.isFinite(parsed) ? clamp(parsed) : DEFAULT_ZOOM;
}

let current = getStoredZoom();

/** Apply a zoom level to the webview and persist it. */
export function applyZoom(level: number): void {
  current = clamp(level);
  localStorage.setItem(STORAGE_KEY, String(current));
  getCurrentWebview()
    .setZoom(current)
    .catch(() => {
      // setZoom may be unavailable in some environments; ignore — zoom is non-critical.
    });
}

/** Re-apply the persisted zoom (call once on app start). */
export function restoreZoom(): void {
  applyZoom(getStoredZoom());
}

export function zoomIn(): void {
  applyZoom(current + STEP);
}

export function zoomOut(): void {
  applyZoom(current - STEP);
}

export function zoomReset(): void {
  applyZoom(DEFAULT_ZOOM);
}
