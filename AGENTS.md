# FastAF — AGENTS.md

## Overview

FastAF is a desktop workspace for AI coding agents (Claude Code, Codex) and plain
shells. It provides multi-project workspaces, live terminal output, session
auto-discovery, permission-aware execution, Git integration, and usage analytics.

**Stack:** React 19 + TypeScript + Vite (frontend) · Tauri 2 + Rust (desktop shell)
· xterm.js (terminal) · Shiki (syntax highlighting).

## Layout

- `src/` — React/TypeScript frontend.
  - `components/` — UI (sidebar, terminal views, file explorer, settings panels, …).
  - `i18n.tsx` — UI strings (English only).
  - `shortcuts.ts`, `zoom.ts` — keyboard handling and webview zoom.
  - `styles/` — inline style objects keyed off CSS variables (theming).
- `src-tauri/` — Rust backend (Tauri commands).
  - `src/pty.rs` — PTY management for plain shells and agent tasks.
  - `src/git.rs`, `src/fs.rs` — Git and filesystem commands.
  - `src/hooks.rs`, `src/event_watcher.rs`, `src/fastaf-hook.mjs` — agent status
    hooks: a script injected into agent configs writes lifecycle events to
    `~/.fastaf/events/<task_id>/`, which the watcher turns into `task-status`
    events and desktop notifications.
  - `src/lib.rs` — command registration and plugin setup.

## Conventions

- Match the surrounding code's style, naming, and comment density.
- UI is English-only; do not reintroduce other UI locales.
- Theming flows through CSS variables (`--bg-*`, `--text-*`, `--accent`, …) — prefer
  those over hard-coded colors.
- Runtime identifiers use the `fastaf` / `FASTAF_*` / `~/.fastaf` / `fastaf:` prefixes.

## Development

```bash
pnpm install
pnpm tauri dev      # builds Rust + serves Vite, opens the app
pnpm tauri build    # release bundle
pnpm test           # vitest
pnpm lint           # eslint (zero warnings)
cargo test --manifest-path src-tauri/Cargo.toml
```
