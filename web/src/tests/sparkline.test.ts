import { expect, it } from "vitest";
import { sparklinePoints } from "../lib/sparkline";

it("handles empty sparkline data", () => {
  expect(sparklinePoints([], 100, 20)).toBe("");
});

it("centers a single point", () => {
  expect(sparklinePoints([10], 100, 20)).toBe("50.00,10.00");
});

it("centers a constant sequence without division by zero", () => {
  expect(sparklinePoints([4, 4, 4], 100, 20)).toBe("2.00,10.00 50.00,10.00 98.00,10.00");
});

it("maps a varying sequence into the plot", () => {
  expect(sparklinePoints([0, 5, 10], 100, 20)).toBe("2.00,18.00 50.00,10.00 98.00,2.00");
});

it("uses a shared domain so two rate series remain comparable", () => {
  expect(sparklinePoints([0, 5], 100, 20, 2, [0, 10])).toBe("2.00,18.00 98.00,10.00");
});
