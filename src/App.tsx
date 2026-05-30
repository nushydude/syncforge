import { useState } from "react";
import "./App.css";
import { HistoryView } from "./components/history/HistoryView";
import { PairsPanel } from "./components/pairs/PairsPanel";

type AppView = "pairs" | "history";

function App() {
  const [view, setView] = useState<AppView>("pairs");

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
              className={view === "pairs" ? "app-nav-btn active" : "app-nav-btn"}
              onClick={() => setView("pairs")}
            >
              Pairs
            </button>
            <button
              type="button"
              className={
                view === "history" ? "app-nav-btn active" : "app-nav-btn"
              }
              onClick={() => setView("history")}
            >
              History
            </button>
          </nav>
        </div>
      </header>
      <div hidden={view !== "pairs"}>
        <PairsPanel />
      </div>
      <div hidden={view !== "history"}>
        <HistoryView active={view === "history"} />
      </div>
    </div>
  );
}

export default App;
