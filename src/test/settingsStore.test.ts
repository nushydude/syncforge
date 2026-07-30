import { beforeEach, describe, expect, it } from "vitest";
import {
  getAppSettings,
  reloadSettingsForTests,
  resetSettingsForTests,
  updateAppSettings,
} from "../store/settingsStore";

describe("settingsStore", () => {
  beforeEach(() => {
    window.localStorage.clear();
    resetSettingsForTests();
  });

  it("persists updates and merges them with defaults", () => {
    updateAppSettings({ verifyHashesAfterCopy: true, theme: "dark" });
    reloadSettingsForTests();
    expect(getAppSettings()).toMatchObject({
      verifyHashesAfterCopy: true,
      theme: "dark",
      moveDeletesToRecycleBin: true,
    });
    expect(window.localStorage.getItem("syncforge.appSettings")).toContain(
      '"theme":"dark"',
    );
  });

  it("reloads persisted values and rejects invalid values", () => {
    window.localStorage.setItem(
      "syncforge.appSettings",
      JSON.stringify({ theme: "neon", confirmBeforeRun: "yes" }),
    );
    reloadSettingsForTests();
    expect(getAppSettings().theme).toBe("system");
    expect(getAppSettings().confirmBeforeRun).toBe(true);
  });
});
