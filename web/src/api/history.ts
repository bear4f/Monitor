export type HistoryRange = "1h" | "6h" | "24h" | "7d";

export interface ResourceHistory {
  node_id: string;
  from: number;
  to: number;
  step: number;
  series: {
    timestamp: number[];
    cpu: (number | null)[];
    memory: (number | null)[];
    disk: (number | null)[];
    rx_rate: (number | null)[];
    tx_rate: (number | null)[];
  };
}

export class PublicApiError extends Error {
  constructor(readonly status: number) {
    super(`public API request failed (${status})`);
  }
}

export type HistoryFetchResult =
  | { kind: "not-modified" }
  | { kind: "history"; history: ResourceHistory; etag: string | null };

export function parseResourceHistory(value: unknown): ResourceHistory | null {
  if (!isRecord(value) || typeof value.node_id !== "string" || !isFiniteNumber(value.from)
    || !isFiniteNumber(value.to) || !isFiniteNumber(value.step) || !isRecord(value.series)) return null;

  const series = value.series;
  const timestamp = numberArray(series.timestamp);
  const cpu = nullableNumberArray(series.cpu);
  const memory = nullableNumberArray(series.memory);
  const disk = nullableNumberArray(series.disk);
  const rxRate = nullableNumberArray(series.rx_rate);
  const txRate = nullableNumberArray(series.tx_rate);
  if (!timestamp || !cpu || !memory || !disk || !rxRate || !txRate) return null;
  const length = timestamp.length;
  if ([cpu, memory, disk, rxRate, txRate].some((items) => items.length !== length)) return null;

  return {
    node_id: value.node_id,
    from: value.from,
    to: value.to,
    step: value.step,
    series: { timestamp, cpu, memory, disk, rx_rate: rxRate, tx_rate: txRate },
  };
}

export async function fetchResourceHistory(
  nodeId: string,
  range: HistoryRange,
  etag: string | null,
  signal: AbortSignal,
): Promise<HistoryFetchResult> {
  const headers = new Headers({ Accept: "application/json" });
  if (etag) headers.set("If-None-Match", etag);
  const response = await fetch(`/api/public/nodes/${nodeId}/history?range=${range}`, {
    method: "GET",
    headers,
    signal,
  });
  if (response.status === 304) return { kind: "not-modified" };
  if (!response.ok) throw new PublicApiError(response.status);
  const history = parseResourceHistory(await response.json());
  if (!history) throw new Error("invalid resource history response");
  return { kind: "history", history, etag: response.headers.get("ETag") };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function numberArray(value: unknown): number[] | null {
  return Array.isArray(value) && value.every(isFiniteNumber) ? value : null;
}

function nullableNumberArray(value: unknown): (number | null)[] | null {
  return Array.isArray(value) && value.every((item) => item === null || isFiniteNumber(item)) ? value : null;
}
