import { useState } from "react";
import "./App.css";
import { HistoryView } from "./components/history/HistoryView";
import { DuplicatesView } from "./components/duplicates/DuplicatesView";
import { PairsPanel } from "./components/pairs/PairsPanel";
import { SettingsView } from "./components/settings/SettingsView";
import { useRunStore } from "./hooks/useRunStore";
import type { RunStoreState } from "./store/runStore";

type AppView = "pairs" | "duplicates" | "history" | "settings";

function App() {
  const [view, setView] = useState<AppView>("pairs");
  const running = useRunStore((s: RunStoreState) => s.running);

  return (
    <div className="app">
      <header className="app-header">
        <div className="app-header-row">
          <div>
            <h1>SyncForge</h1>
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
              Pairs
            </button>
            <button
              type="button"
              className={
                view === "history" ? "app-nav-btn active" : "app-nav-btn"
              }
              onClick={() => setView("history")}
              disabled={running}
            >
              History
            </button>
            <button
              type="button"
              className={
                view === "duplicates" ? "app-nav-btn active" : "app-nav-btn"
              }
              onClick={() => setView("duplicates")}
              disabled={running}
            >
              Duplicates
            </button>
            <button
              type="button"
              className={
                view === "settings" ? "app-nav-btn active" : "app-nav-btn"
              }
              onClick={() => setView("settings")}
              disabled={running}
            >
              Settings
            </button>
          </nav>
        </div>
      </header>
      <div className="app-view" hidden={view !== "pairs"}>
        <PairsPanel />
      </div>
      <div className="app-view" hidden={view !== "history"}>
        <HistoryView active={view === "history"} />
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
