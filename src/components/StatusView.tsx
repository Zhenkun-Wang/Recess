import type { UsageStatus } from "../types";

/** 秒 → "H:MM:SS"（tabular 数字，随每秒 tick 更新）。 */
function formatClock(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  return `${h}:${m.toString().padStart(2, "0")}:${sec.toString().padStart(2, "0")}`;
}

/** 秒 → "X 小时 Y 分" / "Y 分钟"（中文短描述）。 */
function formatDuration(secs: number): string {
  const mins = Math.max(0, Math.floor(secs / 60));
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return h > 0 ? `${h} 小时 ${m} 分` : `${m} 分钟`;
}

/** 上限进度配色：达上限玫红 → 80% 以上琥珀 → 常态品牌紫。 */
function capColor(pct: number, over: boolean): string {
  if (over) return "#fb7185";
  if (pct >= 80) return "#fbbf24";
  return "#6366f1";
}

interface Props {
  status: UsageStatus | null;
}

/**
 * 今日状态主视图：大数字（今日屏幕时间）+ 每日上限进度 + 连续使用状态。
 *
 * 数据由父组件经每秒 `tick` 事件驱动，本组件无状态；后端未就绪时显示占位。
 * 界面展示不依赖提醒开关——关闭提醒只影响 toast，进度照常呈现。
 */
export default function StatusView({ status }: Props) {
  if (!status) {
    return (
      <section className="grid place-items-center rounded-3xl border border-white/10 bg-white/5 py-20">
        <span className="text-lg text-neutral-400">连接中…</span>
      </section>
    );
  }

  const cap = Math.max(1, status.daily_cap_seconds);
  const capPct = Math.min(100, (status.today_screen_secs / cap) * 100);
  const overCap = status.today_screen_secs >= status.daily_cap_seconds;
  const remaining = Math.max(0, status.daily_cap_seconds - status.today_screen_secs);

  const limit = Math.max(1, status.continuous_limit_seconds);
  const consPct = Math.min(100, (status.consecutive_seconds / limit) * 100);

  return (
    <section className="flex flex-col gap-7 rounded-3xl border border-white/10 bg-white/5 px-8 py-9">
      {/* ---- 大数字：今日屏幕时间 ---- */}
      <div className="flex flex-col items-center gap-1.5">
        <span className="text-xs font-medium uppercase tracking-[0.3em] text-neutral-500">
          今日屏幕时间
        </span>
        <span
          className="tabular text-6xl font-semibold"
          style={{ color: overCap ? "#fb7185" : "#ffffff" }}
        >
          {formatClock(status.today_screen_secs)}
        </span>
      </div>

      {/* ---- 每日上限进度 ---- */}
      <div className="flex flex-col gap-2">
        <div className="flex items-baseline justify-between text-xs">
          <span className="text-neutral-400">
            每日上限 {formatDuration(status.daily_cap_seconds)}
          </span>
          <span className="tabular text-neutral-300">
            {overCap ? "已达上限" : `剩余 ${formatDuration(remaining)}`}
          </span>
        </div>
        <div className="h-2 w-full overflow-hidden rounded-full bg-white/10">
          <div
            className="h-full rounded-full transition-all duration-700"
            style={{ width: `${capPct}%`, background: capColor(capPct, overCap) }}
          />
        </div>
      </div>

      {/* ---- 连续使用状态 ---- */}
      <div className="flex flex-col gap-2">
        <div className="flex items-center justify-between text-xs">
          <span className="flex items-center gap-2">
            <span
              className={`h-2 w-2 rounded-full ${status.active ? "bg-emerald-400" : "bg-neutral-500"}`}
            />
            <span className={status.active ? "text-emerald-300" : "text-neutral-400"}>
              {status.active ? "在用中" : "离座"}
            </span>
            {!status.reminders_enabled && (
              <span className="text-neutral-500">· 提醒已关闭</span>
            )}
          </span>
          <span className="tabular text-neutral-300">
            连续 {formatDuration(status.consecutive_seconds)} · 阈值{" "}
            {formatDuration(status.continuous_limit_seconds)}
          </span>
        </div>
        <div className="h-1.5 w-full overflow-hidden rounded-full bg-white/10">
          <div
            className="h-full rounded-full bg-emerald-400 transition-all duration-700"
            style={{ width: `${consPct}%` }}
          />
        </div>
      </div>
    </section>
  );
}
