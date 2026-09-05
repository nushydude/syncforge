export function formatRelativeTime(
  timestamp: number,
  now = Date.now(),
): string {
  const elapsedMs = Math.max(0, now - timestamp);
  const minuteMs = 60_000;
  const hourMs = 60 * minuteMs;
  const dayMs = 24 * hourMs;
  const monthMs = 30 * dayMs;

  if (elapsedMs < minuteMs) {
    return "Just now";
  }
  if (elapsedMs < hourMs) {
    const minutes = Math.floor(elapsedMs / minuteMs);
    return `${minutes} minute${minutes === 1 ? "" : "s"} ago`;
  }
  if (elapsedMs < dayMs) {
    const hours = Math.floor(elapsedMs / hourMs);
    return `${hours} hour${hours === 1 ? "" : "s"} ago`;
  }
  if (elapsedMs < 2 * dayMs) {
    return "Yesterday";
  }
  if (elapsedMs < monthMs) {
    const days = Math.floor(elapsedMs / dayMs);
    return `${days} days ago`;
  }

  return new Date(timestamp).toLocaleDateString(undefined, {
    day: "numeric",
    month: "short",
    ...(elapsedMs >= 365 * dayMs ? { year: "numeric" } : {}),
  });
}

export function formatExactTimestamp(timestamp: number): string {
  return new Date(timestamp).toLocaleString();
}
