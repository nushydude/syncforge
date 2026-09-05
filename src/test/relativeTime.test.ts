import { describe, expect, it } from "vitest";
import { formatRelativeTime } from "../lib/relativeTime";

const now = Date.UTC(2026, 8, 5, 12, 0, 0);

describe("formatRelativeTime", () => {
  it.each([
    [0, "Just now"],
    [60_000, "1 minute ago"],
    [12 * 60_000, "12 minutes ago"],
    [4 * 60 * 60_000, "4 hours ago"],
    [24 * 60 * 60_000, "Yesterday"],
    [2 * 24 * 60 * 60_000, "2 days ago"],
  ])("formats %s ms ago as %s", (elapsedMs, expected) => {
    expect(formatRelativeTime(now - elapsedMs, now)).toBe(expected);
  });

  it("uses a calendar date for older timestamps", () => {
    const timestamp = Date.UTC(2026, 7, 1);
    const formatted = formatRelativeTime(timestamp, now);
    const expectedMonth = new Date(timestamp).toLocaleDateString(undefined, {
      month: "short",
    });

    expect(formatted).toContain(expectedMonth);
    expect(formatted).toContain("1");
  });
});
