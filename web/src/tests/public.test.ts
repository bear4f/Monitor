import { describe, expect, it } from "vitest";
import { parsePublicSnapshot } from "../api/public";
import { mockPublicSnapshot } from "../mock/public-snapshot";

describe("public snapshot parser", () => {
  it("accepts the complete frozen snapshot shape", () => {
    expect(parsePublicSnapshot(mockPublicSnapshot)).toEqual(mockPublicSnapshot);
  });

  it.each([
    [{ ...mockPublicSnapshot, summary: null }, "summary"],
    [{ ...mockPublicSnapshot, nodes: {} }, "nodes"],
    [{ ...mockPublicSnapshot, summary: { ...mockPublicSnapshot.summary, network_rate_history: { timestamps: [], rx: [1], tx: [] } } }, "history"],
    [{ ...mockPublicSnapshot, nodes: [{ ...mockPublicSnapshot.nodes[0], metrics: { ...mockPublicSnapshot.nodes[0].metrics!, current_rx_rate: -1 } }] }, "metrics"],
    [{ ...mockPublicSnapshot, nodes: [{ ...mockPublicSnapshot.nodes[0], billing: { price_micros: 1, currency: null, renewal_cycle: null, expires_at: null } }] }, "billing"],
  ])("rejects malformed %s responses", (value, _label) => {
    expect(() => parsePublicSnapshot(value)).toThrow();
  });
});
