import { describe, expect, it } from "vitest";
import { parseResourceHistory, type ResourceHistory } from "../api/history";
import {
  chartData,
  detailLocation,
  formatAxisTime,
  formatTooltipTime,
  niceByteRateScale,
  parseDetailTab,
  parseHistoryRange,
  timeAxisSplits,
} from "../lib/history";
import { formatAxisBytes } from "../lib/format";
import { isCurrentHistoryRequest } from "../stores/resource-history";

const fixture: ResourceHistory = {
  node_id: "0123456789abcdef0123456789abcdef",
  from: 1_700_000_000,
  to: 1_700_000_180,
  step: 60,
  series: {
    timestamp: [1_700_000_000, 1_700_000_060, 1_700_000_120],
    cpu: [1.2, null, 2.4],
    memory: [100, null, 120],
    disk: [200, null, 210],
    rx_rate: [300, null, 330],
    tx_rate: [400, null, 440],
  },
};

describe("resource history contract", () => {
  it("accepts dense parallel arrays and rejects mismatched shapes", () => {
    expect(parseResourceHistory(fixture)).toEqual(fixture);
    expect(parseResourceHistory({
      ...fixture,
      series: { ...fixture.series, cpu: [1] },
    })).toBeNull();
  });

  it("preserves null gaps in every chart transformation", () => {
    expect(chartData(fixture, "cpu")[1][1]).toBeNull();
    expect(chartData(fixture, "memory")[1][1]).toBeNull();
    expect(chartData(fixture, "disk")[1][1]).toBeNull();
  });

  it("maps network upload to tx and download to rx", () => {
    const network = chartData(fixture, "network");
    expect(network[1]).toBe(fixture.series.tx_rate);
    expect(network[2]).toBe(fixture.series.rx_rate);
    expect(network[1][1]).toBeNull();
    expect(network[2][1]).toBeNull();
  });
});

it("parses only frozen ranges and builds stable range URLs", () => {
  expect(parseHistoryRange("?range=6h")).toBe("6h");
  expect(parseHistoryRange("?range=7d&mock=1")).toBe("7d");
  expect(parseHistoryRange("?range=2h")).toBe("1h");
  expect(parseHistoryRange("?range=6h&range=24h")).toBe("6h");
  expect(parseDetailTab("?tab=latency&range=6h")).toBe("latency");
  expect(parseDetailTab("?tab=unknown")).toBe("resources");
  expect(detailLocation(fixture.node_id, "resources", "24h", false)).toBe(`/nodes/${fixture.node_id}?range=24h`);
  expect(detailLocation(fixture.node_id, "latency", "1h", true)).toBe(
    `/nodes/${fixture.node_id}?tab=latency&range=1h&mock=1`,
  );
});

it("uses readable IEC byte axes", () => {
  expect(formatAxisBytes(0)).toBe("0 B");
  expect(formatAxisBytes(256 * 1024)).toBe("256 KB");
  expect(formatAxisBytes(1.5 * 1024 ** 2)).toBe("1.5 MB");
});

it.each([
  [0, 1024],
  [40 * 1024, 64 * 1024],
  [500 * 1024, 512 * 1024],
  [1024 ** 2, 1024 ** 2],
  [1.5 * 1024 ** 2, 2 * 1024 ** 2],
  [3 * 1024 ** 2, 4 * 1024 ** 2],
  [25 * 1024 ** 2, 32 * 1024 ** 2],
])("creates a readable byte-rate ceiling for %d", (input, expected) => {
  const scale = niceByteRateScale(input);
  expect(scale.maximum).toBe(expected);
  expect(scale.maximum).toBe(scale.increment * 4);
});

it("formats timestamps without treating seconds as milliseconds", () => {
  const value = formatAxisTime(1_700_000_000, "1h");
  expect(value).toMatch(/^\d{2}:\d{2}$/);
  expect(formatTooltipTime(1_700_000_000)).toMatch(/^\d{4}\/\d{1,2}\/\d{1,2} \d{2}:\d{2}:\d{2}$/);
});

it("uses clock-aligned sparse time ticks", () => {
  const from = 1_700_000_000;
  const desktop = timeAxisSplits(from, from + 21_600, "6h", 1_800);
  const mobile = timeAxisSplits(from, from + 21_600, "6h", 360);
  expect(desktop.length).toBeGreaterThanOrEqual(5);
  expect(desktop.every((value) => new Date(value * 1000).getMinutes() === 0)).toBe(true);
  expect(mobile.length).toBeLessThan(desktop.length);
});

it("rejects stale request generations", () => {
  expect(isCurrentHistoryRequest(4, 4)).toBe(true);
  expect(isCurrentHistoryRequest(3, 4)).toBe(false);
});
