import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { isMac, prettyShortcut } from "../lib/hotkey";
import { flush, installTauri, SETTINGS, type TauriMock } from "../test/tauri";
import { Welcome } from "./Welcome";

let tauri: TauriMock;

beforeEach(() => {
  tauri = installTauri("welcome", { get_settings: () => SETTINGS });
});

describe("Welcome dialog", () => {
  it("shows the shortcut once known, then asks Rust to show the window", async () => {
    render(<Welcome />);
    expect(screen.getByText("the shortcut")).toBeTruthy();
    await screen.findByText(prettyShortcut(SETTINGS.hotkey));
    await waitFor(() => expect(tauri.calls("welcome_ready")).toHaveLength(1));
    expect(screen.getByText(new RegExp(isMac ? "menu bar" : "system tray"))).toBeTruthy();
    expect(screen.getByText(new RegExp(isMac ? "top right" : "next to the clock"))).toBeTruthy();
    expect((screen.getByAltText("") as HTMLImageElement).getAttribute("src")).toMatch(/logo/);
  });

  it("still shows itself when the settings cannot be read", async () => {
    tauri.handlers.get_settings = () => Promise.reject("nope");
    render(<Welcome />);
    await waitFor(() => expect(tauri.calls("welcome_ready")).toHaveLength(1));
    expect(screen.getByText("the shortcut")).toBeTruthy();
  });

  it("dismisses with the button, Enter or Escape", async () => {
    render(<Welcome />);
    await flush();
    fireEvent.click(screen.getByText("OK"));
    fireEvent.keyDown(window, { key: "Enter" });
    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.keyDown(window, { key: "a" });
    expect(tauri.calls("dismiss_welcome")).toHaveLength(3);
  });
});
