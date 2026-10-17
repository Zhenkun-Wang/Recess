/**
 * 前后端 IPC 数据契约（与 Rust 侧 serde 结构对齐）。
 *
 * 对应后端：
 * - `UsageStatus` ← lib.rs `UsageStatus`（`get_status` 返回 + `tick` 事件负载）
 * - `SettingsPayload` ← lib.rs `SettingsPayload`（`get_settings` 返回）
 * - `UsageSummary` / `AppUsageStat` ← analytics.rs（`get_analytics` 返回）
 * - `ScreenWeeklyReport` / `ScreenDailyStat` ← analytics.rs（`get_weekly_report` 返回）
 */

/** 使用状态快照（`get_status` 返回 + 每秒 `tick` 事件负载）。 */
export interface UsageStatus {
  /** 当前是否"在用"（空闲 < 60s）。 */
  active: boolean;
  /** 今日屏幕使用总秒数。 */
  today_screen_secs: number;
  /** 当前连续使用秒数（离座 ≥ 120s 后清零）。 */
  consecutive_seconds: number;
  /** 每日上限（秒）。 */
  daily_cap_seconds: number;
  /** 连续使用提醒阈值（秒）。 */
  continuous_limit_seconds: number;
  /** 提醒总开关。 */
  reminders_enabled: boolean;
}

/** 当前配置（分钟 / 开关），设置面板挂载时回显。 */
export interface SettingsPayload {
  /** 每日屏幕时间上限（分钟）。 */
  daily_cap_minutes: number;
  /** 连续使用提醒阈值（分钟）。 */
  continuous_limit_minutes: number;
  /** 提醒总开关。 */
  reminders_enabled: boolean;
}

/** 单个应用的使用时长统计（今日排行的一项）。 */
export interface AppUsageStat {
  /** 应用进程名（不含路径与扩展名，小写）。 */
  app_name: string;
  /** 当日累计使用秒数。 */
  seconds: number;
}

/** 今日使用摘要（`get_analytics` 返回）。 */
export interface UsageSummary {
  /** 今日屏幕使用总秒数（已落库 use 段合计）。 */
  screen_seconds_today: number;
  /** 今日有效休息次数（离座 ≥ 2 分钟）。 */
  breaks_today: number;
  /** 今日提醒次数（两类合计）。 */
  reminders_today: number;
  /** 今日最长连续使用秒数。 */
  longest_streak_secs: number;
  /** 今日应用使用排行（前 10，按秒数降序）。 */
  top_apps: AppUsageStat[];
}

/** 单日屏幕使用统计（周报的一项）。 */
export interface ScreenDailyStat {
  /** 日期标签（本地时区 `YYYY-MM-DD`）。 */
  date: string;
  /** 当日屏幕使用总秒数。 */
  screen_seconds: number;
}

/** 近 7 日屏幕使用周报（按日期升序，末项为今天）。 */
export interface ScreenWeeklyReport {
  days: ScreenDailyStat[];
}
