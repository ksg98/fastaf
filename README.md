<p align="center">
  <img src="src/assets/app-logo.png" alt="FastAF logo" width="120" height="120" />
</p>

<h1 align="center">FastAF</h1>

<p align="center">A lightweight, cross-platform desktop IDE built for the AI-agent era.</p>

<p align="center">
  Multi-project workspaces · plain &amp; agent terminals · session auto-discovery · native Git &amp; worktrees · a fast built-in editor · skill management
</p>

---

## What is FastAF?

FastAF is a fast, native desktop workspace for driving AI coding agents (Claude Code, Codex) and plain shells side by side. It runs many sessions across many projects, tracks each session's status (working / waiting on you / done) through lightweight terminal hooks, and pairs that with a built-in file explorer, code/diff viewer, and Git integration — all in a small Rust + Tauri shell instead of a heavy Electron app.

### Highlights

- **Plain or agent terminals** — open a bare `$SHELL` by default, or launch an agent (Claude Code / Codex) on demand.
- **Unified project sidebar** — projects as collapsible groups with their nested sessions, status dots, and diff badges.
- **Agent status + notifications** — terminal hooks surface when an agent is asking a question vs. finished, with a desktop notification and sound.
- **Native Git** — diff viewer, branch/worktree support, and clone-from-URL when adding a project.
- **VS Code-style UX** — command palette (⌘⇧P), quick-open (⌘P), file/in-file search (⌘⇧F), zoom (⌘ +/-/0), and a toggleable files panel (⌘B).
- **Drag &amp; drop** — drag a file from the tree into a terminal or the agent prompt.

## Tech stack

- **Frontend:** React 19 + TypeScript + Vite, xterm.js (terminal), Shiki (syntax highlighting)
- **Backend / shell:** Tauri 2 + Rust (PTYs, Git, filesystem, hooks)

## Development

Requires Node.js, pnpm (or npm), and the Rust toolchain.

```bash
pnpm install            # install frontend deps
pnpm tauri dev          # run the desktop app in dev mode (builds Rust + serves Vite)
pnpm tauri build        # produce a release bundle
```

Useful checks:

```bash
pnpm test               # vitest
pnpm lint               # eslint
cargo test --manifest-path src-tauri/Cargo.toml
```

## License & attribution

FastAF is licensed under [GPL-3.0](LICENSE).

FastAF is a modified version of the open-source project **NeZha**
(https://github.com/hanshuaikang/nezha) by Hanshuaikang and contributors, also
distributed under the GPL-3.0. See [NOTICE.md](NOTICE.md) for the full
attribution and modification notice.
