export type ThemePreference = "light" | "dark" | "system";
export type RenewalCycle = "monthly" | "quarterly" | "semiannual" | "annual" | "biennial" | "custom";

export interface PublicSnapshot {
  generated_at: number;
  site: {
    name: string;
    timezone: string;
    theme_default: ThemePreference;
  };
  summary: PublicSummary;
  nodes: PublicNode[];
}

export interface PublicSummary {
  online_nodes: number;
  total_nodes: number;
  busiest_node: {
    id: string;
    name: string;
    cpu_usage: number;
  } | null;
  today_rx: number;
  today_tx: number;
  total_rx: number;
  total_tx: number;
  current_rx_rate: number;
  current_tx_rate: number;
  network_rate_history: {
    timestamps: number[];
    rx: number[];
    tx: number[];
  };
}

export interface PublicNode {
  id: string;
  name: string;
  region_code: string;
  sort_order: number;
  online: boolean;
  first_seen_at: number | null;
  last_seen_at: number | null;
  system: {
    hostname: string;
    os_name: string;
    os_version: string;
    kernel: string;
    architecture: "x86_64" | "aarch64";
    virtualization: string;
    agent_version: string;
    cpu_model: string;
    cpu_cores: number;
    process_count: number;
    uptime_seconds: number;
  } | null;
  metrics: {
    cpu_usage: number;
    load_1: number;
    load_5: number;
    load_15: number;
    memory_total: number;
    memory_used: number;
    swap_total: number;
    swap_used: number;
    disk_total: number;
    disk_used: number;
    current_rx_rate: number;
    current_tx_rate: number;
  } | null;
  traffic: {
    today_rx: number;
    today_tx: number;
    cycle_rx: number;
    cycle_tx: number;
    total_rx: number;
    total_tx: number;
    limit: number | null;
    cycle_start_at: number;
    cycle_end_at: number;
  };
  billing: {
    price_micros: number;
    currency: string;
    renewal_cycle: RenewalCycle | null;
    expires_at: number | null;
  } | null;
}

export type SnapshotFetchResult =
  | { kind: "not-modified" }
  | { kind: "snapshot"; snapshot: PublicSnapshot; etag: string | null };

const SAFE_MAX = Number.MAX_SAFE_INTEGER;

export function parsePublicSnapshot(value: unknown): PublicSnapshot {
  const item = record(value);
  if (!isSafeNonnegative(item.generated_at)) throw new Error("invalid public snapshot");
  const site = parseSite(item.site);
  const summary = parseSummary(item.summary);
  if (!Array.isArray(item.nodes)) throw new Error("invalid public snapshot nodes");
  return {
    generated_at: item.generated_at,
    site,
    summary,
    nodes: item.nodes.map(parsePublicNode),
  };
}

function parseSite(value: unknown): PublicSnapshot["site"] {
  const item = record(value);
  if (
    typeof item.name !== "string" ||
    typeof item.timezone !== "string" ||
    !isTheme(item.theme_default)
  ) throw new Error("invalid public snapshot site");
  return { name: item.name, timezone: item.timezone, theme_default: item.theme_default };
}

function parseSummary(value: unknown): PublicSummary {
  const item = record(value);
  if (
    !isSafeNonnegative(item.online_nodes) ||
    !isSafeNonnegative(item.total_nodes) ||
    item.online_nodes > item.total_nodes ||
    !isSafeNonnegative(item.today_rx) ||
    !isSafeNonnegative(item.today_tx) ||
    !isSafeNonnegative(item.total_rx) ||
    !isSafeNonnegative(item.total_tx) ||
    !isSafeNonnegative(item.current_rx_rate) ||
    !isSafeNonnegative(item.current_tx_rate)
  ) throw new Error("invalid public snapshot summary");

  const busiest = item.busiest_node === null ? null : parseBusiestNode(item.busiest_node);
  const history = record(item.network_rate_history);
  const timestamps = safeIntegerArray(history.timestamps);
  const rx = safeIntegerArray(history.rx);
  const tx = safeIntegerArray(history.tx);
  if (!timestamps || !rx || !tx || timestamps.length !== rx.length || timestamps.length !== tx.length) {
    throw new Error("invalid public snapshot history");
  }
  return {
    online_nodes: item.online_nodes,
    total_nodes: item.total_nodes,
    busiest_node: busiest,
    today_rx: item.today_rx,
    today_tx: item.today_tx,
    total_rx: item.total_rx,
    total_tx: item.total_tx,
    current_rx_rate: item.current_rx_rate,
    current_tx_rate: item.current_tx_rate,
    network_rate_history: { timestamps, rx, tx },
  };
}

function parseBusiestNode(value: unknown): PublicSummary["busiest_node"] {
  const item = record(value);
  if (!isId(item.id) || typeof item.name !== "string" || !isPercent(item.cpu_usage)) {
    throw new Error("invalid public snapshot busiest node");
  }
  return { id: item.id, name: item.name, cpu_usage: item.cpu_usage };
}

function parsePublicNode(value: unknown): PublicNode {
  const item = record(value);
  if (
    !isId(item.id) ||
    typeof item.name !== "string" ||
    !isRegion(item.region_code) ||
    !isSafeNonnegative(item.sort_order) ||
    typeof item.online !== "boolean" ||
    !nullable(item.first_seen_at, isSafeNonnegative) ||
    !nullable(item.last_seen_at, isSafeNonnegative)
  ) throw new Error("invalid public node");

  const firstSeen = optionalSafe(item.first_seen_at);
  const lastSeen = optionalSafe(item.last_seen_at);
  const system = item.system === null ? null : parseSystem(item.system);
  const metrics = item.metrics === null ? null : parseMetrics(item.metrics);
  const traffic = parseTraffic(item.traffic);
  const billing = item.billing === null ? null : parseBilling(item.billing);
  return {
    id: item.id,
    name: item.name,
    region_code: item.region_code,
    sort_order: item.sort_order,
    online: item.online,
    first_seen_at: firstSeen,
    last_seen_at: lastSeen,
    system,
    metrics,
    traffic,
    billing,
  };
}

function parseSystem(value: unknown): NonNullable<PublicNode["system"]> {
  const item = record(value);
  if (
    typeof item.hostname !== "string" ||
    typeof item.os_name !== "string" ||
    typeof item.os_version !== "string" ||
    typeof item.kernel !== "string" ||
    !isArchitecture(item.architecture) ||
    typeof item.virtualization !== "string" ||
    typeof item.agent_version !== "string" ||
    typeof item.cpu_model !== "string" ||
    !isSafePositive(item.cpu_cores) ||
    !isSafeNonnegative(item.process_count) ||
    !isSafeNonnegative(item.uptime_seconds)
  ) throw new Error("invalid public node system");
  return {
    hostname: item.hostname,
    os_name: item.os_name,
    os_version: item.os_version,
    kernel: item.kernel,
    architecture: item.architecture,
    virtualization: item.virtualization,
    agent_version: item.agent_version,
    cpu_model: item.cpu_model,
    cpu_cores: item.cpu_cores,
    process_count: item.process_count,
    uptime_seconds: item.uptime_seconds,
  };
}

function parseMetrics(value: unknown): NonNullable<PublicNode["metrics"]> {
  const item = record(value);
  if (
    !isPercent(item.cpu_usage) ||
    !isFiniteNonnegative(item.load_1) ||
    !isFiniteNonnegative(item.load_5) ||
    !isFiniteNonnegative(item.load_15) ||
    !isSafeNonnegative(item.memory_total) ||
    !isSafeNonnegative(item.memory_used) ||
    !isSafeNonnegative(item.swap_total) ||
    !isSafeNonnegative(item.swap_used) ||
    !isSafeNonnegative(item.disk_total) ||
    !isSafeNonnegative(item.disk_used) ||
    !isSafeNonnegative(item.current_rx_rate) ||
    !isSafeNonnegative(item.current_tx_rate)
  ) throw new Error("invalid public node metrics");
  return {
    cpu_usage: item.cpu_usage,
    load_1: item.load_1,
    load_5: item.load_5,
    load_15: item.load_15,
    memory_total: item.memory_total,
    memory_used: item.memory_used,
    swap_total: item.swap_total,
    swap_used: item.swap_used,
    disk_total: item.disk_total,
    disk_used: item.disk_used,
    current_rx_rate: item.current_rx_rate,
    current_tx_rate: item.current_tx_rate,
  };
}

function parseTraffic(value: unknown): PublicNode["traffic"] {
  const item = record(value);
  if (
    !isSafeNonnegative(item.today_rx) ||
    !isSafeNonnegative(item.today_tx) ||
    !isSafeNonnegative(item.cycle_rx) ||
    !isSafeNonnegative(item.cycle_tx) ||
    !isSafeNonnegative(item.total_rx) ||
    !isSafeNonnegative(item.total_tx) ||
    !nullable(item.limit, (entry) => isSafePositive(entry)) ||
    !isSafeNonnegative(item.cycle_start_at) ||
    !isSafeNonnegative(item.cycle_end_at)
  ) throw new Error("invalid public node traffic");
  const limit = optionalSafePositive(item.limit);
  return {
    today_rx: item.today_rx,
    today_tx: item.today_tx,
    cycle_rx: item.cycle_rx,
    cycle_tx: item.cycle_tx,
    total_rx: item.total_rx,
    total_tx: item.total_tx,
    limit,
    cycle_start_at: item.cycle_start_at,
    cycle_end_at: item.cycle_end_at,
  };
}

function parseBilling(value: unknown): NonNullable<PublicNode["billing"]> {
  const item = record(value);
  if (
    !isSafeNonnegative(item.price_micros) ||
    !isCurrency(item.currency) ||
    !nullable(item.renewal_cycle, isRenewal) ||
    !nullable(item.expires_at, isSafeNonnegative)
  ) throw new Error("invalid public node billing");
  const renewalCycle = optionalRenewal(item.renewal_cycle);
  const expiresAt = optionalSafe(item.expires_at);
  return {
    price_micros: item.price_micros,
    currency: item.currency,
    renewal_cycle: renewalCycle,
    expires_at: expiresAt,
  };
}

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("invalid public response");
  return value as Record<string, unknown>;
}

function nullable(value: unknown, predicate: (value: unknown) => boolean): boolean {
  return value === null || predicate(value);
}

function isSafeNonnegative(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 && value <= SAFE_MAX;
}

function isSafePositive(value: unknown): value is number {
  return isSafeNonnegative(value) && value > 0;
}

function optionalSafe(value: unknown): number | null {
  if (value === null) return null;
  if (!isSafeNonnegative(value)) throw new Error("invalid public response");
  return value;
}

function optionalSafePositive(value: unknown): number | null {
  if (value === null) return null;
  if (!isSafePositive(value)) throw new Error("invalid public response");
  return value;
}

function optionalRenewal(value: unknown): RenewalCycle | null {
  if (value === null) return null;
  if (!isRenewal(value)) throw new Error("invalid public response");
  return value;
}

function isFiniteNonnegative(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

function isPercent(value: unknown): value is number {
  return isFiniteNonnegative(value) && value <= 100;
}

function safeIntegerArray(value: unknown): number[] | null {
  return Array.isArray(value) && value.every(isSafeNonnegative) ? value : null;
}

function isId(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{32}$/.test(value);
}

function isRegion(value: unknown): value is string {
  return typeof value === "string" && /^[A-Z]{2}$/.test(value);
}

function isCurrency(value: unknown): value is string {
  return typeof value === "string" && /^[A-Z]{3}$/.test(value);
}

function isTheme(value: unknown): value is ThemePreference {
  return value === "light" || value === "dark" || value === "system";
}

function isRenewal(value: unknown): value is RenewalCycle {
  return value === "monthly" || value === "quarterly" || value === "semiannual"
    || value === "annual" || value === "biennial" || value === "custom";
}

function isArchitecture(value: unknown): value is NonNullable<PublicNode["system"]>["architecture"] {
  return value === "x86_64" || value === "aarch64";
}

export async function fetchPublicSnapshot(etag: string | null, signal: AbortSignal): Promise<SnapshotFetchResult> {
  const headers = new Headers({ Accept: "application/json" });
  if (etag) headers.set("If-None-Match", etag);

  const response = await fetch("/api/public/snapshot", {
    method: "GET",
    headers,
    cache: "no-cache",
    signal,
  });

  if (response.status === 304) return { kind: "not-modified" };
  if (!response.ok) throw new Error(`snapshot request failed (${response.status})`);

  return {
    kind: "snapshot",
    snapshot: parsePublicSnapshot(await response.json()),
    etag: response.headers.get("ETag"),
  };
}
