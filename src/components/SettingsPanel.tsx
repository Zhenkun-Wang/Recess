import { useEffect, useState } from "react";
import { enable, disable, isEnabled } from "@tauri-apps/plugin-autostart";

import { updateSettings, getSettings, clearHistoryData } from "../api";

/** 配置范围（分钟），与后端 settings.rs 常量对齐：上限 [30,960]、阈值 [10,240]。 */
const CAP_MIN = 30;
const CAP_MAX = 960;
const LIMIT_MIN = 10;
const LIMIT_MAX = 240;

type Feedback = { kind: "ok" | "err"; text: string } | null;

/**
 * 设置面板：每日上限 / 连续阈值（数字输入 + 保存按钮）、提醒开关（即时生效）、
 * 开机自启（插件直接操作系统注册状态）。
 *
 * 挂载时 `get_settings` 回显当前配置，避免切页后"失忆"回退到硬编码默认值；
 * 数字项越界时后端拒绝并回传错误文案。
 */
export default function SettingsPanel() {
  const [cap, setCap] = useState(360);
  const [limit, setLimit] = useState(50);
  const [reminders, setReminders] = useState(true);
  const [saving, setSaving] = useState(false);
  const [togglingReminders, setTogglingReminders] = useState(false);
  const [feedback, setFeedback] = useState<Feedback>(null);

  // 挂载时从后端拉取真实配置并回显；失败仅 log，保留默认值。
  useEffect(() => {
    let cancelled = false;
    getSettings()
      .then((s) => {
        if (cancelled) return;
        setCap(s.daily_cap_minutes);
        setLimit(s.continuous_limit_minutes);
        setReminders(s.reminders_enabled);
      })
      .catch((e) => console.error("getSettings 失败:", e));
    return () => {
      cancelled = true;
    };
  }, []);

  // 开机自启：读取系统当前注册状态。
  const [autostart, setAutostart] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    isEnabled()
      .then((v) => {
        if (!cancelled) setAutostart(v);
      })
      .catch((e) => console.error("isEnabled 失败:", e));
    return () => {
      cancelled = true;
    };
  }, []);

  // 删除历史数据：当前正在清空的范围（null = 空闲），按钮互斥防连点。
  const [clearing, setClearing] = useState<"all" | "today" | null>(null);

  async function handleClearHistory(scope: "all" | "today") {
    const confirmed = window.confirm(
      scope === "all"
        ? "确定清空全部历史记录吗？此操作不可恢复（设置不受影响）。"
        : "确定清空今日的使用记录吗？此操作不可恢复。"
    );
    if (!confirmed) return;
    setClearing(scope);
    setFeedback(null);
    try {
      await clearHistoryData(scope);
      setFeedback({
        kind: "ok",
        text: scope === "all" ? "已清空全部历史记录。" : "已清空今日记录。",
      });
    } catch (e) {
      setFeedback({ kind: "err", text: `清空失败：${String(e)}` });
    } finally {
      setClearing(null);
    }
  }

  // 提醒开关：切换即生效（后端热更新引擎，无需保存按钮）。
  async function toggleReminders() {
    setTogglingReminders(true);
    setFeedback(null);
    const next = !reminders;
    try {
      await updateSettings({ remindersEnabled: next });
      setReminders(next);
      setFeedback({ kind: "ok", text: next ? "已开启提醒。" : "已关闭提醒。" });
    } catch (e) {
      setFeedback({ kind: "err", text: `更新提醒开关失败：${String(e)}` });
    } finally {
      setTogglingReminders(false);
    }
  }

  async function toggleAutostart() {
    setAutostartBusy(true);
    setFeedback(null);
    const next = !autostart;
    try {
      if (next) {
        await enable();
      } else {
        await disable();
      }
      setAutostart(next);
      setFeedback({
        kind: "ok",
        text: next ? "已开启开机自启。" : "已关闭开机自启。",
      });
    } catch (e) {
      setFeedback({ kind: "err", text: `设置开机自启失败：${String(e)}` });
    } finally {
      setAutostartBusy(false);
    }
  }

  const capValid = Number.isFinite(cap) && cap >= CAP_MIN && cap <= CAP_MAX;
  const limitValid = Number.isFinite(limit) && limit >= LIMIT_MIN && limit <= LIMIT_MAX;
  const canSave = capValid && limitValid && !saving;

  async function handleSave() {
    setSaving(true);
    setFeedback(null);
    try {
      await updateSettings({
        dailyCapMinutes: Math.round(cap),
        continuousLimitMinutes: Math.round(limit),
      });
      setFeedback({ kind: "ok", text: "设置已保存，下一拍立即生效。" });
    } catch (e) {
      setFeedback({ kind: "err", text: `保存失败：${String(e)}` });
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="mx-auto flex w-full max-w-md flex-col gap-6">
      <Field
        label="每日屏幕上限（分钟）"
        value={cap}
        min={CAP_MIN}
        max={CAP_MAX}
        valid={capValid}
        onChange={setCap}
        hint="若新上限低于今日已用时长，保存后将立即触发一次上限提醒，属预期行为。"
      />
      <Field
        label="连续使用提醒阈值（分钟）"
        value={limit}
        min={LIMIT_MIN}
        max={LIMIT_MAX}
        valid={limitValid}
        onChange={setLimit}
        hint="连续使用达到该时长即提醒休息；离开电脑 2 分钟可重置连续计时。"
      />

      <button
        type="button"
        onClick={handleSave}
        disabled={!canSave}
        className="rounded-xl bg-indigo-500 px-4 py-2.5 text-sm font-semibold text-white transition enabled:hover:bg-indigo-400 disabled:cursor-not-allowed disabled:opacity-40"
      >
        {saving ? "保存中…" : "保存设置"}
      </button>

      <SwitchRow
        title="提醒总开关"
        description="关闭后不再弹出任何提醒（今日页进度照常展示）"
        checked={reminders}
        busy={togglingReminders}
        onToggle={toggleReminders}
      />

      <SwitchRow
        title="开机自启"
        description="登录系统后自动在后台启动 · 删除程序前请先关闭"
        checked={autostart}
        busy={autostartBusy}
        onToggle={toggleAutostart}
      />

      {/* 数据管理：本地数据本地删，两档范围均带确认弹窗。 */}
      <div className="flex flex-col gap-3 rounded-xl border border-white/10 bg-white/5 px-4 py-4">
        <div>
          <span className="text-sm text-neutral-200">删除历史数据</span>
          <p className="mt-1 text-xs text-neutral-500">
            仅删除本地使用记录，不可恢复；今日计数立即归零，切回"今日"页自动刷新，设置不受影响。
          </p>
        </div>
        <div className="flex gap-2">
          <button
            type="button"
            onClick={() => handleClearHistory("today")}
            disabled={clearing !== null}
            className="rounded-xl border border-white/15 px-3 py-2 text-sm text-neutral-200 transition enabled:hover:border-white/30 enabled:hover:text-white disabled:cursor-not-allowed disabled:opacity-40"
          >
            {clearing === "today" ? "清空中…" : "仅清空今日"}
          </button>
          <button
            type="button"
            onClick={() => handleClearHistory("all")}
            disabled={clearing !== null}
            className="rounded-xl border border-rose-500/40 bg-rose-500/10 px-3 py-2 text-sm text-rose-300 transition enabled:hover:border-rose-500/60 enabled:hover:text-rose-200 disabled:cursor-not-allowed disabled:opacity-40"
          >
            {clearing === "all" ? "清空中…" : "清空全部历史"}
          </button>
        </div>
      </div>

      {feedback && (
        <p
          className={
            feedback.kind === "ok"
              ? "text-sm text-emerald-400"
              : "text-sm text-rose-400"
          }
        >
          {feedback.text}
        </p>
      )}
    </section>
  );
}

interface FieldProps {
  label: string;
  value: number;
  min: number;
  max: number;
  valid: boolean;
  onChange: (v: number) => void;
  hint?: string;
}

function Field({ label, value, min, max, valid, onChange, hint }: FieldProps) {
  return (
    <label className="flex flex-col gap-2">
      <span className="text-sm text-neutral-300">{label}</span>
      <input
        type="number"
        min={min}
        max={max}
        value={Number.isNaN(value) ? "" : value}
        onChange={(e) => onChange(e.target.valueAsNumber)}
        className={`rounded-xl border bg-white/5 px-4 py-2.5 text-white outline-none transition tabular ${
          valid
            ? "border-white/10 focus:border-indigo-400"
            : "border-rose-500/60 focus:border-rose-400"
        }`}
      />
      {!valid ? (
        <span className="text-xs text-rose-400">
          请输入 {min}–{max} 之间的整数
        </span>
      ) : (
        hint && <span className="text-xs text-neutral-500">{hint}</span>
      )}
    </label>
  );
}

interface SwitchRowProps {
  title: string;
  description: string;
  checked: boolean;
  busy: boolean;
  onToggle: () => void;
}

function SwitchRow({ title, description, checked, busy, onToggle }: SwitchRowProps) {
  return (
    <div className="flex items-center justify-between rounded-xl border border-white/10 bg-white/5 px-4 py-3">
      <div className="flex flex-col gap-0.5 pr-4">
        <span className="text-sm text-neutral-200">{title}</span>
        <span className="text-xs text-neutral-500">{description}</span>
      </div>
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        onClick={onToggle}
        disabled={busy}
        className={`relative h-6 w-11 shrink-0 rounded-full transition disabled:opacity-40 ${
          checked ? "bg-indigo-500" : "bg-white/15"
        }`}
      >
        <span
          className={`absolute top-0.5 h-5 w-5 rounded-full bg-white transition-all ${
            checked ? "left-[22px]" : "left-0.5"
          }`}
        />
      </button>
    </div>
  );
}
