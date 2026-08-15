import { useEffect, useState } from "react";

function elapsedFrom(startedAt: number | null): number {
  if (!startedAt) return 0;
  return Math.max(0, Math.floor((Date.now() - startedAt) / 1000));
}

/** A preview walks, then compares contents, then hashes — name the phase so a
 *  long pass is never mistaken for a hang. */
function phaseDescription(side: string | null, count: number): string {
  const n = count.toLocaleString();
  switch (side) {
    case "left":
      return `Listing the left folder — ${n} entries`;
    case "right":
      return `Listing the right folder — ${n} entries`;
    case "comparing":
      return `Comparing file contents — ${n} files read`;
    case "hashing left":
    case "hashing right":
      return `Checking ${side === "hashing left" ? "left" : "right"} contents — ${n} files`;
    default:
      return "Comparing files and preparing your sync plan";
  }
}

export function PreviewScanProgress({
  pairName,
  loading,
  queued = false,
  position = -1,
  startedAt = null,
  scannedEntries = 0,
  scanSide = null,
  scanPath = null,
  onCancel,
}: {
  pairName?: string;
  loading: boolean;
  /** Waiting in the shared work queue rather than scanning yet. */
  queued?: boolean;
  /** Zero-based place in the waiting list, or -1 when unknown. */
  position?: number;
  /** Epoch ms the scan began — from the store, so it survives remounts. */
  startedAt?: number | null;
  scannedEntries?: number;
  scanSide?: string | null;
  scanPath?: string | null;
  onCancel?: () => void;
}) {
  const [elapsedSeconds, setElapsedSeconds] = useState(() =>
    elapsedFrom(startedAt),
  );

  useEffect(() => {
    if (!loading) {
      setElapsedSeconds(0);
      return;
    }
    // Derived from the stored start time, so navigating away and back keeps
    // counting from when the scan actually began.
    setElapsedSeconds(elapsedFrom(startedAt));
    const timer = window.setInterval(() => {
      setElapsedSeconds(elapsedFrom(startedAt));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [loading, startedAt]);

  if (!loading && !queued) return null;

  const title = queued
    ? `Waiting to scan${pairName ? ` “${pairName}”` : ""}…`
    : `Scanning ${pairName ? `“${pairName}”` : "both folders"}…`;

  return (
    <section
      className="preview-scan-progress"
      aria-label="Scan progress"
      aria-live="polite"
      aria-busy={loading}
    >
      {loading && <div className="preview-scan-spinner" aria-hidden="true" />}
      <div className="preview-scan-text">
        <strong>{title}</strong>
        {queued ? (
          <p>
            {position >= 0
              ? `${position + 1} job${position === 0 ? "" : "s"} ahead of this one.`
              : "Queued behind other work."}
          </p>
        ) : (
          <>
            <p>{phaseDescription(scanSide, scannedEntries)}</p>
            {scanPath && (
              <p className="preview-scan-path" title={scanPath}>
                {scanPath}
              </p>
            )}
          </>
        )}
      </div>
      {!queued && <span aria-hidden="true">{elapsedSeconds}s</span>}
      {onCancel && (
        <button type="button" className="btn-danger" onClick={onCancel}>
          Cancel scan
        </button>
      )}
    </section>
  );
}
