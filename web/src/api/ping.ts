import { PublicApiError, type HistoryRange } from "./history";

export interface PingTarget {
  id: number;
  name: string;
  ip_family: 4 | 6;
  sort_order: number;
}

export interface PingSeries {
  target_id: number;
  latency: (number | null)[];
  // Failed samples over total samples per bucket, as a 0..1 ratio. null means the
  // bucket has no samples at all, which is not the same as no loss.
  loss: (number | null)[];
}

export interface PingHistory {
  node_id: string;
  from: number;
  to: number;
  step: number;
  targets: PingTarget[];
  timestamps: number[];
  series: PingSeries[];
}

export type PingFetchResult =
  | { kind: "not-modified" }
  | { kind: "history"; history: PingHistory; etag: string | null };

export function parsePingHistory(value: unknown): PingHistory | null {
  if (!isRecord(value) || typeof value.node_id !== "string" || !isFiniteNumber(value.from)
    || !isFiniteNumber(value.to) || !isFiniteNumber(value.step)) return null;
  if (!Array.isArray(value.targets) || !Array.isArray(value.timestamps) || !Array.isArray(value.series)) return null;
  if (value.targets.length > 6) return null;

  const timestamps = numberArray(value.timestamps);
  if (!timestamps) return null;
  const targets: PingTarget[] = [];
  const targetIds = new Set<number>();
  for (const candidate of value.targets) {
    if (!isRecord(candidate) || !isSafeInteger(candidate.id) || typeof candidate.name !== "string"
      || (candidate.ip_family !== 4 && candidate.ip_family !== 6) || !isSafeInteger(candidate.sort_order)
      || targetIds.has(candidate.id)) return null;
    targetIds.add(candidate.id);
    targets.push({
      id: candidate.id,
      name: candidate.name,
      ip_family: candidate.ip_family,
      sort_order: candidate.sort_order,
    });
  }

  if (value.series.length !== targets.length) return null;
  const series: PingSeries[] = [];
  for (let index = 0; index < value.series.length; index += 1) {
    const candidate = value.series[index];
    if (!isRecord(candidate) || !isSafeInteger(candidate.target_id)
      || candidate.target_id !== targets[index]?.id || !Array.isArray(candidate.latency)
      || !Array.isArray(candidate.loss)) return null;
    const latency = nullableLatencyArray(candidate.latency);
    if (!latency || latency.length !== timestamps.length) return null;
    const loss = nullableRatioArray(candidate.loss);
    if (!loss || loss.length !== timestamps.length) return null;
    series.push({ target_id: candidate.target_id, latency, loss });
  }

  return {
    node_id: value.node_id,
    from: value.from,
    to: value.to,
    step: value.step,
    targets,
    timestamps,
    series,
  };
}

export async function fetchPingHistory(
  nodeId: string,
  range: HistoryRange,
  etag: string | null,
  signal: AbortSignal,
): Promise<PingFetchResult> {
  const headers = new Headers({ Accept: "application/json" });
  if (etag) headers.set("If-None-Match", etag);
  const response = await fetch(`/api/public/nodes/${nodeId}/ping?range=${range}`, {
    method: "GET",
    headers,
    signal,
  });
  if (response.status === 304) return { kind: "not-modified" };
  if (!response.ok) throw new PublicApiError(response.status);
  const history = parsePingHistory(await response.json());
  if (!history) throw new Error("invalid ping history response");
  return { kind: "history", history, etag: response.headers.get("ETag") };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function isSafeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function numberArray(value: unknown[]): number[] | null {
  return value.every(isFiniteNumber) ? value : null;
}

function nullableLatencyArray(value: unknown[]): (number | null)[] | null {
  return value.every((item) => item === null || (isFiniteNumber(item) && item >= 0))
    ? value as (number | null)[]
    : null;
}

function nullableRatioArray(value: unknown[]): (number | null)[] | null {
  return value.every((item) => item === null || (isFiniteNumber(item) && item >= 0 && item <= 1))
    ? value as (number | null)[]
    : null;
}
