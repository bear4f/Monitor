import { describe, expect, it } from "vitest";
import { mockPublicSnapshot } from "../mock/public-snapshot";
import { mockResourceHistory } from "../mock/resource-history";

describe("development snapshot fixture", () => {
  it("matches the public snapshot shape and covers overview states", () => {
    expect(mockPublicSnapshot.nodes).toHaveLength(8);
    expect(mockPublicSnapshot.summary.total_nodes).toBe(8);
    expect(mockPublicSnapshot.summary.network_rate_history.timestamps).toHaveLength(
      mockPublicSnapshot.summary.network_rate_history.rx.length,
    );
    expect(mockPublicSnapshot.summary.network_rate_history.rx).toHaveLength(
      mockPublicSnapshot.summary.network_rate_history.tx.length,
    );
    expect(mockPublicSnapshot.nodes.some((node) => !node.online)).toBe(true);
    expect(mockPublicSnapshot.nodes.some((node) => node.traffic.limit === null)).toBe(true);
    expect(mockPublicSnapshot.nodes.some((node) => node.traffic.limit !== null)).toBe(true);
    expect(mockPublicSnapshot.nodes.some((node) => node.billing?.expires_at === null)).toBe(true);
    expect(mockPublicSnapshot.nodes.some((node) => (node.billing?.expires_at ?? Infinity) - mockPublicSnapshot.generated_at <= 3 * 86400)).toBe(true);
    expect(new Set(mockPublicSnapshot.nodes.map((node) => node.region_code)).size).toBeGreaterThan(4);
    expect(mockPublicSnapshot.nodes.every((node) => /^[0-9a-f]{32}$/.test(node.id))).toBe(true);
    expect(mockPublicSnapshot.nodes.every((node) => node.metrics === null || Number.isSafeInteger(node.metrics.memory_used))).toBe(true);
  });
});

it("builds deterministic dense resource history with real null gaps", () => {
  const history = mockResourceHistory(mockPublicSnapshot.nodes[0].id, "1h");
  const length = history.series.timestamp.length;
  expect(history.node_id).toBe(mockPublicSnapshot.nodes[0].id);
  expect(length).toBe(60);
  expect(history.step).toBe(60);
  expect(history.series.cpu).toHaveLength(length);
  expect(history.series.memory).toHaveLength(length);
  expect(history.series.disk).toHaveLength(length);
  expect(history.series.rx_rate).toHaveLength(length);
  expect(history.series.tx_rate).toHaveLength(length);
  expect(history.series.cpu.some((value) => value === null)).toBe(true);
});
