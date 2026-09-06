import { ArrowDown, ArrowUp } from "lucide-react";
import type { MouseEvent } from "react";
import type { PublicNode } from "../api/public";
import {
  clampProgress,
  formatBytePair,
  formatBytes,
  formatCpu,
  formatExpiry,
  formatLoad,
  formatPercent,
  formatRate,
  formatUptime,
  ratioPercent,
  safeAdd,
} from "../lib/format";
import { navigate } from "../router";
import { Badge, Card } from "../ui/primitives";

export function NodeCard({ node, generatedAt }: { node: PublicNode; generatedAt: number }) {
  const metrics = node.metrics;
  const system = node.system;
  const memoryPercent = metrics ? ratioPercent(metrics.memory_used, metrics.memory_total) : 0;
  const diskPercent = metrics ? ratioPercent(metrics.disk_used, metrics.disk_total) : 0;
  const cycleTraffic = safeAdd(node.traffic.cycle_rx, node.traffic.cycle_tx);
  const trafficPercent = cycleTraffic !== null && node.traffic.limit !== null
    ? ratioPercent(cycleTraffic, node.traffic.limit)
    : 0;
  const systemLabel = system
    ? `${system.os_name}${system.os_version ? ` ${system.os_version}` : ""} · ${system.virtualization} · ${system.architecture}`
    : "等待首次上报";

  const mockQuery = import.meta.env.DEV && new URLSearchParams(window.location.search).get("mock") === "1" ? "?mock=1" : "";
  const href = `/nodes/${node.id}${mockQuery}`;
  const follow = (event: MouseEvent<HTMLAnchorElement>) => {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    navigate(href);
  };

  return (
    <a className="node-card-link" href={href} onClick={follow} aria-label={`查看 ${node.name} 资源详情`}>
      <Card className={`node-card${node.online ? "" : " node-card-offline"}`}>
      <div className="node-heading">
        <div className="node-identity">
          <h2 title={node.name}>{node.name}</h2>
          <Badge className="region-badge">{node.region_code}</Badge>
        </div>
        <Badge className={`status-badge ${node.online ? "status-online" : "status-offline"}`}>
          <span className="status-dot" aria-hidden="true" />
          {node.online && system ? `在线 ${formatUptime(system.uptime_seconds)}` : "离线"}
        </Badge>
      </div>

      <div className="node-subline">
        <span className="node-system" title={systemLabel}>{systemLabel}</span>
        <span className="node-expiry">{formatExpiry(node.billing?.expires_at ?? null, generatedAt)}</span>
      </div>

      <div className="metrics-grid">
        <Metric
          label={system ? `CPU ${system.cpu_cores} 核` : "CPU"}
          value={metrics ? formatCpu(metrics.cpu_usage) : "—"}
          progress={metrics ? clampProgress(metrics.cpu_usage) : null}
          detail={metrics ? `${formatLoad(metrics.load_1)} ${formatLoad(metrics.load_5)} ${formatLoad(metrics.load_15)}` : "—"}
        />
        <Metric
          label="内存"
          value={metrics ? formatPercent(memoryPercent) : "—"}
          progress={metrics ? memoryPercent : null}
          detail={metrics ? formatBytePair(metrics.memory_used, metrics.memory_total) : "—"}
        />
        <Metric
          label="硬盘"
          value={metrics ? formatPercent(diskPercent) : "—"}
          progress={metrics ? diskPercent : null}
          detail={metrics ? formatBytePair(metrics.disk_used, metrics.disk_total) : "—"}
        />
        <Metric
          label="流量"
          value={node.traffic.limit === null ? "∞" : formatPercent(trafficPercent)}
          progress={node.traffic.limit === null ? null : trafficPercent}
          detail={`${cycleTraffic === null ? "—" : formatBytes(cycleTraffic)} / ${node.traffic.limit === null ? "∞" : formatBytes(node.traffic.limit)}`}
        />
      </div>

      <div className="node-footer">
        <TrafficValue icon={<ArrowDown />} value={metrics ? formatRate(metrics.current_rx_rate) : "—"} />
        <TrafficValue icon={<ArrowUp />} value={metrics ? formatRate(metrics.current_tx_rate) : "—"} />
        <TrafficValue muted icon={<ArrowDown />} value={formatBytes(node.traffic.total_rx)} />
        <TrafficValue muted icon={<ArrowUp />} value={formatBytes(node.traffic.total_tx)} />
      </div>
      </Card>
    </a>
  );
}

function Metric({ label, value, progress, detail }: { label: string; value: string; progress: number | null; detail: string }) {
  const progressValue = progress === null ? null : clampProgress(progress);
  return (
    <div className="metric">
      <div className="metric-heading"><span>{label}</span><strong>{value}</strong></div>
      <svg
        className="progress-track"
        viewBox="0 0 100 8"
        preserveAspectRatio="none"
        role={progressValue === null ? undefined : "progressbar"}
        aria-hidden={progressValue === null ? "true" : undefined}
        aria-label={progressValue === null ? undefined : label}
        aria-valuemin={progressValue === null ? undefined : 0}
        aria-valuemax={progressValue === null ? undefined : 100}
        aria-valuenow={progressValue === null ? undefined : Math.round(progressValue)}
      >
        <rect className="progress-track-base" width="100" height="8" rx="4" />
        {progressValue !== null && <rect className="progress-track-value" width={progressValue} height="8" rx="4" />}
      </svg>
      <div className="metric-detail" title={detail}>{detail}</div>
    </div>
  );
}

function TrafficValue({ icon, value, muted = false }: { icon: React.ReactElement; value: string; muted?: boolean }) {
  return (
    <div className={`traffic-value${muted ? " traffic-muted" : ""}`}>
      <span aria-hidden="true">{icon}</span>
      <span>{value}</span>
    </div>
  );
}
