import { describe, expect, test } from "vitest";
import {
  isToggleTerminalShortcut,
  isOpenSearchShortcut,
  isToggleSidebarShortcut,
  isQuickOpenShortcut,
  isCommandPaletteShortcut,
  classifyZoomShortcut,
} from "../shortcuts";

const ev = (over: Partial<{ key: string; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean }>) => ({
  key: "a",
  metaKey: false,
  ctrlKey: false,
  shiftKey: false,
  ...over,
});

describe("isToggleTerminalShortcut", () => {
  test("matches Ctrl+` and Cmd+`", () => {
    expect(isToggleTerminalShortcut(ev({ key: "`", ctrlKey: true }))).toBe(true);
    expect(isToggleTerminalShortcut(ev({ key: "`", metaKey: true }))).toBe(true);
  });

  test("ignores backtick without modifier and Shift+`", () => {
    expect(isToggleTerminalShortcut(ev({ key: "`" }))).toBe(false);
    expect(isToggleTerminalShortcut(ev({ key: "`", ctrlKey: true, shiftKey: true }))).toBe(false);
  });

  test("ignores other keys", () => {
    expect(isToggleTerminalShortcut(ev({ key: "t", ctrlKey: true }))).toBe(false);
  });
});

describe("isOpenSearchShortcut", () => {
  test("Cmd+Shift+F on macOS, Ctrl+Shift+F elsewhere", () => {
    expect(isOpenSearchShortcut(ev({ key: "f", metaKey: true, shiftKey: true }), "macos")).toBe(true);
    expect(isOpenSearchShortcut(ev({ key: "F", ctrlKey: true, shiftKey: true }), "windows")).toBe(true);
  });

  test("requires Shift and the right platform modifier", () => {
    expect(isOpenSearchShortcut(ev({ key: "f", metaKey: true }), "macos")).toBe(false);
    expect(isOpenSearchShortcut(ev({ key: "f", ctrlKey: true, shiftKey: true }), "macos")).toBe(false);
    expect(isOpenSearchShortcut(ev({ key: "f", metaKey: true, shiftKey: true }), "windows")).toBe(false);
  });
});

describe("isToggleSidebarShortcut", () => {
  test("Cmd+B on macOS, Ctrl+B elsewhere", () => {
    expect(isToggleSidebarShortcut(ev({ key: "b", metaKey: true }), "macos")).toBe(true);
    expect(isToggleSidebarShortcut(ev({ key: "B", ctrlKey: true }), "windows")).toBe(true);
  });

  test("ignores Shift+B and the wrong platform modifier", () => {
    expect(isToggleSidebarShortcut(ev({ key: "b", metaKey: true, shiftKey: true }), "macos")).toBe(false);
    expect(isToggleSidebarShortcut(ev({ key: "b", ctrlKey: true }), "macos")).toBe(false);
  });
});

describe("isQuickOpenShortcut", () => {
  test("Cmd+P on macOS, Ctrl+P elsewhere, no Shift", () => {
    expect(isQuickOpenShortcut(ev({ key: "p", metaKey: true }), "macos")).toBe(true);
    expect(isQuickOpenShortcut(ev({ key: "P", ctrlKey: true }), "windows")).toBe(true);
    expect(isQuickOpenShortcut(ev({ key: "p", metaKey: true, shiftKey: true }), "macos")).toBe(false);
  });
});

describe("isCommandPaletteShortcut", () => {
  test("Cmd+Shift+P on macOS, Ctrl+Shift+P elsewhere", () => {
    expect(isCommandPaletteShortcut(ev({ key: "p", metaKey: true, shiftKey: true }), "macos")).toBe(true);
    expect(isCommandPaletteShortcut(ev({ key: "P", ctrlKey: true, shiftKey: true }), "windows")).toBe(true);
  });

  test("requires Shift", () => {
    expect(isCommandPaletteShortcut(ev({ key: "p", metaKey: true }), "macos")).toBe(false);
  });
});

describe("classifyZoomShortcut", () => {
  test("zoom in on + and =", () => {
    expect(classifyZoomShortcut(ev({ key: "+", metaKey: true }), "macos")).toBe("in");
    expect(classifyZoomShortcut(ev({ key: "=", metaKey: true }), "macos")).toBe("in");
    expect(classifyZoomShortcut(ev({ key: "=", ctrlKey: true }), "windows")).toBe("in");
  });

  test("zoom out on - and _ (the key that was broken)", () => {
    expect(classifyZoomShortcut(ev({ key: "-", metaKey: true }), "macos")).toBe("out");
    expect(classifyZoomShortcut(ev({ key: "_", metaKey: true }), "macos")).toBe("out");
    expect(classifyZoomShortcut(ev({ key: "-", ctrlKey: true }), "windows")).toBe("out");
  });

  test("reset on 0; ignores without modifier and wrong platform modifier", () => {
    expect(classifyZoomShortcut(ev({ key: "0", metaKey: true }), "macos")).toBe("reset");
    expect(classifyZoomShortcut(ev({ key: "-", metaKey: true }), "windows")).toBe(null);
    expect(classifyZoomShortcut(ev({ key: "-" }), "macos")).toBe(null);
    expect(classifyZoomShortcut(ev({ key: "a", metaKey: true }), "macos")).toBe(null);
  });
});
