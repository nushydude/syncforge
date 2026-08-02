import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { listen } from "@tauri-apps/api/event";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { pickFolder } from "../../api/pairs";
import {
  deleteSnifferItem,
  renameSnifferItem,
  scanFolderSizes,
  showSnifferItemProperties,
} from "../../api/sniffer";
import type { FolderSizeEntry, FolderSizeResult } from "../../types";

interface TreemapTile {
  entry: FolderSizeEntry;
  left: number;
  top: number;
  width: number;
  height: number;
  tone: number;
}

interface SnifferProgress {
  completed: number;
  total: number;
  current: string;
}

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

function shortPath(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length > 3 ? `…/${parts.slice(-3).join("/")}` : path;
}

function tileLabel(entry: FolderSizeEntry): string {
  return entry.isFolder ? `${entry.name}/` : entry.name;
}

function folderName(path: string): string {
  if (path.endsWith("\\__other__")) return "Other items";
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

function openerPath(path: string): string {
  if (path.startsWith("\\\\?\\UNC\\")) return `\\\\${path.slice(8)}`;
  if (path.startsWith("\\\\?\\")) return path.slice(4);
  return path;
}

function buildTreemap(entries: FolderSizeEntry[]): TreemapTile[] {
  const tiles: TreemapTile[] = [];

  function layout(
    group: FolderSizeEntry[],
    left: number,
    top: number,
    width: number,
    height: number,
  ) {
    if (group.length === 0) return;
    if (group.length === 1) {
      tiles.push({
        entry: group[0],
        left,
        top,
        width,
        height,
        tone: tiles.length % 6,
      });
      return;
    }

    const groupTotal = group.reduce((sum, entry) => sum + entry.size, 0) || 1;
    let running = 0;
    let split = 1;
    let bestDistance = Number.POSITIVE_INFINITY;
    for (let index = 1; index < group.length; index += 1) {
      running += group[index - 1].size;
      const distance = Math.abs(groupTotal - running * 2);
      if (distance < bestDistance) {
        bestDistance = distance;
        split = index;
      }
    }

    const first = group.slice(0, split);
    const second = group.slice(split);
    const firstRatio =
      first.reduce((sum, entry) => sum + entry.size, 0) / groupTotal;
    if (width >= height) {
      const firstWidth = width * firstRatio;
      layout(first, left, top, firstWidth, height);
      layout(second, left + firstWidth, top, width - firstWidth, height);
    } else {
      const firstHeight = height * firstRatio;
      layout(first, left, top, width, firstHeight);
      layout(second, left, top + firstHeight, width, height - firstHeight);
    }
  }

  layout(entries, 0, 0, 100, 100);
  return tiles;
}

function FileSizeView({
  result,
  title = "Largest files",
  description = "This folder contains files rather than subfolders, so the largest files are ranked here.",
  onShowInMap,
  onEntryContextMenu,
  contextEntryPath,
}: {
  result: FolderSizeResult;
  title?: string;
  description?: string;
  onShowInMap?: () => void;
  onEntryContextMenu?: (event: ReactMouseEvent, entry: FolderSizeEntry) => void;
  contextEntryPath?: string;
}) {
  const [expandedFiles, setExpandedFiles] = useState(false);
  const files = result.entries.filter((entry) => !entry.isFolder);
  const visibleFiles = expandedFiles ? files : files.slice(0, 12);
  const otherFiles = files.slice(12);
  const otherSize = otherFiles.reduce((sum, entry) => sum + entry.size, 0);
  const maxSize = Math.max(...visibleFiles.map((entry) => entry.size), 1);

  return (
    <section className="sniffer-file-view" aria-label="Largest files">
      <header className="sniffer-map-header">
        <div>
          <h3>{title}</h3>
          <p>{description}</p>
        </div>
        <div className="sniffer-file-view-actions">
          <span className="sniffer-file-count">
            {expandedFiles
              ? `Showing all ${files.length.toLocaleString()}`
              : `Top ${visibleFiles.length} of ${files.length.toLocaleString()}`}
          </span>
          {onShowInMap && (
            <button type="button" onClick={onShowInMap}>
              Show files in map
            </button>
          )}
        </div>
      </header>
      <div className="sniffer-file-list">
        {visibleFiles.map((entry) => (
          <div
            className={`sniffer-file-row ${contextEntryPath === entry.path ? "context-selected" : ""}`}
            key={entry.path}
            title={entry.path}
            onContextMenu={(event) => onEntryContextMenu?.(event, entry)}
          >
            <div className="sniffer-file-row-label">
              <strong>{entry.name}</strong>
              <span>{formatBytes(entry.size)}</span>
            </div>
            <div className="sniffer-file-bar-track" aria-hidden="true">
              <div
                className="sniffer-file-bar"
                style={{
                  width: `${Math.max(2, (entry.size / maxSize) * 100)}%`,
                }}
              />
            </div>
          </div>
        ))}
        {otherSize > 0 && !expandedFiles && (
          <button
            type="button"
            className="sniffer-file-row other-file-row"
            onClick={() => setExpandedFiles(true)}
            title="Expand the remaining files in this view"
          >
            <div className="sniffer-file-row-label">
              <strong>
                Other files{" "}
                <span className="sniffer-inline-hint">· click to expand</span>
              </strong>
              <span>{formatBytes(otherSize)}</span>
            </div>
            <div className="sniffer-file-bar-track" aria-hidden="true">
              <div
                className="sniffer-file-bar"
                style={{
                  width: `${Math.max(2, (otherSize / maxSize) * 100)}%`,
                }}
              />
            </div>
          </button>
        )}
      </div>
    </section>
  );
}

export function FolderSnifferView() {
  const snifferViewRef = useRef<HTMLElement | null>(null);
  const [currentPath, setCurrentPath] = useState("");
  const [result, setResult] = useState<FolderSizeResult | null>(null);
  const [pathTrail, setPathTrail] = useState<string[]>([]);
  const [showingCached, setShowingCached] = useState(false);
  const [showFilesInMap, setShowFilesInMap] = useState(false);
  const [expandedOther, setExpandedOther] = useState(false);
  const [scanProgress, setScanProgress] = useState<SnifferProgress | null>(
    null,
  );
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    entry: FolderSizeEntry;
  } | null>(null);
  const [toast, setToast] = useState("");
  const scanCache = useRef(new Map<string, FolderSizeResult>());
  const mapLimit = 40;

  function handleEntryContextMenu(
    event: ReactMouseEvent,
    entry: FolderSizeEntry,
  ) {
    event.preventDefault();
    event.stopPropagation();
    const view = snifferViewRef.current;
    if (!view) return;
    const bounds = view.getBoundingClientRect();
    setContextMenu({
      x: event.clientX - bounds.left + view.scrollLeft,
      y: event.clientY - bounds.top + view.scrollTop,
      entry,
    });
  }

  async function runContextAction(
    action:
      | "open"
      | "reveal"
      | "copy-name"
      | "copy-path"
      | "properties"
      | "rename"
      | "delete",
  ) {
    if (!contextMenu) return;
    const { entry } = contextMenu;
    const path = entry.path;
    setContextMenu(null);
    setError("");
    try {
      const systemPath = openerPath(path);
      if (action === "open") await openPath(systemPath);
      else if (action === "reveal") await revealItemInDir(systemPath);
      else if (action === "copy-name") {
        await navigator.clipboard.writeText(entry.name);
        setToast("Filename copied to clipboard");
      } else if (action === "copy-path") {
        await navigator.clipboard.writeText(systemPath);
        setToast("File path copied to clipboard");
      } else if (action === "properties")
        await showSnifferItemProperties(systemPath);
      else if (action === "rename") {
        const newName = window.prompt("Rename item", entry.name);
        if (!newName || newName.trim() === entry.name) return;
        await renameSnifferItem(path, newName.trim());
        scanCache.current.clear();
        await openFolder(currentPath, pathTrail, true);
      } else if (
        window.confirm(
          `Delete ${entry.isFolder ? "folder" : "file"} “${entry.name}”? This cannot be undone.`,
        )
      ) {
        await deleteSnifferItem(path);
        scanCache.current.clear();
        await openFolder(currentPath, pathTrail, true);
      }
    } catch (actionError) {
      setError(
        actionError instanceof Error
          ? actionError.message
          : String(actionError),
      );
    }
  }

  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  const treemap = useMemo(() => {
    if (!result) return [];
    const folders = result.entries.filter((entry) => entry.isFolder);
    const files = result.entries.filter((entry) => !entry.isFolder);
    const mapEntries = showFilesInMap
      ? result.entries
      : folders.length > 0 && files.length > 0
        ? [
            ...folders,
            {
              name: "Loose files",
              path: `${result.path}\\__loose-files__`,
              size: files.reduce((sum, entry) => sum + entry.size, 0),
              isFolder: false,
              childCount: files.length,
            },
          ]
        : result.entries;
    const hidden = expandedOther ? [] : mapEntries.slice(mapLimit);
    const visible = expandedOther ? mapEntries : mapEntries.slice(0, mapLimit);
    if (hidden.length > 0) {
      visible.push({
        name: "Other items",
        path: `${result.path}\\__other__`,
        size: hidden.reduce((sum, entry) => sum + entry.size, 0),
        isFolder: false,
        childCount: hidden.length,
      });
    }
    return buildTreemap(visible);
  }, [expandedOther, result, showFilesInMap]);

  const openFolder = useCallback(
    async (path: string, nextTrail = pathTrail, forceRefresh = false) => {
      setCurrentPath(path);
      setPathTrail(nextTrail);
      setLoading(true);
      setError("");
      setScanProgress(null);

      const cached = scanCache.current.get(path);
      if (cached && !forceRefresh) {
        setResult(cached);
        setCurrentPath(cached.path);
        setPathTrail([...nextTrail.slice(0, -1), cached.path]);
        setShowingCached(true);
        setLoading(false);
        return;
      }

      try {
        const next = await scanFolderSizes(path);
        scanCache.current.set(next.path, next);
        setCurrentPath(next.path);
        setPathTrail([...nextTrail.slice(0, -1), next.path]);
        setResult(next);
        setShowingCached(false);
        setScanProgress(null);
      } catch (scanError) {
        setError(
          scanError instanceof Error ? scanError.message : String(scanError),
        );
      } finally {
        setLoading(false);
      }
    },
    [pathTrail],
  );

  async function chooseFolder() {
    const selected = await pickFolder();
    if (selected) await openFolder(selected, [selected]);
  }

  function toggleOtherItems() {
    setExpandedOther((expanded) => !expanded);
  }

  const goUpOneFolder = useCallback(() => {
    if (loading || pathTrail.length <= 1) return;
    void openFolder(pathTrail[pathTrail.length - 2], pathTrail.slice(0, -1));
  }, [loading, openFolder, pathTrail]);

  useEffect(() => {
    const handleNavigationBack = (event: KeyboardEvent) => {
      if (event.altKey && event.key === "ArrowLeft") {
        event.preventDefault();
        goUpOneFolder();
      }
    };
    const handleMouseBack = (event: MouseEvent) => {
      if (event.button === 3) {
        event.preventDefault();
        goUpOneFolder();
      }
    };
    const closeContextMenu = () => setContextMenu(null);
    window.addEventListener("keydown", handleNavigationBack);
    window.addEventListener("auxclick", handleMouseBack);
    window.addEventListener("click", closeContextMenu);
    return () => {
      window.removeEventListener("keydown", handleNavigationBack);
      window.removeEventListener("auxclick", handleMouseBack);
      window.removeEventListener("click", closeContextMenu);
    };
  }, [goUpOneFolder, loading, pathTrail]);

  useEffect(() => {
    setShowFilesInMap(false);
    setExpandedOther(false);
  }, [result?.path]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen<SnifferProgress>("sniffer://progress", (event) => {
      if (active) setScanProgress(event.payload);
    }).then((stop) => {
      if (active) unlisten = stop;
      else stop();
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (!contextMenu) return;
    const menu = document.querySelector<HTMLElement>(".sniffer-context-menu");
    const view = snifferViewRef.current;
    if (!menu || !view) return;
    const margin = 8;
    const maxX = view.scrollLeft + view.clientWidth - menu.offsetWidth - margin;
    const maxY =
      view.scrollTop + view.clientHeight - menu.offsetHeight - margin;
    const x = Math.max(view.scrollLeft + margin, Math.min(contextMenu.x, maxX));
    const y = Math.max(view.scrollTop + margin, Math.min(contextMenu.y, maxY));
    if (x !== contextMenu.x || y !== contextMenu.y) {
      setContextMenu({ ...contextMenu, x, y });
    }
  }, [contextMenu]);

  useEffect(() => {
    if (!toast) return;
    const timeout = window.setTimeout(() => setToast(""), 2200);
    return () => window.clearTimeout(timeout);
  }, [toast]);

  return (
    <main className="sniffer-view" ref={snifferViewRef}>
      <header className="workspace-header sniffer-header">
        <div>
          <h2>Folder sniffer</h2>
          <p>
            See where the space is going, then open any folder to inspect it.
          </p>
        </div>
        <button
          type="button"
          className="btn-primary"
          onClick={() => void chooseFolder()}
          disabled={loading}
        >
          {result ? "Choose another folder" : "Choose a folder"}
        </button>
      </header>

      {!result && !loading && !error && (
        <section className="sniffer-empty">
          <div className="sniffer-empty-icon" aria-hidden="true">
            ◒
          </div>
          <h3>Find your largest folders</h3>
          <p>Select a folder to map its files and subfolders by size.</p>
          <button
            type="button"
            className="btn-primary"
            onClick={() => void chooseFolder()}
          >
            Scan folder
          </button>
        </section>
      )}

      {loading && (
        <section
          className="sniffer-scan-progress"
          role="status"
          aria-live="polite"
        >
          <div className="sniffer-scan-progress-heading">
            <span className="sniffer-scan-spinner" aria-hidden="true" />
            <strong>
              Scanning folder…{" "}
              {scanProgress && scanProgress.total > 0
                ? `${Math.round((scanProgress.completed / scanProgress.total) * 100)}%`
                : ""}
            </strong>
          </div>
          <p>
            {scanProgress && scanProgress.total > 0
              ? `Completed ${scanProgress.completed.toLocaleString()} of ${scanProgress.total.toLocaleString()} top-level items.`
              : "Walking files and folders. Large drives may take a while."}
          </p>
          <div className="sniffer-progress-track" aria-label="Scan in progress">
            {scanProgress && scanProgress.total > 0 ? (
              <div
                className="sniffer-progress-determinate"
                style={{
                  width: `${(scanProgress.completed / scanProgress.total) * 100}%`,
                }}
              />
            ) : (
              <div className="sniffer-progress-indeterminate" />
            )}
          </div>
          <small>
            {scanProgress?.current
              ? shortPath(scanProgress.current)
              : currentPath && shortPath(currentPath)}
          </small>
        </section>
      )}
      {error && (
        <div className="sniffer-error" role="alert">
          <strong>Could not scan folder</strong>
          <p>{error}</p>
          <button
            type="button"
            onClick={() => void openFolder(currentPath, pathTrail, true)}
          >
            Retry
          </button>
        </div>
      )}

      {result && !loading && (
        <section
          className="sniffer-results"
          aria-label="Folder size visualization"
        >
          <div className="sniffer-breadcrumb">
            <div>
              <span className="sniffer-breadcrumb-label">Current folder</span>
              <nav className="sniffer-trail" aria-label="Folder path">
                {pathTrail.map((path, index) => (
                  <span className="sniffer-trail-segment" key={path}>
                    {index > 0 && (
                      <span
                        className="sniffer-trail-separator"
                        aria-hidden="true"
                      >
                        /
                      </span>
                    )}
                    {index === pathTrail.length - 1 ? (
                      <span
                        className="sniffer-breadcrumb-path"
                        title={path}
                        aria-current="location"
                      >
                        {folderName(path)}
                      </span>
                    ) : (
                      <button
                        type="button"
                        className="sniffer-trail-link"
                        onClick={() =>
                          void openFolder(path, pathTrail.slice(0, index + 1))
                        }
                        title={path}
                      >
                        {index === 0 ? shortPath(path) : folderName(path)}
                      </button>
                    )}
                  </span>
                ))}
              </nav>
            </div>
            <div className="sniffer-breadcrumb-actions">
              {pathTrail.length > 1 && (
                <button
                  type="button"
                  onClick={goUpOneFolder}
                  aria-label="Go up one folder"
                >
                  Back
                </button>
              )}
              {!result.path.endsWith("\\__other__") && (
                <button
                  type="button"
                  onClick={() => void openFolder(result.path, pathTrail, true)}
                >
                  Refresh scan
                </button>
              )}
            </div>
          </div>
          <div className="sniffer-summary">
            <div>
              <strong>{formatBytes(result.size)}</strong>
              <span>Total size</span>
            </div>
            <div>
              <strong>{result.folders.toLocaleString()}</strong>
              <span>Subfolders</span>
            </div>
            <div>
              <strong>{result.files.toLocaleString()}</strong>
              <span>Files</span>
            </div>
          </div>
          {result.folders === 0 && result.files > 0 && !showFilesInMap ? (
            <FileSizeView
              result={result}
              onShowInMap={() => setShowFilesInMap(true)}
              onEntryContextMenu={handleEntryContextMenu}
              contextEntryPath={contextMenu?.entry.path}
            />
          ) : (
            <>
              <div className="sniffer-map-header">
                <div>
                  <h3>Storage map</h3>
                  <p>
                    Larger areas represent more space. Click a folder to open
                    it, or expand the remaining items here.
                  </p>
                </div>
                <div className="sniffer-map-meta">
                  {showingCached && (
                    <span className="sniffer-cache-badge">Cached view</span>
                  )}
                  <span className="sniffer-detail-count">
                    Largest {mapLimit} items
                  </span>
                  {result.files > 0 && (
                    <button
                      type="button"
                      className="sniffer-files-toggle"
                      onClick={() => setShowFilesInMap((visible) => !visible)}
                    >
                      {showFilesInMap ? "Group files" : "Show files in map"}
                    </button>
                  )}
                  <div className="sniffer-legend" aria-label="Map legend">
                    <span>
                      <i className="legend-swatch folder" />
                      Folders
                    </span>
                    <span>
                      <i className="legend-swatch file" />
                      Files
                    </span>
                  </div>
                </div>
              </div>
              {result.entries.length === 0 ? (
                <p className="sniffer-empty-inline">This folder is empty.</p>
              ) : (
                <div className="sniffer-map">
                  {treemap.map(({ entry, left, top, width, height, tone }) => (
                    <div
                      className={`sniffer-tile ${entry.name === "Other items" ? "other" : entry.isFolder ? "folder" : "file"} tone-${tone} ${contextMenu?.entry.path === entry.path ? "context-selected" : ""}`}
                      key={entry.path}
                      style={
                        {
                          "--tile-left": `${left}%`,
                          "--tile-top": `${top}%`,
                          "--tile-width": `${width}%`,
                          "--tile-height": `${height}%`,
                        } as CSSProperties
                      }
                      onContextMenu={(event) => {
                        if (
                          entry.name !== "Other items" &&
                          entry.name !== "Loose files"
                        ) {
                          handleEntryContextMenu(event, entry);
                        }
                      }}
                    >
                      {entry.name === "Other items" ? (
                        <button
                          type="button"
                          className="sniffer-tile-button"
                          onClick={toggleOtherItems}
                          title="Expand the remaining items in this map"
                        >
                          <strong>Other items</strong>
                          <span>{formatBytes(entry.size)}</span>
                          <small>
                            {entry.childCount.toLocaleString()} items · click to
                            expand
                          </small>
                        </button>
                      ) : entry.isFolder ? (
                        <button
                          type="button"
                          className="sniffer-tile-button"
                          onClick={() =>
                            void openFolder(entry.path, [
                              ...pathTrail,
                              entry.path,
                            ])
                          }
                          title={`Open ${entry.name}`}
                        >
                          {width > 8 && height > 12 ? (
                            <>
                              <strong>{tileLabel(entry)}</strong>
                              <span>{formatBytes(entry.size)}</span>
                              {width > 13 && height > 17 && (
                                <small>
                                  {entry.childCount.toLocaleString()} items
                                </small>
                              )}
                            </>
                          ) : (
                            <span
                              className="sniffer-tile-glyph"
                              aria-hidden="true"
                            >
                              ›
                            </span>
                          )}
                        </button>
                      ) : (
                        <div className="sniffer-tile-content">
                          {width > 8 && height > 12 ? (
                            <>
                              <strong>{tileLabel(entry)}</strong>
                              <span>{formatBytes(entry.size)}</span>
                              {entry.childCount > 0 &&
                                width > 13 &&
                                height > 17 && (
                                  <small>
                                    {entry.childCount.toLocaleString()} more
                                    items
                                  </small>
                                )}
                            </>
                          ) : (
                            <span
                              className="sniffer-tile-glyph"
                              aria-hidden="true"
                            >
                              •
                            </span>
                          )}
                        </div>
                      )}
                    </div>
                  ))}
                </div>
              )}
              {!showFilesInMap && result.folders > 0 && result.files > 0 && (
                <FileSizeView
                  result={result}
                  title="Loose files"
                  description="Files stored directly in this folder are ranked separately from its subfolders."
                  onEntryContextMenu={handleEntryContextMenu}
                  contextEntryPath={contextMenu?.entry.path}
                />
              )}
            </>
          )}
          {result.skipped > 0 && (
            <p className="sniffer-warning">
              {result.skipped} item(s) could not be read and are not included.
            </p>
          )}
        </section>
      )}
      {contextMenu && (
        <div
          className="sniffer-context-menu"
          style={{ left: contextMenu.x, top: contextMenu.y }}
          role="menu"
          onClick={(event) => event.stopPropagation()}
        >
          <div
            className="sniffer-context-menu-title"
            title={contextMenu.entry.path}
          >
            {contextMenu.entry.name}
          </div>
          <button type="button" onClick={() => void runContextAction("open")}>
            {contextMenu.entry.isFolder ? "Open folder" : "Open file"}
          </button>
          <button type="button" onClick={() => void runContextAction("reveal")}>
            Show in Explorer
          </button>
          <button
            type="button"
            onClick={() => void runContextAction("copy-name")}
          >
            Copy filename
          </button>
          <button
            type="button"
            onClick={() => void runContextAction("copy-path")}
          >
            Copy full path
          </button>
          <button
            type="button"
            onClick={() => void runContextAction("properties")}
          >
            Properties
          </button>
          <div className="sniffer-context-menu-divider" />
          <button type="button" onClick={() => void runContextAction("rename")}>
            Rename
          </button>
          <button
            type="button"
            className="sniffer-context-danger"
            onClick={() => void runContextAction("delete")}
          >
            Delete
          </button>
        </div>
      )}
      {toast && (
        <div className="sniffer-toast" role="status">
          {toast}
        </div>
      )}
    </main>
  );
}
