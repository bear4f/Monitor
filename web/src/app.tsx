import { usePublicSnapshot } from "./stores/public-snapshot";
import { PublicHeader } from "./components/public-header";
import { OverviewPage } from "./pages/overview";
import { navigate, useRoute } from "./router";

export function App() {
  const route = useRoute();
  const snapshotState = usePublicSnapshot();

  return (
    <div className="app-shell">
      <PublicHeader siteName={snapshotState.snapshot?.site.name ?? "Monitor"} serverTheme={snapshotState.snapshot?.site.theme_default} />
      {route.page === "overview" ? (
        <OverviewPage state={snapshotState} />
      ) : (
        <main className="public-container not-found" aria-labelledby="not-found-title">
          <h1 id="not-found-title">页面不存在</h1>
          <button className="text-link" type="button" onClick={() => navigate("/")}>返回首页</button>
        </main>
      )}
    </div>
  );
}
