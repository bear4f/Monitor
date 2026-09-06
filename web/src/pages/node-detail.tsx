import "uplot/dist/uPlot.min.css";
import "./node-detail.css";

import { useState } from "react";
import type { PublicNode, RenewalCycle } from "../api/public";
import { PingChart } from "../components/ping-chart";
import { ResourceChart } from "../components/resource-chart";
import {
  chartData,
  detailLocation,
  HISTORY_RANGES,
  parseDetailTab,
  parseHistoryRange,
} from "../lib/history";
import {
  formatBytes,
  formatCalendarDate,
  formatPrice,
  formatUptime,
} from "../lib/format";
import { navigate } from "../router";
import { usePingHistory } from "../stores/ping-history";
import type { PublicSnapshotState } from "../stores/public-snapshot";
import { useResourceHistory } from "../stores/resource-history";
import { Badge } from "../ui/primitives";

const RANGE_LABELS = { "1h": "1 小时", "6h": "6 小时", "24h": "24 小时", "7d": "7 天" } as const;
const CYCLE_LABELS: Record<NonNullable<RenewalCycle>, string> = {
  monthly: "月付",
  quarterly: "季付",
  semiannual: "半年付",
  annual: "年付",
  biennial: "两年付",
  custom: "自定义",
};

export function NodeDetailPage({ nodeId, snapshotState }: { nodeId: string; snapshotState: PublicSnapshotState }) {
  const snapshot = snapshotState.snapshot;
  const node = snapshot?.nodes.find((item) => item.id === nodeId) ?? null;
  const tab = parseDetailTab(window.location.search);
  const range = parseHistoryRange(window.location.search);
  const mockMode = import.meta.env.DEV && new URLSearchParams(window.location.search).get("mock") === "1";
  const enabled = snapshot !== null && node !== null;
  const resourceHistory = useResourceHistory(nodeId, range, enabled && tab === "resources", mockMode);
  const pingHistory = usePingHistory(nodeId, range, enabled && tab === "latency", mockMode);
  const [cutPeak, setCutPeak] = useState(false);

  if (!snapshot) return <DetailSkeleton />;
  if (!node || resourceHistory.notFound || pingHistory.notFound) return <DetailNotFound />;

  const metrics = node.metrics;
  const data = resourceHistory.history;
  return (
    <main className="public-container node-detail-main">
      {snapshotState.stale && <div className="snapshot-notice" role="status">数据更新暂时中断，正在显示最近状态</div>}
      <NodeHeading node={node} />
      <NodeFacts node={node} />

      <div className="detail-controls">
        <div className="detail-tabs" role="tablist" aria-label="节点详情">
          <button
            className={`detail-tab${tab === "resources" ? " detail-tab-active" : ""}`}
            type="button"
            role="tab"
            aria-selected={tab === "resources"}
            onClick={() => navigate(detailLocation(nodeId, "resources", range, mockMode))}
          >
            资源
          </button>
          <button
            className={`detail-tab${tab === "latency" ? " detail-tab-active" : ""}`}
            type="button"
            role="tab"
            aria-selected={tab === "latency"}
            onClick={() => navigate(detailLocation(nodeId, "latency", range, mockMode))}
          >
            网络延迟
          </button>
        </div>
        <div className="detail-range-row">
          <div className="detail-ranges" aria-label="历史范围">
            {HISTORY_RANGES.map((item) => (
              <button
                className={`detail-range${range === item ? " detail-range-active" : ""}`}
                type="button"
                aria-pressed={range === item}
                key={item}
                onClick={() => navigate(detailLocation(nodeId, tab, item, mockMode))}
              >
                {RANGE_LABELS[item]}
              </button>
            ))}
          </div>
          {tab === "latency" && (
            <label className="detail-cut-peak">
              <input
                type="checkbox"
                checked={cutPeak}
                onChange={(event) => setCutPeak(event.currentTarget.checked)}
              />
              <span>削峰</span>
            </label>
          )}
        </div>
      </div>

      {tab === "resources" ? <div className="resource-charts">
        <ResourceChart
          kind="cpu"
          title="CPU"
          nodeName={node.name}
          range={range}
          data={data ? chartData(data, "cpu") : null}
          loading={resourceHistory.loading}
          error={resourceHistory.error}
          onRetry={resourceHistory.retry}
        />
        <ResourceChart
          kind="memory"
          title={`内存 · ${metrics ? formatBytes(metrics.memory_total) : "—"}`}
          nodeName={node.name}
          range={range}
          data={data ? chartData(data, "memory") : null}
          total={metrics?.memory_total}
          loading={resourceHistory.loading}
          error={resourceHistory.error}
          onRetry={resourceHistory.retry}
        />
        <ResourceChart
          kind="network"
          title="网络速率"
          nodeName={node.name}
          range={range}
          data={data ? chartData(data, "network") : null}
          loading={resourceHistory.loading}
          error={resourceHistory.error}
          onRetry={resourceHistory.retry}
        />
        <ResourceChart
          kind="disk"
          title={`硬盘 · ${metrics ? formatBytes(metrics.disk_total) : "—"}`}
          nodeName={node.name}
          range={range}
          data={data ? chartData(data, "disk") : null}
          total={metrics?.disk_total}
          loading={resourceHistory.loading}
          error={resourceHistory.error}
          onRetry={resourceHistory.retry}
        />
      </div> : (
        <div className="latency-content">
          <PingChart
            nodeName={node.name}
            range={range}
            history={pingHistory.history}
            loading={pingHistory.loading}
            error={pingHistory.error}
            cutPeak={cutPeak}
            onRetry={pingHistory.retry}
          />
        </div>
      )}
    </main>
  );
}

function NodeHeading({ node }: { node: PublicNode }) {
  return (
    <div className="detail-heading">
      <h1 title={node.name}>{node.name}</h1>
      <Badge className="region-badge">{node.region_code}</Badge>
      <Badge className={`status-badge ${node.online ? "status-online" : "status-offline"}`}>
        <span className="status-dot" aria-hidden="true" />
        {node.online && node.system ? `在线 ${formatUptime(node.system.uptime_seconds)}` : "离线"}
      </Badge>
      <Badge className="agent-badge">agent {node.system?.agent_version ?? "—"}</Badge>
    </div>
  );
}

function NodeFacts({ node }: { node: PublicNode }) {
  const system = node.system;
  const metrics = node.metrics;
  const renewal = renewalText(node);
  return (
    <dl className="detail-facts">
      <Fact label="系统" value={system ? `${system.os_name} ${system.os_version} · ${system.kernel}` : "—"} />
      <Fact label="CPU" value={system ? `${system.cpu_model} × ${system.cpu_cores}` : "—"} />
      <Fact label="内存 / 硬盘" value={metrics ? `${formatBytes(metrics.memory_total)} / ${formatBytes(metrics.disk_total)}` : "—"} />
      <Fact label="架构" value={system ? `${system.architecture} · ${system.virtualization} · ${system.process_count} 进程` : "—"} />
      <Fact label="今日流量" value={`↓ ${formatBytes(node.traffic.today_rx)} · ↑ ${formatBytes(node.traffic.today_tx)}`} />
      <Fact label="续费" value={renewal} />
    </dl>
  );
}

function Fact({ label, value }: { label: string; value: string }) {
  return (
    <div className="detail-fact">
      <dt>{label}</dt>
      <dd title={value}>{value}</dd>
    </div>
  );
}

function renewalText(node: PublicNode): string {
  const billing = node.billing;
  if (!billing) return "∞";
  const parts = [formatPrice(billing.price_micros, billing.currency)];
  if (billing.renewal_cycle) parts[0] += ` / ${CYCLE_LABELS[billing.renewal_cycle]}`;
  parts.push(billing.expires_at === null ? "∞" : `${formatCalendarDate(billing.expires_at)} 到期`);
  return parts.join(" · ");
}

function DetailSkeleton() {
  return (
    <main className="public-container node-detail-main" aria-label="正在加载节点详情">
      <div className="skeleton-line detail-heading-skeleton" />
      <div className="detail-facts">
        {Array.from({ length: 6 }, (_, index) => <div className="skeleton-line detail-fact-skeleton" key={index} />)}
      </div>
      <div className="detail-controls detail-controls-skeleton" />
      <div className="resource-chart-placeholder chart-skeleton" />
    </main>
  );
}

function DetailNotFound() {
  return (
    <main className="public-container not-found" aria-labelledby="node-not-found-title">
      <h1 id="node-not-found-title">节点不存在</h1>
      <button className="text-link" type="button" onClick={() => navigate("/")}>返回首页</button>
    </main>
  );
}
