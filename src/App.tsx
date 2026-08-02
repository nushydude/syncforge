import { useState } from "react";
import "./App.css";
import { DuplicatesView } from "./components/duplicates/DuplicatesView";
import { PairsPanel } from "./components/pairs/PairsPanel";
import { SettingsView } from "./components/settings/SettingsView";
import { useRunStore } from "./hooks/useRunStore";
import type { RunStoreState } from "./store/runStore";
import { FolderSnifferView } from "./components/sniffer/FolderSnifferView";

type AppView = "pairs" | "sniffer" | "duplicates" | "settings";

function NavIcon({ children }: { children: string }) {
  return <span className="nav-icon" aria-hidden="true">{children}</span>;
}

function App() {
  const [view, setView] = useState<AppView>("pairs");
  const running = useRunStore((s: RunStoreState) => s.running);

  return (
    <div className="app">
      <header className="app-header">
        <div className="app-header-row">
          <div>
            <div className="app-brand-line">
              <h1>SyncForge</h1>
              {import.meta.env.DEV && <span className="dev-build-badge">DEV BUILD</span>}
            </div>
            <p className="tagline">Modern folder sync for your desktop</p>
          </div>
          <nav className="app-nav" aria-label="Main">
            <button
              type="button"
              className={
                view === "pairs" ? "app-nav-btn active" : "app-nav-btn"
              }
              onClick={() => setView("pairs")}
              disabled={running}
            >
              <NavIcon>⇄</NavIcon>Sync folder pairs
            </button>
            <button
              type="button"
              className={view === "sniffer" ? "app-nav-btn active" : "app-nav-btn"}
              onClick={() => setView("sniffer")}
              disabled={running}
            >
              <NavIcon>◈</NavIcon>Folder sniffer
            </button>
            <button
              type="button"
              className={
                view === "duplicates" ? "app-nav-btn active" : "app-nav-btn"
              }
              onClick={() => setView("duplicates")}
              disabled={running}
            >
              <NavIcon>⊞</NavIcon>Duplicates
            </button>
            <span className="app-nav-divider" aria-hidden="true" />
            <button
              type="button"
              className={view === "settings" ? "app-nav-btn app-nav-settings active" : "app-nav-btn app-nav-settings"}
              onClick={() => setView("settings")}
              disabled={running}
            >
              <NavIcon>⚙</NavIcon>Settings
            </button>
          </nav>
        </div>
      </header>
      <div className="app-view" hidden={view !== "pairs"}>
        <PairsPanel />
      </div>
      <div className="app-view" hidden={view !== "sniffer"}>
        <FolderSnifferView />
      </div>
      <div className="app-view" hidden={view !== "duplicates"}>
        <DuplicatesView />
      </div>
      <div className="app-view" hidden={view !== "settings"}>
        <SettingsView />
      </div>
    </div>
  );
}

export default App;
