import { useSyncExternalStore } from "react";
import { fetchPublicSnapshot, type PublicSnapshot } from "../api/public";

export interface PublicSnapshotState {
  snapshot: PublicSnapshot | null;
  stale: boolean;
  error: string | null;
}

const VISIBLE_INTERVAL_MS = 2_000;
const HIDDEN_INTERVAL_MS = 10_000;
const listeners = new Set<() => void>();

let state: PublicSnapshotState = Object.freeze({ snapshot: null, stale: false, error: null });
let timer: number | undefined;
let controller: AbortController | null = null;
let inFlight = false;
let active = false;
let refreshQueued = false;
let etag: string | null = null;

const mockMode = import.meta.env.DEV && new URLSearchParams(window.location.search).get("mock") === "1";

function emit(next: PublicSnapshotState): void {
  state = Object.freeze(next);
  for (const listener of listeners) listener();
}

function delayForVisibility(): number {
  return document.visibilityState === "hidden" ? HIDDEN_INTERVAL_MS : VISIBLE_INTERVAL_MS;
}

function clearTimer(): void {
  if (timer !== undefined) {
    window.clearTimeout(timer);
    timer = undefined;
  }
}

function scheduleNext(): void {
  clearTimer();
  if (!active || mockMode) return;
  timer = window.setTimeout(() => void refresh(), delayForVisibility());
}

async function load(): Promise<PublicSnapshot | "not-modified"> {
  if (mockMode) {
    const { mockPublicSnapshot } = await import("../mock/public-snapshot");
    return mockPublicSnapshot;
  }

  controller = new AbortController();
  const result = await fetchPublicSnapshot(etag, controller.signal);
  if (result.kind === "not-modified") return "not-modified";
  etag = result.etag;
  return result.snapshot;
}

export async function refresh(): Promise<void> {
  if (!active) return;
  if (inFlight) {
    refreshQueued = true;
    return;
  }

  inFlight = true;
  clearTimer();
  try {
    const result = await load();
    if (!active) return;
    if (result === "not-modified") {
      if (state.error || state.stale) emit({ ...state, stale: false, error: null });
    } else {
      emit({ snapshot: result, stale: false, error: null });
    }
  } catch (error) {
    if (!active || (error instanceof DOMException && error.name === "AbortError")) return;
    emit({
      snapshot: state.snapshot,
      stale: state.snapshot !== null,
      error: "暂时无法更新状态",
    });
  } finally {
    controller = null;
    inFlight = false;
    if (refreshQueued && active) {
      refreshQueued = false;
      void refresh();
    } else {
      scheduleNext();
    }
  }
}

function onVisibilityChange(): void {
  if (document.visibilityState === "visible") {
    clearTimer();
    void refresh();
  } else {
    scheduleNext();
  }
}

function start(): void {
  if (active) return;
  active = true;
  document.addEventListener("visibilitychange", onVisibilityChange);
  void refresh();
}

function stop(): void {
  active = false;
  refreshQueued = false;
  clearTimer();
  document.removeEventListener("visibilitychange", onVisibilityChange);
  controller?.abort();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (listeners.size === 1) start();
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) stop();
  };
}

function getSnapshot(): PublicSnapshotState {
  return state;
}

export function usePublicSnapshot(): PublicSnapshotState {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

export function retryPublicSnapshot(): void {
  clearTimer();
  void refresh();
}
