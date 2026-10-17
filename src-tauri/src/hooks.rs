//! 系统空闲检测
//!
//! Win32 `GetLastInputInfo`——系统级"最后输入时间"，供使用引擎每拍判定
//! 在用 / 离座。即便全局键鼠监听不可用，系统空闲计数始终由 OS 维护，可靠且零开销。
//!
//! 设计约束（遵循核心开发规则）：
//! 所有 `unsafe` Win32 调用封装在独立安全函数中，向外返回 `Result`，禁止 `unwrap()`。

/// 获取系统空闲时长（秒）——距离最后一次键鼠输入至今的时间。
///
/// 封装 `GetLastInputInfo` + `GetTickCount` 两个 Win32 调用；两者同为 32 位
/// tick 计数，直接相减可借 `wrapping_sub` 自然处理约 49.7 天的回绕。
///
/// 注意：系统休眠期间 tick 计数不走，唤醒后空闲值仍是休眠前的小值——
/// 休眠检测由使用引擎以墙钟跳变兜底（见 usage.rs 的 `WAKE_GAP_SECS`）。
#[cfg(windows)]
pub fn system_idle_seconds() -> Result<u64, String> {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };

    // SAFETY: `info` 已正确初始化 `cbSize` 且大小匹配；GetLastInputInfo 仅向
    // 该指针写入数据、不会保留指针。指针来自栈上有效可变引用。
    let ok = unsafe { GetLastInputInfo(&mut info) };
    if !ok.as_bool() {
        return Err("GetLastInputInfo 调用返回 FALSE".to_string());
    }

    // SAFETY: GetTickCount 无参数、无副作用，仅返回自系统启动以来的毫秒数。
    let now = unsafe { GetTickCount() };
    let idle_ms = now.wrapping_sub(info.dwTime);
    Ok((idle_ms / 1000) as u64)
}

/// 非 Windows 平台的占位实现（本项目仅面向 Windows，此处保证可跨平台编译）。
#[cfg(not(windows))]
pub fn system_idle_seconds() -> Result<u64, String> {
    Err("system_idle_seconds 仅在 Windows 平台可用".to_string())
}
