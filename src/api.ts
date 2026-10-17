/**
 * Tauri 命令调用封装：集中管理 IPC 契约，组件只依赖此模块，不散落 `invoke` 字符串。
 */

import { invoke } from "@tauri-apps/api/core";
import type { ScreenWeeklyReport, SettingsPayload, UsageStatus, UsageSummary } from "./types";

/** 读取使用状态快照（首屏加载用；实时更新走 `tick` 事件）。 */
export function getStatus(): Promise<UsageStatus> {
  return invoke<UsageStatus>("get_status");
}

/** 读取今日使用摘要（屏幕秒 / 休息 / 提醒 / 最长连续 / 应用排行）。 */
export function getAnalytics(): Promise<UsageSummary> {
  return invoke<UsageSummary>("get_analytics");
}

/** 读取近 7 日屏幕使用周报。 */
export function getWeeklyReport(): Promise<ScreenWeeklyReport> {
  return invoke<ScreenWeeklyReport>("get_weekly_report");
}

/** 读取当前配置，供设置面板挂载时回显。 */
export function getSettings(): Promise<SettingsPayload> {
  return invoke<SettingsPayload>("get_settings");
}

/**
 * 更新配置：任一参数可省略（保持不变）；越界后端返回 Err。
 * 写库成功后引擎热更新，下一拍立即生效（上限调低到低于今日已用时
 * 下一拍即提醒，属预期行为）。
 */
export function updateSettings(args: {
  dailyCapMinutes?: number;
  continuousLimitMinutes?: number;
  remindersEnabled?: boolean;
}): Promise<void> {
  return invoke<void>("update_settings", {
    dailyCapMinutes: args.dailyCapMinutes,
    continuousLimitMinutes: args.continuousLimitMinutes,
    remindersEnabled: args.remindersEnabled,
  });
}

/**
 * 清空历史数据："all" 清空全部（保留设置）；"today" 仅清今日。
 * 后端同拍重置引擎今日计数（防止未冲刷增量写回），今日页由下一秒 tick 自动归零。
 */
export function clearHistoryData(scope: "all" | "today"): Promise<void> {
  return invoke<void>("clear_history_data", { scope });
}
