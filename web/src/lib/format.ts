const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB"] as const;
const DAY_SECONDS = 86_400;

function trimDecimals(value: number, maximumFractionDigits = 2): string {
  return value.toFixed(maximumFractionDigits).replace(/\.?0+$/, "");
}

function byteUnitIndex(bytes: number): number {
  return Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), BYTE_UNITS.length - 1);
}

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes === 0) return "0 B";

  const unitIndex = byteUnitIndex(bytes);
  return `${trimDecimals(bytes / 1024 ** unitIndex)} ${BYTE_UNITS[unitIndex]}`;
}

export function formatBytePair(used: number, total: number): string {
  if (!Number.isFinite(used) || !Number.isFinite(total) || used < 0 || total < 0) return "—";
  if (used > 0 && total > 0 && byteUnitIndex(used) === byteUnitIndex(total)) {
    const unitIndex = byteUnitIndex(total);
    return `${trimDecimals(used / 1024 ** unitIndex)} / ${trimDecimals(total / 1024 ** unitIndex)} ${BYTE_UNITS[unitIndex]}`;
  }
  return `${formatBytes(used)} / ${formatBytes(total)}`;
}

export function formatRate(bytesPerSecond: number): string {
  const bytes = formatBytes(bytesPerSecond);
  return bytes === "—" ? bytes : `${bytes}/s`;
}

export function formatCpu(value: number): string {
  if (!Number.isFinite(value)) return "—";
  const clamped = Math.min(100, Math.max(0, value));
  return `${clamped < 10 ? clamped.toFixed(1) : trimDecimals(clamped, 1)}%`;
}

export function formatPercent(value: number): string {
  if (!Number.isFinite(value)) return "—";
  return `${trimDecimals(Math.min(100, Math.max(0, value)), 1)}%`;
}

export function formatLoad(value: number): string {
  return Number.isFinite(value) && value >= 0 ? value.toFixed(2) : "—";
}

export function formatUptime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "—";
  const whole = Math.floor(seconds);
  const units = [
    [86_400, "天"],
    [3_600, "小时"],
    [60, "分钟"],
    [1, "秒"],
  ] as const;
  let remaining = whole;
  const parts: string[] = [];

  for (const [size, label] of units) {
    const amount = Math.floor(remaining / size);
    if (amount > 0 || (size === 1 && parts.length === 0)) {
      parts.push(`${amount} ${label}`);
      remaining %= size;
    }
    if (parts.length === 2) break;
  }
  return parts.join(" ");
}

export function formatExpiry(expiresAt: number | null, now: number): string {
  if (expiresAt === null) return "∞";
  if (expiresAt >= now) return `${Math.ceil((expiresAt - now) / DAY_SECONDS)} 天后到期`;
  return `已过期 ${Math.ceil((now - expiresAt) / DAY_SECONDS)} 天`;
}

export function clampProgress(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(100, Math.max(0, value));
}

export function ratioPercent(used: number, total: number): number {
  if (!Number.isFinite(used) || !Number.isFinite(total) || total <= 0) return 0;
  return clampProgress((used / total) * 100);
}

export function safeAdd(left: number, right: number): number | null {
  const result = left + right;
  return Number.isSafeInteger(left) && Number.isSafeInteger(right) && Number.isSafeInteger(result) ? result : null;
}
