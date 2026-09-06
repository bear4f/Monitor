import { FormEvent, useEffect, useState } from "react";
import {
  AdminApiError,
  AdminSettings,
  buildSettingsPatch,
  changePassword,
  getSettings,
  parseBoundedInteger,
  passwordFormError,
  updateSettings,
} from "../api/admin";
import { navigate } from "../router";
import { handleAdminError, setUnauthenticated } from "../stores/auth";
import { Button } from "../ui/primitives";

const fields: Array<{ key: keyof AdminSettings; label: string; min: number; max: number }> = [
  { key: "history_retention_days", label: "历史保留天数", min: 1, max: 30 },
  { key: "agent_report_interval_seconds", label: "Agent 上报间隔（秒）", min: 2, max: 60 },
  { key: "ping_interval_seconds", label: "延迟监控间隔（秒）", min: 10, max: 300 },
  { key: "offline_after_seconds", label: "离线判定时间（秒）", min: 5, max: 600 },
  { key: "default_traffic_reset_day", label: "默认流量重置日", min: 1, max: 31 },
];

export function AdminSettingsPage() {
  const [settings, setSettings] = useState<AdminSettings | null>(null);
  const [form, setForm] = useState<AdminSettings | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [passwordError, setPasswordError] = useState<string | null>(null);
  const [passwordBusy, setPasswordBusy] = useState(false);

  useEffect(() => {
    void getSettings()
      .then((value) => { setSettings(value); setForm(value); })
      .catch((caught) => setError(handleAdminError(caught)))
      .finally(() => setLoading(false));
  }, []);

  if (loading) return <section className="admin-settings-page"><div className="admin-toolbar"><h1>设置</h1></div><div className="settings-skeleton" /></section>;
  if (!form || !settings) return <section className="admin-settings-page"><div className="admin-error" role="alert">{error ?? "暂时无法加载设置"}</div></section>;

  const update = (key: keyof AdminSettings, value: string | number) => {
    setForm((value0) => value0 ? { ...value0, [key]: value } : value0);
  };
  const submitSettings = async (event: FormEvent) => {
    event.preventDefault();
    if (busy) return;
    setError(null);
    if (!form.site_name.trim() || form.site_name.trim().length > 64 || !form.site_timezone.trim() || form.site_timezone.trim().length > 64 || !isTimezone(form.site_timezone)) {
      setError(form.site_timezone.trim() ? "设置无效，请检查输入值" : "时区无效，请填写有效的 IANA 时区名称");
      return;
    }
    const patch = buildSettingsPatch(settings, form);
    if (Object.keys(patch).length === 0) return;
    setBusy(true);
    try {
      const value = await updateSettings(patch);
      setSettings(value);
      setForm(value);
    } catch (caught) {
      setError(caught instanceof AdminApiError && caught.status === 400 ? "设置无效，请检查输入值" : handleAdminError(caught));
    } finally {
      setBusy(false);
    }
  };
  const submitPassword = async (event: FormEvent) => {
    event.preventDefault();
    if (passwordBusy) return;
    const validation = passwordFormError(current, next, confirm);
    if (validation) { setPasswordError(validation); return; }
    setPasswordBusy(true);
    setPasswordError(null);
    try {
      await changePassword(current, next);
      setCurrent(""); setNext(""); setConfirm(""); setUnauthenticated(); navigate("/login");
    } catch (caught) {
      if (caught instanceof AdminApiError && caught.status === 401 && caught.code === "invalid_credentials") setPasswordError("当前密码错误");
      else setPasswordError(handleAdminError(caught));
    } finally {
      setPasswordBusy(false);
    }
  };

  return (
    <section className="admin-settings-page">
      <div className="admin-toolbar"><h1>设置</h1></div>
      {error && <div className="admin-error" role="alert">{error}</div>}
      <form className="settings-form" onSubmit={submitSettings}>
        <section className="settings-section"><h2>站点</h2><div className="settings-grid">
          <label>站点名称<input value={form.site_name} onChange={(event) => update("site_name", event.target.value)} /></label>
          <label>站点时区<input value={form.site_timezone} onChange={(event) => update("site_timezone", event.target.value)} /></label>
          <label>默认主题<select value={form.theme_default} onChange={(event) => update("theme_default", event.target.value as AdminSettings["theme_default"])}><option value="light">浅色</option><option value="dark">深色</option><option value="system">跟随系统</option></select></label>
        </div></section>
        <section className="settings-section"><h2>采集与历史</h2><div className="settings-grid">
          {fields.map(({ key, label, min, max }) => <label key={key}>{label}<input type="number" min={min} max={max} step="1" value={String(form[key])} onChange={(event) => { const parsed = parseBoundedInteger(event.target.value, min, max); update(key, parsed ?? (event.target.value as unknown as number)); }} /></label>)}
        </div><p className="admin-muted">离线判定时间必须大于 Agent 上报间隔。</p></section>
        <div className="dialog-actions"><Button type="submit" disabled={busy}>{busy ? "保存中…" : "保存设置"}</Button></div>
      </form>
      <form className="settings-form password-form" onSubmit={submitPassword}><section className="settings-section"><h2>修改密码</h2>
        <label>当前密码<input type="password" autoComplete="current-password" value={current} onChange={(event) => setCurrent(event.target.value)} /></label>
        <label>新密码<input type="password" autoComplete="new-password" value={next} onChange={(event) => setNext(event.target.value)} /></label>
        <label>确认新密码<input type="password" autoComplete="new-password" value={confirm} onChange={(event) => setConfirm(event.target.value)} /></label>
        {passwordError && <div className="admin-error" role="alert">{passwordError}</div>}
        <div className="dialog-actions"><Button type="submit" disabled={passwordBusy}>{passwordBusy ? "提交中…" : "更新密码"}</Button></div>
      </section></form>
    </section>
  );
}

function isTimezone(value: string): boolean {
  const zone = value.trim();
  if (!zone) return false;
  if (zone === "UTC") return true;
  try {
    const supported = (Intl as typeof Intl & { supportedValuesOf?: (key: string) => string[] }).supportedValuesOf;
    if (supported) return supported("timeZone").includes(zone);
    new Intl.DateTimeFormat("en", { timeZone: zone }).format();
    return true;
  } catch { return false; }
}
