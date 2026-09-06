import { describe, expect, it } from "vitest";
import { parsePingHistory, type PingHistory } from "../api/ping";
import {
  clampViewport,
  cutPeakCap,
  displayLatencySeries,
  median,
  medianAbsoluteDeviation,
  nearestRankP95,
  niceLatencyScale,
  pingAlignedData,
  pingSeriesColor,
  pingSeriesDash,
} from "../lib/ping";
import { isCurrentPingRequest } from "../stores/ping-history";

const fixture: PingHistory = {
  node_id: "0123456789abcdef0123456789abcdef",
  from: 1_700_000_000,
  to: 1_700_000_180,
  step: 60,
  targets: [
    { id: 10, name: "线路 v4", ip_family: 4, sort_order: 0 },
    { id: 11, name: "线路 v6", ip_family: 6, sort_order: 1 },
  ],
  timestamps: [1_700_000_000, 1_700_000_060, 1_700_000_120],
  series: [
    { target_id: 10, latency: [20, null, 22] },
    { target_id: 11, latency: [40, 41, null] },
  ],
};

describe("ping API contract", () => {
  it("accepts valid aligned series and preserves null", () => {
    const parsed = parsePingHistory(fixture);
    expect(parsed).toEqual(fixture);
    expect(parsed?.series[0].latency[1]).toBeNull();
  });

  it("rejects length mismatch, unknown target, and negative latency", () => {
    expect(parsePingHistory({
      ...fixture,
      series: [{ target_id: 10, latency: [20] }, fixture.series[1]],
    })).toBeNull();
    expect(parsePingHistory({
      ...fixture,
      series: [{ target_id: 99, latency: [20, null, 22] }, fixture.series[1]],
    })).toBeNull();
    expect(parsePingHistory({
      ...fixture,
      series: [{ target_id: 10, latency: [20, -1, 22] }, fixture.series[1]],
    })).toBeNull();
    expect(parsePingHistory({
      ...fixture,
      targets: Array.from({ length: 7 }, (_, index) => ({
        id: index + 1,
        name: `线路 ${index + 1}`,
        ip_family: 4,
        sort_order: index,
      })),
      series: [],
    })).toBeNull();
  });
});

describe("cut peak statistics", () => {
  it("calculates deterministic median, MAD, and nearest-rank p95", () => {
    expect(median([7, 1, 3])).toBe(3);
    expect(median([4, 1, 2, 3])).toBe(2.5);
    expect(medianAbsoluteDeviation([1, 2, 3, 4, 5])).toBe(1);
    expect(medianAbsoluteDeviation([10, 10, 10])).toBe(0);
    expect(nearestRankP95(Array.from({ length: 20 }, (_, index) => index + 1))).toBe(19);
  });

  it("does not clip fewer than 20 values and clips a giant twentieth spike", () => {
    expect(cutPeakCap(Array(19).fill(10))).toBeNull();
    expect(cutPeakCap([...Array(19).fill(10), 1_000])).toBe(30);
  });

  it("keeps raw data immutable and null gaps intact", () => {
    const raw: (number | null)[] = [...Array(18).fill(10), null, 10, 1_000];
    const unchanged = [...raw];
    expect(displayLatencySeries(raw, false, { start: 0, end: 20 })).toBe(raw);
    const display = displayLatencySeries(raw, true, { start: 0, end: 20 });
    expect(raw).toEqual(unchanged);
    expect(display[18]).toBeNull();
    expect(display[20]).toBeLessThanOrEqual(raw[20] as number);
  });

  it("scopes clipping to the selected viewport", () => {
    const raw = [...Array(20).fill(10), ...Array(20).fill(100), 2_000];
    const first = displayLatencySeries(raw, true, { start: 0, end: 19 });
    const second = displayLatencySeries(raw, true, { start: 20, end: 40 });
    expect(first[19]).toBe(10);
    expect(second[40]).toBeLessThan(2_000);
  });
});

it("clamps navigator indices and keeps at least two buckets", () => {
  expect(clampViewport(-5, 99, 10)).toEqual({ start: 0, end: 9 });
  expect(clampViewport(7, 7, 10)).toEqual({ start: 7, end: 8 });
  expect(clampViewport(9, 2, 10)).toEqual({ start: 8, end: 9 });
});

it.each([
  [0, 1],
  [0.4, 1],
  [10, 10],
  [42, 80],
  [90, 100],
  [180, 200],
  [850, 1_000],
  [3_000, 4_000],
])("uses a readable latency ceiling for %d", (input, expected) => {
  const scale = niceLatencyScale(input);
  expect(scale.maximum).toBe(expected);
  expect(scale.maximum).toBe(scale.increment * 4);
});

it("maps stable distinct target identity colors for supported target counts", () => {
  for (const total of [1, 2, 4, 6]) {
    const light = Array.from({ length: total }, (_, index) => pingSeriesColor(index, total, false, true));
    const dark = Array.from({ length: total }, (_, index) => pingSeriesColor(index, total, true, true));
    expect(new Set(light).size).toBe(total);
    expect(new Set(dark).size).toBe(total);
    expect(light).not.toEqual(dark);
    expect(light[0]).toBe(pingSeriesColor(0, total, false, true));
  }
  expect(pingSeriesColor(0, 0, false, false)).toBe("hsl(0 52% 52%)");
});

it("keeps the six frozen dash identities stable", () => {
  expect(Array.from({ length: 6 }, (_, index) => pingSeriesDash(index))).toEqual([
    [],
    [],
    [6, 4],
    [2, 4],
    [10, 4, 2, 4],
    [1, 4],
  ]);
});

it("keeps an all-null target in its fixed style slot and data order", () => {
  const history: PingHistory = {
    ...fixture,
    series: [fixture.series[0], { target_id: 11, latency: [null, null, null] }],
  };
  const data = pingAlignedData(history, true, { start: 0, end: 2 });
  expect(data[2]).toEqual([null, null, null]);
  expect(data).toHaveLength(3);
});

it("rejects stale ping request generations", () => {
  expect(isCurrentPingRequest(5, 5)).toBe(true);
  expect(isCurrentPingRequest(4, 5)).toBe(false);
});
