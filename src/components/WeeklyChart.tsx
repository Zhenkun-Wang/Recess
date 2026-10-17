import { useEffect, useRef } from "react";

import * as echarts from "echarts/core";
import { BarChart } from "echarts/charts";
import { GridComponent, TooltipComponent } from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";

import type { ScreenDailyStat } from "../types";

// 按需注册 ECharts 模块（tree-shaking，避免打包整个 echarts）。
echarts.use([BarChart, GridComponent, TooltipComponent, CanvasRenderer]);

interface Props {
  days: ScreenDailyStat[];
}

/** 近 7 日屏幕时间柱状图（分钟）。 */
export default function WeeklyChart({ days }: Props) {
  const elRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);

  // 初始化与销毁只做一次；随窗口尺寸自适应。
  useEffect(() => {
    if (!elRef.current) return;
    const chart = echarts.init(elRef.current);
    chartRef.current = chart;
    const onResize = () => chart.resize();
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      chart.dispose();
      chartRef.current = null;
    };
  }, []);

  // 数据变化时刷新配置。
  useEffect(() => {
    const chart = chartRef.current;
    if (!chart) return;
    const labels = days.map((d) => d.date.slice(5)); // MM-DD
    const minutes = days.map((d) => Math.round(d.screen_seconds / 60));
    chart.setOption({
      grid: { top: 16, right: 16, bottom: 28, left: 44 },
      tooltip: { trigger: "axis", formatter: "{b}<br/>屏幕 {c} 分钟" },
      xAxis: {
        type: "category",
        data: labels,
        axisLine: { lineStyle: { color: "rgba(255,255,255,0.2)" } },
        axisLabel: { color: "#9ca3af" },
      },
      yAxis: {
        type: "value",
        splitLine: { lineStyle: { color: "rgba(255,255,255,0.06)" } },
        axisLabel: { color: "#9ca3af" },
      },
      series: [
        {
          type: "bar",
          data: minutes,
          barWidth: "48%",
          itemStyle: { color: "#6366f1", borderRadius: [6, 6, 0, 0] },
        },
      ],
    });
  }, [days]);

  return <div ref={elRef} className="h-56 w-full" />;
}
