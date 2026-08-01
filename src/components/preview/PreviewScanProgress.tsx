import { useEffect, useState } from "react";

export function PreviewScanProgress({ loading }: { loading: boolean }) {
  const [elapsedSeconds, setElapsedSeconds] = useState(0);

  useEffect(() => {
    if (!loading) {
      setElapsedSeconds(0);
      return;
    }
    const startedAt = Date.now();
    const timer = window.setInterval(() => {
      setElapsedSeconds(Math.floor((Date.now() - startedAt) / 1000));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [loading]);

  if (!loading) return null;

  return (
    <section
      className="preview-scan-progress"
      aria-label="Scan progress"
      aria-live="polite"
      aria-busy="true"
    >
      <div className="preview-scan-spinner" aria-hidden="true" />
      <div>
        <strong>Scanning both folders…</strong>
        <p>Comparing files and preparing your sync plan</p>
      </div>
      <span aria-hidden="true">{elapsedSeconds}s</span>
    </section>
  );
}
