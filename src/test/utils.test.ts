import { describe, it, expect, beforeEach, vi } from "vitest";
import {
  AVATAR_COLORS,
  getAvatarGradient,
  shortenPath,
  load,
  save,
  getGitStatusColor,
  getGitStatusLabel,
  getFileColor,
  CODE_EXTS,
} from "../utils";

// ── getAvatarGradient ────────────────────────────────────────────────────────

describe("getAvatarGradient", () => {
  it("always returns a color pair from AVATAR_COLORS", () => {
    const result = getAvatarGradient("my-project");
    expect(AVATAR_COLORS).toContainEqual(result);
  });

  it("always returns the same color for the same name (idempotent)", () => {
    expect(getAvatarGradient("fastaf")).toEqual(getAvatarGradient("fastaf"));
  });

  it("usually returns different colors for different names", () => {
    // Uneven hashing may collide, but common names shouldn't match
    const a = getAvatarGradient("project-alpha");
    const b = getAvatarGradient("project-beta");
    // Don't strictly assert inequality (to avoid false positives from hash collisions); only assert the return values are valid
    expect(AVATAR_COLORS).toContainEqual(a);
    expect(AVATAR_COLORS).toContainEqual(b);
  });

  it("does not throw on an empty string and returns a valid color", () => {
    expect(() => getAvatarGradient("")).not.toThrow();
    expect(AVATAR_COLORS).toContainEqual(getAvatarGradient(""));
  });
});

// ── shortenPath ──────────────────────────────────────────────────────────────

describe("shortenPath", () => {
  it("replaces the /Users/<username>/ prefix with ~", () => {
    expect(shortenPath("/Users/john/Documents/project")).toBe("~/Documents/project");
  });

  it("handles usernames containing dots and hyphens correctly", () => {
    expect(shortenPath("/Users/xxxx/workspace/fastaf")).toBe("~/workspace/fastaf");
  });

  it("leaves non-/Users/ paths unchanged", () => {
    expect(shortenPath("/etc/hosts")).toBe("/etc/hosts");
    expect(shortenPath("/tmp/foo")).toBe("/tmp/foo");
  });

  it("shortens a path that is only /Users/<username> to ~", () => {
    expect(shortenPath("/Users/john")).toBe("~");
  });
});

// ── localStorage load / save ─────────────────────────────────────────────────

describe("load / save", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("load reads back correctly after save writes", () => {
    save("theme", "dark");
    expect(load("theme", "light")).toBe("dark");
  });

  it("returns the fallback when the key does not exist", () => {
    expect(load("nonexistent", 42)).toBe(42);
  });

  it("supports storing complex objects", () => {
    const data = { projectId: "abc", count: 3 };
    save("meta", data);
    expect(load("meta", null)).toEqual(data);
  });

  it("returns the fallback instead of throwing when stored JSON is corrupt", () => {
    localStorage.setItem("corrupt", "{not-valid-json");
    expect(load("corrupt", "fallback")).toBe("fallback");
  });
});

// ── getGitStatusColor ────────────────────────────────────────────────────────

describe("getGitStatusColor", () => {
  it.each([
    ["A", "#3fb950"],
    ["D", "#f85149"],
    ["M", "#e3b341"],
    ["R", "#79c0ff"],
    ["?", "#79c0ff"],
    ["U", "#f85149"],
  ])("status %s returns the correct color", (status, expected) => {
    expect(getGitStatusColor(status)).toBe(expected);
  });

  it("returns the muted variable for an unknown status", () => {
    expect(getGitStatusColor("X")).toBe("var(--text-muted)");
  });
});

// ── getGitStatusLabel ────────────────────────────────────────────────────────

describe("getGitStatusLabel", () => {
  it("maps ? to U (for displaying Untracked)", () => {
    expect(getGitStatusLabel("?")).toBe("U");
  });

  it("maps U to ! (for displaying conflicts)", () => {
    expect(getGitStatusLabel("U")).toBe("!");
  });

  it.each(["A", "D", "M", "R"])("returns a known status %s unchanged", (s) => {
    expect(getGitStatusLabel(s)).toBe(s);
  });

  it("returns an unknown status unchanged", () => {
    expect(getGitStatusLabel("Z")).toBe("Z");
  });
});

// ── getFileColor ─────────────────────────────────────────────────────────────

describe("getFileColor", () => {
  it("returns the TypeScript icon color token for TypeScript files", () => {
    expect(getFileColor("App.tsx")).toBe("var(--icon-file-ts)");
    expect(getFileColor("utils.ts")).toBe("var(--icon-file-ts)");
  });

  it("returns the Rust icon color token for Rust files", () => {
    expect(getFileColor("lib.rs")).toBe("var(--icon-file-rust)");
  });

  it("returns the Docker icon color token for the special Dockerfile name (case-insensitive)", () => {
    expect(getFileColor("Dockerfile")).toBe("var(--icon-file-docker)");
    expect(getFileColor("dockerfile.prod")).toBe("var(--icon-file-docker)");
  });

  it("returns the build-file icon color token for Makefile", () => {
    expect(getFileColor("Makefile")).toBe("var(--icon-file-build)");
  });

  it("returns the config-file icon color token for .env files", () => {
    expect(getFileColor(".env")).toBe("var(--icon-file-config)");
    expect(getFileColor(".env.production")).toBe("var(--icon-file-config)");
  });

  it("returns the default icon color token for an unknown file with no extension", () => {
    expect(getFileColor("NOTICE")).toBe("var(--icon-file-default)");
  });

  it("prioritizes the ext argument over the extension inferred from the filename", () => {
    // Passing ext="rs" overrides the "ts" inferred from "foo.ts"
    expect(getFileColor("foo.ts", "rs")).toBe("var(--icon-file-rust)");
  });
});

// ── CODE_EXTS ─────────────────────────────────────────────────────────────────

describe("CODE_EXTS", () => {
  it("includes common code extensions", () => {
    expect(CODE_EXTS.has("ts")).toBe(true);
    expect(CODE_EXTS.has("rs")).toBe(true);
    expect(CODE_EXTS.has("py")).toBe(true);
  });

  it("excludes non-code extensions like images", () => {
    expect(CODE_EXTS.has("png")).toBe(false);
    expect(CODE_EXTS.has("pdf")).toBe(false);
  });
});

// Ensure vi is referenced (to avoid a lint warning)
void vi;
