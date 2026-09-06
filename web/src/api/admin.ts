export type ThemePreference = "light" | "dark" | "system";
export type RenewalCycle =
  | "monthly"
  | "quarterly"
  | "semiannual"
  | "annual"
  | "biennial"
  | "custom";
export const RENEWAL_CYCLES = [
  "monthly",
  "quarterly",
  "semiannual",
  "annual",
  "biennial",
  "custom",
] as const;
const SAFE_MAX = Number.MAX_SAFE_INTEGER;

export interface AuthResponse {
  authenticated: boolean;
  expires_at: number;
}
export interface AdminNode {
  id: string;
  name: string;
  region_code: string;
  sort_order: number;
  last_ip: string | null;
  online: boolean;
  last_seen_at: number | null;
  cycle_rx: number;
  cycle_tx: number;
  traffic_limit: number | null;
  price_micros: number | null;
  currency: string | null;
  renewal_cycle: RenewalCycle | null;
  expires_at: number | null;
}
export interface NodeConfig {
  id: string;
  name: string;
  region_code: string;
  sort_order: number;
  traffic_limit: number | null;
  traffic_reset_day: number;
  price_micros: number | null;
  currency: string | null;
  renewal_cycle: RenewalCycle | null;
  expires_at: number | null;
}
export interface PingTarget {
  id: number;
  name: string;
  host: string;
  ip_family: 4 | 6;
  enabled: boolean;
  sort_order: number;
}

export function readCookie(name: string): string | null {
  if (typeof document === "undefined") return null;
  const prefix = `${encodeURIComponent(name)}=`;
  const item = document.cookie
    .split(";")
    .map((part) => part.trim())
    .find((part) => part.startsWith(prefix));
  return item ? decodeURIComponent(item.slice(prefix.length)) : null;
}
export const csrfToken = () => readCookie("__Host-monitor_csrf");
export class AdminApiError extends Error {
  constructor(
    public status: number,
    message: string,
    public retryAfter: number | null = null,
  ) {
    super(message);
  }
}

async function request<T>(
  path: string,
  init: RequestInit = {},
  mutation = false,
): Promise<T> {
  const headers = new Headers(init.headers);
  headers.set("Accept", "application/json");
  if (mutation) {
    const token = csrfToken();
    if (!token) throw new AdminApiError(403, "missing csrf token");
    headers.set("X-CSRF-Token", token);
  }
  if (init.body && !headers.has("Content-Type"))
    headers.set("Content-Type", "application/json");
  const response = await fetch(path, {
    ...init,
    headers,
    credentials: "same-origin",
    cache: "no-store",
  });
  if (response.status === 204) return undefined as T;
  let payload: unknown = null;
  try {
    payload = await response.json();
  } catch {
    /* empty */
  }
  if (!response.ok)
    throw new AdminApiError(
      response.status,
      errorMessage(payload),
      response.status === 429
        ? Number(response.headers.get("Retry-After")) || null
        : null,
    );
  return payload as T;
}
function errorMessage(payload: unknown): string {
  if (
    !payload ||
    typeof payload !== "object" ||
    !("error" in payload) ||
    !payload.error ||
    typeof payload.error !== "object" ||
    !("message" in payload.error) ||
    typeof payload.error.message !== "string"
  )
    return "请求失败";
  return payload.error.message;
}

export const authMe = async () =>
  parseAuthResponse(await request<unknown>("/api/auth/me"));
export const login = async (password: string) =>
  parseAuthResponse(
    await request<unknown>("/api/auth/login", {
      method: "POST",
      body: JSON.stringify({ password }),
    }),
  );
export const logout = () =>
  request<void>("/api/auth/logout", { method: "POST" }, true);
export const listNodes = async () => ({
  nodes: parseAdminNodes(await request<unknown>("/api/admin/nodes")),
});
export const createNode = async (body: Record<string, unknown>) =>
  parseCreateNodeResponse(
    await request<unknown>(
      "/api/admin/nodes",
      { method: "POST", body: JSON.stringify(body) },
      true,
    ),
  );
export const updateNode = async (id: string, body: Record<string, unknown>) =>
  parseNodeConfig(
    await request<unknown>(
      `/api/admin/nodes/${id}`,
      { method: "PATCH", body: JSON.stringify(body) },
      true,
    ),
  );
export const deleteNode = (id: string) =>
  request<void>(`/api/admin/nodes/${id}`, { method: "DELETE" }, true);
export const rotateToken = async (id: string) =>
  parseRotateTokenResponse(
    await request<unknown>(
      `/api/admin/nodes/${id}/rotate-token`,
      { method: "POST" },
      true,
    ),
  );
export const listPingTargets = async () =>
  parsePingTargets(await request<unknown>("/api/admin/ping-targets"));
export const createPingTarget = async (body: Record<string, unknown>) =>
  parsePingTargetResponse(
    await request<unknown>(
      "/api/admin/ping-targets",
      { method: "POST", body: JSON.stringify(body) },
      true,
    ),
  );
export const updatePingTarget = async (
  id: number,
  body: Record<string, unknown>,
) =>
  parsePingTargetResponse(
    await request<unknown>(
      `/api/admin/ping-targets/${id}`,
      { method: "PATCH", body: JSON.stringify(body) },
      true,
    ),
  );
export const deletePingTarget = (id: number) =>
  request<void>(`/api/admin/ping-targets/${id}`, { method: "DELETE" }, true);

function isSafeInt(value: unknown, positive = false): value is number {
  return (
    typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value >= (positive ? 1 : 0) &&
    value <= SAFE_MAX
  );
}
function isId(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{32}$/.test(value);
}
function isCode(value: unknown, length: number): value is string {
  return (
    typeof value === "string" && new RegExp(`^[A-Z]{${length}}$`).test(value)
  );
}
function isRenewal(value: unknown): value is RenewalCycle {
  return (
    typeof value === "string" &&
    (RENEWAL_CYCLES as readonly string[]).includes(value)
  );
}
function nullable(
  value: unknown,
  predicate: (item: unknown) => boolean,
): boolean {
  return value === null || predicate(value);
}
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("invalid admin response");
  return value as Record<string, unknown>;
}

export function parseAuthResponse(value: unknown): AuthResponse {
  const item = object(value);
  if (item.authenticated !== true || !isSafeInt(item.expires_at))
    throw new Error("invalid auth response");
  return { authenticated: true, expires_at: item.expires_at };
}
export function parseAdminNode(value: unknown): AdminNode {
  const item = object(value);
  if (
    !isId(item.id) ||
    typeof item.name !== "string" ||
    !isCode(item.region_code, 2) ||
    !isSafeInt(item.sort_order) ||
    (typeof item.last_ip !== "string" && item.last_ip !== null) ||
    typeof item.online !== "boolean" ||
    !nullable(item.last_seen_at, isSafeInt) ||
    !isSafeInt(item.cycle_rx) ||
    !isSafeInt(item.cycle_tx) ||
    !nullable(item.traffic_limit, (v) => isSafeInt(v, true)) ||
    !nullable(item.price_micros, isSafeInt) ||
    !nullable(item.currency, (v) => isCode(v, 3)) ||
    !nullable(item.renewal_cycle, isRenewal) ||
    !nullable(item.expires_at, isSafeInt) ||
    (item.price_micros === null) !== (item.currency === null)
  )
    throw new Error("invalid admin node");
  return item as unknown as AdminNode;
}
export function parseAdminNodes(value: unknown): AdminNode[] {
  const item = object(value);
  if (!Array.isArray(item.nodes)) throw new Error("invalid nodes response");
  return item.nodes.map(parseAdminNode);
}
export function parseNodeConfig(value: unknown): NodeConfig {
  const item = object(value);
  if (
    !isId(item.id) ||
    typeof item.name !== "string" ||
    !isCode(item.region_code, 2) ||
    !isSafeInt(item.sort_order) ||
    !nullable(item.traffic_limit, (v) => isSafeInt(v, true)) ||
    !isSafeInt(item.traffic_reset_day, true) ||
    item.traffic_reset_day > 31 ||
    !nullable(item.price_micros, isSafeInt) ||
    !nullable(item.currency, (v) => isCode(v, 3)) ||
    !nullable(item.renewal_cycle, isRenewal) ||
    !nullable(item.expires_at, isSafeInt) ||
    (item.price_micros === null) !== (item.currency === null)
  )
    throw new Error("invalid node config");
  return item as unknown as NodeConfig;
}
export function parseCreateNodeResponse(value: unknown): {
  node: NodeConfig;
  agent_token: string;
} {
  const item = object(value);
  if (
    typeof item.agent_token !== "string" ||
    !/^[0-9a-f]{64}$/.test(item.agent_token)
  )
    throw new Error("invalid create response");
  return { node: parseNodeConfig(item.node), agent_token: item.agent_token };
}
export function parseRotateTokenResponse(value: unknown): {
  agent_token: string;
} {
  const item = object(value);
  if (
    typeof item.agent_token !== "string" ||
    !/^[0-9a-f]{64}$/.test(item.agent_token)
  )
    throw new Error("invalid rotate response");
  return { agent_token: item.agent_token };
}

export function parsePingTarget(value: unknown): PingTarget {
  const item = object(value);
  if (
    !isSafeInt(item.id) ||
    typeof item.name !== "string" ||
    item.name.length < 1 ||
    item.name.length > 64 ||
    typeof item.host !== "string" ||
    item.host.length < 1 ||
    item.host.length > 253 ||
    (item.ip_family !== 4 && item.ip_family !== 6) ||
    typeof item.enabled !== "boolean" ||
    !isSafeInt(item.sort_order)
  )
    throw new Error("invalid ping target");
  return item as unknown as PingTarget;
}
export function parsePingTargets(value: unknown): { targets: PingTarget[] } {
  const item = object(value);
  if (!Array.isArray(item.targets)) throw new Error("invalid targets response");
  return { targets: item.targets.map(parsePingTarget) };
}
export function parsePingTargetResponse(value: unknown): { target: PingTarget } {
  const item = object(value);
  return { target: parsePingTarget(item.target) };
}
export function validatePingTargetHost(value: string): boolean {
  const host = value.trim();
  return (
    host.length > 0 &&
    host.length <= 253 &&
    !/\s/.test(host) &&
    !/^https?:\/\//i.test(host) &&
    !/[\/?#]/.test(host)
  );
}
export function buildPingTargetPatch(
  original: PingTarget,
  form: Pick<PingTarget, "name" | "host" | "ip_family" | "enabled">,
): Record<string, unknown> {
  const patch: Record<string, unknown> = {};
  const name = form.name.trim();
  const host = form.host.trim();
  if (name !== original.name) patch.name = name;
  if (host !== original.host) patch.host = host;
  if (form.ip_family !== original.ip_family) patch.ip_family = form.ip_family;
  if (form.enabled !== original.enabled) patch.enabled = form.enabled;
  return patch;
}
export function targetSortOrder(
  current: number,
  delta: number,
  length: number,
): number | null {
  const next = current + delta;
  return next >= 0 && next < length ? next : null;
}

export function parseMoneyToMicros(input: string): number | null {
  const value = input.trim();
  if (!/^\d+(?:\.\d{1,6})?$/.test(value)) return null;
  const [whole, fraction = ""] = value.split(".");
  const micros = Number(whole) * 1_000_000 + Number(fraction.padEnd(6, "0"));
  return Number.isSafeInteger(micros) ? micros : null;
}
export function formatMicrosForInput(micros: number): string {
  if (!Number.isSafeInteger(micros) || micros < 0) return "";
  const whole = Math.floor(micros / 1_000_000);
  const fraction = String(micros % 1_000_000)
    .padStart(6, "0")
    .replace(/0+$/, "");
  return fraction ? `${whole}.${fraction}` : String(whole);
}
export function normalizeCode(value: string, length: number): string | null {
  const normalized = value.trim().toUpperCase();
  return isCode(normalized, length) ? normalized : null;
}
export function trafficUnitBytes(value: string, unit: string): number | null {
  const normalized = value.trim();
  const amount = Number(normalized);
  const exponent = unit === "GB" ? 3 : unit === "TB" ? 4 : 0;
  if (
    !/^(?:\d+|\d+\.\d+)$/.test(normalized) ||
    !Number.isFinite(amount) ||
    amount <= 0 ||
    exponent === 0
  )
    return null;
  const bytes = amount * 1024 ** exponent;
  return Number.isSafeInteger(bytes) ? bytes : null;
}
export function localDateToExpiryEpoch(value: string): number | null {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) return null;
  const [year, month, day] = value.split("-").map(Number);
  const date = new Date(year, month - 1, day, 23, 59, 59, 0);
  return date.getFullYear() === year &&
    date.getMonth() === month - 1 &&
    date.getDate() === day
    ? Math.floor(date.getTime() / 1000)
    : null;
}
export function expiryEpochToLocalDate(epoch: number | null): string {
  if (epoch === null || !Number.isSafeInteger(epoch)) return "";
  const date = new Date(epoch * 1000);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}
export interface NodeEditForm {
  name: string;
  region_code: string;
  traffic_limit: number | null;
  price_micros: number | null;
  currency: string | null;
  renewal_cycle: RenewalCycle | null;
  expires_at: number | null;
}
export function buildNodePatch(
  original: AdminNode,
  form: NodeEditForm,
): Record<string, unknown> {
  const patch: Record<string, unknown> = {};
  const name = form.name.trim();
  const region = form.region_code.trim().toUpperCase();
  if (name !== original.name) patch.name = name;
  if (region !== original.region_code) patch.region_code = region;
  if (form.traffic_limit !== original.traffic_limit)
    patch.traffic_limit = form.traffic_limit;
  const currency = form.price_micros === null ? null : form.currency;
  if (form.price_micros !== original.price_micros)
    patch.price_micros = form.price_micros;
  if (currency !== original.currency) patch.currency = currency;
  if (form.renewal_cycle !== original.renewal_cycle)
    patch.renewal_cycle = form.renewal_cycle;
  if (form.expires_at !== original.expires_at)
    patch.expires_at = form.expires_at;
  return patch;
}
export function buildRuntimeConfig(origin: string, token: string): string {
  return `MONITOR_SERVER=${origin}\nMONITOR_TOKEN=${token}`;
}
