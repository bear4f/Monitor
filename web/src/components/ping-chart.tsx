import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent,
} from "react";
import uPlot from "uplot";
import type { HistoryRange } from "../api/history";
import type { PingHistory } from "../api/ping";
import { formatAxisTime, formatTooltipTime, timeAxisSplits } from "../lib/history";
import {
  clampViewport,
  fullViewport,
  latestLatency,
  niceLatencyScale,
  pingAlignedData,
  pingSeriesColor,
  pingSeriesDash,
  viewportMaximum,
  type IndexViewport,
} from "../lib/ping";
import { Button } from "../ui/primitives";

interface PingChartProps {
  nodeName: string;
  range: HistoryRange;
  history: PingHistory | null;
  loading: boolean;
  error: string | null;
  cutPeak: boolean;
  onRetry: () => void;
}

export function PingChart(props: PingChartProps) {
  if (!props.history) {
    return props.error ? (
      <div className="ping-placeholder">
        <ChartError message={props.error} onRetry={props.onRetry} />
      </div>
    ) : <div className="ping-placeholder chart-skeleton" aria-label="正在加载网络延迟历史图" />;
  }
  if (props.history.targets.length === 0) {
    return <div className="ping-empty">暂无延迟监控目标</div>;
  }
  return (
    <div className="ping-chart-block">
      <LatencyPlot {...props} history={props.history} />
      {props.loading && <div className="chart-refreshing" role="status">正在更新</div>}
      {props.error && <div className="ping-inline-error"><ChartError message={props.error} onRetry={props.onRetry} /></div>}
    </div>
  );
}

function LatencyPlot({ nodeName, range, history, cutPeak }: PingChartProps & { history: PingHistory }) {
  const target = useRef<HTMLDivElement>(null);
  const plot = useRef<uPlot | null>(null);
  const rawHistory = useRef(history);
  const displayData = useRef<uPlot.AlignedData>([[]]);
  const [dark, setDark] = useState(() => document.documentElement.dataset.theme === "dark");
  const [viewport, setViewport] = useState(() => fullViewport(history.timestamps.length));
  const targetSignature = history.targets.map((item) => `${item.id}:${item.name}:${item.ip_family}`).join("|");
  const colors = useMemo(
    () => history.targets.map((_, index) => pingSeriesColor(index, history.targets.length, dark)),
    [dark, targetSignature],
  );
  const shownData = useMemo(
    () => pingAlignedData(history, cutPeak, viewport),
    [cutPeak, history, viewport],
  );
  rawHistory.current = history;
  displayData.current = shownData;

  useEffect(() => {
    const observer = new MutationObserver(() => setDark(document.documentElement.dataset.theme === "dark"));
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    setViewport(fullViewport(history.timestamps.length));
  }, [history]);

  useEffect(() => {
    const element = target.current;
    if (!element) return;
    let tooltip: HTMLDivElement | null = null;
    const options = pingOptions(range, history, colors, element, (current) => {
      tooltip = current;
    }, rawHistory, displayData);
    plot.current = new uPlot({
      ...options,
      width: Math.max(1, element.clientWidth),
      height: Math.max(1, element.clientHeight),
    }, displayData.current, element);
    applyDataAndScales(plot.current, displayData.current, rawHistory.current, viewport);

    const resize = new ResizeObserver(() => {
      if (!plot.current) return;
      const width = Math.max(1, element.clientWidth);
      const height = Math.max(1, element.clientHeight);
      if (plot.current.width !== width || plot.current.height !== height) plot.current.setSize({ width, height });
    });
    resize.observe(element);
    return () => {
      resize.disconnect();
      tooltip?.remove();
      plot.current?.destroy();
      plot.current = null;
    };
  }, [colors, dark, range, targetSignature]);

  useEffect(() => {
    applyDataAndScales(plot.current, shownData, history, viewport);
  }, [history, shownData, viewport]);

  const previewViewport = (next: IndexViewport) => applyXScale(plot.current, history, next);
  const allNull = history.series.every((series) => series.latency.every((value) => value === null));

  return (
    <>
      <div className="ping-plot-wrap">
        <div className="ping-plot" ref={target} role="img" aria-label={`${nodeName} 网络延迟历史图`} />
        {allNull && <div className="ping-no-samples">暂无有效延迟样本</div>}
      </div>
      <PingNavigator
        length={history.timestamps.length}
        viewport={viewport}
        onPreview={previewViewport}
        onCommit={setViewport}
      />
      <PingLegend history={history} colors={colors} />
    </>
  );
}

function pingOptions(
  range: HistoryRange,
  history: PingHistory,
  colors: readonly string[],
  element: HTMLElement,
  setTooltip: (tooltip: HTMLDivElement) => void,
  rawHistory: { readonly current: PingHistory },
  displayData: { readonly current: uPlot.AlignedData },
): Omit<uPlot.Options, "width" | "height"> {
  const styles = getComputedStyle(document.documentElement);
  const grid = styles.getPropertyValue("--chart-grid").trim();
  const muted = styles.getPropertyValue("--muted-foreground").trim();
  const card = styles.getPropertyValue("--chart-tooltip").trim();
  const mobile = element.clientWidth < 768;
  let tooltip: HTMLDivElement | null = null;
  const hideTooltip = () => {
    if (tooltip) tooltip.hidden = true;
  };

  return {
    ms: 1e-3,
    padding: [4, mobile ? 10 : 68, 0, mobile ? 3 : 18],
    legend: { show: false },
    cursor: {
      x: true,
      y: false,
      drag: { x: false, y: false, setScale: false },
      points: {
        show: true,
        size: 8,
        width: 2,
        fill: card,
        stroke: (_self, seriesIndex) => colors[seriesIndex - 1] ?? muted,
      },
    },
    scales: {
      x: { time: true, auto: false },
      y: { auto: false },
    },
    series: [
      { label: "时间" },
      ...history.targets.map((target, index) => ({
        label: target.name,
        stroke: colors[index],
        width: 1.4,
        dash: [...pingSeriesDash(index)],
        spanGaps: false,
        points: { show: false },
      })),
    ],
    axes: [
      {
        stroke: muted,
        font: `${mobile ? 14 : 15}px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif`,
        size: 30,
        splits: (_self, _axis, minimum, maximum) => timeAxisSplits(minimum, maximum, range, element.clientWidth),
        values: (_self, splits) => splits.map((value) => formatAxisTime(value, range)),
        grid: { show: false },
        ticks: { show: false },
        border: { show: false },
      },
      {
        scale: "y",
        side: 3,
        size: mobile ? 68 : 96,
        gap: mobile ? 6 : 10,
        stroke: muted,
        font: `${mobile ? 14 : 15}px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif`,
        splits: (_self, _axis, _minimum, maximum) => [0, 0.25, 0.5, 0.75, 1].map((part) => maximum * part),
        values: (_self, splits) => splits.map((value) => `${Number(value.toFixed(value < 10 ? 1 : 0))}ms`),
        grid: { stroke: grid, width: 1, dash: [4, 4] },
        ticks: { show: false },
        border: { show: false },
      },
    ],
    hooks: {
      ready: [(self) => {
        tooltip = document.createElement("div");
        tooltip.className = "chart-tooltip ping-tooltip";
        tooltip.setAttribute("aria-hidden", "true");
        tooltip.hidden = true;
        self.root.append(tooltip);
        self.root.addEventListener("mouseleave", hideTooltip);
        setTooltip(tooltip);
      }],
      setCursor: [(self) => updatePingTooltip(self, tooltip, rawHistory.current, displayData.current, colors)],
      destroy: [(self) => self.root.removeEventListener("mouseleave", hideTooltip)],
    },
  };
}

function updatePingTooltip(
  plot: uPlot,
  tooltip: HTMLDivElement | null,
  raw: PingHistory,
  display: uPlot.AlignedData,
  colors: readonly string[],
): void {
  const index = plot.cursor.idx;
  if (!tooltip || index === null || index === undefined || plot.cursor.left === undefined || plot.cursor.left < 0) {
    if (tooltip) tooltip.hidden = true;
    return;
  }

  const time = document.createElement("div");
  time.className = "chart-tooltip-time";
  time.textContent = formatTooltipTime(raw.timestamps[index]);
  const elements: HTMLElement[] = [time];
  raw.targets.forEach((target, targetIndex) => {
    const value = raw.series[targetIndex].latency[index];
    const shown = display[targetIndex + 1]?.[index];
    const row = document.createElement("div");
    row.className = "ping-tooltip-row";
    const identity = document.createElement("span");
    identity.className = "ping-tooltip-identity";
    const dot = document.createElement("i");
    dot.style.backgroundColor = colors[targetIndex];
    const name = document.createElement("span");
    name.textContent = target.name;
    identity.append(dot, name);
    const result = document.createElement("span");
    result.className = "ping-tooltip-value";
    const amount = document.createElement("strong");
    amount.textContent = value === null ? "—" : `${formatLatency(value)} ms`;
    result.append(amount);
    if (value !== null && typeof shown === "number" && shown < value) {
      const marker = document.createElement("small");
      marker.textContent = "显示已削峰";
      result.append(marker);
    }
    row.append(identity, result);
    elements.push(row);
  });
  tooltip.replaceChildren(...elements);
  tooltip.hidden = false;

  const plotLeft = plot.bbox.left / uPlot.pxRatio;
  const plotTop = plot.bbox.top / uPlot.pxRatio;
  const desiredLeft = plotLeft + plot.cursor.left + 16;
  const maximumLeft = Math.max(4, plot.width - tooltip.offsetWidth - 4);
  const maximumTop = Math.max(4, plot.height - tooltip.offsetHeight - 4);
  tooltip.style.left = `${Math.min(desiredLeft, maximumLeft)}px`;
  tooltip.style.top = `${Math.min(plotTop + 10, maximumTop)}px`;
}

function applyDataAndScales(
  plot: uPlot | null,
  data: uPlot.AlignedData,
  history: PingHistory,
  viewport: IndexViewport,
): void {
  if (!plot) return;
  plot.setData(data, false);
  applyXScale(plot, history, viewport);
  const maximum = niceLatencyScale(viewportMaximum(data, viewport)).maximum;
  plot.setScale("y", { min: 0, max: maximum });
}

function applyXScale(plot: uPlot | null, history: PingHistory, viewport: IndexViewport): void {
  if (!plot) return;
  const bounds = clampViewport(viewport.start, viewport.end, history.timestamps.length);
  const minimum = history.timestamps[bounds.start] ?? history.from;
  const maximum = history.timestamps[bounds.end] ?? history.to;
  plot.setScale("x", { min: minimum, max: Math.max(minimum + history.step, maximum) });
}

function PingNavigator({
  length,
  viewport,
  onPreview,
  onCommit,
}: {
  length: number;
  viewport: IndexViewport;
  onPreview: (viewport: IndexViewport) => void;
  onCommit: (viewport: IndexViewport) => void;
}) {
  const track = useRef<HTMLDivElement>(null);
  const dragging = useRef<"start" | "end" | null>(null);
  const draftRef = useRef(viewport);
  const [draft, setDraft] = useState(viewport);
  const denominator = Math.max(1, length - 1);

  useEffect(() => {
    draftRef.current = viewport;
    setDraft(viewport);
  }, [viewport]);

  const update = (side: "start" | "end", index: number, commit: boolean) => {
    const current = draftRef.current;
    const next = side === "start"
      ? clampViewport(Math.min(index, current.end - 1), current.end, length)
      : clampViewport(current.start, Math.max(index, current.start + 1), length);
    draftRef.current = next;
    setDraft(next);
    onPreview(next);
    if (commit) onCommit(next);
  };

  const indexAt = (clientX: number) => {
    const rect = track.current?.getBoundingClientRect();
    if (!rect || rect.width <= 0) return 0;
    return Math.round(Math.min(1, Math.max(0, (clientX - rect.left) / rect.width)) * denominator);
  };

  const pointerDown = (side: "start" | "end") => (event: PointerEvent<HTMLButtonElement>) => {
    dragging.current = side;
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const pointerMove = (event: PointerEvent<HTMLButtonElement>) => {
    if (dragging.current) update(dragging.current, indexAt(event.clientX), false);
  };
  const pointerUp = (event: PointerEvent<HTMLButtonElement>) => {
    if (!dragging.current) return;
    const side = dragging.current;
    dragging.current = null;
    update(side, indexAt(event.clientX), true);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  };
  const pointerCancel = (event: PointerEvent<HTMLButtonElement>) => {
    dragging.current = null;
    draftRef.current = viewport;
    setDraft(viewport);
    onPreview(viewport);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  };
  const keyboard = (side: "start" | "end") => (event: KeyboardEvent<HTMLButtonElement>) => {
    const current = side === "start" ? draftRef.current.start : draftRef.current.end;
    const next = event.key === "ArrowLeft" ? current - 1
      : event.key === "ArrowRight" ? current + 1
        : event.key === "Home" ? 0
          : event.key === "End" ? length - 1
            : null;
    if (next === null) return;
    event.preventDefault();
    update(side, next, true);
  };

  const left = `${(draft.start / denominator) * 100}%`;
  const right = `${(draft.end / denominator) * 100}%`;
  return (
    <div className="ping-navigator" ref={track}>
      <div className="ping-navigator-selection" style={{ left, width: `calc(${right} - ${left})` }} />
      <button
        className="ping-navigator-handle"
        style={{ left }}
        type="button"
        role="slider"
        aria-label="延迟图可视范围起点"
        aria-valuemin={0}
        aria-valuemax={Math.max(0, draft.end - 1)}
        aria-valuenow={draft.start}
        onPointerDown={pointerDown("start")}
        onPointerMove={pointerMove}
        onPointerUp={pointerUp}
        onPointerCancel={pointerCancel}
        onKeyDown={keyboard("start")}
      />
      <button
        className="ping-navigator-handle"
        style={{ left: right }}
        type="button"
        role="slider"
        aria-label="延迟图可视范围终点"
        aria-valuemin={Math.min(length - 1, draft.start + 1)}
        aria-valuemax={Math.max(0, length - 1)}
        aria-valuenow={draft.end}
        onPointerDown={pointerDown("end")}
        onPointerMove={pointerMove}
        onPointerUp={pointerUp}
        onPointerCancel={pointerCancel}
        onKeyDown={keyboard("end")}
      />
    </div>
  );
}

function PingLegend({ history, colors }: { history: PingHistory; colors: readonly string[] }) {
  return (
    <ul className="ping-legend" aria-label="延迟监控线路">
      {history.targets.map((target, index) => {
        const latest = latestLatency(history.series[index].latency);
        const style = { "--ping-color": colors[index] } as CSSProperties;
        return (
          <li className="ping-legend-item" style={style} key={target.id}>
            <svg className="ping-legend-line" viewBox="0 0 20 4" aria-hidden="true">
              <line x1="0" y1="2" x2="20" y2="2" stroke="currentColor" strokeWidth="1.5" strokeDasharray={pingSeriesDash(index).join(" ")} />
            </svg>
            <span>{target.name}</span>
            <span className="ping-legend-latency">{latest === null ? "—" : `${formatLatency(latest)} ms`}</span>
          </li>
        );
      })}
    </ul>
  );
}

function ChartError({ message, onRetry }: { message: string; onRetry: () => void }) {
  return (
    <div className="chart-error" role="alert">
      <span>{message}</span>
      <Button type="button" onClick={onRetry}>重试</Button>
    </div>
  );
}

function formatLatency(value: number): string {
  return Number(value.toFixed(1)).toString();
}
