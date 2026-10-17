// 前台应用采样：每拍取前台窗口进程名（进程名粒度），供使用引擎按应用累计时长。
//
// 纯 Win32 封装，失败只记录日志并回退 "unknown"，绝不 panic（项目红线）。

use std::collections::HashMap;

/// 查询当前前台窗口所属进程名（不含路径与扩展名，小写）。
///
/// - 前台为桌面（HWND 为 NULL）或查询失败时返回 `None`（调用方仍计屏幕秒，不归属应用）。
/// - 名称解析失败但进程存在时缓存并返回 `"unknown"`，避免每拍重复开句柄。
/// - `cache` 为 PID → 名称缓存，由调用方持有（进程名在其生命周期内不变）。
#[cfg(windows)]
pub fn foreground_process_name(cache: &mut HashMap<u32, String>) -> Option<String> {
    match resolve_inner(cache) {
        Ok(name) => name,
        Err(e) => {
            log::error!("前台进程名查询失败: {e}");
            None
        }
    }
}

#[cfg(windows)]
fn resolve_inner(cache: &mut HashMap<u32, String>) -> Result<Option<String>, String> {
    use windows::Win32::Foundation::{CloseHandle, HWND};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    use windows::core::PWSTR;

    // 前台窗口（桌面时为 NULL：不归属应用，返回 None）。
    let hwnd: HWND = unsafe { GetForegroundWindow() };
    if hwnd == HWND(std::ptr::null_mut()) {
        return Ok(None);
    }

    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return Err("GetWindowThreadProcessId 未返回 PID".into());
    }

    if let Some(name) = cache.get(&pid) {
        return Ok(Some(name.clone()));
    }

    let name = unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            .map_err(|e| format!("OpenProcess(pid={pid}) 失败: {e}"))?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        result.map(|_| String::from_utf16_lossy(&buf[..len as usize]))
    };

    let stem = match name {
        Ok(full) => std::path::Path::new(&full)
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_else(|| "unknown".to_string()),
        Err(e) => {
            // 进程存在但无权读取名称等：缓存为 unknown，避免每拍重试。
            log::error!("QueryFullProcessImageNameW(pid={pid}) 失败: {e}");
            "unknown".to_string()
        }
    };

    cache.insert(pid, stem.clone());
    Ok(Some(stem))
}

/// 非 Windows 平台桩（本项目仅面向 Windows，保留以通过跨平台编译检查）。
#[cfg(not(windows))]
pub fn foreground_process_name(_cache: &mut HashMap<u32, String>) -> Option<String> {
    None
}
