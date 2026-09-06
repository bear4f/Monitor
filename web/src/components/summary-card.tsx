import { Activity, ArrowDown, ArrowDownUp, ArrowUp, Gauge, Server } from "lucide-react";
import type { PublicSummary } from "../api/public";
import { formatBytes, formatCpu, formatRate } from "../lib/format";
import { sparklinePoints } from "../lib/sparkline";
import { Card } from "../ui/primitives";

interface SummaryCardsProps {
  summary: PublicSummary;
}

export function SummaryCards({ summary }: SummaryCardsProps) {
  const offline = Math.max(0, summary.total_nodes - summary.online_nodes);
  const history = summary.network_rate_history;
  const rateMaximum = Math.max(1, ...history.rx, ...history.tx);
  const rateDomain = [0, rateMaximum] as const;
  const rxPoints = sparklinePoints(history.rx, 420, 42, 2, rateDomain);
  const txPoints = sparklinePoints(history.tx, 420, 42, 2, rateDomain);

  return (
    <section className="summary-grid" aria-label="全站摘要">
      <SummaryCard label="节点" icon={<Server />}>
        <div className="summary-primary">{summary.online_nodes} / {summary.total_nodes}</div>
        <div className="summary-caption">{offline === 0 ? "全部在线" : `${offline} 个节点离线`}</div>
      </SummaryCard>

      <SummaryCard label="最忙节点" icon={<Activity />}>
        <div className="summary-primary">{summary.busiest_node ? formatCpu(summary.busiest_node.cpu_usage) : "—"}</div>
        <div className="summary-caption summary-ellipsis" title={summary.busiest_node?.name}>{summary.busiest_node?.name ?? "暂无在线节点"}</div>
      </SummaryCard>

      <SummaryCard label="今日流量" icon={<ArrowDownUp />}>
        <div className="summary-pair summary-pair-primary">
          <SummaryDirection icon={<ArrowDown />} value={formatBytes(summary.today_rx)} />
          <SummaryDirection icon={<ArrowUp />} value={formatBytes(summary.today_tx)} />
        </div>
        <div className="summary-total-label">总流量</div>
        <div className="summary-pair summary-pair-secondary">
          <SummaryDirection icon={<ArrowDown />} value={formatBytes(summary.total_rx)} />
          <SummaryDirection icon={<ArrowUp />} value={formatBytes(summary.total_tx)} />
        </div>
      </SummaryCard>

      <SummaryCard label="实时网速" icon={<Gauge />}>
        <div className="summary-pair summary-pair-primary">
          <SummaryDirection icon={<ArrowDown />} value={formatRate(summary.current_rx_rate)} />
          <SummaryDirection icon={<ArrowUp />} value={formatRate(summary.current_tx_rate)} />
        </div>
        <svg className="network-sparkline" viewBox="0 0 420 42" preserveAspectRatio="none" role="img" aria-label="最近全站下载和上传速率">
          {rxPoints && <polyline className="sparkline-rx" points={rxPoints} />}
          {txPoints && <polyline className="sparkline-tx" points={txPoints} />}
        </svg>
      </SummaryCard>
    </section>
  );
}

function SummaryCard({ label, icon, children }: { label: string; icon: React.ReactElement; children: React.ReactNode }) {
  return (
    <Card className="summary-card">
      <div className="summary-label">
        <span className="summary-icon" aria-hidden="true">{icon}</span>
        <span>{label}</span>
      </div>
      {children}
    </Card>
  );
}

function SummaryDirection({ icon, value }: { icon: React.ReactElement; value: string }) {
  return (
    <div className="summary-direction">
      <span aria-hidden="true">{icon}</span>
      <span>{value}</span>
    </div>
  );
}
