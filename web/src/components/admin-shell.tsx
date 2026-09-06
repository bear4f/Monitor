import { CircleHelp, LogOut, Monitor, Moon, Settings, Sun, Tag, Users } from "lucide-react";
import { useState } from "react";
import type { ReactNode } from "react";
import { logout } from "../api/admin";
import { navigate } from "../router";
import { useTheme } from "../theme/theme";
import { setUnauthenticated } from "../stores/auth";
import "../admin.css";

export function AdminShell({ children }: { children: ReactNode }) {
  const theme = useTheme("system");
  const [logoutError, setLogoutError] = useState<string | null>(null);
  const signOut = async () => { setLogoutError(null); try { await logout(); setUnauthenticated(); navigate("/"); } catch { setLogoutError("退出失败，请重试"); } };
  const ThemeIcon = theme.selected === "light" ? Sun : theme.selected === "dark" ? Moon : Monitor;
  return <div className="admin-shell"><header className="admin-header"><div className="admin-header-inner"><a href="/" onClick={(e) => { if (e.button === 0 && !e.metaKey && !e.ctrlKey) { e.preventDefault(); navigate("/"); } }} className="admin-brand"><strong>Monitor</strong><span>后台</span></a><div className="admin-header-actions"><button className="admin-header-button" type="button" aria-label="切换主题" onClick={() => theme.setTheme(theme.selected === "light" ? "dark" : theme.selected === "dark" ? "system" : "light")}><ThemeIcon size={18} aria-hidden="true" />主题</button><button className="admin-header-button" type="button" onClick={signOut}><LogOut size={18} aria-hidden="true" />退出</button></div></div></header><div className="admin-layout"><aside className="admin-sidebar" aria-label="后台导航"><NavItem icon={<Users />} label="节点" active onClick={() => navigate("/admin/nodes")} /><NavItem icon={<CircleHelp />} label="延迟监控" disabled /><NavItem icon={<Tag />} label="主题" disabled /><NavItem icon={<Settings />} label="设置" disabled /></aside><main className="admin-content">{children}{logoutError && <div className="admin-inline-error" role="alert">{logoutError}</div>}</main></div></div>;
}
function NavItem({ icon, label, active, disabled, onClick }: { icon: ReactNode; label: string; active?: boolean; disabled?: boolean; onClick?: () => void }) { return <button type="button" className={`admin-nav-item${active ? " active" : ""}`} disabled={disabled} aria-disabled={disabled || undefined} onClick={onClick}>{icon}<span>{label}</span></button>; }
