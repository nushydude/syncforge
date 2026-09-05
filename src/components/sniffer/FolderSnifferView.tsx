import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { pickFolder } from "../../api/pairs";
import {
  cancelSnifferScan,
  executeSnifferAction,
  getSnifferScan,
  getSnifferNode,
  getSnifferSummary,
  pinSnifferScan,
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
  const refreshTrail = useRef<SnifferEntry[]>([]);
  const acceptedRevisions = useRef(new Map<string, number>());
  const pendingEvents = useRef(new Map<string, SnifferScan>());
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
  const [issueCursors, setIssueCursors] = useState<(string | undefined)[]>([
    undefined,
  ]);
  const [issuePageIndex, setIssuePageIndex] = useState(0);
  const [otherOpen, setOtherOpen] = useState(false);
  const [actionPending, setActionPending] = useState(false);
  const [mutationWarning, setMutationWarning] = useState<string | null>(null);
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
        let nextPage: SnifferEntryPage | undefined;
        let nextSummary: SnifferSummary | undefined;
        for (let attempt = 0; attempt < 3; attempt += 1) {
          [nextPage, nextSummary] = await Promise.all([
            querySnifferEntries(request),
            getSnifferSummary(
              activeScan.id,
              activeScan.generationId,
              activeDirectory,
              request,
            ),
          ]);
          if (nextPage.revision === nextSummary.revision) break;
          nextPage = undefined;
          nextSummary = undefined;
        }
        if (!nextPage || !nextSummary)
          throw {
            code: "staleCursor",
            operation: "query",
            retryable: true,
            message: "The index changed while loading. Refresh the results.",
          } satisfies SnifferError;
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

  const resolveTrail = useCallback(
    async (activeScan: SnifferScan, previousTrail: SnifferEntry[]) => {
      if (!activeScan.rootNodeId) return [];
      const resolved: SnifferEntry[] = [];
      let parentId = activeScan.rootNodeId;
      for (const previous of previousTrail) {
        let cursor: string | undefined;
        let match: SnifferEntry | undefined;
        do {
          const result = await querySnifferEntries({
            scanId: activeScan.id,
            generationId: activeScan.generationId,
            directoryId: parentId,
            scope: "children",
            sortBy: "name",
            sortDirection: "asc",
            search: previous.name,
            itemKind: "directory",
            cursor,
            limit: PAGE_SIZE,
          });
          match = result.rows.find(
            (entry) =>
              entry.name === previous.name &&
              entry.fullPath === previous.fullPath,
          );
          cursor = result.nextCursor ?? undefined;
        } while (!match && cursor);
        if (!match) break;
        resolved.push(match);
        parentId = match.nodeId;
      }
      return resolved;
    },
    [],
  );
  const loadResultsRef = useRef(loadResults);
  const resolveTrailRef = useRef(resolveTrail);
  loadResultsRef.current = loadResults;
  resolveTrailRef.current = resolveTrail;

  const acceptScan = useCallback((next: SnifferScan, allowNew = false) => {
    if (
      !next ||
      typeof next.id !== "string" ||
      typeof next.revision !== "number" ||
      typeof next.logicalBytes !== "string"
    )
      return;
    const currentRevision = acceptedRevisions.current.get(next.id);
    if (currentRevision !== undefined && currentRevision > next.revision)
      return;
    const knownJob =
      next.id === scanRef.current?.id ||
      next.id === pendingRefreshId.current ||
      (!scanRef.current && !pendingRefreshId.current);
    if (!allowNew && !knownJob) {
      const buffered = pendingEvents.current.get(next.id);
      if (!buffered || buffered.revision < next.revision)
        pendingEvents.current.set(next.id, next);
      return;
    }
    acceptedRevisions.current.set(next.id, next.revision);
    if (pendingRefreshId.current === next.id) {
      if (next.status === "completed" && next.rootNodeId) {
        pendingRefreshId.current = null;
        setRefreshJob(null);
        scanRef.current = next;
        setScan(next);
        setHistory([]);
        setForward([]);
        setCursors([undefined]);
        setPageIndex(0);
        const previousTrail = refreshTrail.current;
        void resolveTrailRef
          .current(next, previousTrail)
          .then((resolved) => {
            if (scanRef.current?.id !== next.id) return;
            const target =
              resolved[resolved.length - 1]?.nodeId ?? next.rootNodeId!;
            setTrail(resolved);
            setDirectoryId(target);
            return loadResultsRef.current(next, target, undefined);
          })
          .catch((nextError) => {
            if (scanRef.current?.id !== next.id) return;
            setTrail([]);
            setDirectoryId(next.rootNodeId);
            setError(errorDetail(nextError));
          });
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

  const adoptCommandScan = useCallback(
    (next: SnifferScan) => {
      acceptScan(next, true);
      const buffered = pendingEvents.current.get(next.id);
      if (buffered) {
        pendingEvents.current.delete(next.id);
        acceptScan(buffered);
      }
      void getSnifferScan(next.id)
        .then((latest) => latest && acceptScan(latest))
        .catch((nextError) => setError(errorDetail(nextError)));
    },
    [acceptScan],
  );

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
        if (active && snapshot) acceptScan(snapshot, true);
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

  const navigateToTrail = useCallback(
    async (
      nextTrail: SnifferEntry[],
      mode: "push" | "back" | "forward" = "push",
    ) => {
      if (!scan?.rootNodeId) return;
      const targetId =
        nextTrail[nextTrail.length - 1]?.nodeId ?? scan.rootNodeId;
      const loaded = await loadResults(scan, targetId, undefined);
      if (!loaded) return;
      if (mode === "back") {
        setForward((items) => [trail, ...items]);
        setHistory((items) => items.slice(0, -1));
      } else if (mode === "forward") {
        setHistory((items) => [...items, trail]);
        setForward((items) => items.slice(1));
      } else {
        setHistory((items) => [...items, trail]);
        setForward([]);
      }
      setTrail(nextTrail);
      setDirectoryId(targetId);
      setCursors([undefined]);
      setPageIndex(0);
    },
    [loadResults, scan, trail],
  );

  const navigate = useCallback(
    async (entry: SnifferEntry) => {
      if (entry.kind === "directory") await navigateToTrail([...trail, entry]);
    },
    [navigateToTrail, trail],
  );

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (viewRef.current?.closest("[hidden]")) return;
      const target = event.target;
      if (
        target instanceof HTMLInputElement ||
        target instanceof HTMLSelectElement ||
        target instanceof HTMLTextAreaElement ||
        target instanceof HTMLButtonElement ||
        (target instanceof HTMLElement && target.isContentEditable)
      )
        return;
      if (event.key === "Enter" && selected?.kind === "directory") {
        event.preventDefault();
        void navigate(selected);
      }
      if (event.altKey && event.key === "ArrowLeft" && trail.length > 0) {
        event.preventDefault();
        void navigateToTrail(trail.slice(0, -1));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate, navigateToTrail, selected, trail]);

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
      setMutationWarning(null);
      adoptCommandScan(next);
    } catch (nextError) {
      setError(errorDetail(nextError));
    }
  }

  async function startRefresh(directory: string) {
    if (!scan) return;
    refreshTrail.current = trail;
    const next = await refreshSnifferSubtree(
      scan.id,
      scan.generationId,
      directory,
    );
    pendingRefreshId.current = next.id;
    adoptCommandScan(next);
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
      const result = await executeSnifferAction(review.token);
      setSelected(null);
      setError(null);
      setMutationWarning(result.warning);
      setScan((current) =>
        current
          ? { ...current, stale: true, revision: current.revision + 1 }
          : current,
      );
      setSummary((current) =>
        current ? { ...current, stale: true } : current,
      );
      const refreshDirectory = selected.parentId ?? directoryId;
      if (refreshDirectory) {
        void startRefresh(refreshDirectory).catch((refreshError) => {
          const detail = errorDetail(refreshError);
          setMutationWarning(
            [result.warning, `Refresh failed: ${detail.message}`]
              .filter(Boolean)
              .join(" "),
          );
        });
      }
    } catch (nextError) {
      setError({ ...errorDetail(nextError), operation: action });
    } finally {
      setActionPending(false);
    }
  }

  async function loadIssuePage(cursor?: string) {
    if (!scan) return;
    try {
      setIssues(await querySnifferIssues(scan.id, undefined, cursor));
    } catch (nextError) {
      setError(errorDetail(nextError));
    }
  }

  useEffect(() => {
    setIssues(null);
    setIssuesOpen(false);
    setIssueCursors([undefined]);
    setIssuePageIndex(0);
  }, [scan?.id]);

  useEffect(() => {
    void pinSnifferScan(scan?.id ?? null).catch((nextError) =>
      setError(errorDetail(nextError)),
    );
    return () => {
      void pinSnifferScan(null);
    };
  }, [scan?.id]);

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

      {mutationWarning && (
        <p className="form-warning" role="status">
          The file action succeeded. {mutationWarning}
        </p>
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
                onClick={() => void navigateToTrail([])}
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
                    onClick={() =>
                      void navigateToTrail(trail.slice(0, index + 1))
                    }
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
                  void navigateToTrail(previous, "back");
                }}
              >
                Back
              </button>
              <button
                type="button"
                disabled={forward.length === 0}
                onClick={() => {
                  const next = forward[0];
                  if (next) void navigateToTrail(next, "forward");
                }}
              >
                Forward
              </button>
              <button
                type="button"
                disabled={directoryId === scan.rootNodeId}
                onClick={() => void navigateToTrail(trail.slice(0, -1))}
              >
                Up
              </button>
              <button
                type="button"
                disabled={!terminal.has(scan.status) || Boolean(refreshJob)}
                onClick={() =>
                  void startRefresh(directoryId).catch((e) =>
                    setError(errorDetail(e)),
                  )
                }
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
              {(summary.stale || scan.stale) && (
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
                      setIssueCursors([undefined]);
                      setIssuePageIndex(0);
                      void loadIssuePage();
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
                  <nav className="preview-pagination" aria-label="Issue pages">
                    <button
                      type="button"
                      disabled={issuePageIndex === 0}
                      onClick={() => {
                        const nextIndex = issuePageIndex - 1;
                        setIssuePageIndex(nextIndex);
                        void loadIssuePage(issueCursors[nextIndex]);
                      }}
                    >
                      Previous issues
                    </button>
                    <span>Page {issuePageIndex + 1}</span>
                    <button
                      type="button"
                      disabled={!issues.nextCursor}
                      onClick={() => {
                        if (!issues.nextCursor) return;
                        const nextIndex = issuePageIndex + 1;
                        setIssueCursors((items) => [
                          ...items.slice(0, nextIndex),
                          issues.nextCursor ?? undefined,
                        ]);
                        setIssuePageIndex(nextIndex);
                        void loadIssuePage(issues.nextCursor);
                      }}
                    >
                      Next issues
                    </button>
                  </nav>
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
                      if (!tile.nodeId) {
                        setOtherOpen(true);
                        document
                          .querySelector<HTMLTableElement>(".sniffer-table")
                          ?.focus();
                        return;
                      }
                      void getSnifferNode(
                        scan.id,
                        scan.generationId,
                        tile.nodeId,
                      )
                        .then((entry) => {
                          if (entry.kind === "directory") void navigate(entry);
                          else setSelected(entry);
                        })
                        .catch((nextError) => setError(errorDetail(nextError)));
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
              {otherOpen &&
                summary.tiles.some((tile) => tile.kind === "other") && (
                  <section
                    className="sniffer-other"
                    aria-label="Other indexed items"
                  >
                    <div>
                      <strong>Other items</strong>
                      <span>
                        Browse the paged table below for items outside the top
                        40.
                      </span>
                    </div>
                    <button type="button" onClick={() => setOtherOpen(false)}>
                      Close
                    </button>
                  </section>
                )}
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
            <table className="sniffer-table" tabIndex={-1}>
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
                      onFocus={() => setSelected(entry)}
                      onKeyDown={(event) => {
                        if (event.key !== "Enter" && event.key !== " ") return;
                        event.preventDefault();
                        setSelected(entry);
                        if (event.key === "Enter" && entry.kind === "directory")
                          void navigate(entry);
                      }}
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
