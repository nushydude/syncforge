import { useSyncExternalStore } from "react";
import { getAppSettings, subscribeSettings } from "../store/settingsStore";

export function useSettingsStore() {
  return useSyncExternalStore(subscribeSettings, getAppSettings, getAppSettings);
}
