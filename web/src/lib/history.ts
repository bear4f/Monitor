import type uPlot from "uplot";
import type { HistoryRange, ResourceHistory } from "../api/history";

export const HISTORY_RANGES: readonly HistoryRange[] = ["1h", "6h", "24h", "7d"];

export type ResourceChartKind = "cpu" | "memory" | "network" | "disk";

export function parseHistoryRange(search: string): HistoryRange {
  const value = new URLSearchParams(search).get("range");
  return HISTORY_RANGES.includes(value as HistoryRange) ? value as HistoryRange : "1h";
}

export function historyLocation(nodeId: string, range: HistoryRange, mock: boolean): string {
  const query = new URLSearchParams({ range });
  if (mock) query.set("mock", "1");
  return `/nodes/${nodeId}?${query}`;
}

export function chartData(history: ResourceHistory, kind: ResourceChartKind): uPlot.AlignedData {
  const { timestamp, cpu, memory, disk, tx_rate: txRate, rx_rate: rxRate } = history.series;
  switch (kind) {
    case "cpu": return [timestamp, cpu];
    case "memory": return [timestamp, memory];
    case "network": return [timestamp, txRate, rxRate];
    case "disk": return [timestamp, disk];
  }
}

export interface NiceScale {
  maximum: number;
  increment: number;
}

export function niceByteRateScale(maximum: number): NiceScale {
  const finiteMaximum = Number.isFinite(maximum) && maximum > 0 ? maximum : 0;
  const targetStep = Math.max(finiteMaximum, 1024) / 4;
  const magnitude = 1024 ** Math.floor(Math.log(targetStep) / Math.log(1024));
  const ladder = [1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024];
  const increment = ladder.map((value) => value * magnitude).find((value) => value >= targetStep)
    ?? targetStep;
  return { maximum: increment * 4, increment };
}

const SHORT_CLOCK = new Intl.DateTimeFormat("zh-CN", {
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});

const LONG_CLOCK = new Intl.DateTimeFormat("zh-CN", {
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});

export function formatAxisTime(timestamp: number, range: HistoryRange): string {
  return (range === "7d" ? LONG_CLOCK : SHORT_CLOCK).format(timestamp * 1000);
}

export function formatTooltipTime(timestamp: number): string {
  const date = new Date(timestamp * 1000);
  const clock = [date.getHours(), date.getMinutes(), date.getSeconds()]
    .map((value) => String(value).padStart(2, "0"))
    .join(":");
  return `${date.getFullYear()}/${date.getMonth() + 1}/${date.getDate()} ${clock}`;
}

export function timeAxisSplits(
  minimum: number,
  maximum: number,
  range: HistoryRange,
  width: number,
): number[] {
  const compact = width < 768;
  const step = {
    "1h": compact ? 1_200 : 600,
    "6h": compact ? 7_200 : 3_600,
    "24h": compact ? 21_600 : 14_400,
    "7d": compact ? 172_800 : 86_400,
  }[range];
  const timezoneOffset = new Date(minimum * 1000).getTimezoneOffset() * 60;
  const first = Math.ceil((minimum - timezoneOffset) / step) * step + timezoneOffset;
  const splits: number[] = [];
  for (let timestamp = first; timestamp <= maximum; timestamp += step) splits.push(timestamp);
  return splits;
}
