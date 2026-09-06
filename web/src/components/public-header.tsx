import { LogIn, Monitor, Moon, Sun } from "lucide-react";
import type { MouseEvent } from "react";
import type { ThemePreference } from "../api/public";
import { navigate } from "../router";
import { useTheme } from "../theme/theme";
import { Button } from "../ui/primitives";

const nextTheme: Record<ThemePreference, ThemePreference> = {
  light: "dark",
  dark: "system",
  system: "light",
};

const themeLabel: Record<ThemePreference, string> = {
  light: "浅色",
  dark: "深色",
  system: "跟随系统",
};

export function PublicHeader({ siteName, serverTheme = "system" }: { siteName: string; serverTheme?: ThemePreference }) {
  const theme = useTheme(serverTheme);
  const ThemeIcon = theme.selected === "light" ? Sun : theme.selected === "dark" ? Moon : Monitor;

  const handleLogin = (event: MouseEvent<HTMLAnchorElement>) => {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    navigate("/login");
  };

  return (
    <header className="public-header">
      <div className="public-header-inner">
        <div className="site-name" title={siteName}>{siteName}</div>
        <nav className="header-actions" aria-label="页面操作">
          <a className="login-link" href="/login" onClick={handleLogin}>
            <LogIn size={21} strokeWidth={2} aria-hidden="true" />
            <span>登录</span>
          </a>
          <Button
            className="icon-button"
            type="button"
            aria-label={`当前主题：${themeLabel[theme.selected]}；切换到${themeLabel[nextTheme[theme.selected]]}`}
            title={`主题：${themeLabel[theme.selected]}`}
            onClick={() => theme.setTheme(nextTheme[theme.selected])}
            icon={<ThemeIcon size={21} strokeWidth={2} aria-hidden="true" />}
          />
        </nav>
      </div>
    </header>
  );
}
