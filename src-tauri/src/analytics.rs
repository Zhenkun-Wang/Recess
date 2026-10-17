//! 使用统计聚合查询
//!
//! 从 `sessions`（use / break / reminder 段）与 `app_usage` 表统计今日使用摘要、
//! 应用排行与近 7 日屏幕秒数，供前端 `get_analytics` / `get_weekly_report` 命令读取；
//! 并提供启动基线查询（今日已落库屏幕秒，供引擎重建内存基线）。
//!
//! 设计约束（遵循核心开发规则）：
//! - 聚合函数返回 `rusqlite::Result`，禁止 `unwrap()`；日期边界在 Rust 侧用 chrono
//!   计算后以 RFC3339 传入，SQL 内用 `strftime('%s', ...)` 转 UTC 纪元比较，
//!   规避时区/字符串比较歧义。
//! - 旧番茄钟的 `work` / `postponed` 行按类型过滤自然忽略，无需迁移。

use chrono::{DateTime, Duration as ChronoDuration, Local, Timelike};
use rusqlite::Connection;
use serde::Serialize;

use crate::usage::BREAK_SECS;

// ============================ 数据结构 ============================

/// 单个应用的使用时长统计（今日排行的一项）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AppUsageStat {
    /// 应用进程名（不含路径与扩展名，小写）。
    pub app_name: String,
    /// 当日累计使用秒数。
    pub seconds: i64,
}

/// 今日使用摘要（`get_analytics` 负载）。
///
/// 对应 TS 类型：
/// ```ts
/// interface UsageSummary {
///   screen_seconds_today: number;
///   breaks_today: number;
///   reminders_today: number;
///   longest_streak_secs: number;
///   top_apps: { app_name: string; seconds: number }[];
/// }
/// ```
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct UsageSummary {
    /// 今日屏幕使用总秒数（`use` 段时长和）。
    pub screen_seconds_today: i64,
    /// 今日有效休息次数（`type='break'` 且 `context='away'`，排除旧番茄钟 break 行）。
    pub breaks_today: i64,
    /// 今日提醒次数（`type='reminder'`，两种提醒合计）。
    pub reminders_today: i64,
    /// 今日最长连续使用秒数（相邻 use 段间隔 < BREAK_SECS 视为连续）。
    pub longest_streak_secs: i64,
    /// 今日应用使用排行（前 10，按秒数降序，同秒按名称升序）。
    pub top_apps: Vec<AppUsageStat>,
}

/// 单日屏幕使用统计（周报的一项）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ScreenDailyStat {
    /// 日期标签（本地时区 `YYYY-MM-DD`）。
    pub date: String,
    /// 当日屏幕使用总秒数。
    pub screen_seconds: i64,
}

/// 近 7 日屏幕使用周报（按日期升序排列，最后一项为今天）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ScreenWeeklyReport {
    pub days: Vec<ScreenDailyStat>,
}

// ============================ 时间边界 ============================

/// 计算“今天 00:00:00”对应的本地时刻。
///
/// 用“当前时刻减去当日已过秒数与亚秒纳秒”得到本地午夜，避免直接构造
/// `DateTime<Local>` 在 DST 切换日产生的歧义/不存在时刻。
pub fn start_of_today() -> DateTime<Local> {
    let now = Local::now();
    let secs = i64::from(now.num_seconds_from_midnight());
    let nanos = i64::from(now.nanosecond());
    now - ChronoDuration::seconds(secs) - ChronoDuration::nanoseconds(nanos)
}

/// 返回今天的 `[起, 止)` 边界（RFC3339 字符串）。
pub fn today_bounds() -> (String, String) {
    let start = start_of_today();
    let end = start + ChronoDuration::days(1);
    (start.to_rfc3339(), end.to_rfc3339())
}

// ============================ 聚合查询 ============================

/// 今日应用使用排行条数上限。
const TOP_APPS_LIMIT: i64 = 10;

/// 统计 `[start, end)` 内满足 `predicate` 的 `sessions` 行数。
///
/// `predicate` 仅接收**编译期常量**（本模块内部调用），不含用户输入，故字符串拼接安全。
fn count_sessions_where(
    conn: &Connection,
    start: &str,
    end: &str,
    predicate: &'static str,
) -> rusqlite::Result<i64> {
    let sql = format!(
        "SELECT COUNT(*) FROM sessions \
         WHERE {predicate} \
           AND strftime('%s', start_time) >= strftime('%s', ?1) \
           AND strftime('%s', start_time) <  strftime('%s', ?2)"
    );
    conn.query_row(&sql, rusqlite::params![start, end], |row| row.get(0))
}

/// 读取 `[start, end)` 内全部 `use` 段（起止纪元秒，按起始时间升序）。
fn use_segments(conn: &Connection, start: &str, end: &str) -> rusqlite::Result<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare(
        // strftime 返回 TEXT，直接取列必须 CAST 成 INTEGER（SUM 内则会隐式转数值）。
        "SELECT CAST(strftime('%s', start_time) AS INTEGER), \
                CAST(strftime('%s', end_time) AS INTEGER) \
         FROM sessions \
           WHERE type = 'use' \
           AND strftime('%s', start_time) >= strftime('%s', ?1) \
           AND strftime('%s', start_time) <  strftime('%s', ?2) \
         ORDER BY strftime('%s', start_time)",
    )?;
    let rows = stmt.query_map(rusqlite::params![start, end], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    rows.collect()
}

/// 由 use 段序列计算总秒数与最长连续秒数。
///
/// 相邻段间隔 < [`BREAK_SECS`]（即离座未达有效休息阈值）不打断连续使用，
/// 与引擎内 consecutive_seconds 的清零条件保持一致。
fn sum_and_longest_streak(segments: &[(i64, i64)]) -> (i64, i64) {
    let mut total = 0;
    let mut longest = 0;
    let mut streak = 0;
    let mut prev_end: Option<i64> = None;
    for &(s, e) in segments {
        let dur = (e - s).max(0);
        total += dur;
        streak = match prev_end {
            Some(pe) if s - pe < BREAK_SECS as i64 => streak + dur,
            _ => dur,
        };
        longest = longest.max(streak);
        prev_end = Some(e);
    }
    (total, longest)
}

/// 启动基线：`[start, end)` 内已落库的屏幕秒数（use 段 SUM）。
///
/// 引擎启动时以此为今日基线，内存只累计之后的增量；
/// 崩溃时未关闭的段不在库中，对应增量自然丢失（已知限制）。
pub fn today_use_seconds(conn: &Connection, day_start: &str, day_end: &str) -> rusqlite::Result<i64> {
    Ok(sum_and_longest_streak(&use_segments(conn, day_start, day_end)?).0)
}

/// 今日应用使用排行（前 10，按秒数降序，同秒按名称升序保证稳定）。
fn top_apps(conn: &Connection, today: &str) -> rusqlite::Result<Vec<AppUsageStat>> {
    let mut stmt = conn.prepare(
        "SELECT app_name, seconds FROM app_usage \
         WHERE date = ?1 \
         ORDER BY seconds DESC, app_name ASC \
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![today, TOP_APPS_LIMIT], |row| {
        Ok(AppUsageStat {
            app_name: row.get::<_, String>(0)?,
            seconds: row.get::<_, i64>(1)?,
        })
    })?;
    rows.collect()
}

/// 计算今日使用摘要（`get_analytics` 负载）。
///
/// `day_start` / `day_end` 为今日 `[起, 止)` RFC3339 边界；`today` 为本地日期
/// `YYYY-MM-DD`（`app_usage` 按日期字符串匹配）。
pub fn compute_usage_summary(
    conn: &Connection,
    day_start: &str,
    day_end: &str,
    today: &str,
) -> rusqlite::Result<UsageSummary> {
    let (screen_seconds_today, longest_streak_secs) =
        sum_and_longest_streak(&use_segments(conn, day_start, day_end)?);
    // context='away'：只统计新引擎的离座休息，自然排除旧番茄钟的 break 行。
    let breaks_today =
        count_sessions_where(conn, day_start, day_end, "type = 'break' AND context = 'away'")?;
    let reminders_today = count_sessions_where(conn, day_start, day_end, "type = 'reminder'")?;
    let top_apps = top_apps(conn, today)?;
    Ok(UsageSummary {
        screen_seconds_today,
        breaks_today,
        reminders_today,
        longest_streak_secs,
        top_apps,
    })
}

/// 计算近 7 日屏幕使用周报（按日期升序，最后一项为今天）。
pub fn compute_screen_weekly_report(
    conn: &Connection,
    today_start: DateTime<Local>,
) -> rusqlite::Result<ScreenWeeklyReport> {
    let mut days = Vec::with_capacity(7);
    for i in (0..7).rev() {
        let d_start = today_start - ChronoDuration::days(i);
        let d_end = d_start + ChronoDuration::days(1);
        days.push(ScreenDailyStat {
            date: d_start.format("%Y-%m-%d").to_string(),
            screen_seconds: today_use_seconds(conn, &d_start.to_rfc3339(), &d_end.to_rfc3339())?,
        });
    }
    Ok(ScreenWeeklyReport { days })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// 构建含 sessions / app_usage 两表的内存数据库（Schema 对齐 db.rs）。
    fn setup() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                start_time DATETIME NOT NULL,
                end_time DATETIME NOT NULL,
                type TEXT NOT NULL,
                context TEXT,
                completed BOOLEAN DEFAULT 1
            );
            CREATE TABLE app_usage (
                date     TEXT NOT NULL,
                app_name TEXT NOT NULL,
                seconds  INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (date, app_name)
            );",
        )
        .expect("create tables");
        conn
    }

    fn insert_session(conn: &Connection, start: &str, end: &str, ty: &str, ctx: &str) {
        conn.execute(
            "INSERT INTO sessions (start_time, end_time, type, context, completed) \
             VALUES (?1, ?2, ?3, ?4, 1)",
            rusqlite::params![start, end, ty, ctx],
        )
        .expect("insert session");
    }

    /// 用本地时区构造一个固定午夜，保证测试与运行机器时区无关。
    fn local_midnight(y: i32, mo: u32, d: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(y, mo, d, 0, 0, 0)
            .single()
            .expect("valid local midnight")
    }

    #[test]
    fn usage_summary_aggregates_new_types() {
        let conn = setup();
        let base = local_midnight(2026, 8, 31);
        let day_start = base.to_rfc3339();
        let day_end = (base + ChronoDuration::days(1)).to_rfc3339();

        // 三个 use 段：09:00–09:10、09:11–09:20（间隔 60s，连续）、11:00–11:10（间隔 100 分钟，断开）。
        let h = |n: i64| (base + ChronoDuration::hours(n)).to_rfc3339();
        let hm = |n: i64, m: i64| {
            (base + ChronoDuration::hours(n) + ChronoDuration::minutes(m)).to_rfc3339()
        };
        insert_session(&conn, &h(9), &hm(9, 10), "use", "normal");
        insert_session(&conn, &hm(9, 11), &hm(9, 20), "use", "normal");
        insert_session(&conn, &h(11), &hm(11, 10), "use", "normal");
        // 一次有效休息（context='away'）与两条提醒。
        insert_session(&conn, &hm(10, 0), &hm(10, 5), "break", "away");
        insert_session(&conn, &hm(9, 12), &hm(9, 12), "reminder", "continuous");
        insert_session(&conn, &hm(9, 13), &hm(9, 13), "reminder", "daily_cap");
        // 旧番茄钟 break 行（context 非 away）不计入新休息次数。
        insert_session(&conn, &hm(10, 30), &hm(10, 35), "break", "normal");
        // 昨日 use 段应被排除。
        insert_session(
            &conn,
            &(base - ChronoDuration::hours(2)).to_rfc3339(),
            &(base - ChronoDuration::hours(1)).to_rfc3339(),
            "use",
            "normal",
        );

        // 应用排行：code 经两次累加 600s、browser 300s、chat 300s（同秒按名称升序）。
        crate::db::upsert_app_usage(&conn, "2026-08-31", "code", 500).expect("upsert");
        crate::db::upsert_app_usage(&conn, "2026-08-31", "code", 100).expect("upsert");
        crate::db::upsert_app_usage(&conn, "2026-08-31", "browser", 300).expect("upsert");
        crate::db::upsert_app_usage(&conn, "2026-08-31", "chat", 300).expect("upsert");
        // 昨日行不计入今日排行。
        crate::db::upsert_app_usage(&conn, "2026-08-30", "code", 999).expect("upsert");

        let s =
            compute_usage_summary(&conn, &day_start, &day_end, "2026-08-31").expect("summary");
        assert_eq!(s.screen_seconds_today, 600 + 540 + 600);
        assert_eq!(s.breaks_today, 1, "只统计 context='away' 的新休息");
        assert_eq!(s.reminders_today, 2);
        assert_eq!(s.longest_streak_secs, 600 + 540, "间隔 60s 不打断连续");
        assert_eq!(s.top_apps.len(), 3);
        assert_eq!(s.top_apps[0].app_name, "code");
        assert_eq!(s.top_apps[0].seconds, 600);
        assert_eq!(s.top_apps[1].app_name, "browser", "同 300s 按名称升序");
        assert_eq!(s.top_apps[2].app_name, "chat");
    }

    #[test]
    fn today_use_seconds_baseline_excludes_other_types() {
        let conn = setup();
        let base = local_midnight(2026, 8, 31);
        let day_start = base.to_rfc3339();
        let day_end = (base + ChronoDuration::days(1)).to_rfc3339();
        // 30 分钟 use 段；随后的 break / reminder 行不参与基线。
        let s = (base + ChronoDuration::hours(9)).to_rfc3339();
        let e = (base + ChronoDuration::hours(9) + ChronoDuration::minutes(30)).to_rfc3339();
        insert_session(&conn, &s, &e, "use", "normal");
        insert_session(&conn, &e, &e, "break", "away");
        insert_session(&conn, &e, &e, "reminder", "daily_cap");
        assert_eq!(
            today_use_seconds(&conn, &day_start, &day_end).expect("baseline"),
            1800
        );
    }

    #[test]
    fn screen_weekly_report_sums_per_day() {
        let conn = setup();
        let today_start = local_midnight(2026, 8, 31);
        // 今天 20 分钟、昨天 10 分钟的 use 段。
        let s1 = (today_start + ChronoDuration::hours(9)).to_rfc3339();
        let e1 = (today_start + ChronoDuration::hours(9) + ChronoDuration::minutes(20)).to_rfc3339();
        insert_session(&conn, &s1, &e1, "use", "normal");
        let y_start = today_start - ChronoDuration::days(1);
        let s2 = (y_start + ChronoDuration::hours(10)).to_rfc3339();
        let e2 = (y_start + ChronoDuration::hours(10) + ChronoDuration::minutes(10)).to_rfc3339();
        insert_session(&conn, &s2, &e2, "use", "normal");

        let report = compute_screen_weekly_report(&conn, today_start).expect("weekly");
        assert_eq!(report.days.len(), 7);
        assert_eq!(report.days[6].date, "2026-08-31");
        assert_eq!(report.days[6].screen_seconds, 1200);
        assert_eq!(report.days[5].date, "2026-08-30");
        assert_eq!(report.days[5].screen_seconds, 600);
        assert_eq!(report.days[0].screen_seconds, 0, "无数据日为 0");
    }
}
