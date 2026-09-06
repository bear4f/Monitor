import { useEffect, useRef, useState } from "react";
import {
  fetchResourceHistory,
  PublicApiError,
  type HistoryRange,
  type ResourceHistory,
} from "../api/history";

interface CacheEntry {
  history: ResourceHistory;
  etag: string | null;
}

export interface ResourceHistoryState {
  history: ResourceHistory | null;
  loading: boolean;
  error: string | null;
  notFound: boolean;
  retry: () => void;
}

export function isCurrentHistoryRequest(requestGeneration: number, currentGeneration: number): boolean {
  return requestGeneration === currentGeneration;
}

export function useResourceHistory(
  nodeId: string,
  range: HistoryRange,
  enabled: boolean,
  mockMode: boolean,
): ResourceHistoryState {
  const cache = useRef(new Map<string, CacheEntry>());
  const lastNode = useRef(nodeId);
  const generation = useRef(0);
  const [retry, setRetry] = useState(0);
  const [state, setState] = useState<Omit<ResourceHistoryState, "retry">>({
    history: null,
    loading: enabled,
    error: null,
    notFound: false,
  });

  if (lastNode.current !== nodeId) {
    cache.current.clear();
    lastNode.current = nodeId;
  }

  useEffect(() => {
    const requestGeneration = ++generation.current;
    if (!enabled) {
      setState({ history: null, loading: false, error: null, notFound: false });
      return;
    }

    const key = `${nodeId}:${range}`;
    const cached = cache.current.get(key);
    const controller = new AbortController();
    setState({ history: cached?.history ?? null, loading: true, error: null, notFound: false });

    const load = async () => {
      try {
        if (import.meta.env.DEV && mockMode) {
          const { mockResourceHistory } = await import("../mock/resource-history");
          const history = mockResourceHistory(nodeId, range);
          if (isCurrentHistoryRequest(requestGeneration, generation.current)) {
            cache.current.set(key, { history, etag: null });
            setState({ history, loading: false, error: null, notFound: false });
          }
          return;
        }

        const result = await fetchResourceHistory(nodeId, range, cached?.etag ?? null, controller.signal);
        if (!isCurrentHistoryRequest(requestGeneration, generation.current)) return;
        if (result.kind === "not-modified") {
          if (!cached) throw new Error("history cache missing after 304");
          setState({ history: cached.history, loading: false, error: null, notFound: false });
          return;
        }
        cache.current.set(key, { history: result.history, etag: result.etag });
        setState({ history: result.history, loading: false, error: null, notFound: false });
      } catch (error) {
        if (controller.signal.aborted || !isCurrentHistoryRequest(requestGeneration, generation.current)) return;
        const notFound = error instanceof PublicApiError && error.status === 404;
        setState({
          history: cached?.history ?? null,
          loading: false,
          error: notFound ? null : "暂时无法加载历史数据",
          notFound,
        });
      }
    };

    void load();
    return () => controller.abort();
  }, [enabled, mockMode, nodeId, range, retry]);

  return { ...state, retry: () => setRetry((value) => value + 1) };
}
