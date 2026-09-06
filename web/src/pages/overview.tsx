import type { PublicSnapshotState } from "../stores/public-snapshot";
import { retryPublicSnapshot } from "../stores/public-snapshot";
import { NodeCard } from "../components/node-card";
import { SummaryCards } from "../components/summary-card";
import { Button, Card } from "../ui/primitives";

export function OverviewPage({ state }: { state: PublicSnapshotState }) {
  if (!state.snapshot) {
    return (
      <main className="public-container overview-main">
        <SummarySkeleton />
        {state.error ? (
          <div className="initial-error" role="alert">
            <p>{state.error}</p>
            <Button type="button" onClick={retryPublicSnapshot}>重试</Button>
          </div>
        ) : (
          <NodeSkeletons />
        )}
      </main>
    );
  }

  const { snapshot } = state;
  return (
    <main className="public-container overview-main">
      {state.stale && <div className="snapshot-notice" role="status">数据更新暂时中断，正在显示最近状态</div>}
      <SummaryCards summary={snapshot.summary} />
      {snapshot.nodes.length > 0 ? (
        <section className="node-grid" aria-label="节点状态">
          {snapshot.nodes.map((node) => <NodeCard key={node.id} node={node} generatedAt={snapshot.generated_at} />)}
        </section>
      ) : (
        <div className="empty-nodes">暂无节点</div>
      )}
    </main>
  );
}

function SummarySkeleton() {
  return (
    <section className="summary-grid" aria-label="正在加载全站摘要">
      {Array.from({ length: 4 }, (_, index) => (
        <Card className="summary-card skeleton-card" key={index}>
          <div className="skeleton-line skeleton-label" />
          <div className="skeleton-line skeleton-value" />
          <div className="skeleton-line skeleton-caption" />
        </Card>
      ))}
    </section>
  );
}

function NodeSkeletons() {
  return (
    <section className="node-grid" aria-label="正在加载节点">
      {Array.from({ length: 8 }, (_, index) => (
        <Card className="node-card skeleton-card" key={index}>
          <div className="skeleton-line skeleton-node-title" />
          <div className="skeleton-line skeleton-node-subtitle" />
          <div className="skeleton-metric-grid">
            {Array.from({ length: 4 }, (_, metric) => <div className="skeleton-metric" key={metric} />)}
          </div>
          <div className="skeleton-footer" />
        </Card>
      ))}
    </section>
  );
}
