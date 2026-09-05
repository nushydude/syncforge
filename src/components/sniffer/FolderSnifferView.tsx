import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { pickFolder } from "../../api/pairs";
import {
  cancelSnifferScan,
  executeSnifferAction,
  getSnifferScan,
  getSnifferSummary,
  prepareSnifferAction,
  querySnifferEntries,
  querySnifferIssues,
  refreshSnifferSubtree,
  showSnifferItemProperties,
  startSnifferScan,
} from "../../api/sniffer";
import type {
  SnifferEntry,
  SnifferEntryPage,
  SnifferError,
  SnifferIssuePage,
  SnifferQuery,
  SnifferScan,
  SnifferSummary,
} from "../../types";

const PAGE_SIZE = 100;
const terminal = new Set(["completed", "cancelled", "failed"]);

function formatBytes(value: string): string {
  const bytes = BigInt(value);
  if (bytes < 1024n) return `${bytes.toLocaleString()} B`;
  const units = ["KB", "MB", "GB", "TB", "PB", "EB"];
  let divisor = 1024n;
  let unit = 0;
  while (bytes >= divisor * 1024n && unit < units.length - 1) {
    divisor *= 1024n;
    unit += 1;
  }
  const tenths = (bytes * 10n) / divisor;
  return tenths >= 100n
    ? `${(tenths / 10n).toLocaleString()} ${units[unit]}`
    : `${tenths / 10n}.${tenths % 10n} ${units[unit]}`;
}

const formatCount = (value: string | null) =>
  BigInt(value ?? "0").toLocaleString();

function formatPercent(value: string, total: string): string {
  const denominator = BigInt(total);
  if (denominator === 0n) return "—";
  const tenths = (BigInt(value) * 1000n) / denominator;
  return `${tenths / 10n}.${tenths % 10n}%`;
}

function errorDetail(error: unknown): SnifferError {
  if (typeof error === "object" && error && "message" in error) {
    return error as SnifferError;
  }
  return {
    code: "failed",
    operation: "query",
    retryable: true,
    message: String(error),
  };
}

function shortPath(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length > 4 ? `…/${parts.slice(-4).join("/")}` : path;
}

function localDateBoundary(value: string, nextDay = false): number | undefined {
  if (!value) return undefined;
  const date = new Date(`${value}T00:00:00`);
  if (nextDay) date.setDate(date.getDate() + 1);
  return date.getTime();
}

export function FolderSnifferView() {
  const viewRef = useRef<HTMLElement>(null);
  const scanRef = useRef<SnifferScan | null>(null);
  const requestToken = useRef(0);
  const pendingRefreshId = useRef<string | null>(null);
  const refreshStarting = useRef(false);
  const [scan, setScan] = useState<SnifferScan | null>(null);
  const [page, setPage] = useState<SnifferEntryPage | null>(null);
  const [summary, setSummary] = useState<SnifferSummary | null>(null);
  const [directoryId, setDirectoryId] = useState<string | null>(null);
  const [trail, setTrail] = useState<SnifferEntry[]>([]);
  const [history, setHistory] = useState<SnifferEntry[][]>([]);
  const [forward, setForward] = useState<SnifferEntry[][]>([]);
  const [selected, setSelected] = useState<SnifferEntry | null>(null);
  const [scope, setScope] = useState<SnifferQuery["scope"]>("children");
  const [sortBy, setSortBy] = useState<SnifferQuery["sortBy"]>("size");
  const [sortDirection, setSortDirection] =
    useState<SnifferQuery["sortDirection"]>("desc");
  const [search, setSearch] = useState("");
  const [kind, setKind] = useState("all");
  const [extension, setExtension] = useState("");
  const [minSize, setMinSize] = useState("");
  const [maxSize, setMaxSize] = useState("");
  const [modifiedFrom, setModifiedFrom] = useState("");
  const [modifiedTo, setModifiedTo] = useState("");
  const [cursors, setCursors] = useState<(string | undefined)[]>([undefined]);
  const [pageIndex, setPageIndex] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<SnifferError | null>(null);
  const [issuesOpen, setIssuesOpen] = useState(false);
  const [issues, setIssues] = useState<SnifferIssuePage | null>(null);
  const [actionPending, setActionPending] = useState(false);
  const [refreshJob, setRefreshJob] = useState<SnifferScan | null>(null);

  const loadResults = useCallback(
    async (
      activeScan: SnifferScan,
      activeDirectory: string,
      cursor?: string,
    ) => {
      const token = ++requestToken.current;
      setLoading(true);
      try {
        const request: SnifferQuery = {
          scanId: activeScan.id,
          generationId: activeScan.generationId,
          directoryId: activeDirectory,
          scope,
          sortBy,
          sortDirection,
          search,
          extension: extension || undefined,
          minSize: minSize || undefined,
          maxSize: maxSize || undefined,
          modifiedFrom: localDateBoundary(modifiedFrom),
          modifiedTo: localDateBoundary(modifiedTo, true),
          itemKind: kind,
          cursor,
          limit: PAGE_SIZE,
        };
        const [nextPage, nextSummary] = await Promise.all([
          querySnifferEntries(request),
          getSnifferSummary(
            activeScan.id,
            activeScan.generationId,
            activeDirectory,
          ),
        ]);
        if (token !== requestToken.current) return;
        setPage(nextPage);
        setSummary(nextSummary);
        setSelected((current) =>
          current
            ? (nextPage.rows.find((row) => row.nodeId === current.nodeId) ??
              null)
            : null,
        );
        setError(null);
        return true;
      } catch (nextError) {
        if (token === requestToken.current) setError(errorDetail(nextError));
        return false;
      } finally {
        if (token === requestToken.current) setLoading(false);
      }
    },
    [
      extension,
      kind,
      maxSize,
      minSize,
      modifiedFrom,
      modifiedTo,
      scope,
      search,
      sortBy,
      sortDirection,
    ],
  );

  const acceptScan = useCallback((next: SnifferScan) => {
    if (
      !next ||
      typeof next.id !== "string" ||
      typeof next.revision !== "number" ||
      typeof next.logicalBytes !== "string"
    )
      return;
    if (refreshStarting.current && next.id !== scanRef.current?.id) {
      pendingRefreshId.current = next.id;
      refreshStarting.current = false;
    }
    if (pendingRefreshId.current === next.id) {
      if (next.status === "completed" && next.rootNodeId) {
        pendingRefreshId.current = null;
        setRefreshJob(null);
        scanRef.current = next;
        setScan(next);
        setDirectoryId(next.rootNodeId);
        setTrail([]);
        setHistory([]);
        setForward([]);
        setCursors([undefined]);
        setPageIndex(0);
      } else if (next.status === "failed" || next.status === "cancelled") {
        pendingRefreshId.current = null;
        setRefreshJob(null);
        if (next.status === "failed")
          setError(
            next.error ?? {
              code: "failed",
              operation: "refresh",
              retryable: true,
              message: "Refresh failed. The previous scan is still available.",
            },
          );
      } else {
        setRefreshJob(next);
      }
      return;
    }
    setScan((current) => {
      if (current && current.id === next.id && current.revision > next.revision)
        return current;
      scanRef.current = next;
      return next;
    });
    if (next.rootNodeId)
      setDirectoryId((current) => current ?? next.rootNodeId);
  }, []);

  useEffect(() => {
    let active = true;
    let stop: (() => void) | undefined;
    void listen<SnifferScan>("syncforge://sniffer-progress", ({ payload }) => {
      if (active) acceptScan(payload);
    })
      .then((unlisten) => {
        stop = unlisten;
        return getSnifferScan();
      })
      .then((snapshot) => {
        if (active && snapshot) acceptScan(snapshot);
      })
      .catch((nextError) => {
        if (active) setError(errorDetail(nextError));
      });
    return () => {
      active = false;
      stop?.();
    };
  }, [acceptScan]);

  useEffect(() => {
    const activeScan = scanRef.current;
    if (!activeScan?.rootNodeId || !directoryId) return;
    void loadResults(activeScan, directoryId, cursors[pageIndex]);
  }, [
    scan?.id,
    scan?.rootNodeId,
    scan?.status,
    directoryId,
    loadResults,
    pageIndex,
    cursors,
  ]);

  useEffect(() => {
    setCursors([undefined]);
    setPageIndex(0);
  }, [
    scope,
    sortBy,
    sortDirection,
    search,
    kind,
    extension,
    minSize,
    maxSize,
    modifiedFrom,
    modifiedTo,
  ]);

  const navigate = useCallback(
    async (entry: SnifferEntry, fromHistory = false) => {
      if (!scan || entry.kind !== "directory") return;
      const oldTrail = trail;
      const nextTrail = fromHistory ? trail : [...trail, entry];
      setLoading(true);
      try {
        const loaded = await loadResults(scan, entry.nodeId);
        if (!loaded) return;
        if (!fromHistory) {
          setHistory((items) => [...items, oldTrail]);
          setForward([]);
        }
        setTrail(nextTrail);
        setDirectoryId(entry.nodeId);
        setCursors([undefined]);
        setPageIndex(0);
      } catch {
        // loadResults keeps the prior location and reports the query error.
      }
    },
    [loadResults, scan, trail],
  );

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (viewRef.current?.closest("[hidden]")) return;
      if (event.key === "Enter" && selected?.kind === "directory") {
        event.preventDefault();
        void navigate(selected);
      }
      if (event.altKey && event.key === "ArrowLeft" && trail.length > 1) {
        event.preventDefault();
        const parent = trail[trail.length - 2];
        setTrail((items) => items.slice(0, -1));
        setDirectoryId(parent.nodeId);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate, selected, trail]);

  async function chooseFolder() {
    try {
      const root = await pickFolder();
      if (!root) return;
      const next = await startSnifferScan(root);
      requestToken.current += 1;
      setPage(null);
      setSummary(null);
      setDirectoryId(null);
      setTrail([]);
      setHistory([]);
      setForward([]);
      setError(null);
      acceptScan(next);
    } catch (nextError) {
      setError(errorDetail(nextError));
    }
  }

  async function runAction(action: "rename" | "recycle") {
    if (!scan || !selected || actionPending) return;
    const newName =
      action === "rename"
        ? window.prompt("New name", selected.name)?.trim()
        : undefined;
    if (action === "rename" && !newName) return;
    try {
      const review = await prepareSnifferAction({
        scanId: scan.id,
        generationId: scan.generationId,
        nodeId: selected.nodeId,
        action,
        newName,
      });
      const message =
        action === "recycle"
          ? `Move this ${review.kind} to the Recycle Bin?\n\n${review.fullPath}`
          : `Rename this ${review.kind}?\n\n${review.fullPath}\n→ ${review.newPath}`;
      if (!window.confirm(message)) return;
      setActionPending(true);
      await executeSnifferAction(review.token);
      setSelected(null);
      setError(null);
      setScan((current) =>
        current
          ? { ...current, stale: true, revision: current.revision + 1 }
          : current,
      );
    } catch (nextError) {
      setError({ ...errorDetail(nextError), operation: action });
    } finally {
      setActionPending(false);
    }
  }

  const elapsed = scan
    ? Math.max(0, (scan.finishedAt ?? Date.now()) - scan.startedAt)
    : 0;
  const filtered = Boolean(
    search ||
    extension ||
    minSize ||
    maxSize ||
    modifiedFrom ||
    modifiedTo ||
    kind !== "all",
  );
  const mapTotal = useMemo(
    () =>
      (
        summary?.tiles.reduce(
          (sum, tile) => sum + BigInt(tile.logicalSize),
          0n,
        ) ?? 0n
      ).toString(),
    [summary],
  );
  const largestTile = summary?.tiles[0]?.logicalSize ?? "1";

  return (
    <main className="sniffer-view" ref={viewRef}>
      <header className="workspace-header sniffer-header">
        <div>
          <h2>Folder sniffer</h2>
          <p>
            Scan once, then explore every indexed folder without rescanning.
          </p>
        </div>
        <button
          type="button"
          className="btn-primary"
          onClick={() => void chooseFolder()}
          disabled={Boolean(scan && !terminal.has(scan.status))}
        >
          {scan ? "Choose another folder" : "Choose a folder"}
        </button>
      </header>

      {!scan && !error && (
        <section className="sniffer-empty">
          <div className="sniffer-empty-icon" aria-hidden="true">
            ◒
          </div>
          <h3>Find where your storage is going</h3>
          <p>Select one folder to build a retained, browsable storage index.</p>
          <button
            type="button"
            className="btn-primary"
            onClick={() => void chooseFolder()}
          >
            Scan folder
          </button>
        </section>
      )}

      {scan && (
        <section
          className="sniffer-scan-progress"
          aria-live="polite"
          aria-atomic="true"
        >
          <div className="sniffer-scan-progress-heading">
            {!terminal.has(scan.status) && (
              <span className="sniffer-scan-spinner" aria-hidden="true" />
            )}
            <strong>
              {scan.status === "queued"
                ? "Queued"
                : scan.status === "cancelling"
                  ? "Cancelling…"
                  : scan.status === "scanning"
                    ? "Scanning…"
                    : scan.status}
            </strong>
            {!terminal.has(scan.status) && (
              <button
                type="button"
                onClick={() => void cancelSnifferScan(scan.id).then(acceptScan)}
              >
                Cancel
              </button>
            )}
          </div>
          <p className="sniffer-root-path" title={scan.root}>
            {scan.root}
          </p>
          {!terminal.has(scan.status) && (
            <div
              className="sniffer-progress-track"
              aria-label="Scan in progress"
            >
              <div className="sniffer-progress-indeterminate" />
            </div>
          )}
          <small>
            {formatCount(scan.filesVisited)} files ·{" "}
            {formatCount(scan.foldersVisited)} folders ·{" "}
            {formatBytes(scan.logicalBytes)} provisional ·{" "}
            {(elapsed / 1000).toFixed(1)}s
            {scan.currentDirectory
              ? ` · ${shortPath(scan.currentDirectory)}`
              : ""}
          </small>
        </section>
      )}

      {error && (
        <section className="sniffer-error" role="alert">
          <strong>
            {error.operation === "rename"
              ? "Rename failed"
              : error.operation === "recycle"
                ? "Recycle failed"
                : error.operation === "scan"
                  ? "Scan failed"
                  : "Could not load Folder Sniffer results"}
          </strong>
          <p>{error.message}</p>
          {error.retryable && scan?.rootNodeId && (
            <button
              type="button"
              onClick={() => directoryId && void loadResults(scan, directoryId)}
            >
              Retry
            </button>
          )}
        </section>
      )}

      {scan?.rootNodeId && directoryId && (
        <section
          className="sniffer-results"
          aria-label="Indexed folder contents"
        >
          <div className="sniffer-breadcrumb">
            <nav className="sniffer-trail" aria-label="Folder path">
              <button
                type="button"
                className="sniffer-trail-link"
                onClick={() => {
                  setDirectoryId(scan.rootNodeId);
                  setTrail([]);
                }}
              >
                Root
              </button>
              {trail.map((item, index) => (
                <span key={item.nodeId} className="sniffer-trail-segment">
                  <span aria-hidden="true"> / </span>
                  <button
                    type="button"
                    className="sniffer-trail-link"
                    aria-current={
                      index === trail.length - 1 ? "location" : undefined
                    }
                    onClick={() => {
                      setDirectoryId(item.nodeId);
                      setTrail(trail.slice(0, index + 1));
                    }}
                  >
                    {item.name}
                  </button>
                </span>
              ))}
            </nav>
            <div className="sniffer-breadcrumb-actions">
              <button
                type="button"
                disabled={history.length === 0}
                onClick={() => {
                  const previous = history[history.length - 1];
                  if (!previous) return;
                  setForward((items) => [trail, ...items]);
                  setHistory((items) => items.slice(0, -1));
                  setTrail(previous);
                  setDirectoryId(
                    previous[previous.length - 1]?.nodeId ?? scan.rootNodeId,
                  );
                }}
              >
                Back
              </button>
              <button
                type="button"
                disabled={forward.length === 0}
                onClick={() => {
                  const next = forward[0];
                  setHistory((items) => [...items, trail]);
                  setForward((items) => items.slice(1));
                  setTrail(next);
                  setDirectoryId(
                    next[next.length - 1]?.nodeId ?? scan.rootNodeId,
                  );
                }}
              >
                Forward
              </button>
              <button
                type="button"
                disabled={directoryId === scan.rootNodeId}
                onClick={() => {
                  const next = trail.slice(0, -1);
                  setTrail(next);
                  setDirectoryId(
                    next[next.length - 1]?.nodeId ?? scan.rootNodeId,
                  );
                }}
              >
                Up
              </button>
              <button
                type="button"
                disabled={!terminal.has(scan.status) || Boolean(refreshJob)}
                onClick={() => {
                  refreshStarting.current = true;
                  void refreshSnifferSubtree(
                    scan.id,
                    scan.generationId,
                    directoryId,
                  )
                    .then((next) => {
                      refreshStarting.current = false;
                      if (
                        scanRef.current?.id === next.id &&
                        terminal.has(scanRef.current.status)
                      )
                        return;
                      pendingRefreshId.current = next.id;
                      setRefreshJob((current) =>
                        current?.id === next.id &&
                        current.revision > next.revision
                          ? current
                          : next,
                      );
                    })
                    .catch((e) => {
                      refreshStarting.current = false;
                      setError(errorDetail(e));
                    });
                }}
              >
                {refreshJob ? "Refreshing…" : "Refresh"}
              </button>
              {refreshJob && (
                <button
                  type="button"
                  onClick={() =>
                    void cancelSnifferScan(refreshJob.id).then(acceptScan)
                  }
                >
                  Cancel refresh
                </button>
              )}
            </div>
          </div>

          <div className="sniffer-toolbar">
            <select
              aria-label="Result scope"
              value={scope}
              onChange={(event) =>
                setScope(event.target.value as SnifferQuery["scope"])
              }
            >
              <option value="children">Current folder</option>
              <option value="subtreeFiles">Largest files in subtree</option>
            </select>
            <input
              aria-label="Search names and paths"
              type="search"
              placeholder="Search names and paths"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
            />
            <select
              aria-label="Item kind"
              value={kind}
              onChange={(event) => setKind(event.target.value)}
            >
              <option value="all">All items</option>
              <option value="directory">Folders</option>
              <option value="file">Files</option>
              <option value="link">Skipped links</option>
            </select>
            <input
              aria-label="Extension"
              placeholder="Extension"
              value={extension}
              onChange={(event) => setExtension(event.target.value)}
            />
            <input
              aria-label="Minimum bytes"
              inputMode="numeric"
              placeholder="Min bytes"
              value={minSize}
              onChange={(event) =>
                setMinSize(event.target.value.replace(/\D/g, ""))
              }
            />
            <input
              aria-label="Maximum bytes"
              inputMode="numeric"
              placeholder="Max bytes"
              value={maxSize}
              onChange={(event) =>
                setMaxSize(event.target.value.replace(/\D/g, ""))
              }
            />
            <input
              aria-label="Modified on or after"
              type="date"
              value={modifiedFrom}
              onChange={(event) => setModifiedFrom(event.target.value)}
            />
            <input
              aria-label="Modified on or before"
              type="date"
              value={modifiedTo}
              onChange={(event) => setModifiedTo(event.target.value)}
            />
          </div>

          {summary && (
            <>
              <div className="sniffer-summary">
                <div>
                  <strong>{formatBytes(summary.logicalBytes)}</strong>
                  <span>Logical size</span>
                </div>
                <div>
                  <strong>{formatCount(summary.folders)}</strong>
                  <span>Subfolders</span>
                </div>
                <div>
                  <strong>{formatCount(summary.files)}</strong>
                  <span>Files</span>
                </div>
                <div>
                  <strong>{formatCount(summary.zeroSizeCount)}</strong>
                  <span>Zero-size items</span>
                </div>
              </div>
              {(!summary.coverageComplete || scan.status === "cancelled") && (
                <p className="form-warning" role="status">
                  Partial{scan.status === "cancelled" ? " — cancelled" : ""}.
                  Some entries could not be measured; unknown folders are not
                  treated as empty.
                </p>
              )}
              {summary.stale && (
                <p className="form-warning" role="status">
                  Stale — files changed after this observation. Refresh to
                  measure again.
                </p>
              )}
              {BigInt(scan.issueCount) > 0n && (
                <button
                  type="button"
                  onClick={() => {
                    const opening = !issuesOpen;
                    setIssuesOpen(opening);
                    if (opening && !issues) {
                      void querySnifferIssues(scan.id)
                        .then(setIssues)
                        .catch((nextError) => setError(errorDetail(nextError)));
                    }
                  }}
                >
                  {formatCount(scan.issueCount)} scan issues
                </button>
              )}
              {issuesOpen && issues && (
                <section className="sniffer-issues" aria-label="Scan issues">
                  <h3>Scan issues</h3>
                  {issues.rows.map((issue) => (
                    <article key={issue.id}>
                      <strong>{issue.category}</strong>
                      <span title={issue.path}>{issue.path}</span>
                      <small>
                        {issue.message}
                        {issue.code ? ` (OS ${issue.code})` : ""}
                      </small>
                    </article>
                  ))}
                  {Number(issues.omittedDetails) > 0 && (
                    <p>
                      {issues.omittedDetails} issue details were omitted after
                      the retention cap.
                    </p>
                  )}
                </section>
              )}
              <header className="sniffer-map-header">
                <div>
                  <h3>{filtered ? "Filtered results" : "Storage map"}</h3>
                  <p>
                    At most 40 measured items plus an aggregate Other tile.
                    Displayed: {formatBytes(mapTotal)}.
                  </p>
                </div>
              </header>
              <div className="sniffer-map sniffer-map-indexed">
                {summary.tiles.map((tile) => (
                  <button
                    type="button"
                    key={`${tile.kind}-${tile.nodeId ?? "other"}`}
                    className={`sniffer-tile ${tile.kind === "other" ? "other" : "folder"}`}
                    style={{
                      flexGrow: Number(
                        (BigInt(tile.logicalSize) * 10_000n) /
                          (BigInt(largestTile) || 1n),
                      ),
                    }}
                    onClick={() => {
                      if (tile.nodeId) {
                        const entry = page?.rows.find(
                          (row) => row.nodeId === tile.nodeId,
                        );
                        if (entry?.kind === "directory") void navigate(entry);
                        else if (entry) setSelected(entry);
                      }
                    }}
                  >
                    <strong>{tile.name}</strong>
                    <span>{formatBytes(tile.logicalSize)}</span>
                    {tile.kind === "other" && (
                      <small>{tile.itemCount} omitted items</small>
                    )}
                  </button>
                ))}
              </div>
            </>
          )}

          <div className="sniffer-table-meta">
            <span>
              {page
                ? `${formatCount(page.matchCount)} matches · ${formatBytes(page.matchedBytes)} filtered`
                : "Loading results…"}
            </span>
            {scan.status === "scanning" && (
              <button
                type="button"
                onClick={() =>
                  void loadResults(scan, directoryId, cursors[pageIndex])
                }
              >
                Refresh provisional results
              </button>
            )}
          </div>
          <div className="sniffer-table-wrap">
            <table className="sniffer-table">
              <thead>
                <tr>
                  {[
                    ["name", "Name"],
                    ["size", "Logical size"],
                    ["files", "Files"],
                    ["modified", "Modified"],
                  ].map(([key, label]) => (
                    <th key={key}>
                      <button
                        type="button"
                        onClick={() => {
                          if (sortBy === key)
                            setSortDirection((value) =>
                              value === "asc" ? "desc" : "asc",
                            );
                          else setSortBy(key as SnifferQuery["sortBy"]);
                        }}
                      >
                        {label}
                        {sortBy === key
                          ? sortDirection === "asc"
                            ? " ↑"
                            : " ↓"
                          : ""}
                      </button>
                    </th>
                  ))}
                  <th>% of folder</th>
                  <th>Path</th>
                  <th>Status</th>
                </tr>
              </thead>
              <tbody>
                {page?.rows.map((entry) => {
                  return (
                    <tr
                      key={entry.nodeId}
                      tabIndex={0}
                      aria-selected={selected?.nodeId === entry.nodeId}
                      onClick={() => setSelected(entry)}
                      onDoubleClick={() =>
                        entry.kind === "directory" && void navigate(entry)
                      }
                    >
                      <td>
                        {entry.kind === "directory"
                          ? "📁 "
                          : entry.kind === "file"
                            ? "📄 "
                            : "↗ "}
                        {entry.name}
                      </td>
                      <td>{formatBytes(entry.logicalSize)}</td>
                      <td>
                        {entry.kind === "directory"
                          ? formatCount(entry.files)
                          : ""}
                      </td>
                      <td>
                        {entry.modifiedAt
                          ? new Date(entry.modifiedAt).toLocaleString()
                          : "—"}
                      </td>
                      <td>
                        {formatPercent(entry.logicalSize, page.directoryBytes)}
                        {scan.status === "scanning" &&
                        page.directoryBytes !== "0"
                          ? "*"
                          : ""}
                      </td>
                      <td title={entry.fullPath}>
                        {scope === "subtreeFiles"
                          ? entry.relativePath
                          : entry.name}
                      </td>
                      <td>{entry.status}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          <nav className="preview-pagination" aria-label="Result pages">
            <button
              type="button"
              disabled={pageIndex === 0 || loading}
              onClick={() => setPageIndex((value) => value - 1)}
            >
              Previous
            </button>
            <span>Page {pageIndex + 1}</span>
            <button
              type="button"
              disabled={!page?.nextCursor || loading}
              onClick={() => {
                if (!page?.nextCursor) return;
                setCursors((items) => [
                  ...items.slice(0, pageIndex + 1),
                  page.nextCursor!,
                ]);
                setPageIndex((value) => value + 1);
              }}
            >
              Next
            </button>
          </nav>

          {selected && (
            <aside
              className="sniffer-details"
              aria-label="Selected item details"
            >
              <h3>{selected.name}</h3>
              <p title={selected.fullPath}>{selected.fullPath}</p>
              <dl>
                <dt>Logical size</dt>
                <dd>{formatBytes(selected.logicalSize)}</dd>
                <dt>Completeness</dt>
                <dd>{selected.status}</dd>
                <dt>Scan time</dt>
                <dd>{new Date(scan.startedAt).toLocaleString()}</dd>
              </dl>
              <div className="sniffer-detail-actions">
                {selected.kind === "directory" && (
                  <button type="button" onClick={() => void navigate(selected)}>
                    Open folder
                  </button>
                )}
                <button
                  type="button"
                  onClick={() =>
                    void openPath(selected.fullPath).catch((e) =>
                      setError({ ...errorDetail(e), operation: "open" }),
                    )
                  }
                >
                  Open externally
                </button>
                <button
                  type="button"
                  onClick={() => void revealItemInDir(selected.fullPath)}
                >
                  Show in Explorer
                </button>
                <button
                  type="button"
                  onClick={() =>
                    void navigator.clipboard.writeText(selected.fullPath)
                  }
                >
                  Copy path
                </button>
                <button
                  type="button"
                  onClick={() =>
                    void showSnifferItemProperties(
                      scan.id,
                      selected.nodeId,
                    ).catch((e) => setError(errorDetail(e)))
                  }
                >
                  Properties
                </button>
                <button
                  type="button"
                  disabled={actionPending}
                  onClick={() => void runAction("rename")}
                >
                  Rename…
                </button>
                <button
                  type="button"
                  className="sniffer-context-danger"
                  disabled={actionPending}
                  onClick={() => void runAction("recycle")}
                >
                  Move to Recycle Bin…
                </button>
              </div>
            </aside>
          )}
        </section>
      )}
    </main>
  );
}
