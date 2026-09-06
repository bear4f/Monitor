import { useEffect, useSyncExternalStore } from "react";
import { AdminApiError, authMe } from "../api/admin";
import { navigate } from "../router";

export type AuthStatus = "unknown" | "authenticated" | "unauthenticated" | "error";
let status: AuthStatus = "unknown";
let started = false;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((listener) => listener());

export function refreshAuth(): void {
  if (started) return;
  started = true;
  authMe().then(() => { status = "authenticated"; emit(); }).catch((error) => { status = error instanceof AdminApiError && error.status === 401 ? "unauthenticated" : "error"; emit(); });
}
export function retryAuth(): void { started = false; status = "unknown"; emit(); refreshAuth(); }
export function setUnauthenticated(): void { status = "unauthenticated"; emit(); }
export function setAuthenticated(): void { status = "authenticated"; emit(); }
export function useAdminSession() {
  const value = useSyncExternalStore((listener) => { listeners.add(listener); return () => listeners.delete(listener); }, () => status, () => "unknown");
  useEffect(refreshAuth, []);
  return value;
}
export function handleAdminError(error: unknown): string {
  if (error instanceof AdminApiError && error.status === 401) { setUnauthenticated(); navigate("/login"); return "会话已过期"; }
  if (error instanceof AdminApiError && error.status === 403) return "会话验证失败，请重新登录";
  if (error instanceof AdminApiError && error.status === 409) return "最多只能启用 6 个延迟监控目标";
  if (error instanceof AdminApiError && error.status === 429) return "登录尝试过于频繁，请稍后再试";
  return error instanceof Error ? error.message : "暂时无法完成请求";
}
