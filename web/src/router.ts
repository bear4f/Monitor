import { useSyncExternalStore } from "react";

export type Route = { page: "overview" } | { page: "not-found" };

const NAVIGATION_EVENT = "monitor:navigate";

export function matchRoute(pathname: string): Route {
  return pathname === "/" ? { page: "overview" } : { page: "not-found" };
}

function subscribe(listener: () => void): () => void {
  window.addEventListener("popstate", listener);
  window.addEventListener(NAVIGATION_EVENT, listener);
  return () => {
    window.removeEventListener("popstate", listener);
    window.removeEventListener(NAVIGATION_EVENT, listener);
  };
}

function currentPath(): string {
  return window.location.pathname;
}

export function navigate(path: string): void {
  if (path === currentPath()) return;
  window.history.pushState(null, "", path);
  window.dispatchEvent(new Event(NAVIGATION_EVENT));
}

export function useRoute(): Route {
  const path = useSyncExternalStore(subscribe, currentPath, () => "/");
  return matchRoute(path);
}
