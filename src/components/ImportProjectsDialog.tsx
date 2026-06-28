import type React from "react";
import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Folder, Loader2, X, Check, Download, MessageSquare } from "lucide-react";
import type { AgentType } from "../types";
import { useI18n } from "../i18n";
import s from "../styles";

export interface DiscoveredSession {
  id: string;
  path: string;
  title: string;
  agent: AgentType;
  modifiedMs: number;
}

export interface DiscoveredProject {
  path: string;
  name: string;
  agents: string[];
  sessionCount: number;
  lastActiveMs: number;
  alreadyImported: boolean;
  sessions: DiscoveredSession[];
}

/**
 * Migration import: scans `~/.claude` and `~/.codex` (via the backend) for existing
 * projects + chat sessions and lets the user bring them into FastAF in one step.
 * Projects already added are listed but unchecked by default.
 */
export function ImportProjectsDialog({
  onClose,
  onImport,
}: {
  onClose: () => void;
  onImport: (selected: DiscoveredProject[], includeChats: boolean) => void;
}) {
  const { t } = useI18n();
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [discovered, setDiscovered] = useState<DiscoveredProject[]>([]);
  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(new Set());
  const [includeChats, setIncludeChats] = useState(true);

  useEffect(() => {
    let cancelled = false;
    invoke<DiscoveredProject[]>("discover_importable_projects")
      .then((list) => {
        if (cancelled) return;
        setDiscovered(list);
        // Default-select everything not already imported.
        setSelectedPaths(new Set(list.filter((p) => !p.alreadyImported).map((p) => p.path)));
        setLoading(false);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(String(e));
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const toggle = (path: string) => {
    setSelectedPaths((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  const totalChats = useMemo(
    () =>
      discovered
        .filter((p) => selectedPaths.has(p.path))
        .reduce((sum, p) => sum + p.sessionCount, 0),
    [discovered, selectedPaths],
  );

  const allSelectable = discovered.length > 0;
  const allSelected = allSelectable && discovered.every((p) => selectedPaths.has(p.path));
  const toggleAll = () => {
    setSelectedPaths(allSelected ? new Set() : new Set(discovered.map((p) => p.path)));
  };

  const handleImport = () => {
    onImport(
      discovered.filter((p) => selectedPaths.has(p.path)),
      includeChats,
    );
  };

  return (
    <div style={s.modalOverlay} onPointerDown={onClose}>
      <div style={box} onPointerDown={(e) => e.stopPropagation()}>
        <div style={header}>
          <div>
            <div style={{ fontSize: 15, fontWeight: 700, color: "var(--text-primary)" }}>
              {t("import.title")}
            </div>
            <div style={{ fontSize: 12, color: "var(--text-hint)", marginTop: 2 }}>
              {t("import.subtitle")}
            </div>
          </div>
          <button style={s.modalCloseBtn} onClick={onClose} aria-label={t("common.close")}>
            <X size={16} />
          </button>
        </div>

        <div style={listWrap}>
          {loading ? (
            <div style={centerState}>
              <Loader2 size={18} className="spin" />
              <span>{t("import.scanning")}</span>
            </div>
          ) : error ? (
            <div style={{ ...centerState, color: "var(--danger)" }}>{error}</div>
          ) : discovered.length === 0 ? (
            <div style={centerState}>{t("import.empty")}</div>
          ) : (
            <>
              <button style={selectAllBtn} onClick={toggleAll}>
                {allSelected ? t("import.deselectAll") : t("import.selectAll")}
              </button>
              {discovered.map((p) => {
                const selected = selectedPaths.has(p.path);
                return (
                  <button
                    key={p.path}
                    onClick={() => toggle(p.path)}
                    style={{
                      ...row,
                      background: selected ? "var(--bg-hover)" : "transparent",
                      borderColor: selected ? "var(--accent)" : "var(--border-dim)",
                    }}
                  >
                    <span
                      style={{
                        ...checkbox,
                        background: selected ? "var(--accent)" : "transparent",
                        borderColor: selected ? "var(--accent)" : "var(--border-strong)",
                      }}
                    >
                      {selected && <Check size={12} color="var(--fg-on-accent)" />}
                    </span>
                    <Folder size={15} color="var(--text-muted)" style={{ flexShrink: 0 }} />
                    <span style={rowMain}>
                      <span style={rowName}>
                        {p.name}
                        {p.alreadyImported && (
                          <span style={badge}>{t("import.alreadyAdded")}</span>
                        )}
                      </span>
                      <span style={rowPath}>{p.path}</span>
                    </span>
                    <span style={rowMeta}>
                      <span style={metaChip}>
                        <MessageSquare size={11} />
                        {p.sessionCount}
                      </span>
                      {p.agents.map((a) => (
                        <span key={a} style={agentChip}>
                          {a}
                        </span>
                      ))}
                    </span>
                  </button>
                );
              })}
            </>
          )}
        </div>

        <div style={footer}>
          <label style={chatsToggle}>
            <input
              type="checkbox"
              checked={includeChats}
              onChange={(e) => setIncludeChats(e.target.checked)}
            />
            {t("import.includeChats", { count: totalChats })}
          </label>
          <div style={{ flex: 1 }} />
          <button style={s.modalCancelBtn} onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            style={{
              ...s.modalSaveBtn,
              display: "inline-flex",
              alignItems: "center",
              gap: 7,
              opacity: selectedPaths.size === 0 ? 0.55 : 1,
              cursor: selectedPaths.size === 0 ? "not-allowed" : "pointer",
            }}
            disabled={selectedPaths.size === 0}
            onClick={handleImport}
          >
            <Download size={14} />
            {t("import.importN", { count: selectedPaths.size })}
          </button>
        </div>
      </div>
    </div>
  );
}

const box: React.CSSProperties = {
  width: "min(640px, calc(100vw - 48px))",
  maxHeight: "min(72vh, 720px)",
  background: "var(--bg-card)",
  border: "1px solid var(--border-medium)",
  borderRadius: 14,
  boxShadow: "var(--shadow-popover)",
  display: "flex",
  flexDirection: "column",
  overflow: "hidden",
};

const header: React.CSSProperties = {
  display: "flex",
  alignItems: "flex-start",
  justifyContent: "space-between",
  padding: "16px 18px 14px",
  borderBottom: "1px solid var(--border-dim)",
  flexShrink: 0,
};

const listWrap: React.CSSProperties = {
  flex: 1,
  overflowY: "auto",
  padding: "12px 14px",
  display: "flex",
  flexDirection: "column",
  gap: 6,
  minHeight: 120,
};

const centerState: React.CSSProperties = {
  flex: 1,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  gap: 8,
  color: "var(--text-hint)",
  fontSize: 13,
  padding: "32px 0",
};

const selectAllBtn: React.CSSProperties = {
  alignSelf: "flex-end",
  background: "none",
  border: "none",
  color: "var(--accent)",
  fontSize: 12,
  fontWeight: 600,
  cursor: "pointer",
  padding: "2px 4px",
};

const row: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 10,
  padding: "9px 11px",
  borderRadius: 9,
  border: "1px solid var(--border-dim)",
  cursor: "pointer",
  textAlign: "left",
  width: "100%",
};

const checkbox: React.CSSProperties = {
  width: 17,
  height: 17,
  borderRadius: 5,
  border: "1.5px solid var(--border-strong)",
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  flexShrink: 0,
};

const rowMain: React.CSSProperties = {
  flex: 1,
  minWidth: 0,
  display: "flex",
  flexDirection: "column",
  gap: 1,
};

const rowName: React.CSSProperties = {
  fontSize: 13,
  fontWeight: 600,
  color: "var(--text-primary)",
  display: "flex",
  alignItems: "center",
  gap: 8,
  whiteSpace: "nowrap",
  overflow: "hidden",
  textOverflow: "ellipsis",
};

const rowPath: React.CSSProperties = {
  fontSize: 11,
  color: "var(--text-hint)",
  whiteSpace: "nowrap",
  overflow: "hidden",
  textOverflow: "ellipsis",
};

const rowMeta: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 5,
  flexShrink: 0,
};

const metaChip: React.CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 3,
  fontSize: 11,
  color: "var(--text-muted)",
};

const agentChip: React.CSSProperties = {
  fontSize: 10,
  fontWeight: 600,
  textTransform: "uppercase",
  letterSpacing: 0.4,
  color: "var(--text-secondary)",
  background: "var(--bg-input)",
  border: "1px solid var(--border-dim)",
  borderRadius: 5,
  padding: "1px 5px",
};

const badge: React.CSSProperties = {
  fontSize: 9.5,
  fontWeight: 600,
  textTransform: "uppercase",
  letterSpacing: 0.4,
  color: "var(--text-hint)",
  background: "var(--bg-input)",
  borderRadius: 5,
  padding: "1px 5px",
};

const footer: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 10,
  padding: "12px 16px",
  borderTop: "1px solid var(--border-dim)",
  flexShrink: 0,
};

const chatsToggle: React.CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 7,
  fontSize: 12.5,
  color: "var(--text-secondary)",
  cursor: "pointer",
};
