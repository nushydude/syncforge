import { useEffect, useState } from "react";
import {
  formatExactTimestamp,
  formatRelativeTime,
} from "../../lib/relativeTime";

interface LastSyncedTextProps {
  timestamp: number | null | undefined;
}

export function LastSyncedText({ timestamp }: LastSyncedTextProps) {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (timestamp == null) {
      return;
    }
    const interval = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(interval);
  }, [timestamp]);

  if (timestamp == null) {
    return <>Never synced</>;
  }

  return (
    <time
      dateTime={new Date(timestamp).toISOString()}
      title={formatExactTimestamp(timestamp)}
    >
      {formatRelativeTime(timestamp, now)}
    </time>
  );
}
