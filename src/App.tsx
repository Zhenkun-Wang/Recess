import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

import { getStatus } from "./api";
import type { UsageStatus } from "./types";
import StatusView from "./components/StatusView";
import AnalyticsView from "./components/AnalyticsView";
import SettingsPanel from "./components/SettingsPanel";

type Tab = "today" | "settings";

const TABS: { id: Tab; label: string }[] = [
  { id: "today", label: "今日" },
  { id: "settings", label: "设置" },
];

function App() {
  const [status, setStatus] = useState<UsageStatus | null>(null);
  const [tab, setTab] = useState<Tab>("today");

  useEffect(() => {
    // 首屏拉取一次当前状态（窗口重建 / 冷启动场景）。
    getStatus()
      .then(setStatus)
      .catch((e) => console.error("get_status 失败:", e));

    // 订阅后端每秒广播的 tick 事件；实时更新只走事件，不做轮询。
    const unlistenPromise = listen<UsageStatus>("tick", (event) => {
      setStatus(event.payload);
    });

    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  return (
    <main className="mx-auto flex min-h-screen max-w-3xl flex-col px-6 py-6">
      <header className="mb-5 flex items-center justify-between">
        <h1 className="text-lg font-semibold tracking-tight text-white">
          Re<span className="text-indigo-400">cess</span>
        </h1>
        <nav className="flex gap-1 rounded-full border border-white/10 bg-white/5 p-1">
          {TABS.map((t) => (
            <button
              key={t.id}
              type="button"
              onClick={() => setTab(t.id)}
              className={`rounded-full px-4 py-1.5 text-sm font-medium transition ${
                tab === t.id
                  ? "bg-indigo-500 text-white"
                  : "text-neutral-300 hover:text-white"
              }`}
            >
              {t.label}
            </button>
          ))}
        </nav>
      </header>

      {/* 今日 = 实时状态（tick 驱动）+ 数据统计（挂载时拉取一次）；切换 tab 会重新挂载 AnalyticsView 从而重新拉取。 */}
      {tab === "today" && (
        <div className="flex flex-1 flex-col gap-5 pb-4">
          <StatusView status={status} />
          <AnalyticsView />
        </div>
      )}
      {tab === "settings" && (
        <div className="flex flex-1 items-start justify-center py-4">
          <SettingsPanel />
        </div>
      )}
    </main>
  );
}

export default App;
