import { useEffect, useState } from "react";
import type { ThemePreference } from "../api/public";

export const THEME_STORAGE_KEY = "monitor-theme";

export function parseTheme(value: string | null): ThemePreference | null {
  return value === "light" || value === "dark" || value === "system" ? value : null;
}

export function resolveTheme(
  stored: ThemePreference | null,
  serverDefault: ThemePreference,
  systemDark: boolean,
): "light" | "dark" {
  const selected = stored ?? serverDefault;
  if (selected === "system") return systemDark ? "dark" : "light";
  return selected;
}

function readStoredTheme(): ThemePreference | null {
  try {
    return parseTheme(window.localStorage.getItem(THEME_STORAGE_KEY));
  } catch {
    return null;
  }
}

function applyTheme(theme: "light" | "dark"): void {
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
}

export function useTheme(serverDefault: ThemePreference = "system") {
  const [stored, setStored] = useState<ThemePreference | null>(readStoredTheme);
  const [systemDark, setSystemDark] = useState(() => window.matchMedia("(prefers-color-scheme: dark)").matches);
  const selected = stored ?? serverDefault;
  const effective = resolveTheme(stored, serverDefault, systemDark);

  useEffect(() => {
    applyTheme(effective);
  }, [effective]);

  useEffect(() => {
    if (selected !== "system") return;
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const update = (event: MediaQueryListEvent) => setSystemDark(event.matches);
    setSystemDark(media.matches);
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, [selected]);

  const setTheme = (preference: ThemePreference) => {
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, preference);
    } catch {
      // The in-memory choice still works when storage is unavailable.
    }
    setStored(preference);
  };

  return { selected, effective, setTheme };
}
