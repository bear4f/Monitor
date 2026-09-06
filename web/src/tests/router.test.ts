import { expect, it } from "vitest";
import { matchRoute } from "../router";

it("matches the overview and strict public node detail routes", () => {
  expect(matchRoute("/")).toEqual({ page: "overview" });
  expect(matchRoute("/nodes/0123")).toEqual({ page: "not-found" });
  expect(matchRoute("/nodes/0123456789abcdef0123456789abcdef")).toEqual({
    page: "node",
    nodeId: "0123456789abcdef0123456789abcdef",
  });
  expect(matchRoute("/nodes/0123456789ABCDEF0123456789ABCDEF")).toEqual({ page: "not-found" });
  expect(matchRoute("/login")).toEqual({ page: "login" });
  expect(matchRoute("/admin/nodes")).toEqual({ page: "admin-nodes" });
  expect(matchRoute("/admin/settings")).toEqual({ page: "admin-disabled", path: "/admin/settings" });
});
