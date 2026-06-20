import type React from "react";
import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { FolderOpen, GitFork, Loader2, ChevronLeft } from "lucide-react";
import { useToast } from "./Toast";
import s from "../styles";

const VIEWPORT_MARGIN = 8;

/**
 * "Add Project" menu: anchored at the click position like a context menu (rather than a centered dialog).
 * Offers "Open Folder" and "Clone from GitHub"; cloning goes through the backend `git_clone`,
 * and on success returns the cloned local path via onCloned.
 */
export function AddProjectDialog({
  anchor,
  onClose,
  onOpenFolder,
  onCloned,
}: {
  anchor: { x: number; y: number };
  onClose: () => void;
  onOpenFolder: () => void;
  onCloned: (path: string) => void;
}) {
  const { showToast } = useToast();
  const [mode, setMode] = useState<"choose" | "clone">("choose");
  const [url, setUrl] = useState("");
  const [parentDir, setParentDir] = useState("");
  const [cloning, setCloning] = useState(false);

  const panelRef = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ x: anchor.x, y: anchor.y });

  const updatePosition = useCallback(() => {
    const panel = panelRef.current;
    if (!panel) return;
    const rect = panel.getBoundingClientRect();
    const vw = document.documentElement.clientWidth || window.innerWidth;
    const vh = document.documentElement.clientHeight || window.innerHeight;
    const maxX = Math.max(VIEWPORT_MARGIN, vw - rect.width - VIEWPORT_MARGIN);
    const maxY = Math.max(VIEWPORT_MARGIN, vh - rect.height - VIEWPORT_MARGIN);
    setPosition({
      x: Math.min(Math.max(anchor.x, VIEWPORT_MARGIN), maxX),
      y: Math.min(Math.max(anchor.y, VIEWPORT_MARGIN), maxY),
    });
  }, [anchor.x, anchor.y]);

  // Re-measure: switching between choose/clone modes changes the size, so refit to the viewport.
  useLayoutEffect(() => {
    updatePosition();
  }, [updatePosition, mode]);

  useLayoutEffect(() => {
    window.addEventListener("resize", updatePosition);
    return () => window.removeEventListener("resize", updatePosition);
  }, [updatePosition]);

  async function pickDestination() {
    const selected = await openDialog({ directory: true, multiple: false });
    if (selected) setParentDir(selected as string);
  }

  async function handleClone() {
    if (cloning) return;
    if (!url.trim()) {
      showToast("Enter a repository URL", "warning");
      return;
    }
    if (!parentDir) {
      showToast("Choose a destination folder", "warning");
      return;
    }
    setCloning(true);
    try {
      const path = await invoke<string>("git_clone", {
        url: url.trim(),
        parentDir,
        name: null,
      });
      showToast("Repository cloned", "success");
      onCloned(path);
    } catch (e) {
      showToast(`Clone failed: ${String(e)}`, "error");
    } finally {
      setCloning(false);
    }
  }

  return (
    <>
      <div style={s.fileCtxBackdrop} onPointerDown={onClose} />
      <div
        ref={panelRef}
        style={
          mode === "choose"
            ? { ...s.fileCtxMenu, left: position.x, top: position.y, minWidth: 220 }
            : { ...clonePanel, left: position.x, top: position.y }
        }
        onPointerDown={(e) => e.stopPropagation()}
        onClick={(e) => e.stopPropagation()}
      >
        {mode === "choose" ? (
          <>
            <MenuItem icon={<FolderOpen size={15} />} title="Open folder…" onClick={onOpenFolder} />
            <MenuItem
              icon={<GitFork size={15} />}
              title="Clone from GitHub…"
              onClick={() => setMode("clone")}
            />
          </>
        ) : (
          <>
            <div style={cloneHeader}>
              <button type="button" style={backBtn} onClick={() => setMode("choose")}>
                <ChevronLeft size={14} />
              </button>
              <span style={{ fontSize: 13, fontWeight: 650, color: "var(--text-primary)" }}>
                Clone from GitHub
              </span>
            </div>
            <input
              autoFocus
              style={input}
              placeholder="https://github.com/owner/repo.git"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") handleClone();
                if (e.key === "Escape") onClose();
              }}
            />
            <button type="button" style={destBtn} onClick={pickDestination}>
              <FolderOpen size={13} style={{ color: "var(--text-muted)", flexShrink: 0 }} />
              <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                {parentDir || "Choose destination…"}
              </span>
            </button>
            <button
              type="button"
              style={{ ...primaryBtn, opacity: cloning ? 0.7 : 1, cursor: cloning ? "wait" : "pointer" }}
              disabled={cloning}
              onClick={handleClone}
            >
              {cloning ? <Loader2 size={14} className="spin" /> : <GitFork size={14} />}
              {cloning ? "Cloning…" : "Clone"}
            </button>
          </>
        )}
      </div>
    </>
  );
}

function MenuItem({
  icon,
  title,
  onClick,
}: {
  icon: React.ReactNode;
  title: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      style={{
        ...s.fileCtxMenuItem,
        display: "flex",
        alignItems: "center",
        gap: 9,
        color: "var(--text-primary)",
      }}
      onMouseEnter={(e) => {
        e.currentTarget.style.background = "var(--accent)";
        e.currentTarget.style.color = "var(--fg-on-accent)";
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.background = "transparent";
        e.currentTarget.style.color = "var(--text-primary)";
      }}
      onClick={onClick}
    >
      {icon}
      {title}
    </button>
  );
}

const clonePanel: React.CSSProperties = {
  position: "fixed",
  zIndex: 1000,
  width: 300,
  display: "flex",
  flexDirection: "column",
  gap: 8,
  padding: 12,
  background: "var(--bg-card)",
  border: "1px solid var(--border-medium)",
  borderRadius: 10,
  boxShadow: "0 10px 34px rgba(0,0,0,0.32)",
};

const cloneHeader: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 6,
  marginBottom: 2,
};

const backBtn: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  width: 22,
  height: 22,
  border: "none",
  borderRadius: 5,
  background: "transparent",
  color: "var(--text-muted)",
  cursor: "pointer",
};

const input: React.CSSProperties = {
  padding: "7px 9px",
  fontSize: 13,
  color: "var(--text-primary)",
  background: "var(--bg-input)",
  border: "1px solid var(--border-medium)",
  borderRadius: 7,
  outline: "none",
};

const destBtn: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 7,
  padding: "7px 9px",
  fontSize: 12.5,
  color: "var(--text-primary)",
  background: "var(--bg-input)",
  border: "1px solid var(--border-medium)",
  borderRadius: 7,
  cursor: "pointer",
};

const primaryBtn: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  gap: 7,
  padding: "8px 12px",
  fontSize: 13,
  fontWeight: 650,
  borderRadius: 7,
  border: "none",
  background: "var(--control-active-bg)",
  color: "var(--control-active-fg)",
  marginTop: 2,
};
