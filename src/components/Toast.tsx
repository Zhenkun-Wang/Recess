import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

/** toast 提醒负载（对应后端 `ToastPayload`）。 */
interface ToastPayload {
  kind: "continuous" | "daily_cap";
  title: string;
  body: string;
}

/**
 * 右上角轻量提醒卡片（运行于 `toast` 窗口）。
 *
 * 窗口的定位 / 显示 / 8s 自动隐藏均由后端控制；本组件监听 `toast` 事件
 * 渲染文案。窗口隐藏时用户不可见，收到新提醒（事件 + show）即更新内容。
 */
export default function Toast() {
  const [payload, setPayload] = useState<ToastPayload | null>(null);

  useEffect(() => {
    const unlistenPromise = listen<ToastPayload>("toast", (event) => {
      setPayload(event.payload);
    });
    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  if (!payload) {
    // 尚未收到提醒：窗口本身处于隐藏状态，渲染空内容即可。
    return null;
  }

  const dotColor =
    payload.kind === "daily_cap" ? "bg-amber-400" : "bg-emerald-400";

  return (
    <main className="flex h-full w-full select-none items-center rounded-2xl border border-white/10 bg-[var(--color-surface)] px-5">
      <div className="flex flex-col gap-1">
        <div className="flex items-center gap-2">
          <span className={`h-1.5 w-1.5 rounded-full ${dotColor}`} />
          <span className="text-[10px] font-medium uppercase tracking-[0.2em] text-neutral-500">
            Recess
          </span>
        </div>
        <span className="text-sm font-semibold text-white">{payload.title}</span>
        <p className="text-xs leading-relaxed text-neutral-400">{payload.body}</p>
      </div>
    </main>
  );
}
