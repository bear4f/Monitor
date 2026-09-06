import { lazy, Suspense } from "react";
import { PublicHeader } from "./components/public-header";
import { OverviewPage } from "./pages/overview";
import type { Route } from "./router";
import { navigate } from "./router";
import { usePublicSnapshot } from "./stores/public-snapshot";

const NodeDetailPage = lazy(() => import("./pages/node-detail").then((module) => ({ default: module.NodeDetailPage })));

export function PublicApp({ route }: { route: Route }) {
  const snapshotState = usePublicSnapshot();
  return <div className="app-shell"><PublicHeader siteName={snapshotState.snapshot?.site.name ?? "Monitor"} serverTheme={snapshotState.snapshot?.site.theme_default} />
    {route.page === "overview" ? <OverviewPage state={snapshotState} /> : route.page === "node" ? <Suspense fallback={<DetailLoading />}><NodeDetailPage nodeId={route.nodeId} snapshotState={snapshotState} /></Suspense> : <main className="public-container not-found" aria-labelledby="not-found-title"><h1 id="not-found-title">页面不存在</h1><button className="text-link" type="button" onClick={() => navigate("/")}>返回首页</button></main>}
  </div>;
}
function DetailLoading() { return <main className="public-container detail-loading" aria-label="正在加载节点详情"><div className="skeleton-line detail-loading-title" /><div className="detail-loading-facts">{Array.from({ length: 6 }, (_, index) => <div className="skeleton-line detail-loading-fact" key={index} />)}</div></main>; }
