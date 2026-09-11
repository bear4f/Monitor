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
  trafficUsedBytes,
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
  validatePingTargetEndpoint,
  formatPingTargetEndpoint,
  probeKindLabel,
  retargetForProbeKind,
  PROBE_KINDS,
  trafficLimitToForm,
  trafficUnitBytes,
  AdminSettings,
  buildSettingsPatch,
  parseAdminSettings,
  parseApiError,
  parseBoundedInteger,
  parseSettingsForm,
  settingsToForm,
  passwordApiErrorAction,
  AdminApiError,
  validateTimezoneInput,
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
  it("keeps settings form numbers as strings until submit validation", () => {
    const form = settingsToForm(settings);
    expect(form.history_retention_days).toBe("7");
    expect(parseSettingsForm({ ...form, site_name: " New " })).toEqual({
      ok: true,
      value: {
        site_name: "New",
        site_timezone: "UTC",
        history_retention_days: 7,
        agent_report_interval_seconds: 10,
        ping_interval_seconds: 30,
        offline_after_seconds: 60,
        default_traffic_reset_day: 1,
      },
    });
  });
  it("rejects each invalid settings range and cross-field relation", () => {
    const form = settingsToForm(settings);
    const invalid: Array<keyof typeof form> = [
      "history_retention_days", "agent_report_interval_seconds", "ping_interval_seconds",
      "offline_after_seconds", "default_traffic_reset_day",
    ];
    for (const key of invalid) expect(parseSettingsForm({ ...form, [key]: "" }).ok).toBe(false);
    expect(parseSettingsForm({ ...form, history_retention_days: "0" }).ok).toBe(false);
    expect(parseSettingsForm({ ...form, history_retention_days: "31" }).ok).toBe(false);
    expect(parseSettingsForm({ ...form, agent_report_interval_seconds: "1" }).ok).toBe(false);
    expect(parseSettingsForm({ ...form, agent_report_interval_seconds: "61" }).ok).toBe(false);
    expect(parseSettingsForm({ ...form, ping_interval_seconds: "9" }).ok).toBe(false);
    expect(parseSettingsForm({ ...form, ping_interval_seconds: "301" }).ok).toBe(false);
    expect(parseSettingsForm({ ...form, offline_after_seconds: "4" }).ok).toBe(false);
    expect(parseSettingsForm({ ...form, offline_after_seconds: "601" }).ok).toBe(false);
    expect(parseSettingsForm({ ...form, default_traffic_reset_day: "0" }).ok).toBe(false);
    expect(parseSettingsForm({ ...form, default_traffic_reset_day: "32" }).ok).toBe(false);
    expect(parseSettingsForm({ ...form, agent_report_interval_seconds: "10", offline_after_seconds: "10" })).toEqual({ ok: false, error: "离线判定时间必须大于 Agent 上报间隔" });
    expect(parseSettingsForm({ ...form, agent_report_interval_seconds: "10", offline_after_seconds: "9" }).ok).toBe(false);
    expect(parseSettingsForm({ ...form, agent_report_interval_seconds: "10", offline_after_seconds: "11" }).ok).toBe(true);
    expect(parseSettingsForm({ ...form, history_retention_days: "1" }).ok).toBe(true);
    expect(parseSettingsForm({ ...form, history_retention_days: "30" }).ok).toBe(true);
    expect(parseSettingsForm({ ...form, agent_report_interval_seconds: "1.5" }).ok).toBe(false);
    expect(parseSettingsForm({ ...form, ping_interval_seconds: "1e2" }).ok).toBe(false);
    expect(parseSettingsForm({ ...form, offline_after_seconds: "-1" }).ok).toBe(false);
    expect(parseSettingsForm({ ...form, site_name: "  Site  " }).ok).toBe(true);
    expect(validateTimezoneInput(" Asia/Shanghai ")).toBe(true);
    expect(validateTimezoneInput(" ")).toBe(false);
    expect(validateTimezoneInput("x".repeat(65))).toBe(false);
  });
  it("uses Unicode character counts for bounded text fields", () => {
    expect(parseSettingsForm({
      ...settingsToForm(settings),
      site_name: "😀".repeat(64),
    }).ok).toBe(true);
    expect(parseSettingsForm({
      ...settingsToForm(settings),
      site_name: "😀".repeat(65),
    }).ok).toBe(false);
    expect(() => parsePingTarget({
      id: 1,
      name: "😀".repeat(65),
      host: "example.com",
      port: null,
      ip_family: 4,
      probe_kind: "icmp",
      enabled: true,
      sort_order: 0,
    })).toThrow();
  });
  it("preserves API error codes and password byte rules", () => {
    expect(parseApiError({ error: { code: "invalid_credentials", message: "bad" } })).toEqual({ code: "invalid_credentials", message: "bad" });
    expect(parseApiError({ error: { code: 1, message: "bad" } })).toBeNull();
    expect(passwordByteLength("密码")).toBe(6);
    expect(passwordFormError("old", "new", "different")).toContain("不一致");
    expect(passwordFormError("same", "same", "same")).toContain("相同");
    expect(passwordFormError("old", "new", "new")).toBeNull();
    expect(passwordFormError("", "new", "new")).toContain("长度");
    expect(passwordFormError("old", "new", "new2")).toContain("不一致");
    expect(passwordApiErrorAction(new AdminApiError(401, "bad", null, "invalid_credentials"))).toBe("invalid-current");
    expect(passwordApiErrorAction(new AdminApiError(401, "expired", null, "unauthorized"))).toBe("session-expired");
    expect(passwordApiErrorAction(new AdminApiError(401, "expired"))).toBe("session-expired");
  });
  const target = {
    id: 1,
    name: "电信 v4",
    host: "203.0.113.1",
    port: null,
    ip_family: 4 as const,
    probe_kind: "icmp" as const,
    enabled: true,
    sort_order: 0,
  };
  const tcpTarget = { ...target, probe_kind: "tcp" as const, port: 443 };
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
  it("binds probe kind to the port it requires", () => {
    expect(parsePingTarget(tcpTarget)).toEqual(tcpTarget);
    // ICMP carries no port, TCP must carry a usable one, and nothing else is a kind.
    expect(() => parsePingTarget({ ...target, port: 443 })).toThrow();
    expect(() => parsePingTarget({ ...tcpTarget, port: null })).toThrow();
    expect(() => parsePingTarget({ ...tcpTarget, port: 0 })).toThrow();
    expect(() => parsePingTarget({ ...tcpTarget, port: 65_536 })).toThrow();
    expect(() => parsePingTarget({ ...tcpTarget, port: 443.5 })).toThrow();
    expect(() => parsePingTarget({ ...tcpTarget, port: "443" })).toThrow();
    expect(() => parsePingTarget({ ...target, probe_kind: "http" })).toThrow();
    expect(() => parsePingTarget({ ...target, probe_kind: "ICMP" })).toThrow();
    expect(() => parsePingTarget({ ...target, probe_kind: undefined })).toThrow();
    expect(PROBE_KINDS).toEqual(["icmp", "tcp"]);
    expect([probeKindLabel("icmp"), probeKindLabel("tcp")]).toEqual(["ICMP", "TCP"]);
  });
  it("formats endpoints the API would accept back", () => {
    expect(formatPingTargetEndpoint(target)).toBe("203.0.113.1");
    expect(formatPingTargetEndpoint(tcpTarget)).toBe("203.0.113.1:443");
    expect(formatPingTargetEndpoint({ ...tcpTarget, host: "2001:db8::1" })).toBe(
      "[2001:db8::1]:443",
    );
    expect(formatPingTargetEndpoint({ ...target, host: "2001:db8::1" })).toBe("2001:db8::1");
  });
  it("keeps the endpoint field consistent when the probe kind changes", () => {
    expect(retargetForProbeKind("icmp", "203.0.113.1:443")).toBe("203.0.113.1");
    expect(retargetForProbeKind("icmp", "[2001:db8::1]:443")).toBe("2001:db8::1");
    // A bare IPv6 literal keeps every hextet: none of them is a port.
    expect(retargetForProbeKind("icmp", "2001:db8::1")).toBe("2001:db8::1");
    expect(retargetForProbeKind("tcp", "203.0.113.1:443")).toBe("203.0.113.1:443");
  });
  it("enforces the six-enabled target limit without blocking enabled edits", () => {
    expect(canEnablePingTarget(5, false)).toBe(true);
    expect(canEnablePingTarget(6, false)).toBe(false);
    expect(canEnablePingTarget(6, true)).toBe(true);
  });
  it("maps target mutation errors to form-safe messages", () => {
    expect(pingTargetMutationMessage(400)).toBe(
      "目标配置无效，请检查名称、探测方式和目标地址（TCP 需 host:port）",
    );
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
  it("checks endpoints per probe kind without owning the syntax", () => {
    expect(validatePingTargetEndpoint("icmp", "2001:db8::1")).toBe(true);
    expect(validatePingTargetEndpoint("tcp", "example.com:443")).toBe(true);
    expect(validatePingTargetEndpoint("tcp", "[2001:db8::1]:443")).toBe(true);
    expect(validatePingTargetEndpoint("tcp", "203.0.113.1:65535")).toBe(true);
    for (const value of [
      "example.com",
      "example.com:",
      "example.com:0",
      "example.com:65536",
      "example.com:0443",
      "example.com:443x",
      "2001:db8::1",
      "https://example.com:443",
    ]) {
      expect(validatePingTargetEndpoint("tcp", value)).toBe(false);
    }
  });
  it("builds non-destructive target patches and clamps sorting", () => {
    const form = {
      name: target.name,
      target: "203.0.113.1",
      probe_kind: "icmp" as const,
      ip_family: 4 as const,
      enabled: true,
    };
    expect(buildPingTargetPatch(target, form)).toEqual({});
    expect(buildPingTargetPatch(target, { ...form, name: "new" })).toEqual({ name: "new" });
    expect(
      buildPingTargetPatch(target, { ...form, target: "::1", ip_family: 6, enabled: false }),
    ).toEqual({ target: "::1", ip_family: 6, enabled: false });
    // Switching kinds sends both halves of the new endpoint, and only those.
    expect(
      buildPingTargetPatch(target, { ...form, probe_kind: "tcp", target: "203.0.113.1:443" }),
    ).toEqual({ probe_kind: "tcp", target: "203.0.113.1:443" });
    // An unchanged TCP target compares against its formatted endpoint, not its host.
    expect(
      buildPingTargetPatch(tcpTarget, { ...form, probe_kind: "tcp", target: "203.0.113.1:443" }),
    ).toEqual({});
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
  it("converts terabyte limits with binary units", () => {
    expect(trafficUnitBytes("1", "TB")).toBe(1024 ** 4);
    expect(trafficUnitBytes("2", "TB")).toBe(2 * 1024 ** 4);
    expect(trafficUnitBytes("1.5", "TB")).toBe(1649267441664);
    // 1 TB is 1024 GB, never 1000 GB.
    expect(trafficUnitBytes("1", "TB")).toBe(trafficUnitBytes("1024", "GB"));
  });
  it("rejects invalid traffic amounts and units", () => {
    for (const amount of ["0", "-1", "-1.5", "", " ", "abc", "1e3", "1.", ".5", "0x10", "1,5", "Infinity", "NaN"]) {
      expect(trafficUnitBytes(amount, "GB")).toBeNull();
      expect(trafficUnitBytes(amount, "TB")).toBeNull();
    }
    for (const unit of ["", "MB", "gb", "tb", "KB", "PB", "B"]) {
      expect(trafficUnitBytes("100", unit)).toBeNull();
    }
    // Beyond the JSON safe integer range there is no exact byte count.
    expect(trafficUnitBytes("100000000", "TB")).toBeNull();
  });
  it("renders a stored traffic limit back into the shortest exact form", () => {
    expect(trafficLimitToForm(null)).toEqual({ amount: "", unit: "GB" });
    expect(trafficLimitToForm(107374182400)).toEqual({ amount: "100", unit: "GB" });
    // Just below 1 TiB stays in GB, exactly 1 TiB switches to TB.
    expect(trafficLimitToForm(1024 ** 4 - 1024 ** 3)).toEqual({ amount: "1023", unit: "GB" });
    expect(trafficLimitToForm(1099511627776)).toEqual({ amount: "1", unit: "TB" });
    expect(trafficLimitToForm(2199023255552)).toEqual({ amount: "2", unit: "TB" });
    expect(trafficLimitToForm(1649267441664)).toEqual({ amount: "1.5", unit: "TB" });
    expect(trafficLimitToForm(1024 ** 4 + 1024 ** 3 * 512)).toEqual({ amount: "1.5", unit: "TB" });
  });
  it("round-trips an arbitrary safe-integer byte count exactly", () => {
    // Regression: the amount used to be produced by rounding to a fixed number
    // of decimals, which cannot represent bytes that are not a whole multiple of
    // the unit. A GB amount needs up to 30 decimal places and a TB amount up to
    // 40, so a rounded value converted back to null or to a different limit.
    const bytes = 1802465118929447;
    const form = trafficLimitToForm(bytes);
    expect(trafficUnitBytes(form.amount, form.unit)).toBe(bytes);
  });
  it("round-trips awkward safe integers on both sides of the terabyte boundary", () => {
    const samples = [
      1, 3, 1023, 1025, 999999937,
      1024 ** 3 + 1, 1024 ** 3 - 1, 107374182401, 549755813887,
      1024 ** 4 - 1, 1024 ** 4 + 1, 1802465118929447, 1319413953331201,
      4503599627370497, 7036874417766401,
      Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER - 1, Number.MAX_SAFE_INTEGER - 12345,
    ];
    for (const bytes of samples) {
      const form = trafficLimitToForm(bytes);
      expect(form.unit).toBe(bytes >= 1024 ** 4 ? "TB" : "GB");
      // The regex in trafficUnitBytes rejects scientific notation.
      expect(form.amount).toMatch(/^(?:\d+|\d+\.\d+)$/);
      expect(trafficUnitBytes(form.amount, form.unit)).toBe(bytes);
    }
  });
  it("never writes trailing zeros and always round-trips back to the same bytes", () => {
    for (const bytes of [
      1024 ** 3,
      100 * 1024 ** 3,
      1024 ** 4,
      1024 ** 4 * 2,
      1649267441664,
      1024 ** 3 * 1536,
      1024 ** 3 * 10,
      1024 ** 4 * 30,
    ]) {
      const form = trafficLimitToForm(bytes);
      expect(form.amount).not.toMatch(/\.\d*0$/);
      expect(form.amount).not.toMatch(/\.$/);
      expect(trafficUnitBytes(form.amount, form.unit)).toBe(bytes);
    }
  });
  it("treats absent or unusable traffic limits as an empty gigabyte field", () => {
    for (const bytes of [0, -1, -(1024 ** 3), 1.5, Number.NaN, Number.MAX_SAFE_INTEGER + 2]) {
      expect(trafficLimitToForm(bytes)).toEqual({ amount: "", unit: "GB" });
    }
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
      total_rx: 10,
      total_tx: 20,
      traffic_limit: null,
      traffic_reset_day: 1,
      traffic_reset_mode: "monthly",
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
      traffic_reset_mode: "monthly",
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
      total_rx: 400,
      total_tx: 500,
      traffic_limit: 1_610_612_736,
      traffic_reset_day: 15,
      traffic_reset_mode: "monthly" as const,
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
        traffic_reset_day: original.traffic_reset_day,
        traffic_reset_mode: original.traffic_reset_mode,
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
        traffic_reset_day: original.traffic_reset_day,
        traffic_reset_mode: original.traffic_reset_mode,
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
        traffic_reset_day: original.traffic_reset_day,
        traffic_reset_mode: original.traffic_reset_mode,
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
        traffic_reset_day: original.traffic_reset_day,
        traffic_reset_mode: original.traffic_reset_mode,
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

describe("traffic reset mode", () => {
  const node = {
    id: "b".repeat(32),
    name: "N",
    region_code: "JP",
    sort_order: 0,
    last_ip: null,
    online: true,
    last_seen_at: 1_700_000_000,
    cycle_rx: 30,
    cycle_tx: 40,
    total_rx: 700,
    total_tx: 800,
    traffic_limit: 1_073_741_824,
    traffic_reset_day: 15,
    traffic_reset_mode: "monthly" as const,
    price_micros: null,
    currency: null,
    renewal_cycle: null,
    expires_at: null,
  };

  it("rejects an unknown mode instead of loosening the parser", () => {
    expect(parseAdminNode(node).traffic_reset_mode).toBe("monthly");
    expect(parseAdminNode({ ...node, traffic_reset_mode: "never" }).traffic_reset_mode).toBe("never");
    for (const invalid of ["", "Monthly", "NEVER", "daily", " never", null, 1, undefined]) {
      expect(() => parseAdminNode({ ...node, traffic_reset_mode: invalid })).toThrow();
    }
    expect(() => parseAdminNode({ ...node, traffic_reset_day: 0 })).toThrow();
    expect(() => parseAdminNode({ ...node, traffic_reset_day: 32 })).toThrow();
    expect(() => parseAdminNode({ ...node, total_rx: -1 })).toThrow();
    const { total_tx: _omitted, ...withoutTotal } = node;
    expect(() => parseAdminNode(withoutTotal)).toThrow();
  });

  it("selects the cycle pair for monthly and the lifetime pair for never", () => {
    expect(trafficUsedBytes(node)).toBe(70);
    expect(trafficUsedBytes({ ...node, traffic_reset_mode: "never" })).toBe(1_500);
    // Unsafe sums degrade to null rather than to a wrong number.
    expect(
      trafficUsedBytes({
        ...node,
        traffic_reset_mode: "never",
        total_rx: Number.MAX_SAFE_INTEGER,
        total_tx: 1,
      }),
    ).toBeNull();
  });

  const form = (overrides: Record<string, unknown> = {}) => ({
    name: node.name,
    region_code: node.region_code,
    traffic_limit: node.traffic_limit,
    traffic_reset_day: node.traffic_reset_day,
    traffic_reset_mode: node.traffic_reset_mode,
    price_micros: node.price_micros,
    currency: node.currency,
    renewal_cycle: node.renewal_cycle,
    expires_at: node.expires_at,
    ...overrides,
  });

  it("sends neither field when nothing about the billing configuration changed", () => {
    expect(buildNodePatch(node, form())).toEqual({});
    expect(buildNodePatch(node, form({ name: "Renamed" }))).toEqual({ name: "Renamed" });
  });

  it("sends only the mode when only the mode changed", () => {
    expect(buildNodePatch(node, form({ traffic_reset_mode: "never" }))).toEqual({
      traffic_reset_mode: "never",
    });
  });

  it("sends only the day when only the day changed", () => {
    expect(buildNodePatch(node, form({ traffic_reset_day: 1 }))).toEqual({
      traffic_reset_day: 1,
    });
  });

  it("keeps the stored reset day when switching back to monthly", () => {
    const never = { ...node, traffic_reset_mode: "never" as const };
    // The day was never erased while in never mode, so returning to monthly
    // needs no day in the patch.
    expect(buildNodePatch(never, form({ traffic_reset_mode: "monthly" }))).toEqual({
      traffic_reset_mode: "monthly",
    });
    expect(
      buildNodePatch(never, form({ traffic_reset_mode: "monthly", traffic_reset_day: 3 })),
    ).toEqual({ traffic_reset_mode: "monthly", traffic_reset_day: 3 });
  });

  it("never treats the lifetime total as a writable field", () => {
    const patch = buildNodePatch(node, form({ traffic_reset_mode: "never" }));
    expect(patch).not.toHaveProperty("total_rx");
    expect(patch).not.toHaveProperty("total_tx");
    expect(patch).not.toHaveProperty("cycle_rx");
    expect(patch).not.toHaveProperty("cycle_tx");
    expect(patch).not.toHaveProperty("traffic_used");
  });
});
