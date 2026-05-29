import "./App.css";
import { PairsPanel } from "./components/pairs/PairsPanel";

function App() {
  return (
    <div className="app">
      <header className="app-header">
        <h1>SyncForge</h1>
        <p className="tagline">Modern folder sync for your desktop</p>
      </header>
      <PairsPanel />
    </div>
  );
}

export default App;
