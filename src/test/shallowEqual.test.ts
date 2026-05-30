import { describe, expect, it } from "vitest";
import { shallowEqual } from "../lib/shallowEqual";

describe("shallowEqual", () => {
  it("compares primitives with Object.is", () => {
    expect(shallowEqual(1, 1)).toBe(true);
    expect(shallowEqual(1, 2)).toBe(false);
    expect(shallowEqual(NaN, NaN)).toBe(true);
  });

  it("compares object keys shallowly", () => {
    const a = { x: 1, y: "two" };
    const b = { x: 1, y: "two" };
    const c = { x: 1, y: "other" };
    expect(shallowEqual(a, b)).toBe(true);
    expect(shallowEqual(a, c)).toBe(false);
  });

  it("compares arrays by element reference", () => {
    const item = { id: "a" };
    expect(shallowEqual([item], [item])).toBe(true);
    expect(shallowEqual([item], [{ id: "a" }])).toBe(false);
  });
});
