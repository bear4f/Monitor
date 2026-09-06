import { lazy, Suspense, useEffect } from "react";
import { usePublicSnapshot } from "./stores/public-snapshot";
import { PublicHeader } from "./components/public-header";
import { OverviewPage } from "./pages/overview";
import { navigate, useRoute } from "./router";
import { useAdminSession } from "./stores/auth";

const NodeDetailPage = lazy(() => import("./pages/node-detail").then((module) => ({ default: module.NodeDetailPage })));
const LoginPage = lazy(() => import("./pages/login").then((module) => ({ default: module.LoginPage })));
const AdminNodesPage = lazy(() => import("./pages/admin-nodes").then((module) => ({ default: module.AdminNodesPage })));
const AdminShell = lazy(() => import("./components/admin-shell").then((module) => ({ default: module.AdminShell })));

export function App() {
  const route = useRoute();
  const snapshotState = usePublicSnapshot();
  const auth = useAdminSession();

  if (route.page === "login") return <Suspense fallback={<DetailLoading />}><LoginPage /></Suspense>;
  if (route.page === "admin-nodes" || route.page === "admin-disabled") return <AdminGate auth={auth} page={route.page} />;

  return (
    <div className="app-shell">
      <PublicHeader siteName={snapshotState.snapshot?.site.name ?? "Monitor"} serverTheme={snapshotState.snapshot?.site.theme_default} />
      {route.page === "overview" ? (
        <OverviewPage state={snapshotState} />
      ) : route.page === "node" ? (
        <Suspense fallback={<DetailLoading />}>
          <NodeDetailPage nodeId={route.nodeId} snapshotState={snapshotState} />
        </Suspense>
      ) : (
        <main className="public-container not-found" aria-labelledby="not-found-title">
          <h1 id="not-found-title">页面不存在</h1>
          <button className="text-link" type="button" onClick={() => navigate("/")}>返回首页</button>
        </main>
      )}
    </div>
  );
}

function AdminGate({ auth, page }: { auth: ReturnType<typeof useAdminSession>; page: "admin-nodes" | "admin-disabled" }) {
  useEffect(() => { if (auth === "unauthenticated") navigate("/login"); }, [auth]);
  if (auth === "unknown" || auth === "unauthenticated") return <DetailLoading />;
  return <Suspense fallback={<DetailLoading />}><AdminShell>{page === "admin-nodes" ? <AdminNodesPage /> : <main className="admin-disabled-page">此页面将在下一阶段实现</main>}</AdminShell></Suspense>;
}

function DetailLoading() {
  return (
    <main className="public-container detail-loading" aria-label="正在加载节点详情">
      <div className="skeleton-line detail-loading-title" />
      <div className="detail-loading-facts">
        {Array.from({ length: 6 }, (_, index) => <div className="skeleton-line detail-loading-fact" key={index} />)}
      </div>
    </main>
  );
}
