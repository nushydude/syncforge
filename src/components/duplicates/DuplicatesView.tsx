import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { pickFolder } from "../../api/pairs";
import {
  cancelDuplicateScan,
  getDuplicateScan,
  removeDuplicates,
  resumeDuplicateScan,
  startDuplicateScan,
} from "../../api/duplicates";
import type {
  DuplicateGroup,
  DuplicateMatchMode,
  DuplicateScanJob,
  DuplicateScanResult,
} from "../../types";

const DUPLICATE_PROGRESS_EVENT = "syncforge://duplicates-progress";

const modeDetails: Record<
  DuplicateMatchMode,
  { label: string; description: string }
> = {
  hash: {
    label: "Exact content (hash)",
    description:
      "Safest option. Files are grouped by streamed BLAKE3 content hashes after a size check.",
  },
  size: {
    label: "File size",
    description:
      "Fast candidate finder. Same-sized files may have different contents, so review before removing.",
  },
  filename: {
    label: "Filename",
    description:
      "Groups matching names across subfolders. This is a heuristic and does not prove identical content.",
  },
};

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = -1;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unit]}`;
}

function groupTitle(group: DuplicateGroup, mode: DuplicateMatchMode): string {
  const first = group.files[0];
  if (mode === "filename") return `Filename: ${group.key}`;
  if (mode === "size") return `Size: ${formatBytes(first?.size ?? 0)}`;
  return `Hash: ${group.key.slice(0, 16)}...`;
}

function phaseLabel(job: DuplicateScanJob): string {
  switch (job.phase) {
    case "collecting":
      return "Finding files";
    case "hashing":
      return "Comparing file contents";
    case "finalizing":
      return "Building duplicate groups";
    default:
      return "Preparing scan";
  }
}

function progressPercent(job: DuplicateScanJob): number | null {
  if (job.phase === "hashing" && job.bytesTotal) {
    return Math.min(
      100,
      Math.round((job.bytesProcessed / job.bytesTotal) * 100),
    );
  }
  if (job.phase === "collecting" && job.totalFiles) {
    return Math.min(100, Math.round((job.filesFound / job.totalFiles) * 100));
  }
  return null;
}

function progressDetail(job: DuplicateScanJob): string {
  if (job.phase === "hashing") {
    const files = job.totalFiles
      ? `${job.hashedFiles.toLocaleString()} of ${job.totalFiles.toLocaleString()} candidate files hashed`
      : `${job.hashedFiles.toLocaleString()} files hashed`;
    const bytes = job.bytesTotal
      ? `${formatBytes(job.bytesProcessed)} of ${formatBytes(job.bytesTotal)}`
      : formatBytes(job.bytesProcessed);
    return `${files} - ${bytes}`;
  }
  if (job.phase === "collecting") {
    return job.totalFiles
      ? `${job.filesFound.toLocaleString()} of ${job.totalFiles.toLocaleString()} files found`
      : `${job.filesFound.toLocaleString()} files found`;
  }
  return `${job.filesFound.toLocaleString()} files found`;
}

function statusLabel(status: DuplicateScanJob["status"]): string {
  switch (status) {
    case "interrupted":
      return "Previous scan interrupted";
    case "cancelled":
      return "Scan cancelled";
    case "failed":
      return "Scan failed";
    case "completed":
      return "Scan complete";
    default:
      return "Scanning";
  }
}

export function DuplicatesView() {
  const [root, setRoot] = useState("");
  const [mode, setMode] = useState<DuplicateMatchMode>("hash");
  const [job, setJob] = useState<DuplicateScanJob | null>(null);
  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(new Set());
  const [cleaning, setCleaning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cleanupMessage, setCleanupMessage] = useState<string | null>(null);
  const [loadingJob, setLoadingJob] = useState(true);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    const unlistenPromise = listen<DuplicateScanJob>(
      DUPLICATE_PROGRESS_EVENT,
      (event) => {
        if (!active) return;
        setJob(event.payload);
        setRoot(event.payload.root);
        setMode(event.payload.mode);
      },
    );

    void getDuplicateScan()
      .then((savedJob) => {
        if (
          !active ||
          !savedJob ||
          typeof savedJob !== "object" ||
          !("id" in savedJob)
        ) {
          return;
        }
        setJob(savedJob);
        setRoot(savedJob.root);
        setMode(savedJob.mode);
      })
      .catch((loadError) => {
        if (active) setError(String(loadError));
      })
      .finally(() => {
        if (active) setLoadingJob(false);
      });

    void unlistenPromise.then((stop) => {
      if (active) unlisten = stop;
      else stop();
    });

    return () => {
      active = false;
      unlisten?.();
      void unlistenPromise.then((stop) => stop());
    };
  }, []);

  const result: DuplicateScanResult | null = job?.result ?? null;
  const scanning = job?.status === "running";
  const cancelling = scanning && job.cancelRequested;
  const selectedCount = selectedPaths.size;
  const selectedBytes = useMemo(() => {
    if (!result) return 0;
    return result.groups.reduce(
      (total, group) =>
        total +
        group.files.reduce(
          (groupTotal, file) =>
            selectedPaths.has(file.relativePath)
              ? groupTotal + file.size
              : groupTotal,
          0,
        ),
      0,
    );
  }, [result, selectedPaths]);

  async function scan() {
    const trimmedRoot = root.trim();
    if (!trimmedRoot) {
      setError("Choose a folder to scan first.");
      return;
    }
    setError(null);
    setCleanupMessage(null);
    setSelectedPaths(new Set());
    try {
      const nextJob = await startDuplicateScan(trimmedRoot, mode);
      setJob(nextJob);
      setRoot(nextJob.root);
    } catch (scanError) {
      setError(String(scanError));
    }
  }

  async function resume() {
    if (!job) return;
    setError(null);
    setSelectedPaths(new Set());
    try {
      const nextJob = await resumeDuplicateScan(job.id);
      setJob(nextJob);
      setRoot(nextJob.root);
      setMode(nextJob.mode);
    } catch (resumeError) {
      setError(String(resumeError));
    }
  }

  async function cancelScan() {
    if (!job || !scanning || cancelling) return;
    try {
      const nextJob = await cancelDuplicateScan(job.id);
      if (nextJob) setJob(nextJob);
    } catch (cancelError) {
      setError(String(cancelError));
    }
  }

  async function chooseFolder() {
    const folder = await pickFolder();
    if (folder) setRoot(folder);
  }

  function togglePath(path: string) {
    setSelectedPaths((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  function selectAllCopies() {
    if (!result) return;
    const paths = new Set<string>();
    for (const group of result.groups) {
      for (const file of group.files.slice(1)) paths.add(file.relativePath);
    }
    setSelectedPaths(paths);
  }

  async function moveSelectedToRecycleBin() {
    if (!result || selectedCount === 0) return;
    const confirmed = window.confirm(
      `Move ${selectedCount} selected file${selectedCount === 1 ? "" : "s"} to the Recycle Bin?`,
    );
    if (!confirmed) return;

    setCleaning(true);
    setError(null);
    setCleanupMessage(null);
    try {
      const cleanup = await removeDuplicates(result.root, [...selectedPaths]);
      if (cleanup.errors.length > 0) setError(cleanup.errors.join(" "));
      if (cleanup.removed.length > 0) {
        setCleanupMessage(
          `Moved ${cleanup.removed.length} file${cleanup.removed.length === 1 ? "" : "s"} to the Recycle Bin.`,
        );
        setSelectedPaths(new Set());
        await scan();
      }
    } catch (cleanupError) {
      setError(String(cleanupError));
    } finally {
      setCleaning(false);
    }
  }

  const percent = job ? progressPercent(job) : null;
  const selectedModeDetails = modeDetails[mode] ?? modeDetails.hash;

  return (
    <section className="duplicates-view">
      <header className="workspace-header">
        <div>
          <h2>Find duplicate files</h2>
          <p>
            Scan one folder, review duplicate candidates, and move unwanted
            copies to the Recycle Bin.
          </p>
        </div>
      </header>

      <div className="duplicates-controls">
        <label className="field">
          Folder to scan
          <div className="path-row">
            <input
              type="text"
              value={root}
              onChange={(event) => setRoot(event.target.value)}
              placeholder="C:\\Users\\you\\Documents"
              disabled={scanning || cleaning}
            />
            <button
              type="button"
              onClick={() => void chooseFolder()}
              disabled={scanning || cleaning}
            >
              Browse...
            </button>
          </div>
        </label>

        <label className="field">
          Match duplicates by
          <select
            value={mode}
            onChange={(event) =>
              setMode(event.target.value as DuplicateMatchMode)
            }
            disabled={scanning || cleaning}
          >
            <option value="hash">{modeDetails.hash.label}</option>
            <option value="size">{modeDetails.size.label}</option>
            <option value="filename">{modeDetails.filename.label}</option>
          </select>
          <span className="field-hint">{selectedModeDetails.description}</span>
        </label>

        <div className="form-actions duplicates-actions">
          <button
            type="button"
            className="btn-primary"
            onClick={() => void scan()}
            disabled={scanning || cleaning || loadingJob}
          >
            {scanning ? phaseLabel(job!) : "Scan folder"}
          </button>
          {scanning && (
            <button
              type="button"
              className="btn-danger"
              onClick={() => void cancelScan()}
              disabled={cancelling}
            >
              {cancelling ? "Cancelling..." : "Cancel scan"}
            </button>
          )}
          {!scanning &&
            job &&
            (job.status === "interrupted" || job.status === "cancelled") && (
              <button type="button" onClick={() => void resume()}>
                Resume scan
              </button>
            )}
          {result && result.groups.length > 0 && result.mode === "hash" && (
            <button
              type="button"
              onClick={selectAllCopies}
              disabled={scanning || cleaning}
            >
              Select all but first
            </button>
          )}
        </div>
      </div>

      {job && (scanning || job.status !== "completed") && (
        <section className="duplicate-progress" aria-live="polite">
          <div className="duplicate-progress-header">
            <div>
              <p className="duplicate-progress-eyebrow">
                {statusLabel(job.status)}
              </p>
              <h3>{scanning ? phaseLabel(job) : statusLabel(job.status)}</h3>
            </div>
            {scanning && (
              <span>{percent === null ? "Working" : `${percent}%`}</span>
            )}
          </div>
          {scanning && (
            <>
              <progress
                max={100}
                value={percent === null ? undefined : percent}
                aria-label="Duplicate scan progress"
              />
              <div className="duplicate-progress-stats">
                <span>{progressDetail(job)}</span>
                <span>
                  Cancel is safe; no files are changed during analysis.
                </span>
              </div>
              {job.currentPath && (
                <p className="duplicate-progress-path" title={job.currentPath}>
                  {job.currentPath}
                </p>
              )}
            </>
          )}
          {!scanning && job.error && (
            <p className="form-warning">{job.error}</p>
          )}
        </section>
      )}

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      {cleanupMessage && (
        <p className="form-success" role="status">
          {cleanupMessage}
        </p>
      )}

      {result && (
        <>
          <div className="duplicate-summary" aria-live="polite">
            <div>
              <strong>{result.scannedFiles.toLocaleString()}</strong>
              <span>files scanned</span>
            </div>
            <div>
              <strong>{result.groups.length.toLocaleString()}</strong>
              <span>duplicate groups</span>
            </div>
            <div>
              <strong>{formatBytes(result.potentialSavingsBytes)}</strong>
              <span>potential savings</span>
            </div>
            {result.mode === "hash" && (
              <div>
                <strong>{result.hashedFiles.toLocaleString()}</strong>
                <span>files hashed</span>
              </div>
            )}
          </div>

          {result.skippedFiles > 0 && (
            <p className="form-warning" role="status">
              {result.skippedFiles} file{result.skippedFiles === 1 ? "" : "s"}{" "}
              could not be scanned.
            </p>
          )}
          {result.warnings.length > 0 && (
            <details className="duplicate-warnings">
              <summary>Show scan warnings ({result.warnings.length})</summary>
              <ul>
                {result.warnings.map((warning) => (
                  <li key={warning}>{warning}</li>
                ))}
              </ul>
            </details>
          )}

          {result.groups.length === 0 ? (
            <p className="duplicates-empty">
              No duplicate candidates found with this matching method.
            </p>
          ) : (
            <div className="duplicate-groups">
              {result.groups.map((group) => (
                <article
                  className="duplicate-group"
                  key={`${group.key}-${group.files[0]?.relativePath}`}
                >
                  <header className="duplicate-group-header">
                    <div>
                      <h3>{groupTitle(group, result.mode)}</h3>
                      <p>
                        {group.files.length} files -{" "}
                        {formatBytes(group.potentialSavingsBytes)} potential
                        savings
                      </p>
                    </div>
                  </header>
                  <ul className="duplicate-file-list">
                    {group.files.map((file) => (
                      <li key={file.relativePath}>
                        <label className="duplicate-file-row">
                          <input
                            type="checkbox"
                            checked={selectedPaths.has(file.relativePath)}
                            onChange={() => togglePath(file.relativePath)}
                            disabled={cleaning}
                          />
                          <span className="duplicate-file-path">
                            {file.relativePath}
                          </span>
                          <span className="duplicate-file-size">
                            {formatBytes(file.size)}
                          </span>
                        </label>
                      </li>
                    ))}
                  </ul>
                </article>
              ))}
            </div>
          )}

          {selectedCount > 0 && (
            <div className="duplicate-selection-bar">
              <span>
                {selectedCount} selected - {formatBytes(selectedBytes)}
              </span>
              <button
                type="button"
                className="btn-danger"
                onClick={() => void moveSelectedToRecycleBin()}
                disabled={cleaning}
              >
                {cleaning ? "Moving..." : "Move selected to Recycle Bin"}
              </button>
            </div>
          )}
        </>
      )}
    </section>
  );
}
