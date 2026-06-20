import { useEffect, useMemo, useRef, useState } from "react";
import { Search, X } from "lucide-react";
import s from "../styles";

export interface PaletteCommand {
  id: string;
  /** Primary label shown in the row. */
  title: string;
  /** Optional right-aligned hint (e.g. a keybinding like "⌘B"). */
  hint?: string;
  /** Extra words to match against, beyond the title. */
  keywords?: string;
  run: () => void;
}

/**
 * VS Code-style command palette (Cmd/Ctrl+Shift+P). Pure frontend, zero dependencies, disposable:
 * released when the parent component unmounts, no background polling, fitting the low-memory constraint.
 */
export function CommandPalette({
  commands,
  onClose,
}: {
  commands: PaletteCommand[];
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return commands;
    return commands.filter((c) =>
      `${c.title} ${c.keywords ?? ""}`.toLowerCase().includes(q),
    );
  }, [commands, query]);

  // Reset the highlight when the query changes to avoid going out of bounds.
  useEffect(() => {
    setActiveIndex(0);
  }, [query]);

  // Scroll the highlighted item into view.
  useEffect(() => {
    const el = listRef.current?.querySelector<HTMLElement>(`[data-idx="${activeIndex}"]`);
    el?.scrollIntoView({ block: "nearest" });
  }, [activeIndex]);

  const runIndex = (idx: number) => {
    const cmd = filtered[idx];
    if (!cmd) return;
    onClose();
    cmd.run();
  };

  return (
    <div style={s.fileSearchDialogBackdrop} onMouseDown={onClose}>
      <div style={s.fileSearchDialog} onMouseDown={(e) => e.stopPropagation()}>
        <div style={s.fileSearchDialogHeader}>
          <span style={s.fileSearchDialogTitle}>Command Palette</span>
          <button
            type="button"
            title="Close"
            aria-label="Close"
            onClick={onClose}
            style={{ ...s.fileSearchClearBtn, marginLeft: "auto" }}
          >
            <X size={12} />
          </button>
        </div>
        <div style={s.fileSearchBox}>
          <Search size={13} style={s.fileSearchIcon} />
          <input
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                onClose();
                e.preventDefault();
              } else if (e.key === "ArrowDown") {
                setActiveIndex((i) => Math.min(filtered.length - 1, i + 1));
                e.preventDefault();
              } else if (e.key === "ArrowUp") {
                setActiveIndex((i) => Math.max(0, i - 1));
                e.preventDefault();
              } else if (e.key === "Enter") {
                runIndex(activeIndex);
                e.preventDefault();
              }
            }}
            placeholder="Type a command…"
            style={s.fileSearchInput}
          />
        </div>
        <div ref={listRef} style={s.fileSearchResults}>
          {filtered.length === 0 ? (
            <div style={s.fileSearchEmpty}>No matching commands</div>
          ) : (
            filtered.map((cmd, idx) => (
              <button
                key={cmd.id}
                type="button"
                data-idx={idx}
                onMouseEnter={() => setActiveIndex(idx)}
                onClick={() => runIndex(idx)}
                style={{
                  ...s.fileSearchResult,
                  ...(idx === activeIndex ? s.fileSearchResultActive : null),
                }}
              >
                <div style={s.fileSearchResultMain}>
                  <span style={s.fileSearchResultName}>{cmd.title}</span>
                </div>
                {cmd.hint && (
                  <span style={{ ...s.fileSearchResultDir, flexShrink: 0 }}>{cmd.hint}</span>
                )}
              </button>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
