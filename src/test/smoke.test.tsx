import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import App from "../App";

describe("App", () => {
  it("renders SyncForge placeholder", () => {
    render(<App />);
    expect(screen.getByRole("heading", { name: /syncforge/i })).toBeInTheDocument();
  });
});
