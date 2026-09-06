import { expect, it } from "vitest";
import { parseTheme, resolveTheme } from "../theme/theme";

it("accepts only the three frozen theme choices", () => {
  expect(parseTheme("light")).toBe("light");
  expect(parseTheme("dark")).toBe("dark");
  expect(parseTheme("system")).toBe("system");
  expect(parseTheme("midnight")).toBeNull();
});

it("resolves stored choice before the server default", () => {
  expect(resolveTheme("dark", "light", false)).toBe("dark");
  expect(resolveTheme(null, "dark", false)).toBe("dark");
  expect(resolveTheme(null, "system", true)).toBe("dark");
  expect(resolveTheme("system", "dark", false)).toBe("light");
});
