import { useEffect, useState } from "react";

import { getAnalytics, getWeeklyReport } from "../api";
import type { UsageSummary, ScreenWeeklyReport } from "../types";
import WeeklyChart from "./WeeklyChart";

/** 秒 → "X 小时 Y 分" / "Y 分钟"。 */
function formatDuration(seconds: number): string {
  const mins = Math.max(0, Math.round(seconds / 60));
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return h > 0 ? `${h} 小时 ${m} 分` : `${m} 分钟`;
}

/** 秒 → 紧凑时长（排行条右侧标注）：不足 1 分钟按秒展示，避免"0 分钟"。 */
function formatCompact(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  if (s < 60) return `${s} 秒`;
  return formatDuration(s);
}

interface StatCard {
  label: string;
  value: string;
}

/** 数据面板：今日指标卡 + 应用排行 + 近 7 日屏幕时间柱状图。挂载时拉取一次。 */
export default function AnalyticsView() {
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [weekly, setWeekly] = useState<ScreenWeeklyReport | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    Promise.all([getAnalytics(), getWeeklyReport()])
      .then(([s, w]) => {
        if (cancelled) return;
        setSummary(s);
        setWeekly(w);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (error) {
    return <p className="text-center text-sm text-rose-400">加载失败：{error}</p>;
  }
  if (!summary || !weekly) {
    return <p className="text-center text-sm text-neutral-400">加载中…</p>;
  }

  const cards: StatCard[] = [
    { label: "有效休息", value: `${summary.breaks_today} 次` },
    { label: "提醒次数", value: `${summary.reminders_today} 次` },
    { label: "最长连续", value: formatDuration(summary.longest_streak_secs) },
  ];

  // 后端返回全 0（如全新安装首次启动）时展示友好空态，避免误导"已有数据"。
  const isEmptyToday =
    summary.screen_seconds_today === 0 &&
    summary.breaks_today === 0 &&
    summary.reminders_today === 0 &&
    summary.top_apps.length === 0;

  // 排行条以榜首为 100%（相对占比，突出梯队差异）。
  const topSeconds = Math.max(1, summary.top_apps[0]?.seconds ?? 1);

  return (
    <section className="flex w-full flex-col gap-4">
      {isEmptyToday && (
        <p className="rounded-2xl border border-white/10 bg-white/5 px-4 py-3 text-center text-sm text-neutral-400">
          今日暂无数据，开始正常使用电脑即可 ✨
        </p>
      )}

      {/* ---- 今日指标 ---- */}
      <div className="grid grid-cols-3 gap-3">
        {cards.map((c) => (
          <div
            key={c.label}
            className="rounded-2xl border border-white/10 bg-white/5 px-4 py-3"
          >
            <div className="text-xs text-neutral-400">{c.label}</div>
            <div className="tabular mt-1 text-xl font-semibold text-white">
              {c.value}
            </div>
          </div>
        ))}
      </div>

      {/* ---- 应用排行 ---- */}
      <div className="rounded-2xl border border-white/10 bg-white/5 p-4">
        <h2 className="mb-3 text-sm font-medium text-neutral-300">今日应用排行</h2>
        {summary.top_apps.length === 0 ? (
          <p className="py-2 text-center text-xs text-neutral-500">
            暂无应用使用记录（后台应用不计入）
          </p>
        ) : (
          <ul className="flex flex-col gap-2.5">
            {summary.top_apps.map((app, i) => (
              <li key={app.app_name} className="flex items-center gap-3">
                <span className="tabular w-4 text-right text-xs text-neutral-500">
                  {i + 1}
                </span>
                <span
                  className="w-32 truncate text-sm text-neutral-200"
                  title={app.app_name}
                >
                  {app.app_name}
                </span>
                <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-white/10">
                  <div
                    className="h-full rounded-full bg-indigo-400"
                    style={{ width: `${(app.seconds / topSeconds) * 100}%` }}
                  />
                </div>
                <span className="tabular w-20 text-right text-xs text-neutral-400">
                  {formatCompact(app.seconds)}
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>

      {/* ---- 近 7 日屏幕时间 ---- */}
      <div className="rounded-2xl border border-white/10 bg-white/5 p-4">
        <h2 className="mb-2 text-sm font-medium text-neutral-300">
          近 7 日屏幕时间（分钟）
        </h2>
        <WeeklyChart days={weekly.days} />
      </div>
    </section>
  );
}
