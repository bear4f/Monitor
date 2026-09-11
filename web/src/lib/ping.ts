import type uPlot from "uplot";
import type { PingHistory } from "../api/ping";

export interface IndexViewport {
  start: number;
  end: number;
}

export interface NiceLatencyScale {
  maximum: number;
  increment: number;
}

const DASH_PATTERNS: readonly (readonly number[])[] = [
  [],
  [],
  [6, 4],
  [2, 4],
  [10, 4, 2, 4],
  [1, 4],
];

export function median(values: readonly number[]): number | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0 ? (sorted[middle - 1] + sorted[middle]) / 2 : sorted[middle];
}

export function medianAbsoluteDeviation(values: readonly number[]): number | null {
  const center = median(values);
  return center === null ? null : median(values.map((value) => Math.abs(value - center)));
}

export function nearestRankP95(values: readonly number[]): number | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((left, right) => left - right);
  const index = Math.min(sorted.length - 1, Math.max(0, Math.ceil(sorted.length * 0.95) - 1));
  return sorted[index];
}

export function cutPeakCap(values: readonly number[]): number | null {
  if (values.length < 20) return null;
  const center = median(values);
  const deviation = medianAbsoluteDeviation(values);
  const percentile = nearestRankP95(values);
  if (center === null || deviation === null || percentile === null) return null;
  return Math.max(percentile, center + 6 * deviation, center + 20);
}

export function fullViewport(length: number): IndexViewport {
  return { start: 0, end: Math.max(0, length - 1) };
}

export function clampViewport(start: number, end: number, length: number): IndexViewport {
  if (length <= 1) return fullViewport(length);
  const last = length - 1;
  let safeStart = Math.min(last - 1, Math.max(0, Math.round(start)));
  let safeEnd = Math.min(last, Math.max(1, Math.round(end)));
  if (safeEnd <= safeStart) {
    if (safeStart < last) safeEnd = safeStart + 1;
    else safeStart = safeEnd - 1;
  }
  return { start: safeStart, end: safeEnd };
}

export function displayLatencySeries(
  raw: (number | null)[],
  cutPeak: boolean,
  viewport: IndexViewport,
): (number | null)[] {
  if (!cutPeak) return raw;
  const bounds = clampViewport(viewport.start, viewport.end, raw.length);
  const values = raw.slice(bounds.start, bounds.end + 1).filter((value): value is number => value !== null);
  const cap = cutPeakCap(values);
  if (cap === null) return raw;
  return raw.map((value, index) => (
    value !== null && index >= bounds.start && index <= bounds.end ? Math.min(value, cap) : value
  ));
}

export function pingAlignedData(
  history: PingHistory,
  cutPeak: boolean,
  viewport: IndexViewport,
): uPlot.AlignedData {
  return [
    history.timestamps,
    ...history.series.map((series) => displayLatencySeries(series.latency, cutPeak, viewport)),
  ];
}

export function viewportMaximum(data: uPlot.AlignedData, viewport: IndexViewport): number {
  const length = data[0].length;
  const bounds = clampViewport(viewport.start, viewport.end, length);
  let maximum = 0;
  for (let seriesIndex = 1; seriesIndex < data.length; seriesIndex += 1) {
    const series = data[seriesIndex];
    for (let index = bounds.start; index <= bounds.end; index += 1) {
      const value = series[index];
      if (typeof value === "number" && Number.isFinite(value)) maximum = Math.max(maximum, value);
    }
  }
  return maximum;
}

export function niceLatencyScale(maximum: number): NiceLatencyScale {
  const target = Math.max(Number.isFinite(maximum) && maximum > 0 ? maximum : 0, 1) / 4;
  const magnitude = 10 ** Math.floor(Math.log10(target));
  const increment = [1, 2, 2.5, 5, 10]
    .map((value) => value * magnitude)
    .find((value) => value >= target) ?? target;
  return { maximum: increment * 4, increment };
}

export function pingSeriesColor(
  index: number,
  total: number,
  dark: boolean,
  oklchSupported = supportsOklch(),
): string {
  const safeTotal = Number.isInteger(total) && total > 0 ? total : 1;
  const safeIndex = Number.isInteger(index) ? ((index % safeTotal) + safeTotal) % safeTotal : 0;
  const hue = Math.round((safeIndex * 360) / safeTotal);
  if (oklchSupported) {
    return dark ? `oklch(0.74 0.14 ${hue} / 0.94)` : `oklch(0.64 0.16 ${hue} / 0.92)`;
  }
  return dark ? `hsl(${hue} 48% 68%)` : `hsl(${hue} 52% 52%)`;
}

export function pingSeriesDash(index: number): readonly number[] {
  return DASH_PATTERNS[Math.max(0, index) % DASH_PATTERNS.length];
}

export function latestValue(values: readonly (number | null)[]): number | null {
  for (let index = values.length - 1; index >= 0; index -= 1) {
    if (values[index] !== null) return values[index];
  }
  return null;
}

function supportsOklch(): boolean {
  return typeof CSS !== "undefined" && typeof CSS.supports === "function"
    && CSS.supports("color", "oklch(0.7 0.16 120 / 0.9)");
}

// Loss arrives as a 0..1 ratio; the legend shows it as a percentage.
export function formatLossPercent(ratio: number): string {
  const percent = ratio * 100;
  const rounded = percent >= 10 ? Math.round(percent) : Number(percent.toFixed(1));
  return `${rounded}%`;
}
