// 使用引擎接线：每秒采样（系统空闲 + 前台应用）喂给 usage::UsageEngine，
// 事件落库（use/break/reminder 段）、每 60s 冲刷各应用 pending 增量、
// 退出时冲刷全部状态；设置热更新与统计查询通过 IPC 命令暴露。
//
// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
mod analytics;
mod db;
mod foreground;
mod hooks;
mod settings;
pub mod usage;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Local};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, LogicalPosition, Manager, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use db::SharedDb;
use usage::{ReminderKind, UsageEngine, UsageEvent};

/// 前端监听的 tick 事件名（负载为 [`UsageStatus`]，每秒广播）。
pub const EVENT_TICK: &str = "tick";

/// toast 提醒窗口 label（前端 main.tsx 按此分流渲染 Toast 组件）。
const TOAST_LABEL: &str = "toast";

/// toast 窗口监听的事件名（负载为 [`ToastPayload`]）。
pub const EVENT_TOAST: &str = "toast";

/// toast 窗口逻辑尺寸（dp）。
const TOAST_WIDTH: f64 = 380.0;
const TOAST_HEIGHT: f64 = 130.0;

/// toast 距主显示器右上角的边距（dp）。
const TOAST_MARGIN: f64 = 16.0;

/// toast 自动隐藏延迟（秒）。
const TOAST_HIDE_SECS: u64 = 8;

/// toast 显示序号：每次弹出新提醒时递增，使先前的隐藏任务过期
/// （实现"新提醒重置 8s 计时"）。
static TOAST_SEQ: AtomicU64 = AtomicU64::new(0);

/// 应用增量冲刷间隔（秒）：引擎 pending 的各应用秒数定期 UPSERT 入库，
/// 崩溃最多丢失该窗口内的增量（已知限制）。
const APP_FLUSH_INTERVAL_SECS: u64 = 60;

/// 线程安全共享的使用引擎（后台 Task 每秒 tick；IPC 命令读状态/热更新）。
type SharedEngine = Arc<Mutex<UsageEngine>>;

/// 使用状态负载：`tick` 事件与 `get_status` 命令共用。
///
/// 对应 TS 类型：
/// ```ts
/// interface UsageStatus {
///   active: boolean;
///   today_screen_secs: number;
///   consecutive_seconds: number;
///   daily_cap_seconds: number;
///   continuous_limit_seconds: number;
///   reminders_enabled: boolean;
/// }
/// ```
#[derive(Debug, Clone, serde::Serialize)]
struct UsageStatus {
    /// 当前是否"在用"（空闲 < 60s）。
    active: bool,
    /// 今日屏幕使用总秒数。
    today_screen_secs: i64,
    /// 当前连续使用秒数。
    consecutive_seconds: i64,
    /// 每日上限（秒）。
    daily_cap_seconds: i64,
    /// 连续使用提醒阈值（秒）。
    continuous_limit_seconds: i64,
    /// 提醒总开关。
    reminders_enabled: bool,
}

/// 从引擎快照构造状态负载（全部字段引擎内已有，无需读库）。
fn status_from_engine(engine: &UsageEngine) -> UsageStatus {
    UsageStatus {
        active: engine.is_active(),
        today_screen_secs: engine.today_seconds(),
        consecutive_seconds: engine.consecutive_seconds(),
        daily_cap_seconds: engine.daily_cap_secs(),
        continuous_limit_seconds: engine.continuous_limit_secs(),
        reminders_enabled: engine.reminders_enabled(),
    }
}

// ============================ toast 提醒窗口 ============================

/// toast 提醒负载（`toast` 事件）。
///
/// 对应 TS 类型：
/// ```ts
/// interface ToastPayload {
///   kind: "continuous" | "daily_cap";
///   title: string;
///   body: string;
/// }
/// ```
#[derive(Debug, Clone, serde::Serialize)]
struct ToastPayload {
    /// 提醒种类（前端按此配色：continuous 绿 / daily_cap 琥珀）。
    kind: &'static str,
    /// 标题。
    title: String,
    /// 正文。
    body: String,
}

/// 将秒数格式化为人类可读时长（"X 小时 Y 分钟" / "Y 分钟"）。
fn format_minutes(secs: i64) -> String {
    let mins = secs.max(0) / 60;
    let h = mins / 60;
    let m = mins % 60;
    if h > 0 {
        format!("{h} 小时 {m} 分钟")
    } else {
        format!("{m} 分钟")
    }
}

/// 创建常驻的 toast 提醒窗口（初始隐藏，无边框、置顶、不进任务栏、背景透明）。
///
/// 加载与主窗口相同的前端入口（index.html），由 main.tsx 按窗口 label 分流
/// 渲染 Toast 组件；定位到主显示器右上角。
fn create_toast_window(app: &tauri::App) -> tauri::Result<WebviewWindow> {
    let win = WebviewWindowBuilder::new(app, TOAST_LABEL, WebviewUrl::default())
        .title("Recess")
        .inner_size(TOAST_WIDTH, TOAST_HEIGHT)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .transparent(true)
        .visible(false)
        .build()?;
    position_toast(&win);
    Ok(win)
}

/// 将 toast 窗口定位到主显示器右上角（[`TOAST_MARGIN`] dp 边距）。
///
/// 每次 show 前重算：以 `primary_monitor` 的物理位置/尺寸除以 scale_factor
/// 折算成逻辑坐标（无工作区 API，任务栏高度忽略——边距小，可接受）。
fn position_toast(win: &WebviewWindow) {
    let monitor = match win.primary_monitor() {
        Ok(Some(m)) => m,
        _ => {
            log::error!("获取主显示器信息失败，toast 保持上次位置");
            return;
        }
    };
    let scale = monitor.scale_factor();
    let x = monitor.position().x as f64 / scale
        + monitor.size().width as f64 / scale
        - TOAST_WIDTH
        - TOAST_MARGIN;
    let y = monitor.position().y as f64 / scale + TOAST_MARGIN;
    if let Err(e) = win.set_position(LogicalPosition::new(x, y)) {
        log::error!("toast 定位失败: {e}");
    }
}

/// 弹出提醒 toast：重算坐标 → show → 推送文案 → 8s 后自动隐藏。
///
/// - show 后**不** `set_focus`（不抢焦点）；
/// - 新提醒递增 [`TOAST_SEQ`]，先前的隐藏任务发现自己过期即放弃隐藏，
///   实现"8s 计时被新提醒重置"。
fn show_reminder_toast(app: &AppHandle, kind: ReminderKind, engine: &SharedEngine) {
    let Some(win) = app.get_webview_window(TOAST_LABEL) else {
        log::error!("toast 窗口不存在，本次提醒只落库不弹窗");
        return;
    };

    // 文案秒数取引擎当前值：事件在 tick 后产出，与提醒触发条件同拍一致。
    let (consecutive, today) = match engine.lock() {
        Ok(e) => (e.consecutive_seconds(), e.today_seconds()),
        Err(e) => {
            log::error!("获取引擎锁失败（toast 文案不含时长）: {e}");
            (0, 0)
        }
    };
    let payload = match kind {
        ReminderKind::Continuous => ToastPayload {
            kind: "continuous",
            title: "该休息一下了".to_string(),
            body: format!(
                "已连续使用 {}。离开电脑 2 分钟，连续计时就会重置。",
                format_minutes(consecutive)
            ),
        },
        ReminderKind::DailyCap => ToastPayload {
            kind: "daily_cap",
            title: "今日屏幕时间已达上限".to_string(),
            body: format!("今天已使用 {}。适度收工，注意休息。", format_minutes(today)),
        },
    };

    position_toast(&win);
    if let Err(e) = win.show() {
        log::error!("toast 显示失败: {e}");
    }
    // 规范要求：show 后不 set_focus（不抢焦点）。
    if let Err(e) = app.emit_to(TOAST_LABEL, EVENT_TOAST, &payload) {
        log::error!("推送 toast 文案失败: {e}");
    }

    let seq = TOAST_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(TOAST_HIDE_SECS)).await;
        if TOAST_SEQ.load(Ordering::SeqCst) != seq {
            return; // 期间有新提醒弹出，8s 计时已被重置。
        }
        if let Some(win) = app.get_webview_window(TOAST_LABEL) {
            if let Err(e) = win.hide() {
                log::error!("toast 隐藏失败: {e}");
            }
        }
    });
}

// ============================ 使用引擎 Task ============================

/// 在后台启动使用引擎 Task（非阻塞，立即返回）。
fn spawn_usage_engine(app_handle: AppHandle, db: SharedDb, engine: SharedEngine) {
    tauri::async_runtime::spawn(async move {
        run_usage_engine(app_handle, db, engine).await;
    });
}

/// 引擎主循环：启动基线重建 → 每秒采样 tick → 每 60 拍冲刷应用增量。
async fn run_usage_engine(app_handle: AppHandle, db: SharedDb, engine: SharedEngine) {
    // ---- 启动基线：今日已落库的屏幕秒重建到内存（崩溃恢复 / 重启续计）。
    {
        let (start, end) = analytics::today_bounds();
        let baseline = match db.lock() {
            Ok(conn) => analytics::today_use_seconds(&conn, &start, &end),
            Err(e) => {
                log::error!("获取数据库锁失败（基线重建跳过）: {e}");
                Ok(0)
            }
        };
        match baseline {
            Ok(secs) => {
                if let Ok(mut e) = engine.lock() {
                    e.set_baseline(secs);
                }
                log::info!("使用引擎启动，今日基线 {secs} 秒");
            }
            Err(e) => log::error!("查询今日基线失败: {e}"),
        }
    }

    // PID → 进程名缓存（进程名在其生命周期内不变，缓存后免重复开句柄）。
    let mut pid_cache: HashMap<u32, String> = HashMap::new();

    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    // interval 首个 tick 立即触发，先消费掉，保证后续为稳定的每秒节拍。
    ticker.tick().await;
    // 距下次应用增量冲刷的剩余拍数。
    let mut ticks_until_flush = APP_FLUSH_INTERVAL_SECS;

    loop {
        ticker.tick().await;

        // ---- 采样：墙钟 + 系统空闲 + 前台进程名。
        let now = Local::now();
        // 空闲检测失败（几乎不会发生）保守视为 0（在用），避免误离座。
        let idle_secs = hooks::system_idle_seconds().unwrap_or(0);
        let foreground = foreground::foreground_process_name(&mut pid_cache);

        // ---- 推进引擎并处理事件。
        let events = match engine.lock() {
            Ok(mut e) => e.tick(now, idle_secs, foreground.as_deref()),
            Err(e) => {
                log::error!("获取引擎锁失败: {e}");
                Vec::new()
            }
        };
        for event in &events {
            persist_usage_event(&db, event);
            // 提醒事件：落库之外还要弹出 toast（不抢焦点，8s 自动隐藏）。
            if let UsageEvent::Reminder { kind } = event {
                show_reminder_toast(&app_handle, *kind, &engine);
            }
        }

        // ---- 广播 tick（负载与 get_status 一致；失败仅记录日志）。
        if let Ok(e) = engine.lock() {
            let payload = status_from_engine(&e);
            if let Err(e) = app_handle.emit(EVENT_TICK, payload) {
                log::error!("广播 tick 事件失败: {e}");
            }
        }

        // ---- 定期冲刷各应用 pending 增量。
        ticks_until_flush = ticks_until_flush.saturating_sub(1);
        if ticks_until_flush == 0 {
            ticks_until_flush = APP_FLUSH_INTERVAL_SECS;
            flush_app_usage(&db, &engine);
        }
    }
}

/// 将一个引擎事件落库（提醒/休息另记日志；DateChanged 负责带走旧日期的
/// 应用增量，跨日本身由引擎已重置）。
fn persist_usage_event(db: &SharedDb, event: &UsageEvent) {
    match event {
        UsageEvent::UseSegmentClosed { start, end } => {
            insert_session_row(db, "use", "normal", start, end);
        }
        UsageEvent::Break { start, end } => {
            insert_session_row(db, "break", "away", start, end);
            log::info!("记录一次有效休息: {start:?} ~ {end:?}");
        }
        UsageEvent::Reminder { kind } => {
            let context = match kind {
                ReminderKind::Continuous => "continuous",
                ReminderKind::DailyCap => "daily_cap",
            };
            let now = Local::now();
            insert_session_row(db, "reminder", context, &now, &now);
            log::info!("触发提醒: {context}");
        }
        UsageEvent::DateChanged { old_date, app_usage } => {
            // 旧日期的 pending 应用增量随事件冲刷（以 old_date 入库，杜绝跨日串数）。
            if !app_usage.is_empty() {
                write_app_usage(db, &old_date.format("%Y-%m-%d").to_string(), app_usage);
            }
            log::info!("日期变化，今日计数已重置（原日期 {old_date}）");
        }
    }
}

/// 写入一条 session 记录（锁获取或写入失败仅记录日志，不中断引擎）。
fn insert_session_row(
    db: &SharedDb,
    session_type: &str,
    context: &str,
    start: &DateTime<Local>,
    end: &DateTime<Local>,
) {
    let conn = match db.lock() {
        Ok(conn) => conn,
        Err(e) => {
            log::error!("获取数据库锁失败: {e}");
            return;
        }
    };
    if let Err(e) = db::insert_session(
        &conn,
        session_type,
        context,
        &start.to_rfc3339(),
        &end.to_rfc3339(),
    ) {
        log::error!("写入 session 记录失败: {e}");
    }
}

/// 冲刷引擎内 pending 的各应用秒数增量（以引擎当前日期入库）。
///
/// 锁顺序约定：先取引擎锁拿数据并释放，再取数据库锁写入——与
/// `update_settings`（先库后引擎）方向相反但均为非嵌套持锁，无死锁风险。
fn flush_app_usage(db: &SharedDb, engine: &SharedEngine) {
    let (date, pending) = match engine.lock() {
        Ok(mut e) => (
            e.date().format("%Y-%m-%d").to_string(),
            e.drain_app_usage(),
        ),
        Err(e) => {
            log::error!("获取引擎锁失败（应用增量冲刷跳过）: {e}");
            return;
        }
    };
    if pending.is_empty() {
        return;
    }
    write_app_usage(db, &date, &pending);
}

/// 将应用增量逐条 UPSERT 入库。
fn write_app_usage(db: &SharedDb, date: &str, pending: &std::collections::BTreeMap<String, i64>) {
    let conn = match db.lock() {
        Ok(conn) => conn,
        Err(e) => {
            log::error!("获取数据库锁失败: {e}");
            return;
        }
    };
    for (app_name, seconds) in pending {
        if let Err(e) = db::upsert_app_usage(&conn, date, app_name, *seconds) {
            log::error!("写入 app_usage 失败（{date}/{app_name}）: {e}");
        }
    }
}

/// 退出冲刷：闭合未关闭的在用段并落库、冲刷全部应用增量。
///
/// 在 `RunEvent::Exit`（含托盘退出路径）调用；引擎或数据库状态未注册时静默跳过。
fn flush_on_exit(app: &AppHandle) {
    let Some(engine) = app.try_state::<SharedEngine>() else {
        return;
    };
    let Some(db) = app.try_state::<SharedDb>() else {
        return;
    };
    let now = Local::now();

    // 1) 闭合未关闭的在用段。
    let events = match engine.lock() {
        Ok(mut e) => e.flush(now),
        Err(e) => {
            log::error!("获取引擎锁失败（退出冲刷段关闭跳过）: {e}");
            Vec::new()
        }
    };
    for event in &events {
        persist_usage_event(&db, event);
    }

    // 2) 冲刷剩余应用增量。
    flush_app_usage(&db, &engine);
    log::info!("退出冲刷完成");
}

// ============================ IPC 命令 ============================

/// 读取使用状态快照（在用 / 今日屏幕秒 / 连续秒 / 阈值配置）。
///
/// 供前端首屏加载或窗口重建时拉取一次；实时更新走 `tick` 事件，前端**不应**高频轮询。
#[tauri::command]
fn get_status(engine: tauri::State<'_, SharedEngine>) -> Result<UsageStatus, String> {
    engine
        .lock()
        .map(|guard| status_from_engine(&guard))
        .map_err(|e| format!("读取引擎状态失败: {e}"))
}

/// 当前配置（分钟 / 开关），供设置面板挂载时回显。
#[derive(serde::Serialize)]
struct SettingsPayload {
    /// 每日屏幕时间上限（分钟）。
    daily_cap_minutes: i64,
    /// 连续使用提醒阈值（分钟）。
    continuous_limit_minutes: i64,
    /// 提醒总开关。
    reminders_enabled: bool,
}

/// 读取当前配置。
#[tauri::command]
fn get_settings(db: tauri::State<'_, SharedDb>) -> Result<SettingsPayload, String> {
    let conn = db.lock().map_err(|e| format!("获取数据库锁失败: {e}"))?;
    Ok(SettingsPayload {
        daily_cap_minutes: settings::daily_cap_minutes(&conn),
        continuous_limit_minutes: settings::continuous_limit_minutes(&conn),
        reminders_enabled: settings::reminders_enabled(&conn),
    })
}

/// 更新配置：任一参数为 `None` 时保持不变。写库成功后热更新引擎
/// （`update_limits`，下一拍立即生效——上限调低到低于今日已用时下一拍即提醒，
/// 属预期行为）。超出允许范围时返回错误（不写库、不热更新）。
#[tauri::command]
fn update_settings(
    db: tauri::State<'_, SharedDb>,
    engine: tauri::State<'_, SharedEngine>,
    daily_cap_minutes: Option<i64>,
    continuous_limit_minutes: Option<i64>,
    reminders_enabled: Option<bool>,
) -> Result<(), String> {
    // 写库单独作用域，锁在此块结束即释放，随后的引擎热更新不持有 DB 锁。
    {
        let conn = db.lock().map_err(|e| format!("获取数据库锁失败: {e}"))?;
        if let Some(minutes) = daily_cap_minutes {
            settings::set_daily_cap_minutes(&conn, minutes)?;
        }
        if let Some(minutes) = continuous_limit_minutes {
            settings::set_continuous_limit_minutes(&conn, minutes)?;
        }
        if let Some(enabled) = reminders_enabled {
            settings::set_reminders_enabled(&conn, enabled)?;
        }
    }
    // 热更新引擎：重新读回三项配置并应用（读回保证与库中最终值一致）。
    let (cap_secs, limit_secs, reminders) = {
        let conn = db.lock().map_err(|e| format!("获取数据库锁失败: {e}"))?;
        (
            settings::daily_cap_secs(&conn),
            settings::continuous_limit_secs(&conn),
            settings::reminders_enabled(&conn),
        )
    };
    if let Ok(mut e) = engine.lock() {
        e.update_limits(cap_secs, limit_secs, reminders);
    }
    log::info!("配置已热更新：上限 {cap_secs}s / 连续阈值 {limit_secs}s / 提醒 {reminders}");
    Ok(())
}

/// 读取今日使用摘要（屏幕秒 / 休息次数 / 提醒次数 / 最长连续 / 应用排行）。
#[tauri::command]
fn get_analytics(db: tauri::State<'_, SharedDb>) -> Result<analytics::UsageSummary, String> {
    let conn = db.lock().map_err(|e| format!("获取数据库锁失败: {e}"))?;
    let (start, end) = analytics::today_bounds();
    let today = Local::now().format("%Y-%m-%d").to_string();
    analytics::compute_usage_summary(&conn, &start, &end, &today)
        .map_err(|e| format!("统计分析失败: {e}"))
}

/// 生成近 7 日屏幕使用周报（每日屏幕秒，按日期升序）。
#[tauri::command]
fn get_weekly_report(
    db: tauri::State<'_, SharedDb>,
) -> Result<analytics::ScreenWeeklyReport, String> {
    let conn = db.lock().map_err(|e| format!("获取数据库锁失败: {e}"))?;
    analytics::compute_screen_weekly_report(&conn, analytics::start_of_today())
        .map_err(|e| format!("生成周报失败: {e}"))
}

/// 清空历史数据（设置面板"删除历史数据"）。
///
/// - `scope = "all"`：清空 sessions 与 app_usage 全部历史，**保留 settings**。
/// - `scope = "today"`：只清今日起始的段与今日应用时长。
///
/// 先锁引擎并保持到清库完成再 `reset_today`：清空期间每秒 tick 被阻塞，
/// 不存在"库已清、引擎未重置"的窗口（否则旧段关段 / pending 冲刷会把
/// 清空前的秒数写回，出现幽灵数据）。引擎任务从不同时持有双锁，
/// 此处"引擎 → 数据库"的持锁顺序无死锁风险。
#[tauri::command]
fn clear_history_data(
    scope: String,
    db: tauri::State<'_, SharedDb>,
    engine: tauri::State<'_, SharedEngine>,
) -> Result<(), String> {
    let mut eng = engine
        .lock()
        .map_err(|e| format!("获取引擎锁失败: {e}"))?;
    let conn = db.lock().map_err(|e| format!("获取数据库锁失败: {e}"))?;
    match scope.as_str() {
        "all" => {
            db::clear_all_history(&conn).map_err(|e| format!("清空全部历史失败: {e}"))?;
            eng.reset_today();
            log::info!("已清空全部历史数据（配置保留）");
        }
        "today" => {
            let today = Local::now().format("%Y-%m-%d").to_string();
            db::clear_today_history(&conn, &today)
                .map_err(|e| format!("清空今日历史失败: {e}"))?;
            eng.reset_today();
            log::info!("已清空今日（{today}）历史数据");
        }
        other => return Err(format!("未知的清空范围: {other}（仅支持 all / today）")),
    }
    Ok(())
}

// ============================ 应用装配 ============================

/// 显示并聚焦主窗口（托盘“显示”菜单与左键单击共用）。
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// 解析绿色数据目录：优先可执行文件所在目录（删除文件夹即完整卸载）；
/// 目录不存在或不可写（如被放在受保护目录）时回退 `%APPDATA%`。
///
/// 本函数在日志后端注册之前执行，失败路径用 `eprintln!` 兜底输出。
fn resolve_data_dir() -> PathBuf {
    match std::env::current_exe() {
        Ok(exe) => {
            if let Some(dir) = exe.parent() {
                if dir_is_writable(dir) {
                    return dir.to_path_buf();
                }
                eprintln!("程序目录 {} 不可写，回退到 %APPDATA%", dir.display());
            }
        }
        Err(e) => eprintln!("无法解析可执行文件路径: {e}，回退到 %APPDATA%"),
    }
    match std::env::var_os("APPDATA") {
        Some(appdata) => PathBuf::from(appdata).join("com.recess.desktop"),
        None => {
            eprintln!("%APPDATA% 环境变量缺失，使用当前工作目录");
            PathBuf::from(".")
        }
    }
}

/// 以"创建并删除探测文件"判断目录是否可写。
fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(".recess_write_probe");
    match std::fs::File::create(&probe) {
        Ok(_) => std::fs::remove_file(&probe).is_ok(),
        Err(_) => false,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 绿色数据目录：数据库与日志均存于程序所在文件夹，删除文件夹即完整卸载。
    let data_dir = resolve_data_dir();
    let setup_data_dir = data_dir.clone();

    let app = tauri::Builder::default()
        // 日志后端：写入数据目录下 logs/。必须最先注册，才能收集后续插件的早期日志。
        .plugin(
            tauri_plugin_log::Builder::new()
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Folder {
                        path: data_dir.join("logs"),
                        file_name: None,
                    },
                ))
                .level(log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        // 开机自启插件：仅注册能力，是否启用由前端设置开关调用 enable/disable 决定（默认不自启）。
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(move |app| {
            // 初始化 SQLite（绿色数据目录），将连接存入共享状态并启动后台引擎。
            // 遵循规则：系统调用失败仅记录日志，不使应用 panic。
            let handle = app.handle().clone();

            // ---- 系统托盘（Tauri v2 tray-icon）----
            let show_item = MenuItem::with_id(app, "show", "显示主界面", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show_item, &quit_item])?;
            let mut tray_builder = TrayIconBuilder::new()
                .tooltip("Recess")
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // 左键单击托盘图标：显示主窗口。
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                });
            // 复用默认窗口图标（缺失时仅记录日志，不 panic）。
            match app.default_window_icon() {
                Some(icon) => tray_builder = tray_builder.icon(icon.clone()),
                None => log::error!("默认窗口图标缺失，系统托盘将无图标"),
            }
            if let Err(e) = tray_builder.build(app) {
                log::error!("系统托盘创建失败: {e}");
            }

            // ---- 关闭主窗口时隐藏到托盘（而非退出进程）----
            if let Some(main_window) = app.get_webview_window("main") {
                let hide_target = main_window.clone();
                main_window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        if let Err(e) = hide_target.hide() {
                            log::error!("隐藏主窗口到托盘失败: {e}");
                        }
                    }
                });
            }

            // ---- toast 提醒窗口（常驻隐藏，提醒时 show）----
            match create_toast_window(app) {
                Ok(win) => {
                    // 防御：toast 被意外关闭（如聚焦后 Alt+F4）时只隐藏不销毁，
                    // 保住常驻窗口对象，后续提醒仍可弹出。
                    let toast_target = win.clone();
                    win.on_window_event(move |event| {
                        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                            api.prevent_close();
                            let _ = toast_target.hide();
                        }
                    });
                }
                Err(e) => log::error!("toast 窗口创建失败（提醒将只落库不弹窗）: {e}"),
            }

            // 绿色存储：数据库初始化于数据目录（程序所在文件夹，含回退）。
            match db::init_db(&setup_data_dir) {
                Ok(conn) => {
                    // 播种默认配置（不覆盖用户已改的值）并清理旧番茄钟遗留键。
                    if let Err(e) = settings::ensure_defaults(&conn) {
                        log::error!("播种默认配置失败: {e}");
                    }

                    // 使用引擎：初始配置从库读取；今日基线由引擎 Task 启动时重建。
                    let engine: SharedEngine = {
                        let (cap, limit, reminders) = (
                            settings::daily_cap_secs(&conn),
                            settings::continuous_limit_secs(&conn),
                            settings::reminders_enabled(&conn),
                        );
                        Arc::new(Mutex::new(UsageEngine::new(
                            Local::now(),
                            cap,
                            limit,
                            reminders,
                        )))
                    };

                    // rusqlite::Connection 非 Sync，用 Arc<Mutex> 包裹后共享。
                    let shared: SharedDb = Arc::new(Mutex::new(conn));
                    // 注册为 Tauri State，供 IPC 命令读写。
                    app.manage(shared.clone());
                    app.manage(engine.clone());

                    // 启动使用引擎 Task（每秒采样空闲/前台并推进状态机）。
                    spawn_usage_engine(handle, shared, engine);
                }
                Err(e) => log::error!("数据库初始化失败: {e}"),
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_settings,
            update_settings,
            get_analytics,
            get_weekly_report,
            clear_history_data
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        // 退出前冲刷：闭合未关闭的在用段、落库全部应用增量（托盘退出同样经过）。
        if let tauri::RunEvent::Exit = event {
            log::info!("应用退出");
            flush_on_exit(app_handle);
        }
    });
}
