import { FormEvent, useEffect, useState } from "react";
import { login } from "../api/admin";
import { navigate } from "../router";
import { setAuthenticated, useAdminSession } from "../stores/auth";
import { Button } from "../ui/primitives";
import "../admin.css";

export function LoginPage() {
  const auth = useAdminSession();
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => { if (auth === "authenticated") navigate("/admin/nodes"); }, [auth]);
  const submit = async (event: FormEvent) => {
    event.preventDefault(); setError(null); setBusy(true);
    try { await login(password); setPassword(""); setAuthenticated(); navigate("/admin/nodes"); }
    catch (caught) { const status = (caught as { status?: number }).status; setError(status === 401 ? "密码错误" : status === 429 ? "请稍后再试" : "暂时无法登录"); }
    finally { setBusy(false); }
  };
  return <main className="login-page"><form className="login-panel" onSubmit={submit}>
    <h1>Monitor</h1><p>管理员登录</p>
    <label htmlFor="admin-password">Password</label>
    <input id="admin-password" type="password" autoComplete="current-password" value={password} onChange={(event) => setPassword(event.target.value)} required />
    {error && <div className="form-error" role="alert">{error}</div>}
    <Button type="submit" disabled={busy}>{busy ? "登录中…" : "登录"}</Button>
  </form></main>;
}
