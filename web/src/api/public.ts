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
    snapshot: (await response.json()) as PublicSnapshot,
    etag: response.headers.get("ETag"),
  };
}
