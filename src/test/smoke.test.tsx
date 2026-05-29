import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import App from "../App";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue([]),
}));

describe("App", () => {
  it("renders SyncForge with folder pairs UI", async () => {
    render(<App />);
    expect(screen.getByRole("heading", { name: /syncforge/i })).toBeInTheDocument();
    await waitFor(() => {
      expect(
        screen.getByRole("heading", { name: /folder pairs/i }),
      ).toBeInTheDocument();
    });
  });
});
