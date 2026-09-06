import type { HistoryRange, ResourceHistory } from "../api/history";

const TO = 1_788_664_400;
const durations: Record<HistoryRange, number> = {
  "1h": 3_600,
  "6h": 21_600,
  "24h": 86_400,
  "7d": 604_800,
};

export function mockResourceHistory(nodeId: string, range: HistoryRange): ResourceHistory {
  const step = range === "7d" ? 300 : 60;
  const from = TO - durations[range];
  const timestamp = Array.from({ length: durations[range] / step }, (_, index) => from + index * step);
  const wave = (index: number, period: number) => Math.sin(index / period);
  const gapStart = Math.floor(timestamp.length * 0.42);
  const gapEnd = gapStart + Math.max(2, Math.floor(timestamp.length * 0.025));
  const nullable = (index: number, value: number) => index >= gapStart && index < gapEnd ? null : value;

  return {
    node_id: nodeId,
    from,
    to: TO,
    step,
    series: {
      timestamp,
      cpu: timestamp.map((_, index) => nullable(index, Math.max(0, 0.8 + wave(index, 5) * 0.18 + (index % 47 === 0 ? 1.8 : 0)))),
      memory: timestamp.map((_, index) => nullable(index, Math.round(286 * 1024 ** 2 + wave(index, 29) * 2.5 * 1024 ** 2))),
      disk: timestamp.map((_, index) => nullable(index, Math.round(1.8 * 1024 ** 3 + index * 18_000))),
      rx_rate: timestamp.map((_, index) => nullable(
        index,
        Math.max(0, Math.round(600 + wave(index, 4) * 240 + (index % 61 < 5 ? 24_000 : 0))),
      )),
      tx_rate: timestamp.map((_, index) => nullable(
        index,
        Math.max(0, Math.round(1_200 + wave(index, 6) * 440 + (index % 61 < 5 ? 31_000 : 0))),
      )),
    },
  };
}
