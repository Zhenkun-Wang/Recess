# ⏸️ Recess

**你已连续在电脑前多久了？**

基于 Rust + Tauri v2 构建的轻量屏幕时间监督桌面应用（Windows）。
不安排番茄钟、不弹全屏遮罩——只安静记录你的实际屏幕使用，
到点在右上角轻轻提醒你该起来走走。休息靠"离座"自动判定，零操作。
**绿色便携**：所有数据存在程序所在文件夹，删除文件夹即完整卸载。

![License](https://img.shields.io/badge/license-MIT-blue.svg)
![Tauri](https://img.shields.io/badge/tauri-v2-FFC131?logo=tauri)
![Platform](https://img.shields.io/badge/platform-Windows-blue)
![Version](https://img.shields.io/badge/version-0.1.0--alpha-success)

## ✨ 核心亮点

- **🪑 离座自动判定休息** — 系统空闲 ≥60 秒判定离座并按最后输入时刻精确回溯关段；离座 ≥2 分钟记一次有效休息并清零连续计时，60–120 秒短暂离开只暂停不清零
- **🔔 双阈值轻提醒** — 连续使用达阈值（默认 50 分钟，每 10 分钟重复）或今日屏幕时间达上限（默认 6 小时，每 30 分钟重复）时，右上角 toast 弹窗提醒；8 秒自动消失、不抢焦点
- **📊 数据洞察** — 今日屏幕时间大数字 + 上限进度、连续使用状态、有效休息 / 提醒次数 / 最长连续、今日应用时长排行（进程名粒度）、ECharts 近 7 日屏幕时间
- **🛡️ 隐私优先** — 全部数据本地 SQLite（WAL 模式），零网络请求；只记录"用了多久"，不记录任何按键内容
- **📦 绿色便携** — 数据库与日志存于可执行文件所在目录；`bundle.active: false`，发布 = 直接拷贝单个 `recess.exe`
- **🪟 系统集成** — 系统托盘常驻（关闭隐藏到托盘）、开机自启（opt-in，默认不启用）、休眠唤醒与跨午夜正确处理

## 🛠️ 技术栈

| 层级 | 技术 | 版本 |
|:---|:---|:---|
| 桌面框架 | Tauri（`tray-icon` feature） | 2.x |
| 后端语言 | Rust（Edition 2021） | — |
| 异步运行时 | Tokio（`full`） | 1.x |
| 本地数据库 | rusqlite（`bundled` SQLite） | 0.32 |
| Win32 API | `windows` crate（仅 `cfg(windows)`：空闲检测 + 前台进程采样） | 0.58 |
| 时间 / 日志 | chrono / log + tauri-plugin-log | 0.4 / 0.4 / 2.x |
| 开机自启 | tauri-plugin-autostart | 2.x |
| 前端框架 | React | 19.x |
| 构建工具 | Vite | 7.x |
| 类型系统 | TypeScript | ~5.8 |
| 样式方案 | TailwindCSS（`@tailwindcss/vite` 插件） | 4.x |
| 数据可视化 | ECharts（按需注册模块） | 6.x |

## 🚀 常用命令

环境要求：Node.js 20+、Rust（stable）、Windows 10/11、MSVC C 工具链（rusqlite `bundled` 需编译 sqlite3.c）。

```powershell
npm install            # 安装前端依赖
npm run tauri dev      # 开发模式（首次编译 Rust 耗时较长）
npm run build          # 前端类型检查 + 构建（tsc && vite build）
cargo check            # Rust 编译检查（在 src-tauri/ 下执行）
cargo test --lib       # Rust 单元测试（28 例，在 src-tauri/ 下执行）
npm run tauri build    # 构建 Release 单文件 recess.exe（src-tauri/target/release/）
```

> PowerShell 若禁用 `npm.ps1`，改用 `npm.cmd`；命令分隔符用 `;` 而非 `&&`。
> 开发模式数据落在 `src-tauri/target/debug/`（构建产物目录，属预期行为）。

## 📁 目录结构

| 目录 | 职责 |
|:---|:---|
| `src/` | 前端（React + TS + Tailwind v4）：`main.tsx` 按窗口 label 分流渲染主界面 / toast 提醒卡片 |
| `src-tauri/` | Rust 后端：8 个模块（usage / foreground / hooks / db / settings / analytics / lib / main） |
| `public/` | 前端静态资源 |
| `dist/` | 前端构建产物（vite build 生成） |

## 📁 数据与隐私（绿色便携）

数据目录 = **可执行文件所在目录**（不可写时回退 `%APPDATA%\com.recess.desktop\`）：

| 路径 | 内容 |
|:---|:---|
| `<exe 目录>\recess.db` | SQLite 主数据库（首启创建），含 `settings` / `sessions` / `app_usage` 三表 |
| `<exe 目录>\recess.db-wal` / `.db-shm` | WAL 日志与共享内存索引（正常关闭后自动清理） |
| `<exe 目录>\logs\` | 运行日志（tauri-plugin-log，Info 级） |

删除程序 = 删除该文件夹即完整卸载。两个已知系统残留（详见 `ai_docs/TASKS.md` 已知限制）：

- WebView2 缓存：`%LOCALAPPDATA%\com.recess.desktop\EBWebView\`（系统管理，纯缓存，Tauri 不支持重定向）
- 开机自启注册表项（仅用户主动开启自启后写入；设置面板已注明"删除程序前请先关闭"）

## 🤝 贡献与许可

提交前请确保：`cargo check` 无警告、`cargo test --lib` 全过、`npx tsc --noEmit` 与 `npx vite build` 成功。
本项目以 **MIT License** 开源，详见 [LICENSE](./LICENSE)。
