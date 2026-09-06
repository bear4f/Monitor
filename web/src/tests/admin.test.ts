import { describe, expect, it } from "vitest";
import { buildRuntimeConfig, normalizeCode, parseMoneyToMicros, trafficUnitBytes } from "../api/admin";

describe("admin pure helpers", () => {
  it("parses decimal prices exactly", () => {
    expect(parseMoneyToMicros("39.90")).toBe(39_900_000);
    expect(parseMoneyToMicros("399.123456")).toBe(399_123_456);
    expect(parseMoneyToMicros("-1")).toBeNull();
    expect(parseMoneyToMicros("0.0000001")).toBeNull();
  });
  it("normalizes region and currency codes", () => {
    expect(normalizeCode(" us ", 2)).toBe("US");
    expect(normalizeCode("usd", 3)).toBe("USD");
    expect(normalizeCode("USA", 2)).toBeNull();
  });
  it("converts traffic units without treating zero as unlimited", () => {
    expect(trafficUnitBytes("100", "GB")).toBe(100 * 1024 ** 3);
    expect(trafficUnitBytes("0", "GB")).toBeNull();
  });
  it("builds runtime config only when explicitly given a token", () => {
    const config = buildRuntimeConfig("https://monitor.example", "a".repeat(64));
    expect(config).toContain("MONITOR_SERVER=https://monitor.example");
    expect(config).toContain("MONITOR_TOKEN=");
    expect(config).not.toContain("/api/");
  });
});
