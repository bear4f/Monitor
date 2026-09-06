import { expect, it } from "vitest";
import { matchRoute } from "../router";

it("matches only the implemented public overview route", () => {
  expect(matchRoute("/")).toEqual({ page: "overview" });
  expect(matchRoute("/nodes/0123")).toEqual({ page: "not-found" });
  expect(matchRoute("/admin/nodes")).toEqual({ page: "not-found" });
});
