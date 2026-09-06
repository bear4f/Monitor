export interface AuthResponse { authenticated: boolean; expires_at: number }
export interface AdminNode {
  id: string; name: string; region_code: string; sort_order: number; last_ip: string | null;
  online: boolean; last_seen_at: number | null; cycle_rx: number; cycle_tx: number;
  traffic_limit: number | null; price_micros: number | null; currency: string | null;
  renewal_cycle: string | null; expires_at: number | null;
}
export interface NodeConfig {
  id: string; name: string; region_code: string; sort_order: number; traffic_limit: number | null;
  traffic_reset_day: number; price_micros: number | null; currency: string | null;
  renewal_cycle: string | null; expires_at: number | null;
}
export interface Settings {
  site_name: string; site_timezone: string; theme_default: "light" | "dark" | "system";
  history_retention_days: number; agent_report_interval_seconds: number; ping_interval_seconds: number;
  offline_after_seconds: number; default_traffic_reset_day: number;
}

export function readCookie(name: string): string | null {
  const prefix = `${encodeURIComponent(name)}=`;
  const item = document.cookie.split(";").map((part) => part.trim()).find((part) => part.startsWith(prefix));
  return item ? decodeURIComponent(item.slice(prefix.length)) : null;
}

export function csrfToken(): string | null { return readCookie("__Host-monitor_csrf"); }

export class AdminApiError extends Error {
  constructor(public status: number, message: string) { super(message); }
}

async function request<T>(path: string, init: RequestInit = {}, mutation = false): Promise<T> {
  const headers = new Headers(init.headers);
  headers.set("Accept", "application/json");
  if (mutation) {
    const token = csrfToken();
    if (!token) throw new AdminApiError(403, "missing csrf token");
    headers.set("X-CSRF-Token", token);
  }
  if (init.body && !headers.has("Content-Type")) headers.set("Content-Type", "application/json");
  const response = await fetch(path, { ...init, headers, credentials: "same-origin", cache: "no-store" });
  if (response.status === 204) return undefined as T;
  let payload: T | { error?: { message?: string } } = {};
  try { payload = await response.json(); } catch { /* empty */ }
  if (!response.ok) throw new AdminApiError(response.status, (payload as { error?: { message?: string } }).error?.message ?? "请求失败");
  return payload as T;
}

export const authMe = () => request<AuthResponse>("/api/auth/me");
export const login = (password: string) => request<AuthResponse>("/api/auth/login", { method: "POST", body: JSON.stringify({ password }) });
export const logout = () => request<void>("/api/auth/logout", { method: "POST" }, true);
export const listNodes = async () => {
  const payload = await request<unknown>("/api/admin/nodes");
  return { nodes: parseAdminNodes(payload) };
};
export const createNode = (body: Record<string, unknown>) => request<{ node: NodeConfig; agent_token: string }>("/api/admin/nodes", { method: "POST", body: JSON.stringify(body) }, true);
export const updateNode = (id: string, body: Record<string, unknown>) => request<{ node: NodeConfig }>(`/api/admin/nodes/${id}`, { method: "PATCH", body: JSON.stringify(body) }, true);
export const deleteNode = (id: string) => request<void>(`/api/admin/nodes/${id}`, { method: "DELETE" }, true);
export const rotateToken = (id: string) => request<{ agent_token: string }>(`/api/admin/nodes/${id}/rotate-token`, { method: "POST" }, true);

export function parseAdminNodes(value: unknown): AdminNode[] {
  if (!value || typeof value !== "object" || !Array.isArray((value as { nodes?: unknown }).nodes)) throw new Error("invalid nodes response");
  const nodes = (value as { nodes: unknown[] }).nodes;
  if (nodes.some((node) => !node || typeof node !== "object" || typeof (node as AdminNode).id !== "string" || typeof (node as AdminNode).name !== "string" || typeof (node as AdminNode).sort_order !== "number")) throw new Error("invalid nodes response");
  return nodes as AdminNode[];
}

export const RENEWAL_CYCLES = ["monthly", "quarterly", "semiannual", "annual", "biennial", "custom"] as const;

export function parseMoneyToMicros(input: string): number | null {
  const value = input.trim();
  if (!/^\d+(?:\.\d{1,6})?$/.test(value)) return null;
  const [whole, fraction = ""] = value.split(".");
  const micros = Number(whole) * 1_000_000 + Number(fraction.padEnd(6, "0"));
  return Number.isSafeInteger(micros) ? micros : null;
}
export function normalizeCode(value: string, length: number): string | null {
  const normalized = value.trim().toUpperCase();
  return new RegExp(`^[A-Z]{${length}}$`).test(normalized) ? normalized : null;
}
export function trafficUnitBytes(value: string, unit: string): number | null {
  const amount = Number(value); const powers: Record<string, number> = { GB: 3, TB: 4 };
  if (!Number.isInteger(amount) || amount <= 0 || !(unit in powers)) return null;
  const bytes = amount * 1024 ** (powers[unit] * 1);
  return Number.isSafeInteger(bytes) ? bytes : null;
}
export function buildRuntimeConfig(origin: string, token: string): string {
  return `MONITOR_SERVER=${origin}\nMONITOR_TOKEN=${token}`;
}
