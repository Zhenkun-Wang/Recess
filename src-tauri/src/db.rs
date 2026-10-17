//! SQLite 数据层
//!
//! 在绿色数据目录下创建并初始化数据库，建立使用引擎所需的全部数据表
//! （settings / sessions / app_usage），并提供会话与应用时长的写入原语。
//!
//! 设计约束（遵循核心开发规则）：
//! - 所有对外函数返回 `rusqlite::Result<T>`，禁止在库代码中 `unwrap()`。
//! - 建表使用 `IF NOT EXISTS`，保证初始化幂等，可安全重复调用。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

/// 线程安全的共享数据库连接。
///
/// `rusqlite::Connection` 是 `Send` 但非 `Sync`，用 `Mutex` 包裹后即可
/// 在 Tauri State 与后台引擎 Task 之间安全共享。
pub type SharedDb = Arc<Mutex<Connection>>;

/// 数据库文件名（存放于绿色数据目录，即可执行文件所在目录）。
pub const DB_FILE_NAME: &str = "recess.db";

/// 在给定的应用数据目录下打开（或创建）数据库连接，并确保表结构就绪。
///
/// # 参数
/// * `app_data_dir` - 应用数据目录（通常由 `AppHandle::path().app_data_dir()` 解析而来）。
///
/// # 返回
/// 已完成初始化的 [`Connection`]，调用方可继续用于 CRUD 或存入 Tauri State。
pub fn init_db(app_data_dir: &Path) -> rusqlite::Result<Connection> {
    // 确保目录存在；目录创建失败仅记录日志，交由后续 open 抛出更精确的错误。
    if let Err(e) = std::fs::create_dir_all(app_data_dir) {
        log::error!("创建数据目录 {} 失败: {e}", app_data_dir.display());
    }

    let db_path = database_path(app_data_dir);
    let conn = Connection::open(&db_path)?;

    // 启用 WAL 提升并发写入表现，并开启外键约束。
    // 使用 execute_batch 以忽略 PRAGMA 返回的结果行。
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;

    create_tables(&conn)?;

    log::info!("SQLite 初始化完成: {}", db_path.display());
    Ok(conn)
}

/// 计算数据库文件的完整路径。
pub fn database_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(DB_FILE_NAME)
}

/// 向 `sessions` 表插入一条已完成的会话记录。
///
/// # 参数
/// * `session_type` - 会话类型：`use`（在用段）/ `break`（离座有效休息）/
///   `reminder`（提醒）。旧番茄钟的 `work` / `postponed` 行不再产生，
///   历史数据由新查询按新类型过滤自然忽略。
/// * `context` - 上下文标记：`use` 段固定 `normal`；`break` 固定 `away`；
///   `reminder` 为 `daily_cap` / `continuous`。
/// * `start` / `end` - RFC3339 格式的起止时间字符串。
pub fn insert_session(
    conn: &Connection,
    session_type: &str,
    context: &str,
    start: &str,
    end: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO sessions (start_time, end_time, type, context, completed) \
         VALUES (?1, ?2, ?3, ?4, 1)",
        rusqlite::params![start, end, session_type, context],
    )?;
    Ok(())
}

/// 累加写入（UPSERT）某日某应用的使用秒数。
///
/// `date` 为本地日期 `YYYY-MM-DD`；`seconds` 为本次增量。
/// 使用引擎每 60s 冲刷一次 pending 增量、退出时再冲刷一次，均调用本函数累加，
/// 崩溃最多丢失自上次冲刷以来的增量（已知限制）。
pub fn upsert_app_usage(
    conn: &Connection,
    date: &str,
    app_name: &str,
    seconds: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_usage (date, app_name, seconds) VALUES (?1, ?2, ?3) \
         ON CONFLICT(date, app_name) DO UPDATE SET seconds = seconds + excluded.seconds",
        rusqlite::params![date, app_name, seconds],
    )?;
    Ok(())
}

/// 清空全部历史数据：sessions 与 app_usage 全删，settings 保留
/// （设置面板"删除历史数据 → 全部"）。调用方须同时重置引擎今日计数。
pub fn clear_all_history(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM sessions", [])?;
    conn.execute("DELETE FROM app_usage", [])?;
    Ok(())
}

/// 清空"今日"历史数据：今日起始的 sessions 行 + 今日 app_usage 行
/// （设置面板"删除历史数据 → 仅今日"）。
///
/// 跨午夜段按 `start_time` 归属昨日不删；其午夜后部分在下次基线重建时
/// 会计回今日，属可接受口径（极端边界，避免为拆分单行引入复杂度）。
pub fn clear_today_history(conn: &Connection, today: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM sessions WHERE substr(start_time, 1, 10) = ?1",
        [today],
    )?;
    conn.execute("DELETE FROM app_usage WHERE date = ?1", [today])?;
    Ok(())
}

/// 创建全部业务表（幂等）。
///
/// 旧版的 `activity_logs` / `badges` 表已随引擎切换移除；已存在的旧数据库中
/// 这两张表不再读写（保留无害），新建库则不再创建。
fn create_tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        -- 1. 用户配置表（key-value）
        CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        -- 2. 使用记录表
        CREATE TABLE IF NOT EXISTS sessions (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            start_time DATETIME NOT NULL,
            end_time   DATETIME NOT NULL,
            type       TEXT NOT NULL,            -- 'use', 'break', 'reminder'
            context    TEXT,                     -- use:'normal' break:'away' reminder:'daily_cap'/'continuous'
            completed  BOOLEAN DEFAULT 1
        );

        -- 3. 每日各应用使用时长（按 (date, app_name) 累计秒数，进程名粒度）
        CREATE TABLE IF NOT EXISTS app_usage (
            date     TEXT NOT NULL,               -- 本地日期 'YYYY-MM-DD'
            app_name TEXT NOT NULL,               -- 进程名（不含路径与扩展名，小写）
            seconds  INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (date, app_name)
        );
        "#,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证建表逻辑：初始化后全部表存在，可重复调用（幂等），
    /// 且不再创建旧版的 activity_logs / badges 表。
    #[test]
    fn init_creates_all_tables() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        create_tables(&conn).expect("first create_tables");
        // 再次调用应保持幂等，不报错。
        create_tables(&conn).expect("idempotent create_tables");

        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .expect("prepare");
        let tables: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query_map")
            .filter_map(Result::ok)
            .collect();

        for expected in ["app_usage", "sessions", "settings"] {
            assert!(
                tables.iter().any(|t| t == expected),
                "缺少数据表: {expected}, 实际: {tables:?}"
            );
        }
        for removed in ["activity_logs", "badges"] {
            assert!(
                !tables.iter().any(|t| t == removed),
                "旧表 {removed} 不应再创建"
            );
        }
    }

    /// app_usage UPSERT：同 (date, app_name) 累加秒数，不同组合独立成行。
    #[test]
    fn upsert_app_usage_accumulates_per_date_and_app() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        create_tables(&conn).expect("create tables");

        upsert_app_usage(&conn, "2026-09-01", "code", 30).expect("upsert 1");
        upsert_app_usage(&conn, "2026-09-01", "code", 45).expect("upsert 2");
        upsert_app_usage(&conn, "2026-09-01", "browser", 10).expect("upsert 3");
        upsert_app_usage(&conn, "2026-09-02", "code", 5).expect("upsert 4");

        let secs = |date: &str, app: &str| {
            conn.query_row(
                "SELECT seconds FROM app_usage WHERE date = ?1 AND app_name = ?2",
                rusqlite::params![date, app],
                |row| row.get::<_, i64>(0),
            )
            .expect("query seconds")
        };
        assert_eq!(secs("2026-09-01", "code"), 75, "同日同应用应累加");
        assert_eq!(secs("2026-09-01", "browser"), 10);
        assert_eq!(secs("2026-09-02", "code"), 5, "不同日独立成行");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM app_usage", [], |row| row.get(0))
            .expect("count");
        assert_eq!(count, 3, "共 3 个 (date, app_name) 组合");
    }

    /// 清空历史：all 全删两表（settings 保留）；today 只删今日行、
    /// 跨午夜段按 start_time 归属昨日不删。
    #[test]
    fn clear_history_scopes() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        create_tables(&conn).expect("create tables");

        // 昨日跨午夜段（start 为昨日）+ 今日段 + 今日应用时长 + 一条配置。
        insert_session(&conn, "use", "normal", "2026-08-31T23:50:00+08:00", "2026-09-01T00:05:00+08:00")
            .expect("insert yesterday");
        insert_session(&conn, "use", "normal", "2026-09-01T10:00:00+08:00", "2026-09-01T10:30:00+08:00")
            .expect("insert today");
        upsert_app_usage(&conn, "2026-09-01", "code", 100).expect("upsert today");
        upsert_app_usage(&conn, "2026-08-31", "code", 50).expect("upsert yesterday");
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('daily_cap_minutes', '360')",
            [],
        )
        .expect("insert setting");

        // 仅今日：删今日段与今日应用行，保留昨日段、昨日应用行与配置。
        clear_today_history(&conn, "2026-09-01").expect("clear today");
        let today_sessions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE substr(start_time, 1, 10) = '2026-09-01'",
                [],
                |row| row.get(0),
            )
            .expect("count today sessions");
        let yesterday_sessions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE substr(start_time, 1, 10) = '2026-08-31'",
                [],
                |row| row.get(0),
            )
            .expect("count yesterday sessions");
        let today_apps: i64 = conn
            .query_row("SELECT COUNT(*) FROM app_usage WHERE date = '2026-09-01'", [], |row| {
                row.get(0)
            })
            .expect("count today apps");
        assert_eq!(today_sessions, 0, "今日段应已删除");
        assert_eq!(yesterday_sessions, 1, "昨日跨午夜段应保留");
        assert_eq!(today_apps, 0, "今日应用时长应已删除");

        // 全部：两表清空，settings 保留。
        clear_all_history(&conn).expect("clear all");
        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .expect("count sessions");
        let apps: i64 = conn
            .query_row("SELECT COUNT(*) FROM app_usage", [], |row| row.get(0))
            .expect("count apps");
        let settings: i64 = conn
            .query_row("SELECT COUNT(*) FROM settings", [], |row| row.get(0))
            .expect("count settings");
        assert_eq!(sessions, 0);
        assert_eq!(apps, 0);
        assert_eq!(settings, 1, "清空历史不应删除配置");
    }
}
