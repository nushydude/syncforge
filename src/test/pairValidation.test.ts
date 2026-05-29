import { describe, expect, it } from "vitest";
import { validatePairForm } from "../lib/pairValidation";

describe("validatePairForm", () => {
  it("requires name and distinct existing paths", () => {
    const errors = validatePairForm(
      { name: "", leftPath: "C:\\a", rightPath: "D:\\b" },
      { leftExists: true, rightExists: true, pathsEqual: false },
    );
    expect(errors).toContain("Name is required");
  });

  it("reports missing folders", () => {
    const errors = validatePairForm(
      { name: "Pair", leftPath: "C:\\a", rightPath: "D:\\b" },
      { leftExists: false, rightExists: false, pathsEqual: false },
    );
    expect(errors).toContain("Left folder does not exist");
    expect(errors).toContain("Right folder does not exist");
  });

  it("rejects identical paths", () => {
    const errors = validatePairForm(
      { name: "Pair", leftPath: "C:\\same", rightPath: "C:\\same" },
      { leftExists: true, rightExists: true, pathsEqual: true },
    );
    expect(errors).toContain("Left and right folders must be different");
  });
});
