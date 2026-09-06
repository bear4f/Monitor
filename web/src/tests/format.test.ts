import { describe, expect, it } from "vitest";
import { clampProgress, formatBytePair, formatBytes, formatExpiry, formatRate, formatUptime } from "../lib/format";

describe("formatBytes", () => {
  it("uses IEC scaling with compact UI suffixes", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1024)).toBe("1 KB");
    expect(formatBytes(1.5 * 1024 ** 2)).toBe("1.5 MB");
    expect(formatBytes(1.234 * 1024 ** 3)).toBe("1.23 GB");
  });
});

it("shares a unit for compact capacity pairs", () => {
  expect(formatBytePair(256 * 1024 ** 2, 1024 ** 3)).toBe("256 MB / 1 GB");
  expect(formatBytePair(1.5 * 1024 ** 3, 20 * 1024 ** 3)).toBe("1.5 / 20 GB");
  expect(formatBytePair(0, 1024 ** 3)).toBe("0 B / 1 GB");
});

it("formats byte rates", () => {
  expect(formatRate(0)).toBe("0 B/s");
  expect(formatRate(1536)).toBe("1.5 KB/s");
});

it("formats uptime with at most two non-zero units", () => {
  expect(formatUptime(0)).toBe("0 秒");
  expect(formatUptime(28 * 86400 + 9 * 3600 + 12 * 60)).toBe("28 天 9 小时");
  expect(formatUptime(3 * 3600 + 12 * 60)).toBe("3 小时 12 分钟");
});

it("formats expiry relative to the snapshot time", () => {
  const now = 1_700_000_000;
  expect(formatExpiry(null, now)).toBe("∞");
  expect(formatExpiry(now + 60 * 86400, now)).toBe("60 天后到期");
  expect(formatExpiry(now - 3 * 86400, now)).toBe("已过期 3 天");
});

it("clamps progress to the display range", () => {
  expect(clampProgress(-2)).toBe(0);
  expect(clampProgress(42.5)).toBe(42.5);
  expect(clampProgress(120)).toBe(100);
  expect(clampProgress(Number.NaN)).toBe(0);
});
