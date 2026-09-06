import { lazy, Suspense } from "react";
import { PublicApp } from "./public-app";
import { useRoute } from "./router";

const LoginPage = lazy(() => import("./pages/login").then((module) => ({ default: module.LoginPage })));
const AdminEntry = lazy(() => import("./admin-entry").then((module) => ({ default: module.AdminEntry })));

export function App() {
  const route = useRoute();
  if (route.page === "login") return <Suspense fallback={<DetailLoading />}><LoginPage /></Suspense>;
  if (route.page === "admin-nodes" || route.page === "admin-disabled") return <Suspense fallback={<DetailLoading />}><AdminEntry page={route.page} /></Suspense>;
  return <PublicApp route={route} />;
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
