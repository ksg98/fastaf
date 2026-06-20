import afMark from "../assets/wordmark-af.png";

/**
 * FastAF wordmark: "fast" is rendered as live, theme-aware text while the merged
 * "AF" ligature is the cyan brand asset extracted from the logo. The AF image is
 * baseline-aligned to the text via a small bottom nudge.
 */
export function Wordmark({
  size = 22,
  color,
  style,
}: {
  /** Font size of "fast" in px; the AF mark scales relative to it. */
  size?: number;
  /** Color of the "fast" text (defaults to the primary text color). */
  color?: string;
  style?: React.CSSProperties;
}) {
  return (
    <span
      aria-label="FastAF"
      style={{
        display: "inline-flex",
        alignItems: "flex-end",
        gap: Math.round(size * 0.03),
        lineHeight: 1,
        userSelect: "none",
        ...style,
      }}
    >
      <span
        aria-hidden
        style={{
          fontSize: size,
          fontWeight: 360,
          letterSpacing: -size * 0.015,
          lineHeight: 1,
          color: color ?? "var(--text-primary)",
        }}
      >
        fast
      </span>
      <img
        src={afMark}
        alt="AF"
        style={{
          height: Math.round(size * 0.78),
          display: "block",
          marginBottom: Math.round(size * 0.02),
        }}
      />
    </span>
  );
}
