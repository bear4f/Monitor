import { useEffect } from "react";
import { AdminShell } from "./components/admin-shell";
import { AdminNodesPage } from "./pages/admin-nodes";
import { AdminPingTargetsPage } from "./pages/admin-ping-targets";
import { navigate } from "./router";
import { retryAuth, useAdminSession } from "./stores/auth";

export function AdminEntry({ page }: { page: "admin-nodes" | "admin-ping-targets" | "admin-disabled" }) {
  const auth = useAdminSession();
  useEffect(() => { if (auth === "unauthenticated") navigate("/login"); }, [auth]);
  if (auth === "error") return <main className="public-container not-found" role="alert"><p>暂时无法验证管理员会话</p><button className="text-link" type="button" onClick={retryAuth}>重试</button></main>;
  if (auth !== "authenticated") return <main className="public-container detail-loading" aria-label="正在验证管理员会话"><div className="skeleton-line detail-loading-title" /></main>;
  return <AdminShell activePage={page === "admin-ping-targets" ? "ping-targets" : "nodes"}>{page === "admin-nodes" ? <AdminNodesPage /> : page === "admin-ping-targets" ? <AdminPingTargetsPage /> : <main className="admin-disabled-page">此页面将在下一阶段实现</main>}</AdminShell>;
}
