import {
  APP_SETTINGS_STORAGE_KEY,
  defaultAppSettings,
  type AppSettings,
} from "../types";

type Listener = () => void;

let state: AppSettings = loadSettings();
const listeners = new Set<Listener>();

function loadSettings(): AppSettings {
  if (typeof window === "undefined") return defaultAppSettings();
  try {
    const saved = window.localStorage.getItem(APP_SETTINGS_STORAGE_KEY);
    if (!saved) return defaultAppSettings();
    const parsed: unknown = JSON.parse(saved);
    if (!isRecord(parsed)) return defaultAppSettings();
    const defaults = defaultAppSettings();
    return {
      defaultConflictPolicy: isConflictPolicy(parsed.defaultConflictPolicy)
        ? parsed.defaultConflictPolicy
        : defaults.defaultConflictPolicy,
      confirmBeforeRun:
        typeof parsed.confirmBeforeRun === "boolean"
          ? parsed.confirmBeforeRun
          : defaults.confirmBeforeRun,
      moveDeletesToRecycleBin:
        typeof parsed.moveDeletesToRecycleBin === "boolean"
          ? parsed.moveDeletesToRecycleBin
          : defaults.moveDeletesToRecycleBin,
      verifyHashesAfterCopy:
        typeof parsed.verifyHashesAfterCopy === "boolean"
          ? parsed.verifyHashesAfterCopy
          : defaults.verifyHashesAfterCopy,
      theme: isTheme(parsed.theme) ? parsed.theme : defaults.theme,
    };
  } catch {
    return defaultAppSettings();
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isConflictPolicy(
  value: unknown,
): value is AppSettings["defaultConflictPolicy"] {
  return (
    value === "newerWins" ||
    value === "left" ||
    value === "right" ||
    value === "keepBoth" ||
    value === "ask"
  );
}

function isTheme(value: unknown): value is AppSettings["theme"] {
  return value === "system" || value === "light" || value === "dark";
}

function emit() {
  listeners.forEach((listener) => listener());
}

export function getAppSettings(): AppSettings {
  return state;
}

export function subscribeSettings(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function updateAppSettings(patch: Partial<AppSettings>): void {
  state = { ...state, ...patch };
  if (typeof window !== "undefined") {
    try {
      window.localStorage.setItem(
        APP_SETTINGS_STORAGE_KEY,
        JSON.stringify(state),
      );
    } catch {
      // Settings still apply for the current session if storage is unavailable.
    }
  }
  emit();
}

export function resetSettingsForTests(): void {
  state = defaultAppSettings();
  emit();
}

export function reloadSettingsForTests(): void {
  state = loadSettings();
  emit();
}
