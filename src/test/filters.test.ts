import { describe, expect, it } from "vitest";
import {
  addFilterPattern,
  filtersToText,
  parseFilterLines,
  removeFilterPattern,
} from "../lib/filters";

describe("filter editor logic", () => {
  it("parses multiline filter input", () => {
    expect(parseFilterLines("*.txt\n  *.doc \n\n")).toEqual(["*.txt", "*.doc"]);
  });

  it("adds unique patterns only", () => {
    expect(addFilterPattern(["*.txt"], "*.doc")).toEqual(["*.txt", "*.doc"]);
    expect(addFilterPattern(["*.txt"], "*.txt")).toEqual(["*.txt"]);
    expect(addFilterPattern(["*.txt"], "  ")).toEqual(["*.txt"]);
  });

  it("removes patterns by index", () => {
    expect(removeFilterPattern(["a", "b", "c"], 1)).toEqual(["a", "c"]);
    expect(removeFilterPattern(["a"], -1)).toEqual(["a"]);
    expect(removeFilterPattern(["a"], 5)).toEqual(["a"]);
  });

  it("serializes patterns for textarea display", () => {
    expect(filtersToText(["*.tmp", "Thumbs.db"])).toBe("*.tmp\nThumbs.db");
  });
});
