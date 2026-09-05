import {
  formatExactTimestamp,
  formatRelativeTime,
} from "../../lib/relativeTime";

interface LastSyncedTextProps {
  timestamp: number | null | undefined;
}

export function LastSyncedText({ timestamp }: LastSyncedTextProps) {
  if (timestamp == null) {
    return <>Never synced</>;
  }

  return (
    <time
      dateTime={new Date(timestamp).toISOString()}
      title={formatExactTimestamp(timestamp)}
    >
      {formatRelativeTime(timestamp)}
    </time>
  );
}
