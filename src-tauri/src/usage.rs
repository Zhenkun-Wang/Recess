// 使用引擎：每秒一拍，由调用方喂入 (墙钟时间, 系统空闲秒数, 前台应用名)，
// 输出状态变化事件（离座关段 / 休息判定 / 提醒触发 / 跨日重置）。
//
// 纯逻辑层，不依赖 Tauri / 数据库 / Win32，便于完整单测覆盖。
// 前台应用由调用方采样并解析为进程名后喂入（PID → 名称的缓存归 foreground.rs），
// 引擎按应用名累计"待落库"秒数增量，上层定期 drain 并 UPSERT 到 app_usage 表。
//
// 核心规则：
// - 空闲 < AWAY_SECS  → "在用"：每拍累加今日屏幕秒与连续使用秒，并归属到前台应用
//   （含纯阅读/观看等无输入但人在屏幕前的场景——规范口径"在用即累计"）。
// - 空闲 ≥ AWAY_SECS  → "离座"：不计秒；关段终点回溯到最后输入时刻
//   （段 end = now - idle_secs，精确不虚增）。
// - 离座 ≥ BREAK_SECS → 记一次休息并清零连续秒；60–120s 短暂离座只暂停不清零。
// - 墙钟跳变 > WAKE_GAP_SECS（休眠唤醒）→ 以"上一拍墙钟"关段，视为离座起点，不算休息。
// - 本地日期变化（含回退）→ 冲刷并重置今日计数；正常跨午夜时在用段精确切在午夜边界，
//   待落库的应用增量随 DateChanged 事件以旧日期带走。

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone};

/// 空闲达到该秒数即判定"离座"。
pub const AWAY_SECS: u64 = 60;
/// 离座达到该秒数即判定"有效休息"（清零连续使用）。
pub const BREAK_SECS: u64 = 120;
/// 相邻两拍墙钟差超过该秒数视为休眠唤醒（GetTickCount 休眠不走，必须靠墙钟补）。
pub const WAKE_GAP_SECS: i64 = 30;
/// 连续使用提醒重复间隔（仍连续使用时每 10 分钟再弹一次）。
pub const CONTINUOUS_REPEAT_SECS: i64 = 600;
/// 每日上限提醒重复间隔（达上限后每 30 分钟再弹一次）。
pub const CAP_REPEAT_SECS: i64 = 1800;

/// 提醒种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderKind {
    /// 连续使用达到阈值。
    Continuous,
    /// 当日屏幕时长达到上限。
    DailyCap,
}

/// 引擎每拍可能产出的事件（供上层落库 / 弹提醒 / 广播）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageEvent {
    /// 一段"在用"区间已闭合（含回溯关段；start/end 为本地时间）。
    UseSegmentClosed {
        start: DateTime<Local>,
        end: DateTime<Local>,
    },
    /// 一次有效休息（离座达到 BREAK_SECS）。
    Break {
        start: DateTime<Local>,
        end: DateTime<Local>,
    },
    /// 需要弹出提醒。
    Reminder { kind: ReminderKind },
    /// 本地日期发生变化（跨午夜或时钟回拨），今日计数已重置；
    /// `old_date` 为变化前日期；`app_usage` 为尚未落库的各应用秒数增量，
    /// 上层必须以 `old_date` 冲刷入库（引擎已清空，不会重复产出）。
    DateChanged {
        old_date: NaiveDate,
        app_usage: BTreeMap<String, i64>,
    },
}

/// 使用引擎状态机。调用方每秒调用一次 [`UsageEngine::tick`]。
#[derive(Debug)]
pub struct UsageEngine {
    /// 当前本地日期（跨午夜 / 时钟回拨检测基准）。
    date: NaiveDate,
    /// 当日已累计屏幕秒数（启动时由基线初始化，之后只加增量）。
    today_seconds: i64,
    /// 当前连续使用秒数（有效休息或跨日后清零）。
    consecutive_seconds: i64,
    /// 当日最长连续使用秒数。
    longest_consecutive: i64,
    /// 当前是否处于"在用"状态。
    active: bool,
    /// 当前在用段的起点（None = 未在段内）。
    segment_start: Option<DateTime<Local>>,
    /// 离座起点（用于休息判定与回溯）。
    away_start: Option<DateTime<Local>>,
    /// 上一拍墙钟时刻（休眠跳变检测）。
    last_wall: Option<DateTime<Local>>,
    /// 尚未落库的各应用秒数增量（自上次冲刷起；由上层定期 drain）。
    app_usage: BTreeMap<String, i64>,
    /// 提醒总开关。
    reminders_enabled: bool,
    /// 每日上限（秒）。
    daily_cap_secs: i64,
    /// 连续使用阈值（秒）。
    continuous_limit_secs: i64,
    /// 上次连续使用提醒时刻（冷却控制；休息后重置）。
    last_continuous_reminder: Option<DateTime<Local>>,
    /// 上次上限提醒时刻（冷却控制；跨日后重置）。
    last_cap_reminder: Option<DateTime<Local>>,
}

impl UsageEngine {
    pub fn new(
        now: DateTime<Local>,
        daily_cap_secs: i64,
        continuous_limit_secs: i64,
        reminders_enabled: bool,
    ) -> Self {
        Self {
            date: now.date_naive(),
            today_seconds: 0,
            consecutive_seconds: 0,
            longest_consecutive: 0,
            active: false,
            segment_start: None,
            away_start: None,
            last_wall: None,
            app_usage: BTreeMap::new(),
            reminders_enabled,
            daily_cap_secs,
            continuous_limit_secs,
            last_continuous_reminder: None,
            last_cap_reminder: None,
        }
    }

    /// 启动基线：从数据库重建今日已累计屏幕秒（内存之后只加增量）。
    ///
    /// 各应用时长无需基线——已落库部分由上层查询直接读库，引擎只管 pending 增量。
    pub fn set_baseline(&mut self, secs: i64) {
        self.today_seconds = secs.max(0);
    }

    /// 设置热更新：修改上限与开关（下一拍立即生效）。
    pub fn update_limits(
        &mut self,
        daily_cap_secs: i64,
        continuous_limit_secs: i64,
        reminders_enabled: bool,
    ) {
        self.daily_cap_secs = daily_cap_secs;
        self.continuous_limit_secs = continuous_limit_secs;
        self.reminders_enabled = reminders_enabled;
    }

    pub fn today_seconds(&self) -> i64 {
        self.today_seconds
    }

    pub fn consecutive_seconds(&self) -> i64 {
        self.consecutive_seconds
    }

    pub fn longest_consecutive(&self) -> i64 {
        self.longest_consecutive
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 当前每日上限（秒，供 tick 负载与前端展示）。
    pub fn daily_cap_secs(&self) -> i64 {
        self.daily_cap_secs
    }

    /// 当前连续使用阈值（秒）。
    pub fn continuous_limit_secs(&self) -> i64 {
        self.continuous_limit_secs
    }

    /// 提醒总开关当前值。
    pub fn reminders_enabled(&self) -> bool {
        self.reminders_enabled
    }

    pub fn date(&self) -> NaiveDate {
        self.date
    }

    /// 取走尚未落库的各应用秒数增量并清空（P4 接线：每 60s 定期冲刷与退出冲刷）。
    ///
    /// 跨日时的 pending 增量已随 [`UsageEvent::DateChanged`] 事件带走，此处只会
    /// 取到当前日期的增量。
    pub fn drain_app_usage(&mut self) -> BTreeMap<String, i64> {
        std::mem::take(&mut self.app_usage)
    }

    /// 用户清空历史数据后重置今日计数（设置面板"删除历史数据"，两种范围共用）。
    ///
    /// - 计数归零、丢弃 pending 应用增量：否则下一次冲刷会把清空前的秒数写回（幽灵数据）。
    /// - 丢弃未闭合的在用段：段的起点在清空之前，若保留，关段时会写回跨清空时刻的整段
    ///   （上层调用方已保证持引擎锁期间完成清库，此处与数据库同拍生效）。
    /// - 提醒冷却一并重置；日期与上一拍墙钟保留（跨日 / 休眠检测不受影响）。
    pub fn reset_today(&mut self) {
        self.today_seconds = 0;
        self.consecutive_seconds = 0;
        self.longest_consecutive = 0;
        self.active = false;
        self.segment_start = None;
        self.away_start = None;
        self.last_continuous_reminder = None;
        self.last_cap_reminder = None;
        self.app_usage.clear();
    }

    /// 推进一拍。
    ///
    /// - `idle_secs`：系统空闲秒数（GetLastInputInfo 推算）。
    /// - `foreground`：当前前台应用进程名；`None`（桌面/无法归属）仍计屏幕秒，
    ///   只是不归属到任何应用。
    ///
    /// 返回本拍产出的事件（可能为空，顺序即因果顺序）。
    pub fn tick(
        &mut self,
        now: DateTime<Local>,
        idle_secs: u64,
        foreground: Option<&str>,
    ) -> Vec<UsageEvent> {
        let mut events = Vec::new();

        // ① 本地日期变化（跨午夜或时钟回拨）：冲刷并重置今日计数。
        let today = now.date_naive();
        if today != self.date {
            // 正常跨午夜：在用段精确切在午夜边界（按日期聚合 / 基线重建不串日）；
            // 时钟回拨：直接丢弃未闭合段（避免产生 end < start 的脏数据）。
            if today > self.date {
                self.close_segment(local_midnight(today), &mut events);
            } else {
                self.segment_start = None;
            }
            // 待落库的应用增量随事件以旧日期带走，上层负责冲刷入库。
            let app_usage = std::mem::take(&mut self.app_usage);
            events.push(UsageEvent::DateChanged {
                old_date: self.date,
                app_usage,
            });
            self.date = today;
            self.today_seconds = 0;
            self.consecutive_seconds = 0;
            self.longest_consecutive = 0;
            self.active = false;
            self.segment_start = None;
            self.away_start = None;
            self.last_wall = None;
            self.last_continuous_reminder = None;
            self.last_cap_reminder = None;
            return events;
        }

        // ② 休眠唤醒（墙钟跳变 > 30s，而 GetTickCount 不走）：
        //    以"上一拍墙钟"关段，休眠不算休息也不计屏幕时间。
        if let Some(last_wall) = self.last_wall {
            if (now - last_wall).num_seconds() > WAKE_GAP_SECS {
                self.close_segment(last_wall, &mut events);
                self.away_start = None;
                // 唤醒后按本拍空闲值重新判定，此处只确保段已干净关闭。
                self.active = false;
            }
        }
        self.last_wall = Some(now);

        // ③ 在用判定与累计：空闲 < AWAY_SECS 即"在用"，每拍计 1 秒
        //    （含纯阅读/观看等无输入但人在屏幕前的场景，规范口径）。
        let last_input = now - Duration::seconds(idle_secs as i64);

        if idle_secs < AWAY_SECS {
            if !self.active {
                // 离座 → 在用：先结算离座时长，达到 BREAK_SECS 判一次休息。
                if let Some(away_start) = self.away_start {
                    let away_secs = (last_input - away_start).num_seconds();
                    if away_secs >= BREAK_SECS as i64 {
                        events.push(UsageEvent::Break {
                            start: away_start,
                            end: last_input,
                        });
                        self.consecutive_seconds = 0;
                        self.last_continuous_reminder = None; // 休息后重新武装连续提醒
                    }
                    // 60–120s 短暂离座：只暂停，不清零连续秒。
                }
                self.away_start = None;
                self.segment_start = Some(last_input);
                self.active = true;
                // 开段首拍不计秒（段起点即恢复输入时刻，次拍起每拍计 1 秒）。
            } else {
                // 持续在用：每拍计 1 秒。
                self.today_seconds += 1;
                self.consecutive_seconds += 1;
                if let Some(app) = foreground {
                    *self.app_usage.entry(app.to_owned()).or_insert(0) += 1;
                }
                if self.consecutive_seconds > self.longest_consecutive {
                    self.longest_consecutive = self.consecutive_seconds;
                }
            }
        } else {
            // 空闲 ≥ AWAY_SECS：离座。
            if self.active {
                // 回溯关段：段终点 = 最后输入时刻，而非"现在"。
                self.close_segment(last_input, &mut events);
                self.away_start = Some(last_input);
                self.active = false;
            }
            // 已在离座中：无需动作，休息判定在恢复"在用"或跨日时结算，
            // 以便精确以最后输入时刻为界。
        }

        // ④ 提醒判定（仅在"在用"且开关打开时）。
        if self.active && self.reminders_enabled {
            if self.consecutive_seconds >= self.continuous_limit_secs {
                let due = match self.last_continuous_reminder {
                    Some(t) => (now - t).num_seconds() >= CONTINUOUS_REPEAT_SECS,
                    None => true,
                };
                if due {
                    self.last_continuous_reminder = Some(now);
                    events.push(UsageEvent::Reminder {
                        kind: ReminderKind::Continuous,
                    });
                }
            }
            if self.today_seconds >= self.daily_cap_secs {
                let due = match self.last_cap_reminder {
                    Some(t) => (now - t).num_seconds() >= CAP_REPEAT_SECS,
                    None => true,
                };
                if due {
                    self.last_cap_reminder = Some(now);
                    events.push(UsageEvent::Reminder {
                        kind: ReminderKind::DailyCap,
                    });
                }
            }
        }

        events
    }

    /// 关闭当前在用段（若在段内），产出 UseSegmentClosed 事件。
    fn close_segment(&mut self, end: DateTime<Local>, events: &mut Vec<UsageEvent>) {
        if let Some(start) = self.segment_start.take() {
            events.push(UsageEvent::UseSegmentClosed { start, end });
        }
    }

    /// 冲刷：闭合尚未关闭的在用段（退出/重启前调用），返回事件。
    ///
    /// 注意：各应用 pending 增量不在此事件中，退出时上层还需调用
    /// [`UsageEngine::drain_app_usage`] 一并落库。
    pub fn flush(&mut self, now: DateTime<Local>) -> Vec<UsageEvent> {
        let mut events = Vec::new();
        self.close_segment(now, &mut events);
        self.active = false;
        events
    }
}

/// 本地时区下某日 00:00 的时刻（供跨午夜关段等需要精确边界的场景）。
/// 夏令时跳变等极端情况下退化为当日 03:00，保证可用。
pub fn local_midnight(date: NaiveDate) -> DateTime<Local> {
    let naive = NaiveDateTime::new(date, chrono::NaiveTime::from_hms_opt(0, 0, 0).expect("00:00 合法"));
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => dt,
        chrono::LocalResult::None => Local
            .from_local_datetime(
                &NaiveDateTime::new(date, chrono::NaiveTime::from_hms_opt(3, 0, 0).expect("03:00 合法")),
            )
            .single()
            .expect("03:00 本地时间必存在"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造固定偏移的"本地时间"（测试中 Local 即系统时区，直接用 FixedOffset 转）。
    fn t(secs: i64) -> DateTime<Local> {
        let base = chrono::FixedOffset::east_opt(8 * 3600)
            .expect("UTC+8 合法")
            .with_ymd_and_hms(2026, 8, 31, 10, 0, 0)
            .single()
            .expect("基准时刻合法");
        (base + Duration::seconds(secs)).with_timezone(&Local)
    }

    /// 次日 00:00:05（用于跨午夜测试）。
    fn next_day() -> DateTime<Local> {
        chrono::FixedOffset::east_opt(8 * 3600)
            .expect("UTC+8 合法")
            .with_ymd_and_hms(2026, 9, 1, 0, 0, 5)
            .single()
            .expect("次日时刻合法")
            .with_timezone(&Local)
    }

    fn engine(now: DateTime<Local>) -> UsageEngine {
        // 上限 6 小时、连续阈值 50 分钟、开启提醒。
        UsageEngine::new(now, 6 * 3600, 50 * 60, true)
    }

    #[test]
    fn active_typing_accumulates_one_second_per_tick() {
        let mut e = engine(t(0));
        for i in 1..=10 {
            let events = e.tick(t(i), 2, Some("code")); // 空闲 2s < 60s，视为在用
            assert!(matches!(events.as_slice(), [] | [UsageEvent::Reminder { .. }]));
        }
        // 首拍只开段不计秒，之后每拍计 1 秒。
        assert_eq!(e.today_seconds(), 9);
        assert_eq!(e.consecutive_seconds(), 9);
        assert_eq!(e.longest_consecutive(), 9);
        assert!(e.is_active());
    }

    /// 清空历史后 reset_today：计数与 pending 归零、丢弃未闭合段；
    /// 后续关段的起点是重置后的新拍（不写回清空前的幽灵段）。
    #[test]
    fn reset_today_clears_counters_pending_and_open_segment() {
        let mut e = engine(t(0));
        for i in 1..=5 {
            e.tick(t(i), 0, Some("code"));
        }
        assert_eq!(e.today_seconds(), 4); // 清空前先确认累计生效
        assert!(e.is_active());

        e.reset_today();
        assert_eq!(e.today_seconds(), 0);
        assert_eq!(e.consecutive_seconds(), 0);
        assert_eq!(e.longest_consecutive(), 0);
        assert!(!e.is_active());
        assert!(e.drain_app_usage().is_empty(), "pending 增量应已清空");

        // 重置后重新开段：t(6) 开段（首拍不计秒），t(7) 计 1 秒。
        e.tick(t(6), 0, Some("code"));
        e.tick(t(7), 0, Some("code"));
        assert_eq!(e.today_seconds(), 1);

        // 空闲逐拍增长（真实节奏），t(67) 拍空闲达 60s 触发离座关段；
        // 段的起点必须是 t(6)（重置后新开段），而非重置前的 t(1)（幽灵段）。
        let mut closed = None;
        for i in 8..=67 {
            for ev in e.tick(t(i), (i - 7) as u64, Some("code")) {
                if matches!(ev, UsageEvent::UseSegmentClosed { .. }) {
                    closed = Some(ev);
                }
            }
        }
        match closed.expect("应产出关段事件") {
            UsageEvent::UseSegmentClosed { start, end } => {
                assert_eq!(start, t(6), "关段起点应为重置后新开段的时刻");
                assert_eq!(end, t(7), "终点应回溯到最后输入时刻");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn away_closes_segment_retroactively_at_last_input() {
        let mut e = engine(t(0));
        // 打字到 t(5)，随后空闲逐拍增长（真实场景每拍间隔 1s）。
        for i in 1..=5 {
            e.tick(t(i), 0, Some("code"));
        }
        let mut closed = None;
        for i in 6..=65 {
            let events = e.tick(t(i), (i - 5) as u64, Some("code"));
            if let Some(ev) = events
                .iter()
                .find(|ev| matches!(ev, UsageEvent::UseSegmentClosed { .. }))
            {
                closed = Some(ev.clone());
            }
        }
        // 空闲在 t(65) 拍达到 60s：最后输入 = t(65)-60 = t(5)，段应关在 t(5) 而非 t(65)。
        let closed = closed.expect("应产出关段事件");
        match closed {
            UsageEvent::UseSegmentClosed { end, .. } => assert_eq!(end, t(5)),
            _ => unreachable!(),
        }
        assert!(!e.is_active());
    }

    #[test]
    fn short_away_pauses_but_keeps_consecutive() {
        let mut e = engine(t(0));
        for i in 1..=5 {
            e.tick(t(i), 0, Some("code"));
        }
        assert_eq!(e.consecutive_seconds(), 4);
        // 离座：空闲逐拍增长，最长 90s（< BREAK_SECS=120）。
        for i in 6..=95 {
            let events = e.tick(t(i), (i - 5) as u64, Some("code"));
            assert!(
                !events.iter().any(|ev| matches!(ev, UsageEvent::Break { .. })),
                "短暂离座不应判休息"
            );
        }
        // 回到电脑前：短暂离座不清零连续秒。
        let events = e.tick(t(96), 0, Some("code"));
        assert!(!events.iter().any(|ev| matches!(ev, UsageEvent::Break { .. })));
        e.tick(t(97), 0, Some("code"));
        // 连续秒未被清零：4（输入期）+ 59（空闲 1–59s 仍在屏幕前，计入）
        // + 1（恢复后 1 拍）= 64。
        assert_eq!(e.consecutive_seconds(), 64);
    }

    #[test]
    fn long_away_records_break_and_resets_consecutive() {
        let mut e = engine(t(0));
        for i in 1..=5 {
            e.tick(t(i), 0, Some("code"));
        }
        // 离座：空闲逐拍增长到 130s（≥ BREAK_SECS）。
        for i in 6..=135 {
            e.tick(t(i), (i - 5) as u64, Some("code"));
        }
        // 回到电脑前：结算出一次休息，段界为 [最后输入, 恢复输入]。
        let events = e.tick(t(136), 0, Some("code"));
        let brk = events
            .iter()
            .find_map(|ev| match ev {
                UsageEvent::Break { start, end } => Some((*start, *end)),
                _ => None,
            })
            .expect("应产出休息事件");
        assert_eq!(brk.0, t(5));
        assert_eq!(brk.1, t(136));
        e.tick(t(137), 0, Some("code"));
        assert_eq!(e.consecutive_seconds(), 1);
    }

    #[test]
    fn continuous_reminder_fires_repeats_and_rearms_after_break() {
        let mut e = UsageEngine::new(t(0), 24 * 3600, 10, true);
        // 连续打 10 秒达到阈值。
        let mut fired = 0;
        for i in 1..=12 {
            let events = e.tick(t(i), 0, None);
            fired += events
                .iter()
                .filter(|ev| matches!(ev, UsageEvent::Reminder { kind: ReminderKind::Continuous }))
                .count();
        }
        assert_eq!(fired, 1, "达到阈值时弹一次");

        // 冷却内（<600s）不重复（逐拍连续推进，避免触发休眠分支）。
        for i in 13..=610 {
            let events = e.tick(t(i), 0, None);
            assert!(!events
                .iter()
                .any(|ev| matches!(ev, UsageEvent::Reminder { kind: ReminderKind::Continuous })));
        }
        // 600s 冷却到期后再弹。
        let events = e.tick(t(611), 0, None);
        assert!(events
            .iter()
            .any(|ev| matches!(ev, UsageEvent::Reminder { kind: ReminderKind::Continuous })));

        // 一次有效休息后重新武装：连续推进到空闲 130s，回到电脑前结算休息。
        for i in 612..=700 {
            e.tick(t(i), 0, None);
        }
        for i in 701..=830 {
            e.tick(t(i), (i - 700) as u64, None);
        }
        let events = e.tick(t(831), 0, None);
        assert!(events.iter().any(|ev| matches!(ev, UsageEvent::Break { .. })));
        assert_eq!(e.consecutive_seconds(), 0);

        // 重新累计到阈值（10s）应立即再弹，且只弹一次。
        let mut fired_after = 0;
        for i in 832..=845 {
            let events = e.tick(t(i), 0, None);
            fired_after += events
                .iter()
                .filter(|ev| matches!(ev, UsageEvent::Reminder { kind: ReminderKind::Continuous }))
                .count();
        }
        assert_eq!(fired_after, 1);
    }

    #[test]
    fn daily_cap_reminder_fires_and_repeats_after_interval() {
        let mut e = UsageEngine::new(t(0), 100, 24 * 3600, true);
        e.set_baseline(99); // 基线接近上限
        let events = e.tick(t(1), 0, None);
        assert!(!events
            .iter()
            .any(|ev| matches!(ev, UsageEvent::Reminder { kind: ReminderKind::DailyCap })));
        let events = e.tick(t(2), 0, None);
        assert!(events
            .iter()
            .any(|ev| matches!(ev, UsageEvent::Reminder { kind: ReminderKind::DailyCap })), "今日达 100s 上限应弹");
        // 冷却内不重复（逐拍连续推进到 1799s 冷却差）。
        for i in 3..=1801 {
            let events = e.tick(t(i), 0, None);
            assert!(!events
                .iter()
                .any(|ev| matches!(ev, UsageEvent::Reminder { kind: ReminderKind::DailyCap })));
        }
        // 1800s 冷却到期后重复。
        let events = e.tick(t(1802), 0, None);
        assert!(events
            .iter()
            .any(|ev| matches!(ev, UsageEvent::Reminder { kind: ReminderKind::DailyCap })));
    }

    #[test]
    fn both_reminders_can_fire_in_same_tick() {
        // 双阈值同为 10s：同拍同时满足，两条提醒应在同一拍产出。
        let mut e = UsageEngine::new(t(0), 10, 10, true);
        for i in 1..=10 {
            let events = e.tick(t(i), 0, None);
            assert!(!events.iter().any(|ev| matches!(ev, UsageEvent::Reminder { .. })));
        }
        let events = e.tick(t(11), 0, None);
        let continuous = events
            .iter()
            .filter(|ev| matches!(ev, UsageEvent::Reminder { kind: ReminderKind::Continuous }))
            .count();
        let cap = events
            .iter()
            .filter(|ev| matches!(ev, UsageEvent::Reminder { kind: ReminderKind::DailyCap }))
            .count();
        assert_eq!((continuous, cap), (1, 1), "双提醒应同拍各弹一次");
    }

    #[test]
    fn reminders_disabled_emit_none() {
        let mut e = UsageEngine::new(t(0), 10, 10, false);
        for i in 1..=30 {
            let events = e.tick(t(i), 0, None);
            assert!(!events.iter().any(|ev| matches!(ev, UsageEvent::Reminder { .. })));
        }
        assert!(e.today_seconds() >= 10 && e.consecutive_seconds() >= 10);
    }

    #[test]
    fn sleep_gap_closes_segment_at_last_wall_without_break() {
        let mut e = engine(t(0));
        e.tick(t(1), 0, Some("code"));
        e.tick(t(2), 0, Some("code"));
        // 墙钟跳 40 分钟（休眠），空闲值仍是休眠前的小值。
        let events = e.tick(t(2 + 2400), 3, Some("code"));
        let closed = events
            .iter()
            .find_map(|ev| match ev {
                UsageEvent::UseSegmentClosed { start, end } => Some((*start, *end)),
                _ => None,
            })
            .expect("休眠跳变应关段");
        // 段关在上一拍墙钟，而非休眠前最后输入（保守不丢不增）。
        assert_eq!(closed.1, t(2));
        assert!(
            !events.iter().any(|ev| matches!(ev, UsageEvent::Break { .. })),
            "休眠不算休息"
        );
        // 唤醒当拍重新开段但不补计休眠期间的秒数。
        let today_after_wake = e.today_seconds();
        e.tick(t(2 + 2401), 0, Some("code"));
        assert_eq!(
            e.today_seconds(),
            today_after_wake + 1,
            "唤醒后恢复在用，每拍仍只计 1 秒"
        );
    }

    #[test]
    fn midnight_rollover_closes_segment_and_resets_daily_counters() {
        let mut e = engine(t(0));
        for i in 1..=5 {
            e.tick(t(i), 0, Some("code"));
        }
        assert!(e.today_seconds() > 0);
        let events = e.tick(next_day(), 0, Some("code"));
        assert!(events
            .iter()
            .any(|ev| matches!(ev, UsageEvent::DateChanged { old_date, .. } if *old_date == NaiveDate::from_ymd_opt(2026, 8, 31).expect("日期合法"))));
        // 在用段应精确切在午夜边界（次日 00:00:00），而非跨日拍的当前时刻。
        let midnight = local_midnight(NaiveDate::from_ymd_opt(2026, 9, 1).expect("日期合法"));
        assert!(events
            .iter()
            .any(|ev| matches!(ev, UsageEvent::UseSegmentClosed { end, .. } if *end == midnight)));
        assert_eq!(e.today_seconds(), 0);
        assert_eq!(e.consecutive_seconds(), 0);
        assert_eq!(e.date(), NaiveDate::from_ymd_opt(2026, 9, 1).expect("日期合法"));
        // 新日期提醒冷却已重置：小阈值下可立即再达条件。
    }

    #[test]
    fn date_backward_jump_resets_as_well() {
        let mut e = engine(t(0));
        for i in 1..=5 {
            e.tick(t(i), 0, Some("code"));
        }
        // 时钟回拨到前一天（如系统时间被手动改回）。
        let prev_day = chrono::FixedOffset::east_opt(8 * 3600)
            .expect("UTC+8 合法")
            .with_ymd_and_hms(2026, 8, 30, 23, 0, 0)
            .single()
            .expect("前一日时刻合法")
            .with_timezone(&Local);
        let events = e.tick(prev_day, 0, Some("code"));
        assert!(events
            .iter()
            .any(|ev| matches!(ev, UsageEvent::DateChanged { .. })));
        assert_eq!(e.today_seconds(), 0);
    }

    #[test]
    fn flush_closes_open_segment() {
        let mut e = engine(t(0));
        e.tick(t(1), 0, Some("code"));
        e.tick(t(2), 0, Some("code"));
        let events = e.flush(t(3));
        assert!(events
            .iter()
            .any(|ev| matches!(ev, UsageEvent::UseSegmentClosed { end, .. } if *end == t(3))));
        assert!(!e.is_active());
    }

    #[test]
    fn local_midnight_is_start_of_day() {
        let d = NaiveDate::from_ymd_opt(2026, 8, 31).expect("日期合法");
        let m = local_midnight(d);
        assert_eq!(m.date_naive(), d);
        assert_eq!(m.time(), chrono::NaiveTime::from_hms_opt(0, 0, 0).expect("00:00 合法"));
    }

    #[test]
    fn app_usage_accumulates_per_foreground_app() {
        let mut e = engine(t(0));
        // 前 5 拍在 code，随后切到 browser。
        for i in 1..=5 {
            e.tick(t(i), 0, Some("code"));
        }
        for i in 6..=8 {
            e.tick(t(i), 0, Some("browser"));
        }
        let usage = e.drain_app_usage();
        assert_eq!(usage.get("code"), Some(&4), "code 计 4 秒");
        assert_eq!(usage.get("browser"), Some(&3), "browser 计 3 秒");
        assert_eq!(e.today_seconds(), 7, "应用秒数之和等于今日屏幕秒");
        // 冲刷后 pending 清零，之后继续累计。
        assert!(e.drain_app_usage().is_empty());
        e.tick(t(9), 0, Some("code"));
        let again = e.drain_app_usage();
        assert_eq!(again.get("code"), Some(&1));
    }

    #[test]
    fn app_usage_skips_unattributed_and_away() {
        let mut e = engine(t(0));
        // 前台为桌面（None）：仍计屏幕秒，但不归属任何应用。
        for i in 1..=3 {
            e.tick(t(i), 0, None);
        }
        // 停止输入但仍在屏幕前（空闲 1–59s < 60s）：计入屏幕秒并归属前台应用；
        // i=63 起空闲 ≥ 60s → 离座（不计不归属）。
        for i in 4..=70 {
            e.tick(t(i), (i - 3) as u64, Some("code"));
        }
        // 回到电脑前：code 继续累计恢复在用后的秒。
        for i in 71..=73 {
            e.tick(t(i), 0, Some("code"));
        }
        let usage = e.drain_app_usage();
        assert_eq!(usage.get("code"), Some(&61), "在用期间（含无输入阅读）均归属 code");
        assert_eq!(e.today_seconds(), 63, "桌面 2 秒 + 在用阅读 59 秒 + code 2 秒");
    }

    #[test]
    fn date_change_carries_pending_app_usage_for_old_date() {
        let mut e = engine(t(0));
        for i in 1..=5 {
            e.tick(t(i), 0, Some("code"));
        }
        // 跨午夜：pending 应用增量应随事件以旧日期带走，引擎内清空。
        let events = e.tick(next_day(), 0, Some("code"));
        let carried = events
            .iter()
            .find_map(|ev| match ev {
                UsageEvent::DateChanged { app_usage, .. } => Some(app_usage.clone()),
                _ => None,
            })
            .expect("应有 DateChanged 事件");
        assert_eq!(carried.get("code"), Some(&4), "旧日期的 pending 增量随事件带走");
        assert!(e.drain_app_usage().is_empty(), "引擎内 pending 已清空");
    }
}
