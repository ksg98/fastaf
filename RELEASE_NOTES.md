# FastAF 0.0.1

**FastAF** is a lightweight, cross-platform desktop IDE built for the AI-agent era — a fast, native workspace for driving AI coding agents (Claude Code, Codex) and plain shells side by side, across many projects, all in a small Rust + Tauri shell instead of a heavy Electron app.

This is the first public release.

## Highlights

- **Plain or agent terminals** — open a bare `$SHELL` by default, or launch an agent (Claude Code / Codex) on demand.
- **Unified project sidebar** — projects as collapsible groups with their nested sessions, live status dots, and diff badges.
- **Agent status + notifications** — lightweight terminal hooks surface when an agent is asking you a question versus when it has finished, complete with a desktop notification and sound.
- **Session auto-discovery** — running sessions are tracked and resurfaced so you can pick up where you left off.
- **Native Git** — built-in diff viewer, branch and worktree support, and clone-from-URL when adding a project.
- **File explorer, editor & search** — a fast built-in code/diff viewer with syntax highlighting, file tree, and file/in-file search.
- **VS Code-style UX** — command palette (⌘⇧P), quick-open (⌘P), file/in-file search (⌘⇧F), zoom (⌘ +/-/0), and a toggleable files panel (⌘B).
- **Drag & drop** — drag a file from the tree straight into a terminal or the agent prompt.
- **Themes** — theming flows through CSS variables for a consistent, customizable look.

## Tech stack

- **Frontend:** React 19 + TypeScript + Vite, xterm.js (terminal), Shiki (syntax highlighting)
- **Backend / shell:** Tauri 2 + Rust (PTYs, Git, filesystem, agent hooks)

## Download & install

**Platform:** macOS on Apple Silicon (aarch64).

1. Download `FastAF_0.0.1_aarch64.dmg` below.
2. Open the DMG and drag **FastAF** into your **Applications** folder.
3. This build is **unsigned**, so macOS Gatekeeper may block the first launch. To open it:
   - **Right-click** the app and choose **Open**, then confirm in the dialog, or
   - Clear the quarantine attribute from a terminal:
     ```bash
     xattr -dr com.apple.quarantine /Applications/FastAF.app
     ```

## License

[GPL-3.0](LICENSE).
