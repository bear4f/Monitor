import type { HistoryRange } from "../api/history";
import type { PingHistory, PingTarget } from "../api/ping";

const TO = 1_788_664_400;
const durations: Record<HistoryRange, number> = {
  "1h": 3_600,
  "6h": 21_600,
  "24h": 86_400,
  "7d": 604_800,
};

const TARGETS: PingTarget[] = [
  { id: 101, name: "电信 v4", ip_family: 4, sort_order: 0 },
  { id: 102, name: "电信 v6", ip_family: 6, sort_order: 1 },
  { id: 103, name: "联通 v4", ip_family: 4, sort_order: 2 },
  { id: 104, name: "联通 v6", ip_family: 6, sort_order: 3 },
  { id: 105, name: "移动 v4", ip_family: 4, sort_order: 4 },
  { id: 106, name: "移动 v6", ip_family: 6, sort_order: 5 },
];

export function mockPingHistory(nodeId: string, range: HistoryRange): PingHistory {
  const step = range === "7d" ? 300 : 60;
  const from = TO - durations[range];
  const timestamps = Array.from({ length: durations[range] / step }, (_, index) => from + index * step);
  const baselines = [37, 48, 61, 74, 92, 116];

  return {
    node_id: nodeId,
    from,
    to: TO,
    step,
    targets: TARGETS.map((target) => ({ ...target })),
    timestamps,
    series: TARGETS.map((target, targetIndex) => ({
      target_id: target.id,
      latency: timestamps.map((_, index) => {
        const gapStart = 12 + targetIndex * 3;
        if ((index >= gapStart && index < gapStart + 3) || (targetIndex === 4 && index % 97 === 0)) return null;
        if (targetIndex === 0 && index === Math.floor(timestamps.length * 0.62)) return 920;
        if (targetIndex === 3 && index === Math.floor(timestamps.length * 0.31)) return 410;
        const wave = Math.sin(index / (4.5 + targetIndex)) * (2.2 + targetIndex * 0.55);
        const ripple = Math.cos(index / 17) * 1.4;
        return Number((baselines[targetIndex] + wave + ripple).toFixed(1));
      }),
    })),
  };
}
