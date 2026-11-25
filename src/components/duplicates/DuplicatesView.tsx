import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { pickFolder } from "../../api/pairs";
import {
  cancelDuplicateScan,
  getDuplicateScan,
  getDuplicateGroupsPage,
  getDuplicateMembersPage,
  revealDuplicate,
  resumeDuplicateScan,
  startDuplicateScan,
} from "../../api/duplicates";
import type {
  DuplicateGroupSummary,
  DuplicateGroupsPage,
  DuplicateMembersPage,
  DuplicateMatchMode,
  DuplicateScanJob,
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

function groupTitle(
  group: DuplicateGroupSummary,
  mode: DuplicateMatchMode,
): string {
  if (mode === "filename") return `Filename: ${group.key}`;
  if (mode === "size") return `Size: ${formatBytes(group.firstSize)}`;
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
  const [selectedScope, setSelectedScope] = useState("");
  const [cleaning, setCleaning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [handoffMessage, setHandoffMessage] = useState<string | null>(null);
  const handoffStatusRef = useRef<HTMLParagraphElement>(null);
  const [loadingJob, setLoadingJob] = useState(true);
  const lastSequence = useRef(-1);
  const latestJob = useRef<DuplicateScanJob | null>(null);
  const pendingFailure = useRef<{
    jobId: string;
    attempt: number;
    message: string;
  } | null>(null);
  const [groupCursor, setGroupCursor] = useState(0);
  const [memberCursor, setMemberCursor] = useState(0);
  const [groupPage, setGroupPage] = useState<DuplicateGroupsPage | null>(null);
  const [memberPage, setMemberPage] = useState<DuplicateMembersPage | null>(
    null,
  );
  const [activeGroup, setActiveGroup] = useState<string | null>(null);
  const [pageError, setPageError] = useState<string | null>(null);
  const [retryPage, setRetryPage] = useState(0);
  useEffect(() => {
    if (handoffMessage) handoffStatusRef.current?.focus();
  }, [handoffMessage]);

  const acceptJob = useCallback((next: DuplicateScanJob) => {
    if (next.sequence <= lastSequence.current) return;
    lastSequence.current = next.sequence;
    const failure = pendingFailure.current;
    if (
      failure &&
      next.id === failure.jobId &&
      next.attempt === failure.attempt &&
      next.status === "running"
    ) {
      next = { ...next, status: "interrupted", error: failure.message };
    } else if (
      failure &&
      (next.id !== failure.jobId ||
        next.attempt !== failure.attempt ||
        next.status !== "running")
    ) {
      pendingFailure.current = null;
    }
    latestJob.current = next;
    setJob(next);
    setRoot(next.root);
    setMode(next.mode);
  }, []);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    let stopFailure: (() => void) | undefined;
    const failureListener = listen<{
      jobId: string;
      attempt: number;
      message: string;
    }>("syncforge://duplicates-persistence-error", (event) => {
      if (!active) return;
      const failure = event.payload;
      const current = latestJob.current;
      if (
        current &&
        (current.id !== failure.jobId ||
          current.attempt !== failure.attempt ||
          current.status !== "running")
      )
        return;
      pendingFailure.current = failure;
      if (current) {
        const overlay: DuplicateScanJob = {
          ...current,
          status: "interrupted",
          error: failure.message,
        };
        latestJob.current = overlay;
        setJob(overlay);
      }
    });
    void failureListener
      .then((stop) => {
        if (active) stopFailure = stop;
        else stop();
      })
      .catch((reason) => {
        if (active)
          setError(`Could not listen for scan errors: ${String(reason)}`);
      });
    const unlistenPromise = listen<DuplicateScanJob>(
      DUPLICATE_PROGRESS_EVENT,
      (event) => {
        if (!active) return;
        acceptJob(event.payload);
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
        acceptJob(savedJob);
      })
      .catch((loadError) => {
        if (active) setError(String(loadError));
      })
      .finally(() => {
        if (active) setLoadingJob(false);
      });

    void unlistenPromise
      .then((stop) => {
        if (active) unlisten = stop;
        else stop();
      })
      .catch((reason) => {
        if (active)
          setError(`Could not listen for scan updates: ${String(reason)}`);
      });

    return () => {
      active = false;
      unlisten?.();
      stopFailure?.();
    };
  }, [acceptJob]);

  useEffect(() => {
    if (job?.status !== "running") return;
    let active = true;
    let inFlight = false;
    const refresh = () => {
      if (inFlight) return;
      inFlight = true;
      void getDuplicateScan()
        .then((snapshot) => {
          if (active && snapshot) acceptJob(snapshot);
        })
        .catch((reason) => {
          if (active)
            setError(`Could not refresh scan status: ${String(reason)}`);
        })
        .finally(() => {
          inFlight = false;
        });
    };
    refresh();
    const timer = window.setInterval(refresh, 2000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [job?.id, job?.attempt, job?.status, acceptJob]);

  const result = job?.summary ?? null;
  const scanning = job?.status === "running";
  const cleanupAvailable =
    !!result?.cleanupAvailable && job?.status === "completed";

  useEffect(() => {
    setSelectedPaths(new Set());
    setHandoffMessage(null);
    setGroupCursor(0);
    setMemberCursor(0);
    setGroupPage(null);
    setMemberPage(null);
    setActiveGroup(null);
  }, [job?.id, result?.revision]);

  useEffect(() => {
    if (!job?.id || !result?.revision) return;
    let active = true;
    setPageError(null);
    void getDuplicateGroupsPage(job.id, result.revision, groupCursor)
      .then((page) => {
        if (!active) return;
        setGroupPage(page);
        setActiveGroup(page.groups[0]?.id ?? null);
        setMemberCursor(0);
        setSelectedPaths(new Set());
      })
      .catch((e) => {
        if (active) setPageError(String(e));
      });
    return () => {
      active = false;
    };
  }, [job?.id, result?.revision, groupCursor, retryPage]);

  useEffect(() => {
    if (!job?.id || !result?.revision || !activeGroup) return;
    let active = true;
    setPageError(null);
    void getDuplicateMembersPage(
      job.id,
      result.revision,
      activeGroup,
      memberCursor,
    )
      .then((page) => {
        if (active) {
          setMemberPage(page);
          setSelectedPaths(new Set());
        }
      })
      .catch((e) => {
        if (active) setPageError(String(e));
      });
    return () => {
      active = false;
    };
  }, [job?.id, result?.revision, activeGroup, memberCursor, retryPage]);

  const cancelling = scanning && job.cancelRequested;
  const selectionScope = `${job?.id ?? ""}:${result?.revision ?? ""}:${activeGroup ?? ""}:${memberCursor}`;
  const selectionValid = selectedScope === selectionScope;
  const selectedCount = selectionValid ? selectedPaths.size : 0;
  const selectedBytes = useMemo(
    () =>
      memberPage?.files.reduce(
        (total, file) =>
          total +
          (selectionValid && file.id && selectedPaths.has(file.id)
            ? file.size
            : 0),
        0,
      ) ?? 0,
    [memberPage, selectedPaths, selectionValid],
  );

  async function scan() {
    const trimmedRoot = root.trim();
    if (!trimmedRoot) {
      setError("Choose a folder to scan first.");
      return;
    }
    setError(null);
    setHandoffMessage(null);
    setSelectedPaths(new Set());
    try {
      const nextJob = await startDuplicateScan(trimmedRoot, mode);
      acceptJob(nextJob);
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
      acceptJob(nextJob);
    } catch (resumeError) {
      setError(String(resumeError));
    }
  }

  async function cancelScan() {
    if (!job || !scanning || cancelling) return;
    try {
      const nextJob = await cancelDuplicateScan(job.id, job.attempt);
      if (nextJob) acceptJob(nextJob);
    } catch (cancelError) {
      setError(String(cancelError));
    }
  }

  async function chooseFolder() {
    const folder = await pickFolder();
    if (folder) setRoot(folder);
  }

  function togglePath(path: string) {
    setSelectedScope(selectionScope);
    setSelectedPaths((current) => {
      if (selectionValid && current.has(path)) return new Set<string>();
      return new Set([path]);
    });
  }

  async function openSelectedInExplorer() {
    if (!result?.revision || !job || !cleanupAvailable || selectedCount !== 1)
      return;
    const [itemId] = selectedPaths;
    setCleaning(true);
    setError(null);
    setHandoffMessage(null);
    try {
      await revealDuplicate(job.id, result.revision, itemId);
      setHandoffMessage(
        "Explorer request sent. Find and review the file there before removing it.",
      );
      setSelectedPaths(new Set());
    } catch (handoffError) {
      setError(String(handoffError));
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
            copies in Explorer.
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
      {handoffMessage && (
        <p ref={handoffStatusRef} role="status" tabIndex={-1}>
          {handoffMessage}
        </p>
      )}
      {result && !cleanupAvailable && (
        <p className="form-warning">
          You can browse these results. Opening a reviewed file in Explorer
          requires a fresh Windows scan.
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
              <strong>{result.groupCount.toLocaleString()}</strong>
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

          {result.groupCount === 0 ? (
            <p className="duplicates-empty">
              No duplicate candidates found with this matching method.
            </p>
          ) : (
            <div className="duplicate-groups">
              {pageError && (
                <p role="alert">
                  Could not load results: {pageError}{" "}
                  <button
                    type="button"
                    onClick={() => setRetryPage((n) => n + 1)}
                  >
                    Retry
                  </button>
                </p>
              )}
              <nav aria-label="Duplicate groups">
                {groupPage?.revision === result.revision &&
                  groupPage.groups.map((group) => (
                    <button
                      type="button"
                      key={group.id}
                      aria-pressed={activeGroup === group.id}
                      onClick={() => {
                        setActiveGroup(group.id);
                        setMemberCursor(0);
                        setMemberPage(null);
                        setSelectedPaths(new Set());
                      }}
                    >
                      {groupTitle(group, result.mode)} ({group.fileCount})
                    </button>
                  ))}
                <button
                  type="button"
                  disabled={cleaning || groupCursor === 0}
                  onClick={() => {
                    setGroupPage(null);
                    setMemberPage(null);
                    setGroupCursor(Math.max(0, groupCursor - 50));
                  }}
                >
                  Previous groups
                </button>
                <button
                  type="button"
                  disabled={cleaning || groupPage?.nextCursor == null}
                  onClick={() => {
                    if (groupPage?.nextCursor != null) {
                      setGroupPage(null);
                      setMemberPage(null);
                      setGroupCursor(groupPage.nextCursor);
                    }
                  }}
                >
                  Next groups
                </button>
              </nav>
              {memberPage?.revision === result.revision &&
                memberPage.groupId === activeGroup && (
                  <article className="duplicate-group">
                    <ul className="duplicate-file-list">
                      {memberPage.files.map((file) => (
                        <li key={file.id ?? file.relativePath}>
                          <label className="duplicate-file-row">
                            <input
                              type="radio"
                              name="duplicate-file-selection"
                              checked={
                                selectionValid &&
                                !!file.id &&
                                selectedPaths.has(file.id)
                              }
                              onChange={() => file.id && togglePath(file.id)}
                              disabled={
                                cleaning ||
                                !cleanupAvailable ||
                                !file.id ||
                                !file.fingerprint
                              }
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
                    <button
                      type="button"
                      disabled={cleaning || memberCursor === 0}
                      onClick={() => {
                        setMemberPage(null);
                        setSelectedPaths(new Set());
                        setMemberCursor(Math.max(0, memberCursor - 50));
                      }}
                    >
                      Previous files
                    </button>
                    <button
                      type="button"
                      disabled={cleaning || memberPage.nextCursor == null}
                      onClick={() => {
                        if (memberPage.nextCursor != null) {
                          const next = memberPage.nextCursor;
                          setMemberPage(null);
                          setSelectedPaths(new Set());
                          setMemberCursor(next);
                        }
                      }}
                    >
                      Next files
                    </button>
                  </article>
                )}
            </div>
          )}

          {selectedCount > 0 && (
            <div className="duplicate-selection-bar">
              <span>
                {selectedCount} selected - {formatBytes(selectedBytes)}
              </span>
              <button
                type="button"
                className="btn-primary"
                onClick={() => void openSelectedInExplorer()}
                disabled={cleaning}
              >
                {cleaning ? "Opening..." : "Open in Explorer"}
              </button>
            </div>
          )}
        </>
      )}
    </section>
  );
}
