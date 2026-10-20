import type { CSSProperties } from "react";
import "./generated-cover.css";

/**
 * A cover for entries that have no artwork of their own.
 *
 * Most library items arrive without a poster (a bare folder of episodes, a
 * movie whose TMDB record has no image, or an anime the scraper never matched).
 * Rather than show a grey placeholder — or reach out to the network for every
 * card — draw the cover locally: a type-tinted gradient, the title, the year and
 * a type badge. It is deterministic (same title always looks the same), works
 * offline and follows the theme through CSS variables.
 */
export type GeneratedCoverKind = "movie" | "series" | "anime";

const KIND_LABEL: Record<GeneratedCoverKind, string> = {
  movie: "电影",
  series: "剧集",
  anime: "动画",
};

/** Stable 0–1 hash so a title keeps its variation across reloads and devices. */
function seededOffset(seed: string): number {
  let hash = 0;
  for (let index = 0; index < seed.length; index += 1) {
    hash = (hash * 31 + seed.charCodeAt(index)) >>> 0;
  }
  return (hash % 1000) / 1000;
}

function initials(title: string): string {
  const cleaned = title.replace(/[（(].*?[）)]/g, "").trim();
  if (!cleaned) return "?";
  // CJK titles read better with the first two characters; latin ones with the
  // first letter of the first two words.
  if (/[\u3400-\u9fff]/.test(cleaned)) return cleaned.slice(0, 2);
  const words = cleaned.split(/\s+/).filter(Boolean);
  return words.slice(0, 2).map((word) => word[0]?.toUpperCase() ?? "").join("") || "?";
}

export function GeneratedCover({
  title,
  year,
  kind = "movie",
  className,
  style,
}: {
  title: string;
  year?: string | number | null;
  kind?: GeneratedCoverKind;
  className?: string;
  style?: CSSProperties;
}) {
  const offset = seededOffset(title);
  const hueShift = Math.round(offset * 28 - 14);
  return (
    <div
      className={["lux-generated-cover", `is-${kind}`, className].filter(Boolean).join(" ")}
      style={{ ...style, "--cover-shift": `${hueShift}` } as CSSProperties}
      role="img"
      aria-label={`${title}${year ? ` (${year})` : ""} 自动生成封面`}
    >
      <span className="lux-generated-cover-glow" aria-hidden="true" />
      <span className="lux-generated-cover-initials" aria-hidden="true">{initials(title)}</span>
      <span className="lux-generated-cover-footer">
        <strong title={title}>{title}</strong>
        <span className="lux-generated-cover-meta">
          {year ? <em>{year}</em> : null}
          <i>{KIND_LABEL[kind]}</i>
        </span>
      </span>
    </div>
  );
}
