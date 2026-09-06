import { FormEvent, useEffect, useState } from "react";
import {
  AdminApiError,
  AdminSettings,
  buildSettingsPatch,
  getSettings,
  updateSettings,
} from "../api/admin";
import { handleAdminError } from "../stores/auth";
import { Button } from "../ui/primitives";

export function AdminThemePage() {
  const [settings, setSettings] = useState<AdminSettings | null>(null);
  const [theme, setTheme] = useState<AdminSettings["theme_default"]>("system");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    void getSettings()
      .then((value) => {
        setSettings(value);
        setTheme(value.theme_default);
      })
      .catch((caught) => setError(handleAdminError(caught)))
      .finally(() => setLoading(false));
  }, [attempt]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!settings || busy) return;
    const patch = buildSettingsPatch(settings, { ...settings, theme_default: theme });
    if (Object.keys(patch).length === 0) return;
    setBusy(true);
    setError(null);
    try {
      const next = await updateSettings(patch);
      setSettings(next);
      setTheme(next.theme_default);
    } catch (caught) {
      setError(caught instanceof AdminApiError && caught.status === 400 ? "无法保存默认主题" : handleAdminError(caught));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="admin-settings-page">
      <div className="admin-toolbar"><h1>主题</h1></div>
      <p className="admin-muted">设置新访客看到的默认主题；管理员个人选择不会被修改。</p>
      {loading ? <div className="settings-skeleton" /> : settings ? (
        <form className="settings-form theme-form" onSubmit={submit}>
          <fieldset className="theme-options">
            <legend>默认主题</legend>
            {(["light", "dark", "system"] as const).map((value) => (
              <label className="theme-option" key={value}>
                <input type="radio" name="theme_default" value={value} checked={theme === value} onChange={() => setTheme(value)} />
                <span>{value === "light" ? "浅色" : value === "dark" ? "深色" : "跟随系统"}</span>
              </label>
            ))}
          </fieldset>
          {error && <div className="admin-error" role="alert">{error}</div>}
          <div className="dialog-actions"><Button type="submit" disabled={busy}>{busy ? "保存中…" : "保存"}</Button></div>
        </form>
      ) : <div className="admin-error" role="alert"><span>{error ?? "暂时无法加载设置"}</span><button type="button" className="text-link" onClick={() => { setLoading(true); setAttempt((value) => value + 1); }}>重试</button></div>}
    </section>
  );
}
