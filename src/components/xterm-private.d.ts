/**
 * xterm.js 6 private API type contract (semi-public).
 *
 * Makes `applyDomCharSizeOverride()` in `terminalShared.ts` access `_core` /
 * `_charSizeService` / `_measureStrategy` explicitly — more greppable and
 * reviewable than `as any`. Same approach as OpenSumi's
 * `packages/terminal-next/src/common/xterm-private.d.ts`.
 *
 * When upgrading xterm, check (source reference `node_modules/@xterm/xterm/src/browser/services/CharSizeService.ts`):
 *   - whether `Terminal._core._charSizeService._measureStrategy.measure()` still exists
 *   - whether `IMeasureResult` fields are still `{ width, height }`
 *
 * The field names are not mangled in the minified bundle (DI service token + class field),
 * but the xterm team does not guarantee stability.
 */

import type { Terminal } from "@xterm/xterm";

export interface XTermMeasureResult {
  width: number;
  height: number;
}

export interface XTermMeasureStrategy {
  measure(): Readonly<XTermMeasureResult>;
}

export interface XTermCharSizeService {
  width: number;
  height: number;
  readonly hasValidSize: boolean;
  measure(): void;
  _measureStrategy: XTermMeasureStrategy;
}

export interface XTermCore {
  _charSizeService?: XTermCharSizeService;
}

/** Terminal that includes the `_core` private entry point — named with "Private" for easy grepping. */
export interface XTermWithPrivates extends Terminal {
  readonly _core: XTermCore;
}
