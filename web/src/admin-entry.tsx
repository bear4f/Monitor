import { AdminShell } from "./components/admin-shell";
import { AdminNodesPage } from "./pages/admin-nodes";
import { AdminPingTargetsPage } from "./pages/admin-ping-targets";
import { AdminThemePage } from "./pages/admin-theme";
import { AdminSettingsPage } from "./pages/admin-settings";

// The page mounts immediately and its own first data request carries the session
// check: every /api/admin endpoint calls authenticate_session, and a 401 goes
// through handleAdminError, which marks the session unauthenticated and
// navigates to /login. Gating the mount on a separate /api/auth/me made every
// admin page pay two serial round trips for a verification the data request
// performs anyway.
export function AdminEntry({ page }: { page: "admin-nodes" | "admin-ping-targets" | "admin-theme" | "admin-settings" }) {
  const activePage = page === "admin-ping-targets" ? "ping-targets" : page === "admin-theme" ? "theme" : page === "admin-settings" ? "settings" : "nodes";
  return <AdminShell activePage={activePage}>{page === "admin-nodes" ? <AdminNodesPage /> : page === "admin-ping-targets" ? <AdminPingTargetsPage /> : page === "admin-theme" ? <AdminThemePage /> : <AdminSettingsPage />}</AdminShell>;
}
