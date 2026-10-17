import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";

import App from "./App";
import Toast from "./components/Toast";
import "./App.css";

// 主窗口("main")渲染完整应用；toast 提醒窗口("toast")共用 index.html，
// 依据窗口 label 渲染独立的轻量提醒卡片。
const label = getCurrentWindow().label;
const isToast = label === "toast";

if (isToast) {
  // toast 窗口背景透明：无边框窗口只露出卡片本体（圆角外的区域全透明）。
  document.body.classList.add("toast-body");
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{isToast ? <Toast /> : <App />}</React.StrictMode>,
);
