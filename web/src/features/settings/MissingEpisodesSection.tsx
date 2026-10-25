import { useQuery } from "@tanstack/react-query";
import { ListChecks, RefreshCw } from "lucide-react";
import { api } from "../../lib/api/client";
import { queryKeys } from "../../lib/api/query-keys";

/** 把 [1,2,3,7,8] 压成「E01–E03、E07–E08」，最多列 limit 段，超出加省略号。 */
function episodeRanges(episodes: number[], limit = 24): string {
  const [first, ...rest] = episodes;
  if (first === undefined) return "";
  const ranges: string[] = [];
  let start = first;
  let previous = first;
  for (const episode of rest) {
    if (episode === previous + 1) {
      previous = episode;
      continue;
    }
    ranges.push(formatRange(start, previous));
    if (ranges.length >= limit) return `${ranges.join("、")}…`;
    start = episode;
    previous = episode;
  }
  ranges.push(formatRange(start, previous));
  if (ranges.length > limit) return `${ranges.slice(0, limit).join("、")}…`;
  return ranges.join("、");
}

function formatRange(start: number, end: number): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return start === end ? `E${pad(start)}` : `E${pad(start)}–E${pad(end)}`;
}

/**
 * 补集：哪些剧还缺哪几集（对齐 daquan 的缺集视图）。
 *
 * 数据来自 `GET /admin/archive/telegram/missing`：只读，补集本身由旁挂的定时任务执行，
 * 这里不触发任何搜索或转存。
 */
export function MissingEpisodesSection() {
  const missing = useQuery({
    queryKey: queryKeys.adminArchiveMissing,
    queryFn: () => api.telegramMissing(),
    refetchOnWindowFocus: false,
  });

  const data = missing.data;
  const series = data?.series ?? [];
  const notes = data?.notes ?? [];
  const totalMissing = series.reduce((sum, item) => sum + item.missingTotal, 0);

  return (
    <section className="lux-admin-panel lux-admin-settings-panel" aria-labelledby="missing-episodes-heading">
      <div className="lux-admin-panel-heading">
        <div>
          <h2 id="missing-episodes-heading">补集</h2>
          <p>
            按「追更列表」里的剧统计库里缺的季与集。补集本身由 Telegram 旁挂的定时任务执行（默认 12
            小时一轮，每轮限流），这里只是清单，不会触发转存。
          </p>
        </div>
        <ListChecks size={20} className="lux-admin-panel-icon" />
      </div>
      <div className="lux-admin-settings-form">
        <div className="lux-archive-settings-group">
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
              gap: 12,
              flexWrap: "wrap",
            }}
          >
            <h3 style={{ margin: 0 }}>
              缺集清单
              {series.length > 0 ? `（${series.length} 部剧、${totalMissing} 集）` : ""}
            </h3>
            <button
              type="button"
              onClick={() => void missing.refetch()}
              disabled={missing.isFetching}
              style={{
                display: "inline-flex",
                alignItems: "center",
                gap: 6,
                padding: "6px 12px",
                borderRadius: 8,
                border: "1px solid rgba(128, 128, 128, 0.4)",
                background: "transparent",
                color: "inherit",
                cursor: missing.isFetching ? "wait" : "pointer",
              }}
            >
              <RefreshCw size={16} />
              {missing.isFetching ? "刷新中…" : "刷新"}
            </button>
          </div>

          {missing.isLoading ? (
            <p>正在读取缺集清单…</p>
          ) : missing.error ? (
            <p className="lux-error-copy">
              读取失败：{missing.error instanceof Error ? missing.error.message : String(missing.error)}
            </p>
          ) : series.length === 0 ? (
            <p>{notes.join("；") || "没有缺集：追更列表里的剧要么齐了，要么不在库里。"}</p>
          ) : (
            <>
              {notes.length > 0 ? (
                <p style={{ opacity: 0.8, fontSize: 13 }}>{notes.join("；")}</p>
              ) : null}
              <ul style={{ listStyle: "none", margin: 0, padding: 0, display: "grid", gap: 10 }}>
                {series.map((item) => (
                  <li
                    key={item.id}
                    style={{
                      border: "1px solid rgba(128, 128, 128, 0.25)",
                      borderRadius: 10,
                      padding: "10px 12px",
                    }}
                  >
                    <div
                      style={{
                        display: "flex",
                        justifyContent: "space-between",
                        gap: 12,
                        flexWrap: "wrap",
                      }}
                    >
                      <strong>
                        {item.title}
                        {item.year ? ` (${item.year})` : ""}
                      </strong>
                      <span>缺 {item.missingTotal} 集</span>
                    </div>
                    <div style={{ fontSize: 13, opacity: 0.85, marginTop: 6, display: "grid", gap: 6 }}>
                      {item.seasons.map((season) => (
                        <div key={season.season}>
                          <span>
                            S{String(season.season).padStart(2, "0")} · 已有 {season.present} 集 · 最高
                            E{String(season.highest).padStart(2, "0")} · 缺 {season.missingTotal} 集
                            {season.truncated ? "（清单已截断）" : ""}
                          </span>
                          <div style={{ marginTop: 2, opacity: 0.75 }}>{episodeRanges(season.missing)}</div>
                        </div>
                      ))}
                    </div>
                  </li>
                ))}
              </ul>
            </>
          )}
        </div>
      </div>
    </section>
  );
}
