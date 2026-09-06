import { useSyncExternalStore } from "react";

export type Route =
  | { page: "overview" }
  | { page: "node"; nodeId: string }
  | { page: "not-found" };

const NAVIGATION_EVENT = "monitor:navigate";

export function matchRoute(pathname: string): Route {
  if (pathname === "/") return { page: "overview" };
  const node = /^\/nodes\/([0-9a-f]{32})$/.exec(pathname);
  return node ? { page: "node", nodeId: node[1] } : { page: "not-found" };
}

function subscribe(listener: () => void): () => void {
  window.addEventListener("popstate", listener);
  window.addEventListener(NAVIGATION_EVENT, listener);
  return () => {
    window.removeEventListener("popstate", listener);
    window.removeEventListener(NAVIGATION_EVENT, listener);
  };
}

function currentLocation(): string {
  return `${window.location.pathname}${window.location.search}`;
}

export function navigate(path: string): void {
  if (path === currentLocation()) return;
  window.history.pushState(null, "", path);
  window.dispatchEvent(new Event(NAVIGATION_EVENT));
}

export function useRoute(): Route {
  const location = useSyncExternalStore(subscribe, currentLocation, () => "/");
  return matchRoute(location.split("?", 1)[0]);
}
