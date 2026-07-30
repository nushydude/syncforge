import { useEffect } from "react";
import { useSettingsStore } from "../../hooks/useSettingsStore";
import { updateAppSettings } from "../../store/settingsStore";
import type { ConflictPolicy } from "../../types";

const conflictPolicies: Array<{ value: ConflictPolicy; label: string }> = [
  { value: "newerWins", label: "Newer file wins" },
  { value: "left", label: "Always use left" },
  { value: "right", label: "Always use right" },
  { value: "keepBoth", label: "Keep both files" },
  { value: "ask", label: "Ask before resolving" },
];

export function SettingsView() {
  const settings = useSettingsStore();

  useEffect(() => {
    document.documentElement.dataset.theme = settings.theme;
    return () => {
      delete document.documentElement.dataset.theme;
    };
  }, [settings.theme]);

  return (
    <section className="settings-view" aria-labelledby="settings-title">
      <header className="workspace-header">
        <div>
          <h2 id="settings-title">Settings</h2>
          <p>Choose the defaults SyncForge uses for new pairs and manual sync runs.</p>
        </div>
      </header>

      <div className="settings-card">
        <h3>Sync defaults</h3>
        <label className="field">
          Default conflict policy for new pairs
          <select
            value={settings.defaultConflictPolicy}
            onChange={(event) =>
              updateAppSettings({
                defaultConflictPolicy: event.target.value as ConflictPolicy,
              })
            }
          >
            {conflictPolicies.map((policy) => (
              <option key={policy.value} value={policy.value}>
                {policy.label}
              </option>
            ))}
          </select>
        </label>
        <label className="field checkbox-field">
          <input
            type="checkbox"
            checked={settings.confirmBeforeRun}
            onChange={(event) =>
              updateAppSettings({ confirmBeforeRun: event.target.checked })
            }
          />
          Confirm before starting a manual sync
        </label>
        <label className="field checkbox-field">
          <input
            type="checkbox"
            checked={settings.moveDeletesToRecycleBin}
            onChange={(event) =>
              updateAppSettings({
                moveDeletesToRecycleBin: event.target.checked,
              })
            }
          />
          Move deleted files to the Recycle Bin
        </label>
        <label className="field checkbox-field">
          <input
            type="checkbox"
            checked={settings.verifyHashesAfterCopy}
            onChange={(event) =>
              updateAppSettings({ verifyHashesAfterCopy: event.target.checked })
            }
          />
          Verify file hashes after copying
        </label>
      </div>

      <div className="settings-card">
        <h3>Appearance</h3>
        <label className="field">
          Theme
          <select
            value={settings.theme}
            onChange={(event) =>
              updateAppSettings({
                theme: event.target.value as typeof settings.theme,
              })
            }
          >
            <option value="system">System default</option>
            <option value="light">Light</option>
            <option value="dark">Dark</option>
          </select>
        </label>
      </div>

      <p className="settings-saved" role="status">
        Settings are saved automatically on this computer when local storage is available.
      </p>
    </section>
  );
}
