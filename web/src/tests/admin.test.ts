import { describe, expect, it } from "vitest";
import { safeAdd } from "../lib/format";
import {
  buildNodePatch,
  buildRuntimeConfig,
  expiryEpochToLocalDate,
  formatMicrosForInput,
  localDateToExpiryEpoch,
  normalizeCode,
  parseAdminNode,
  parseAuthResponse,
  parseCreateNodeResponse,
  parseMoneyToMicros,
  parseNodeConfig,
  parseRotateTokenResponse,
  parsePingTarget,
  parsePingTargets,
  parsePingTargetResponse,
  pingTargetMutationMessage,
  buildPingTargetPatch,
  canEnablePingTarget,
  targetSortOrder,
  validatePingTargetHost,
  trafficUnitBytes,
  AdminSettings,
  buildSettingsPatch,
  parseAdminSettings,
  parseApiError,
  parseBoundedInteger,
  passwordByteLength,
  passwordFormError,
} from "../api/admin";

describe("admin pure helpers", () => {
  const settings: AdminSettings = {
    site_name: "Monitor",
    site_timezone: "UTC",
    theme_default: "system",
    history_retention_days: 7,
    agent_report_interval_seconds: 10,
    ping_interval_seconds: 30,
    offline_after_seconds: 60,
    default_traffic_reset_day: 1,
  };
  it("parses and patches settings strictly", () => {
    expect(parseAdminSettings(settings)).toEqual(settings);
    expect(() => parseAdminSettings({ ...settings, offline_after_seconds: 10 })).toThrow();
    expect(() => parseAdminSettings({ ...settings, theme_default: "sepia" })).toThrow();
    expect(buildSettingsPatch(settings, settings)).toEqual({});
    expect(buildSettingsPatch(settings, { ...settings, site_name: " New " })).toEqual({ site_name: "New" });
    expect(buildSettingsPatch(settings, { ...settings, ping_interval_seconds: 45 })).toEqual({ ping_interval_seconds: 45 });
    expect(parseBoundedInteger("10", 2, 60)).toBe(10);
    expect(parseBoundedInteger("", 2, 60)).toBeNull();
    expect(parseBoundedInteger("61", 2, 60)).toBeNull();
  });
  it("preserves API error codes and password byte rules", () => {
    expect(parseApiError({ error: { code: "invalid_credentials", message: "bad" } })).toEqual({ code: "invalid_credentials", message: "bad" });
    expect(parseApiError({ error: { code: 1, message: "bad" } })).toBeNull();
    expect(passwordByteLength("密码")).toBe(6);
    expect(passwordFormError("old", "new", "different")).toContain("不一致");
    expect(passwordFormError("same", "same", "same")).toContain("相同");
    expect(passwordFormError("old", "new", "new")).toBeNull();
  });
  const target = {
    id: 1,
    name: "电信 v4",
    host: "203.0.113.1",
    ip_family: 4 as const,
    enabled: true,
    sort_order: 0,
  };
  it("parses ping target responses strictly", () => {
    expect(parsePingTarget(target)).toEqual(target);
    expect(parsePingTargets({ targets: [target] }).targets).toHaveLength(1);
    expect(parsePingTargetResponse({ target }).target.id).toBe(1);
    expect(() => parsePingTarget({ ...target, id: -1 })).toThrow();
    expect(() => parsePingTarget({ ...target, id: 0 })).toThrow();
    expect(() => parsePingTarget({ ...target, name: "" })).toThrow();
    expect(() => parsePingTarget({ ...target, host: 4 })).toThrow();
    expect(() => parsePingTarget({ ...target, ip_family: 5 })).toThrow();
    expect(() => parsePingTarget({ ...target, enabled: "yes" })).toThrow();
    expect(() => parsePingTarget({ ...target, sort_order: -1 })).toThrow();
    expect(() => parsePingTargets({ targets: {} })).toThrow();
    expect(() => parsePingTargetResponse({ target: null })).toThrow();
  });
  it("enforces the six-enabled target limit without blocking enabled edits", () => {
    expect(canEnablePingTarget(5, false)).toBe(true);
    expect(canEnablePingTarget(6, false)).toBe(false);
    expect(canEnablePingTarget(6, true)).toBe(true);
  });
  it("maps target mutation errors to form-safe messages", () => {
    expect(pingTargetMutationMessage(400)).toBe("目标配置无效，请检查名称、目标地址和 IP 协议");
    expect(pingTargetMutationMessage(409)).toBe("最多只能启用 6 个延迟监控目标");
    expect(pingTargetMutationMessage(503)).toBeNull();
  });
  it("validates target host UX without rejecting IPv6", () => {
    expect(validatePingTargetHost("203.0.113.1")).toBe(true);
    expect(validatePingTargetHost("example.com")).toBe(true);
    expect(validatePingTargetHost("2001:db8::1")).toBe(true);
    for (const value of ["", "https://example.com", "example.com/path", "example.com?q=x", "bad host"]) {
      expect(validatePingTargetHost(value)).toBe(false);
    }
  });
  it("builds non-destructive target patches and clamps sorting", () => {
    expect(buildPingTargetPatch(target, { name: target.name, host: target.host, ip_family: 4, enabled: true })).toEqual({});
    expect(buildPingTargetPatch(target, { name: "new", host: target.host, ip_family: 4, enabled: true })).toEqual({ name: "new" });
    expect(buildPingTargetPatch(target, { name: target.name, host: "::1", ip_family: 6, enabled: false })).toEqual({ host: "::1", ip_family: 6, enabled: false });
    expect(targetSortOrder(0, -1, 3)).toBeNull();
    expect(targetSortOrder(2, 1, 3)).toBeNull();
    expect(targetSortOrder(1, -1, 3)).toBe(0);
    expect(targetSortOrder(1, 1, 3)).toBe(2);
  });
  it("parses decimal prices exactly", () => {
    expect(parseMoneyToMicros("39.90")).toBe(39_900_000);
    expect(parseMoneyToMicros("399.123456")).toBe(399_123_456);
    expect(parseMoneyToMicros("-1")).toBeNull();
    expect(parseMoneyToMicros("0.0000001")).toBeNull();
  });
  it("round-trips six decimal places", () => {
    for (const value of [0, 100000, 39900000, 39123456])
      expect(parseMoneyToMicros(formatMicrosForInput(value))).toBe(value);
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
    const config = buildRuntimeConfig(
      "https://monitor.example",
      "a".repeat(64),
    );
    expect(config).toContain("MONITOR_SERVER=https://monitor.example");
    expect(config).toContain("MONITOR_TOKEN=");
    expect(config).not.toContain("/api/");
  });
  it("validates auth, node, create, rotate, and direct patch responses", () => {
    expect(
      parseAuthResponse({ authenticated: true, expires_at: 10 }).authenticated,
    ).toBe(true);
    expect(() =>
      parseAuthResponse({ authenticated: "yes", expires_at: 10 }),
    ).toThrow();
    const node = {
      id: "a".repeat(32),
      name: "N",
      region_code: "US",
      sort_order: 0,
      last_ip: null,
      online: false,
      last_seen_at: null,
      cycle_rx: 2,
      cycle_tx: 3,
      traffic_limit: null,
      price_micros: null,
      currency: null,
      renewal_cycle: null,
      expires_at: null,
    };
    expect(parseAdminNode(node).id).toHaveLength(32);
    expect(() => parseAdminNode({ ...node, online: "yes" })).toThrow();
    expect(() => parseAdminNode({ ...node, cycle_rx: -1 })).toThrow();
    const config = {
      id: node.id,
      name: "N",
      region_code: "US",
      sort_order: 0,
      traffic_limit: null,
      traffic_reset_day: 1,
      price_micros: null,
      currency: null,
      renewal_cycle: null,
      expires_at: null,
    };
    expect(parseNodeConfig(config).traffic_reset_day).toBe(1);
    expect(
      parseCreateNodeResponse({ node: config, agent_token: "a".repeat(64) })
        .agent_token,
    ).toHaveLength(64);
    expect(
      parseRotateTokenResponse({ agent_token: "b".repeat(64) }).agent_token,
    ).toHaveLength(64);
    expect(() => parseRotateTokenResponse({ agent_token: "bad" })).toThrow();
  });
  it("builds a non-destructive name-only patch", () => {
    const original = {
      id: "a".repeat(32),
      name: "N",
      region_code: "US",
      sort_order: 0,
      last_ip: null,
      online: false,
      last_seen_at: null,
      cycle_rx: 2,
      cycle_tx: 3,
      traffic_limit: 1_610_612_736,
      price_micros: 39123456,
      currency: "USD",
      renewal_cycle: null,
      expires_at: 1_700_000_000,
    };
    expect(
      buildNodePatch(original, {
        name: "New",
        region_code: "US",
        traffic_limit: original.traffic_limit,
        price_micros: original.price_micros,
        currency: original.currency,
        renewal_cycle: null,
        expires_at: original.expires_at,
      }),
    ).toEqual({ name: "New" });
    expect(
      buildNodePatch(original, {
        name: "N",
        region_code: "US",
        traffic_limit: original.traffic_limit,
        price_micros: original.price_micros,
        currency: original.currency,
        renewal_cycle: null,
        expires_at: original.expires_at,
      }),
    ).toEqual({});
    expect(
      buildNodePatch(original, {
        name: "N",
        region_code: "US",
        traffic_limit: original.traffic_limit,
        price_micros: null,
        currency: null,
        renewal_cycle: null,
        expires_at: original.expires_at,
      }),
    ).toEqual({ price_micros: null, currency: null });
    expect(
      buildNodePatch(original, {
        name: "N",
        region_code: "US",
        traffic_limit: original.traffic_limit,
        price_micros: original.price_micros,
        currency: "EUR",
        renewal_cycle: null,
        expires_at: original.expires_at,
      }),
    ).toEqual({ currency: "EUR" });
  });
  it("rejects mismatched price and currency response fields", () => {
    const node = {
      id: "a".repeat(32),
      name: "N",
      region_code: "US",
      sort_order: 0,
      last_ip: null,
      online: false,
      last_seen_at: null,
      cycle_rx: 0,
      cycle_tx: 0,
      traffic_limit: null,
      price_micros: null,
      currency: null,
      renewal_cycle: null,
      expires_at: null,
    };
    expect(() =>
      parseAdminNode({ ...node, price_micros: null, currency: "USD" }),
    ).toThrow();
    const config = {
      id: node.id,
      name: "N",
      region_code: "US",
      sort_order: 0,
      traffic_limit: null,
      traffic_reset_day: 1,
      price_micros: null,
      currency: null,
      renewal_cycle: null,
      expires_at: null,
    };
    expect(() =>
      parseNodeConfig({ ...config, price_micros: 1, currency: null }),
    ).toThrow();
  });
  it("keeps unsafe traffic sums out of the UI", () => {
    expect(safeAdd(Number.MAX_SAFE_INTEGER, 1)).toBeNull();
    expect(safeAdd(10, 20)).toBe(30);
  });
  it("round-trips local expiry dates", () => {
    const epoch = localDateToExpiryEpoch("2030-02-03");
    expect(epoch).not.toBeNull();
    expect(expiryEpochToLocalDate(epoch)).toBe("2030-02-03");
    expect(localDateToExpiryEpoch("2030-02-31")).toBeNull();
  });
});
