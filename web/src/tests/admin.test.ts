import { describe, expect, it } from "vitest";
import { buildNodePatch, buildRuntimeConfig, expiryEpochToLocalDate, formatMicrosForInput, localDateToExpiryEpoch, normalizeCode, parseAdminNode, parseAuthResponse, parseCreateNodeResponse, parseMoneyToMicros, parseNodeConfig, parseRotateTokenResponse, trafficUnitBytes } from "../api/admin";

describe("admin pure helpers", () => {
  it("parses decimal prices exactly", () => {
    expect(parseMoneyToMicros("39.90")).toBe(39_900_000);
    expect(parseMoneyToMicros("399.123456")).toBe(399_123_456);
    expect(parseMoneyToMicros("-1")).toBeNull();
    expect(parseMoneyToMicros("0.0000001")).toBeNull();
  });
  it("round-trips six decimal places", () => {
    for (const value of [0, 100000, 39900000, 39123456]) expect(parseMoneyToMicros(formatMicrosForInput(value))).toBe(value);
  });
  it("normalizes region and currency codes", () => {
    expect(normalizeCode(" us ", 2)).toBe("US");
    expect(normalizeCode("usd", 3)).toBe("USD");
    expect(normalizeCode("USA", 2)).toBeNull();
  });
  it("converts traffic units without treating zero as unlimited", () => {
    expect(trafficUnitBytes("100", "GB")).toBe(100 * 1024 ** 3);
    expect(trafficUnitBytes("1.5", "GB")).toBe(1.5 * 1024 ** 3);
    expect(trafficUnitBytes("0", "GB")).toBeNull();
  });
  it("builds runtime config only when explicitly given a token", () => {
    const config = buildRuntimeConfig("https://monitor.example", "a".repeat(64));
    expect(config).toContain("MONITOR_SERVER=https://monitor.example");
    expect(config).toContain("MONITOR_TOKEN=");
    expect(config).not.toContain("/api/");
  });
  it("validates auth, node, create, rotate, and direct patch responses", () => {
    expect(parseAuthResponse({ authenticated: true, expires_at: 10 }).authenticated).toBe(true);
    expect(() => parseAuthResponse({ authenticated: "yes", expires_at: 10 })).toThrow();
    const node = { id: "a".repeat(32), name: "N", region_code: "US", sort_order: 0, last_ip: null, online: false, last_seen_at: null, cycle_rx: 2, cycle_tx: 3, traffic_limit: null, price_micros: null, currency: null, renewal_cycle: null, expires_at: null };
    expect(parseAdminNode(node).id).toHaveLength(32);
    expect(() => parseAdminNode({ ...node, online: "yes" })).toThrow();
    expect(() => parseAdminNode({ ...node, cycle_rx: -1 })).toThrow();
    const config = { id: node.id, name: "N", region_code: "US", sort_order: 0, traffic_limit: null, traffic_reset_day: 1, price_micros: null, currency: null, renewal_cycle: null, expires_at: null };
    expect(parseNodeConfig(config).traffic_reset_day).toBe(1);
    expect(parseCreateNodeResponse({ node: config, agent_token: "a".repeat(64) }).agent_token).toHaveLength(64);
    expect(parseRotateTokenResponse({ agent_token: "b".repeat(64) }).agent_token).toHaveLength(64);
    expect(() => parseRotateTokenResponse({ agent_token: "bad" })).toThrow();
  });
  it("builds a non-destructive name-only patch", () => {
    const original = { id: "a".repeat(32), name: "N", region_code: "US", sort_order: 0, last_ip: null, online: false, last_seen_at: null, cycle_rx: 2, cycle_tx: 3, traffic_limit: 1_610_612_736, price_micros: 39123456, currency: "USD", renewal_cycle: null, expires_at: 1_700_000_000 };
    expect(buildNodePatch(original, { name: "New", region_code: "US", traffic_limit: original.traffic_limit, price_micros: original.price_micros, currency: original.currency, renewal_cycle: null, expires_at: original.expires_at })).toEqual({ name: "New" });
  });
  it("round-trips local expiry dates", () => {
    const epoch = localDateToExpiryEpoch("2030-02-03");
    expect(epoch).not.toBeNull();
    expect(expiryEpochToLocalDate(epoch)).toBe("2030-02-03");
    expect(localDateToExpiryEpoch("2030-02-31")).toBeNull();
  });
});
