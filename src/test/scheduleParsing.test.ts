import { describe, expect, it } from "vitest";
import {
  describeCronExpression,
  validateCronExpression,
} from "../lib/scheduleParsing";

describe("validateCronExpression", () => {
  it("accepts a standard daily schedule", () => {
    expect(validateCronExpression("0 9 * * *")).toEqual({ valid: true });
  });

  it("accepts step and range syntax", () => {
    expect(validateCronExpression("*/15 * * * *")).toEqual({ valid: true });
    expect(validateCronExpression("0 9-17 * * 1-5")).toEqual({ valid: true });
  });

  it("rejects empty expressions", () => {
    expect(validateCronExpression("")).toEqual({
      valid: false,
      error: "Cron expression is required",
    });
  });

  it("rejects wrong field counts", () => {
    expect(validateCronExpression("0 9 * *")).toEqual({
      valid: false,
      error:
        "Cron expression must have exactly 5 fields (minute hour day month weekday)",
    });
  });

  it("rejects out-of-range values", () => {
    expect(validateCronExpression("60 9 * * *")).toEqual({
      valid: false,
      error: "Invalid value in minute field",
    });
  });
});

describe("describeCronExpression", () => {
  it("describes common daily schedules", () => {
    expect(describeCronExpression("0 9 * * *")).toBe("Every day at 9:00 AM");
  });

  it("describes interval schedules", () => {
    expect(describeCronExpression("*/15 * * * *")).toBe("Every 15 minutes");
  });

  it("describes weekday schedules", () => {
    expect(describeCronExpression("0 0 * * 0")).toBe(
      "Every Sunday at 12:00 AM",
    );
  });

  it("returns validation errors for invalid expressions", () => {
    expect(describeCronExpression("bad cron")).toContain("exactly 5 fields");
  });
});
