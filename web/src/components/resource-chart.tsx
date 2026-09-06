import { useEffect, useRef } from "react";
import uPlot from "uplot";
import type { HistoryRange } from "../api/history";
import {
  formatAxisTime,
  formatTooltipTime,
  niceByteRateScale,
  timeAxisSplits,
  type ResourceChartKind,
} from "../lib/history";
import { formatAxisBytes, formatBytes, formatCpu, formatRate } from "../lib/format";
import { Button } from "../ui/primitives";

interface ResourceChartProps {
  kind: ResourceChartKind;
  title: string;
  nodeName: string;
  range: HistoryRange;
  data: uPlot.AlignedData | null;
  total?: number;
  loading: boolean;
  error: string | null;
  onRetry: () => void;
}

export function ResourceChart(props: ResourceChartProps) {
  return (
    <section className="resource-chart-section" aria-labelledby={`${props.kind}-chart-title`}>
      <h2 id={`${props.kind}-chart-title`}>{props.title}</h2>
      {props.data ? (
        <div className="resource-plot-wrap">
          <UPlotChart {...props} data={props.data} />
          {props.loading && <div className="chart-refreshing" role="status">正在更新</div>}
          {props.error && <ChartError message={props.error} onRetry={props.onRetry} compact />}
        </div>
      ) : props.error ? (
        <div className="resource-chart-placeholder">
          <ChartError message={props.error} onRetry={props.onRetry} />
        </div>
      ) : (
        <div className="resource-chart-placeholder chart-skeleton" aria-label={`正在加载${props.title}历史图`} />
      )}
    </section>
  );
}

function ChartError({ message, onRetry, compact = false }: { message: string; onRetry: () => void; compact?: boolean }) {
  return (
    <div className={compact ? "chart-inline-error" : "chart-error"} role="alert">
      <span>{message}</span>
      <Button type="button" onClick={onRetry}>重试</Button>
    </div>
  );
}

function UPlotChart({ kind, nodeName, range, data, total }: ResourceChartProps & { data: uPlot.AlignedData }) {
  const target = useRef<HTMLDivElement>(null);
  const plot = useRef<uPlot | null>(null);
  const latestData = useRef(data);
  latestData.current = data;

  useEffect(() => {
    const element = target.current;
    if (!element) return;
    let tooltip: HTMLDivElement | null = null;

    const build = () => {
      plot.current?.destroy();
      const width = Math.max(1, element.clientWidth);
      const height = Math.max(1, element.clientHeight);
      const options = chartOptions(kind, range, total, element, (current) => {
        tooltip = current;
      });
      plot.current = new uPlot({ ...options, width, height }, latestData.current, element);
    };

    build();
    const resize = new ResizeObserver(() => {
      if (!plot.current) return;
      const width = Math.max(1, element.clientWidth);
      const height = Math.max(1, element.clientHeight);
      if (plot.current.width !== width || plot.current.height !== height) plot.current.setSize({ width, height });
    });
    resize.observe(element);

    const theme = new MutationObserver(() => build());
    theme.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });

    return () => {
      resize.disconnect();
      theme.disconnect();
      tooltip?.remove();
      plot.current?.destroy();
      plot.current = null;
    };
  }, [kind, range, total]);

  useEffect(() => {
    plot.current?.setData(data);
  }, [data]);

  return <div className="resource-plot" ref={target} role="img" aria-label={`${nodeName} ${chartName(kind)}历史图`} />;
}

function chartOptions(
  kind: ResourceChartKind,
  range: HistoryRange,
  total: number | undefined,
  element: HTMLElement,
  setTooltip: (tooltip: HTMLDivElement) => void,
): Omit<uPlot.Options, "width" | "height"> {
  const styles = getComputedStyle(document.documentElement);
  const line = styles.getPropertyValue("--chart-line").trim();
  const secondary = styles.getPropertyValue("--chart-line-secondary").trim();
  const fill = styles.getPropertyValue("--chart-fill").trim();
  const grid = styles.getPropertyValue("--chart-grid").trim();
  const muted = styles.getPropertyValue("--muted-foreground").trim();
  const card = styles.getPropertyValue("--chart-tooltip").trim();
  const mobile = element.clientWidth < 768;
  const yMaximum = Math.max(1, total ?? 1);
  const series: uPlot.Series[] = [{ label: "时间" }];

  if (kind === "network") {
    series.push(
      lineSeries("上传", line),
      { ...lineSeries("下载", secondary), dash: [8, 5], width: 1.2 },
    );
  } else {
    series.push({ ...lineSeries(chartName(kind), line), fill });
  }

  const rangeForScale: uPlot.Scale.Range = kind === "memory" || kind === "disk"
    ? [0, yMaximum]
    : kind === "cpu"
      ? (_self, _minimum, maximum) => [0, nicePercentMaximum(maximum)]
      : (_self, _minimum, maximum) => [0, niceByteRateScale(maximum).maximum];

  let tooltip: HTMLDivElement | null = null;
  return {
    ms: 1e-3,
    padding: [4, mobile ? 10 : 58, 0, mobile ? 4 : 16],
    legend: { show: false },
    cursor: {
      x: true,
      y: false,
      drag: { x: false, y: false, setScale: false },
      points: { show: true, size: 8, width: 2, fill: card },
    },
    scales: { x: { time: true }, y: { auto: true, range: rangeForScale } },
    series,
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
        size: mobile ? 60 : 88,
        gap: mobile ? 6 : 10,
        stroke: muted,
        font: `${mobile ? 14 : 15}px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif`,
        splits: (_self, _axis, _minimum, maximum) => [0, 0.25, 0.5, 0.75, 1].map((part) => maximum * part),
        values: (_self, splits) => splits.map((value) => formatAxisValue(kind, value)),
        grid: { stroke: grid, width: 1, dash: [4, 4] },
        ticks: { show: false },
        border: { show: false },
      },
    ],
    hooks: {
      ready: [(self) => {
        tooltip = document.createElement("div");
        tooltip.className = "chart-tooltip";
        tooltip.setAttribute("aria-hidden", "true");
        tooltip.hidden = true;
        self.root.append(tooltip);
        setTooltip(tooltip);
      }],
      setCursor: [(self) => updateTooltip(self, kind, tooltip)],
    },
  };
}

function lineSeries(label: string, stroke: string): uPlot.Series {
  return {
    label,
    stroke,
    width: 1.5,
    spanGaps: false,
    points: { show: false },
    value: (_self, value) => value == null ? "—" : String(value),
  };
}

function updateTooltip(plot: uPlot, kind: ResourceChartKind, tooltip: HTMLDivElement | null): void {
  const index = plot.cursor.idx;
  if (!tooltip || index === null || index === undefined || plot.cursor.left === undefined || plot.cursor.left < 0) {
    if (tooltip) tooltip.hidden = true;
    return;
  }

  const timestamp = Number(plot.data[0][index]);
  const rows = kind === "network"
    ? [["上传", plot.data[1][index]], ["下载", plot.data[2][index]]] as const
    : [[chartName(kind), plot.data[1][index]]] as const;
  const time = document.createElement("div");
  time.className = "chart-tooltip-time";
  time.textContent = formatTooltipTime(timestamp);
  const elements: HTMLElement[] = [time];
  for (const [label, value] of rows) {
    const row = document.createElement("div");
    row.className = "chart-tooltip-row";
    const name = document.createElement("span");
    name.textContent = label;
    const formatted = document.createElement("strong");
    formatted.textContent = value == null ? "—" : formatTooltipValue(kind, Number(value));
    row.append(name, formatted);
    elements.push(row);
  }
  tooltip.replaceChildren(...elements);
  tooltip.hidden = false;

  const plotLeft = plot.bbox.left / uPlot.pxRatio;
  const plotTop = plot.bbox.top / uPlot.pxRatio;
  const desiredLeft = plotLeft + plot.cursor.left + 14;
  const maximumLeft = Math.max(4, plot.width - tooltip.offsetWidth - 4);
  tooltip.style.left = `${Math.min(desiredLeft, maximumLeft)}px`;
  tooltip.style.top = `${plotTop + 10}px`;
}

function nicePercentMaximum(maximum: number): number {
  const target = Math.min(100, Math.max(4, maximum)) / 4;
  const magnitude = 10 ** Math.floor(Math.log10(target));
  const step = [1, 1.5, 2, 2.5, 4, 5, 7.5, 10]
    .map((value) => value * magnitude)
    .find((value) => value >= target) ?? target;
  return Math.min(100, step * 4);
}

function formatAxisValue(kind: ResourceChartKind, value: number): string {
  if (kind === "cpu") return `${Number(value.toFixed(1))}%`;
  return kind === "network" ? formatRate(value) : formatAxisBytes(value);
}

function formatTooltipValue(kind: ResourceChartKind, value: number): string {
  if (kind === "cpu") return formatCpu(value);
  return kind === "network" ? formatRate(value) : formatBytes(value);
}

function chartName(kind: ResourceChartKind): string {
  return { cpu: "CPU", memory: "内存", network: "网络", disk: "硬盘" }[kind];
}
