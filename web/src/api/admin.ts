export type ThemePreference = "light" | "dark" | "system";
export type TrafficResetMode = "monthly" | "never";
export const TRAFFIC_RESET_MODES = ["monthly", "never"] as const;
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
  total_rx: number;
  total_tx: number;
  traffic_limit: number | null;
  traffic_reset_day: number;
  traffic_reset_mode: TrafficResetMode;
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
  traffic_reset_mode: TrafficResetMode;
  price_micros: number | null;
  currency: string | null;
  renewal_cycle: RenewalCycle | null;
  expires_at: number | null;
}
export type ProbeKind = "icmp" | "tcp";
export const PROBE_KINDS: readonly ProbeKind[] = ["icmp", "tcp"];
export interface PingTarget {
  id: number;
  name: string;
  host: string;
  port: number | null;
  ip_family: 4 | 6;
  probe_kind: ProbeKind;
  enabled: boolean;
  sort_order: number;
}
export interface AdminSettings {
  site_name: string;
  site_timezone: string;
  theme_default: ThemePreference;
  history_retention_days: number;
  agent_report_interval_seconds: number;
  ping_interval_seconds: number;
  offline_after_seconds: number;
  default_traffic_reset_day: number;
}
export interface SettingsForm {
  site_name: string;
  site_timezone: string;
  history_retention_days: string;
  agent_report_interval_seconds: string;
  ping_interval_seconds: string;
  offline_after_seconds: string;
  default_traffic_reset_day: string;
}
export type EditableSettings = Omit<AdminSettings, "theme_default">;

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
    public code: string | null = null,
  ) {
    super(message);
  }
}

export function parseApiError(value: unknown): { code: string; message: string } | null {
  if (!value || typeof value !== "object" || !("error" in value)) return null;
  const error = value.error;
  if (!error || typeof error !== "object" || !("code" in error) || !("message" in error)) return null;
  return typeof error.code === "string" && typeof error.message === "string"
    ? { code: error.code, message: error.message }
    : null;
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
    if (!token) throw new AdminApiError(403, "missing csrf token", null, "csrf");
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
  if (!response.ok) {
    const parsed = parseApiError(payload);
    throw new AdminApiError(
      response.status,
      parsed?.message ?? "请求失败",
      response.status === 429 ? Number(response.headers.get("Retry-After")) || null : null,
      parsed?.code ?? null,
    );
  }
  return payload as T;
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
export const getSettings = async () =>
  parseAdminSettings(await request<unknown>("/api/admin/settings"));
export const updateSettings = async (body: Record<string, unknown>) =>
  parseAdminSettings(
    await request<unknown>(
      "/api/admin/settings",
      { method: "PATCH", body: JSON.stringify(body) },
      true,
    ),
  );
export const changePassword = (current_password: string, new_password: string) =>
  request<void>(
    "/api/admin/password",
    {
      method: "PATCH",
      body: JSON.stringify({ current_password, new_password }),
    },
    true,
  );

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
function characterLength(value: string): number {
  return Array.from(value).length;
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
    !isSafeInt(item.total_rx) ||
    !isSafeInt(item.total_tx) ||
    !nullable(item.traffic_limit, (v) => isSafeInt(v, true)) ||
    !isSafeInt(item.traffic_reset_day, true) ||
    item.traffic_reset_day > 31 ||
    !isResetMode(item.traffic_reset_mode) ||
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
    !isResetMode(item.traffic_reset_mode) ||
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

function isProbeKind(value: unknown): value is ProbeKind {
  return value === "icmp" || value === "tcp";
}
function isProbePort(kind: ProbeKind, port: unknown): boolean {
  return kind === "tcp"
    ? isSafeInt(port, true) && port <= 65535
    : port === null;
}
export function parsePingTarget(value: unknown): PingTarget {
  const item = object(value);
  if (
    !isSafeInt(item.id, true) ||
    typeof item.name !== "string" ||
    characterLength(item.name) < 1 ||
    characterLength(item.name) > 64 ||
    typeof item.host !== "string" ||
    characterLength(item.host) < 1 ||
    characterLength(item.host) > 253 ||
    (item.ip_family !== 4 && item.ip_family !== 6) ||
    !isProbeKind(item.probe_kind) ||
    !isProbePort(item.probe_kind, item.port) ||
    typeof item.enabled !== "boolean" ||
    !isSafeInt(item.sort_order)
  )
    throw new Error("invalid ping target");
  return item as unknown as PingTarget;
}
export function probeKindLabel(kind: ProbeKind): string {
  return kind === "tcp" ? "TCP" : "ICMP";
}
// One endpoint string, exactly as the API accepts it: a bare host for ICMP,
// `host:port` for TCP with an IPv6 literal bracketed.
export function formatPingTargetEndpoint(
  target: Pick<PingTarget, "host" | "port" | "probe_kind">,
): string {
  if (target.probe_kind !== "tcp" || target.port === null) return target.host;
  const host = target.host.includes(":") ? `[${target.host}]` : target.host;
  return `${host}:${target.port}`;
}
export function canEnablePingTarget(
  enabledCount: number,
  originalEnabled: boolean,
): boolean {
  return originalEnabled || enabledCount < 6;
}
export function pingTargetMutationMessage(status: number): string | null {
  if (status === 400)
    return "目标配置无效，请检查名称、探测方式和目标地址（TCP 需 host:port）";
  if (status === 409) return "最多只能启用 6 个延迟监控目标";
  return null;
}

function isBoundedInteger(value: unknown, min: number, max: number): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= min && value <= max;
}
export function parseAdminSettings(value: unknown): AdminSettings {
  const item = object(value);
  const siteName = typeof item.site_name === "string" ? item.site_name.trim() : "";
  const timezone = typeof item.site_timezone === "string" ? item.site_timezone.trim() : "";
  if (
    characterLength(siteName) < 1 || characterLength(siteName) > 64 ||
    characterLength(timezone) < 1 || characterLength(timezone) > 64 ||
    !(["light", "dark", "system"] as string[]).includes(String(item.theme_default)) ||
    !isBoundedInteger(item.history_retention_days, 1, 30) ||
    !isBoundedInteger(item.agent_report_interval_seconds, 2, 60) ||
    !isBoundedInteger(item.ping_interval_seconds, 10, 300) ||
    !isBoundedInteger(item.offline_after_seconds, 5, 600) ||
    !isBoundedInteger(item.default_traffic_reset_day, 1, 31) ||
    item.offline_after_seconds <= item.agent_report_interval_seconds
  ) throw new Error("invalid admin settings");
  return {
    site_name: siteName,
    site_timezone: timezone,
    theme_default: item.theme_default as ThemePreference,
    history_retention_days: item.history_retention_days,
    agent_report_interval_seconds: item.agent_report_interval_seconds,
    ping_interval_seconds: item.ping_interval_seconds,
    offline_after_seconds: item.offline_after_seconds,
    default_traffic_reset_day: item.default_traffic_reset_day,
  };
}
export function buildSettingsPatch(original: AdminSettings, form: AdminSettings): Record<string, unknown> {
  const patch: Record<string, unknown> = {};
  (Object.keys(original) as (keyof AdminSettings)[]).forEach((key) => {
    const value = typeof form[key] === "string" ? form[key].trim() : form[key];
    if (value !== original[key]) patch[key] = value;
  });
  return patch;
}
export function parseBoundedInteger(value: string, min: number, max: number): number | null {
  if (!/^\d+$/.test(value)) return null;
  const number = Number(value);
  return Number.isSafeInteger(number) && number >= min && number <= max ? number : null;
}
export function settingsToForm(settings: AdminSettings): SettingsForm {
  return {
    site_name: settings.site_name,
    site_timezone: settings.site_timezone,
    history_retention_days: String(settings.history_retention_days),
    agent_report_interval_seconds: String(settings.agent_report_interval_seconds),
    ping_interval_seconds: String(settings.ping_interval_seconds),
    offline_after_seconds: String(settings.offline_after_seconds),
    default_traffic_reset_day: String(settings.default_traffic_reset_day),
  };
}
export function validateTimezoneInput(value: string): boolean {
  const trimmed = value.trim();
  return characterLength(trimmed) >= 1 && characterLength(trimmed) <= 64;
}
export function parseSettingsForm(form: SettingsForm):
  | { ok: true; value: EditableSettings }
  | { ok: false; error: string } {
  const siteName = form.site_name.trim();
  const timezone = form.site_timezone.trim();
  if (characterLength(siteName) < 1 || characterLength(siteName) > 64) return { ok: false, error: "请输入有效的站点名称" };
  if (!validateTimezoneInput(timezone)) return { ok: false, error: "请输入有效的站点时区" };
  const history = parseBoundedInteger(form.history_retention_days, 1, 30);
  const report = parseBoundedInteger(form.agent_report_interval_seconds, 2, 60);
  const ping = parseBoundedInteger(form.ping_interval_seconds, 10, 300);
  const offline = parseBoundedInteger(form.offline_after_seconds, 5, 600);
  const reset = parseBoundedInteger(form.default_traffic_reset_day, 1, 31);
  if (history === null || report === null || ping === null || offline === null || reset === null) return { ok: false, error: "请输入有效的设置值" };
  if (offline <= report) return { ok: false, error: "离线判定时间必须大于 Agent 上报间隔" };
  return {
    ok: true,
    value: {
      site_name: siteName,
      site_timezone: timezone,
      history_retention_days: history,
      agent_report_interval_seconds: report,
      ping_interval_seconds: ping,
      offline_after_seconds: offline,
      default_traffic_reset_day: reset,
    },
  };
}
export type PasswordApiErrorAction = "invalid-current" | "session-expired" | "other";
export function passwordApiErrorAction(error: unknown): PasswordApiErrorAction {
  if (!(error instanceof AdminApiError)) return "other";
  if (error.status !== 401) return "other";
  return error.code === "invalid_credentials" ? "invalid-current" : "session-expired";
}
export function passwordByteLength(value: string): number { return new TextEncoder().encode(value).length; }
export function passwordFormError(current: string, next: string, confirm: string): string | null {
  if (passwordByteLength(current) < 1 || passwordByteLength(current) > 1024 || passwordByteLength(next) < 1 || passwordByteLength(next) > 1024) return "密码长度需为 1–1024 字节";
  if (next !== confirm) return "两次输入的新密码不一致";
  if (current === next) return "新密码不能与当前密码相同";
  return null;
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
function splitTcpEndpoint(value: string): { host: string; port: string } | null {
  const separator = value.lastIndexOf(":");
  if (separator < 1) return null;
  const host = value.slice(0, separator);
  // A bare IPv6 literal has colons of its own; only a bracketed host or a
  // colon-free host can carry a port here.
  if (!(host.startsWith("[") && host.endsWith("]")) && host.includes(":")) return null;
  return { host: host.replace(/^\[|\]$/g, ""), port: value.slice(separator + 1) };
}
// A dialog-level sanity check, not a second parser: the Server decides whether
// an endpoint is usable, and its 400 is what the dialog shows when it refuses.
export function validatePingTargetEndpoint(kind: ProbeKind, value: string): boolean {
  const endpoint = value.trim();
  if (kind === "icmp") return validatePingTargetHost(endpoint);
  const parts = splitTcpEndpoint(endpoint);
  if (parts === null) return false;
  return (
    validatePingTargetHost(parts.host) &&
    /^[1-9][0-9]{0,4}$/.test(parts.port) &&
    Number(parts.port) <= 65535
  );
}
// Switching a form to ICMP drops a port the endpoint no longer has room for,
// so the field keeps matching the kind the user just picked.
export function retargetForProbeKind(kind: ProbeKind, value: string): string {
  const endpoint = value.trim();
  if (kind === "tcp") return endpoint;
  const parts = splitTcpEndpoint(endpoint);
  return parts !== null && /^[0-9]+$/.test(parts.port) ? parts.host : endpoint;
}
export interface PingTargetForm {
  name: string;
  target: string;
  probe_kind: ProbeKind;
  ip_family: 4 | 6;
  enabled: boolean;
}
export function buildPingTargetPatch(
  original: PingTarget,
  form: PingTargetForm,
): Record<string, unknown> {
  const patch: Record<string, unknown> = {};
  const name = form.name.trim();
  const target = form.target.trim();
  if (name !== original.name) patch.name = name;
  if (target !== formatPingTargetEndpoint(original)) patch.target = target;
  if (form.probe_kind !== original.probe_kind) patch.probe_kind = form.probe_kind;
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
export type TrafficUnit = "GB" | "TB";

const TRAFFIC_UNIT_SHIFT: Record<TrafficUnit, number> = { GB: 30, TB: 40 };
const TERABYTE_BYTES = 1024 ** 4;

/**
 * Renders a stored byte limit back into the admin form. TB is preferred once the
 * limit reaches 1 TiB.
 *
 * Both divisors are powers of two, so `bytes / 2**shift` always has a finite
 * decimal expansion: the fraction is `remainder * 5**shift` over `10**shift`.
 * Building that with integers keeps the string exact — a GB amount needs up to
 * 30 decimal places and a TB amount up to 40, which no fixed rounding can
 * reach — so `trafficUnitBytes` always converts it back to the original byte
 * count and reopening a node never rewrites its traffic limit.
 */
export function trafficLimitToForm(
  bytes: number | null,
): { amount: string; unit: TrafficUnit } {
  if (bytes === null || !Number.isSafeInteger(bytes) || bytes <= 0)
    return { amount: "", unit: "GB" };
  const unit: TrafficUnit = bytes >= TERABYTE_BYTES ? "TB" : "GB";
  const shift = BigInt(TRAFFIC_UNIT_SHIFT[unit]);
  const scale = 1n << shift;
  const value = BigInt(bytes);
  const whole = (value >> shift).toString();
  const remainder = value & (scale - 1n);
  if (remainder === 0n) return { amount: whole, unit };
  const fraction = (remainder * 5n ** shift)
    .toString()
    .padStart(TRAFFIC_UNIT_SHIFT[unit], "0")
    .replace(/0+$/, "");
  return { amount: `${whole}.${fraction}`, unit };
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
  traffic_reset_day: number;
  traffic_reset_mode: TrafficResetMode;
  price_micros: number | null;
  currency: string | null;
  renewal_cycle: RenewalCycle | null;
  expires_at: number | null;
}
function isResetMode(value: unknown): value is TrafficResetMode {
  return value === "monthly" || value === "never";
}
/**
 * Quota usage for a node. The mode only selects which already-stored
 * accumulation is presented: the current billing cycle, or the lifetime totals.
 * Both counter pairs stay available independently.
 */
export function trafficUsedBytes(node: {
  traffic_reset_mode: TrafficResetMode;
  cycle_rx: number;
  cycle_tx: number;
  total_rx: number;
  total_tx: number;
}): number | null {
  const [left, right] =
    node.traffic_reset_mode === "never"
      ? [node.total_rx, node.total_tx]
      : [node.cycle_rx, node.cycle_tx];
  const sum = left + right;
  return Number.isSafeInteger(left) &&
    Number.isSafeInteger(right) &&
    Number.isSafeInteger(sum)
    ? sum
    : null;
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
  if (form.traffic_reset_day !== original.traffic_reset_day)
    patch.traffic_reset_day = form.traffic_reset_day;
  if (form.traffic_reset_mode !== original.traffic_reset_mode)
    patch.traffic_reset_mode = form.traffic_reset_mode;
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
