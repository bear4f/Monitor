export function sparklinePoints(values: readonly number[], width: number, height: number, padding = 2): string {
  const finiteValues = values.filter(Number.isFinite);
  if (finiteValues.length === 0 || width <= padding * 2 || height <= padding * 2) return "";

  const minimum = Math.min(...finiteValues);
  const maximum = Math.max(...finiteValues);
  const range = maximum - minimum;
  const horizontalStep = finiteValues.length === 1 ? 0 : (width - padding * 2) / (finiteValues.length - 1);

  return finiteValues
    .map((value, index) => {
      const x = finiteValues.length === 1 ? width / 2 : padding + horizontalStep * index;
      const y = range === 0 ? height / 2 : padding + ((maximum - value) / range) * (height - padding * 2);
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(" ");
}
