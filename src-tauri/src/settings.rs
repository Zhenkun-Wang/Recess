//! 用户配置读写
//!
//! `settings` 表为 key-value 结构（`key TEXT PRIMARY KEY, value TEXT NOT NULL`）。
//! 本模块提供通用 get/set（UPSERT），以及使用引擎三项配置的类型化读取与校验写入，
//! 并在启动时通过 [`ensure_defaults`] 播种默认值（不覆盖用户已改的值）、
//! 清理旧番茄钟遗留配置键。
//!
//! 设计约束（遵循核心开发规则）：
//! - 通用读写返回 `rusqlite::Result`，禁止 `unwrap()`。
//! - 类型化读取容错：读取/解析失败时回退默认值并记录日志，
//!   保证引擎在配置损坏时仍能运行。

use rusqlite::{Connection, OptionalExtension};

/// 每日屏幕时间上限配置键（存储值为分钟）。
pub const KEY_DAILY_CAP_MINUTES: &str = "daily_cap_minutes";
/// 连续使用提醒阈值配置键（存储值为分钟）。
pub const KEY_CONTINUOUS_LIMIT_MINUTES: &str = "continuous_limit_minutes";
/// 提醒总开关配置键（存储值为 "true" / "false"）。
pub const KEY_REMINDERS_ENABLED: &str = "reminders_enabled";

/// 默认每日上限（分钟）——6 小时。
pub const DEFAULT_DAILY_CAP_MINUTES: i64 = 360;
/// 默认连续使用阈值（分钟）——50 分钟。
pub const DEFAULT_CONTINUOUS_LIMIT_MINUTES: i64 = 50;
/// 默认开启提醒。
pub const DEFAULT_REMINDERS_ENABLED: bool = true;

/// 每日上限允许范围（分钟）：[30, 960]。
pub const MIN_DAILY_CAP_MINUTES: i64 = 30;
pub const MAX_DAILY_CAP_MINUTES: i64 = 960;
/// 连续使用阈值允许范围（分钟）：[10, 240]。
pub const MIN_CONTINUOUS_LIMIT_MINUTES: i64 = 10;
pub const MAX_CONTINUOUS_LIMIT_MINUTES: i64 = 240;

/// 读取指定配置项的原始字符串值；不存在时返回 `None`。
pub fn get_string(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        [key],
        |row| row.get::<_, String>(0),
    )
    .optional()
}

/// 写入（或更新）指定配置项（UPSERT）。
pub fn set_string(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, value],
    )?;
    Ok(())
}

/// 读取整数配置项；缺失或解析失败时返回 `default`（并记录日志）。
pub fn get_i64_or(conn: &Connection, key: &str, default: i64) -> i64 {
    match get_string(conn, key) {
        Ok(Some(s)) => match s.trim().parse::<i64>() {
            Ok(v) => v,
            Err(e) => {
                log::error!("配置项 {key} 值 \"{s}\" 解析为整数失败: {e}，使用默认值 {default}");
                default
            }
        },
        Ok(None) => default,
        Err(e) => {
            log::error!("读取配置项 {key} 失败: {e}，使用默认值 {default}");
            default
        }
    }
}

/// 当前每日屏幕时间上限（分钟）。
pub fn daily_cap_minutes(conn: &Connection) -> i64 {
    get_i64_or(conn, KEY_DAILY_CAP_MINUTES, DEFAULT_DAILY_CAP_MINUTES)
}

/// 当前每日屏幕时间上限（秒，供使用引擎）。
pub fn daily_cap_secs(conn: &Connection) -> i64 {
    daily_cap_minutes(conn) * 60
}

/// 当前连续使用提醒阈值（分钟）。
pub fn continuous_limit_minutes(conn: &Connection) -> i64 {
    get_i64_or(
        conn,
        KEY_CONTINUOUS_LIMIT_MINUTES,
        DEFAULT_CONTINUOUS_LIMIT_MINUTES,
    )
}

/// 当前连续使用提醒阈值（秒，供使用引擎）。
pub fn continuous_limit_secs(conn: &Connection) -> i64 {
    continuous_limit_minutes(conn) * 60
}

/// 提醒总开关。值损坏（手改 DB 等）时回退默认值，容错策略与 [`get_i64_or`] 一致。
pub fn reminders_enabled(conn: &Connection) -> bool {
    match get_string(conn, KEY_REMINDERS_ENABLED) {
        Ok(Some(s)) => match s.trim() {
            "true" => true,
            "false" => false,
            _ => DEFAULT_REMINDERS_ENABLED,
        },
        Ok(None) => DEFAULT_REMINDERS_ENABLED,
        Err(e) => {
            log::error!("读取配置项 {KEY_REMINDERS_ENABLED} 失败: {e}，使用默认值 {DEFAULT_REMINDERS_ENABLED}");
            DEFAULT_REMINDERS_ENABLED
        }
    }
}

/// 校验并写入每日上限（分钟），范围 [30, 960]。
pub fn set_daily_cap_minutes(conn: &Connection, minutes: i64) -> Result<(), String> {
    if !(MIN_DAILY_CAP_MINUTES..=MAX_DAILY_CAP_MINUTES).contains(&minutes) {
        return Err(format!(
            "每日上限 {minutes} 分钟超出允许范围 [{MIN_DAILY_CAP_MINUTES}, {MAX_DAILY_CAP_MINUTES}]"
        ));
    }
    set_string(conn, KEY_DAILY_CAP_MINUTES, &minutes.to_string())
        .map_err(|e| format!("写入配置项 {KEY_DAILY_CAP_MINUTES} 失败: {e}"))
}

/// 校验并写入连续使用阈值（分钟），范围 [10, 240]。
pub fn set_continuous_limit_minutes(conn: &Connection, minutes: i64) -> Result<(), String> {
    if !(MIN_CONTINUOUS_LIMIT_MINUTES..=MAX_CONTINUOUS_LIMIT_MINUTES).contains(&minutes) {
        return Err(format!(
            "连续使用阈值 {minutes} 分钟超出允许范围 [{MIN_CONTINUOUS_LIMIT_MINUTES}, {MAX_CONTINUOUS_LIMIT_MINUTES}]"
        ));
    }
    set_string(conn, KEY_CONTINUOUS_LIMIT_MINUTES, &minutes.to_string())
        .map_err(|e| format!("写入配置项 {KEY_CONTINUOUS_LIMIT_MINUTES} 失败: {e}"))
}

/// 写入提醒总开关。
pub fn set_reminders_enabled(conn: &Connection, enabled: bool) -> Result<(), String> {
    let value = if enabled { "true" } else { "false" };
    set_string(conn, KEY_REMINDERS_ENABLED, value)
        .map_err(|e| format!("写入配置项 {KEY_REMINDERS_ENABLED} 失败: {e}"))
}

/// 启动时播种默认配置：仅在键缺失时写入，不覆盖用户已修改的值；
/// 同时清理旧番茄钟遗留的配置键（引擎已切换，旧键不再读取）。
pub fn ensure_defaults(conn: &Connection) -> rusqlite::Result<()> {
    if get_string(conn, KEY_DAILY_CAP_MINUTES)?.is_none() {
        set_string(
            conn,
            KEY_DAILY_CAP_MINUTES,
            &DEFAULT_DAILY_CAP_MINUTES.to_string(),
        )?;
    }
    if get_string(conn, KEY_CONTINUOUS_LIMIT_MINUTES)?.is_none() {
        set_string(
            conn,
            KEY_CONTINUOUS_LIMIT_MINUTES,
            &DEFAULT_CONTINUOUS_LIMIT_MINUTES.to_string(),
        )?;
    }
    if get_string(conn, KEY_REMINDERS_ENABLED)?.is_none() {
        set_string(
            conn,
            KEY_REMINDERS_ENABLED,
            if DEFAULT_REMINDERS_ENABLED { "true" } else { "false" },
        )?;
    }
    // 清理旧番茄钟配置键（work_duration / break_duration）。
    conn.execute(
        "DELETE FROM settings WHERE key IN ('work_duration', 'break_duration')",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构建一个仅含 settings 表的内存数据库。
    fn setup() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            [],
        )
        .expect("create settings table");
        conn
    }

    #[test]
    fn set_get_roundtrip_and_upsert() {
        let conn = setup();
        assert_eq!(get_string(&conn, "k").expect("get"), None);

        set_string(&conn, "k", "1").expect("insert");
        assert_eq!(get_string(&conn, "k").expect("get").as_deref(), Some("1"));

        // 再次写入同一 key 应更新而非报主键冲突。
        set_string(&conn, "k", "2").expect("upsert");
        assert_eq!(get_string(&conn, "k").expect("get").as_deref(), Some("2"));
    }

    #[test]
    fn get_i64_or_falls_back_on_invalid_or_missing() {
        let conn = setup();
        set_string(&conn, "bad", "not_a_number").expect("set");
        assert_eq!(get_i64_or(&conn, "bad", 42), 42);
        assert_eq!(get_i64_or(&conn, "missing", 7), 7);
    }

    #[test]
    fn usage_keys_roundtrip_and_validation() {
        let conn = setup();
        // 缺失时回退默认值。
        assert_eq!(daily_cap_minutes(&conn), DEFAULT_DAILY_CAP_MINUTES);
        assert_eq!(daily_cap_secs(&conn), DEFAULT_DAILY_CAP_MINUTES * 60);
        assert_eq!(
            continuous_limit_secs(&conn),
            DEFAULT_CONTINUOUS_LIMIT_MINUTES * 60
        );
        assert!(reminders_enabled(&conn));

        // 越界拒绝且不写库。
        assert!(set_daily_cap_minutes(&conn, MIN_DAILY_CAP_MINUTES - 1).is_err());
        assert!(set_daily_cap_minutes(&conn, MAX_DAILY_CAP_MINUTES + 1).is_err());
        assert!(
            set_continuous_limit_minutes(&conn, MIN_CONTINUOUS_LIMIT_MINUTES - 1).is_err()
        );
        assert!(
            set_continuous_limit_minutes(&conn, MAX_CONTINUOUS_LIMIT_MINUTES + 1).is_err()
        );
        assert_eq!(get_string(&conn, KEY_DAILY_CAP_MINUTES).expect("get"), None);
        assert_eq!(
            get_string(&conn, KEY_CONTINUOUS_LIMIT_MINUTES).expect("get"),
            None
        );

        // 合法值写入读回：分钟存储、秒换算。
        set_daily_cap_minutes(&conn, 480).expect("set cap");
        set_continuous_limit_minutes(&conn, 30).expect("set limit");
        set_reminders_enabled(&conn, false).expect("set reminders");
        assert_eq!(daily_cap_minutes(&conn), 480);
        assert_eq!(daily_cap_secs(&conn), 480 * 60);
        assert_eq!(continuous_limit_minutes(&conn), 30);
        assert_eq!(continuous_limit_secs(&conn), 1800);
        assert!(!reminders_enabled(&conn));
        set_reminders_enabled(&conn, true).expect("set reminders on");
        assert!(reminders_enabled(&conn));
    }

    #[test]
    fn reminders_enabled_tolerates_bad_values() {
        let conn = setup();
        // 缺失 → 默认开。
        assert!(reminders_enabled(&conn));
        // 坏值（手改 DB）→ 回退默认开，与 get_i64_or 容错策略一致。
        set_string(&conn, KEY_REMINDERS_ENABLED, "garbage").expect("set");
        assert!(reminders_enabled(&conn));
    }

    #[test]
    fn ensure_defaults_seeds_new_keys_and_cleans_legacy() {
        let conn = setup();
        // 模拟旧库：存在番茄钟遗留键。
        set_string(&conn, "work_duration", "1500").expect("set legacy work");
        set_string(&conn, "break_duration", "300").expect("set legacy break");
        // 用户已自定义上限，不应被覆盖。
        set_string(&conn, KEY_DAILY_CAP_MINUTES, "480").expect("set user cap");

        ensure_defaults(&conn).expect("ensure_defaults");

        // 已存在的用户值不被覆盖；缺失键播种默认值。
        assert_eq!(daily_cap_minutes(&conn), 480);
        assert_eq!(
            continuous_limit_minutes(&conn),
            DEFAULT_CONTINUOUS_LIMIT_MINUTES
        );
        assert!(reminders_enabled(&conn));
        // 旧键被清理。
        assert_eq!(get_string(&conn, "work_duration").expect("get"), None);
        assert_eq!(get_string(&conn, "break_duration").expect("get"), None);
    }
}
