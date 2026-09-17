import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import type { UpdateStatus } from "../lib/ipc";
import { installTauri, UPDATE_STATUS, type TauriMock } from "../test/tauri";
import { formatBytes, UpdateNotice } from "./UpdateNotice";

let tauri: TauriMock;
const available: UpdateStatus = { ...UPDATE_STATUS, available: "1.1.0", checkedAt: 1_800_000_000 };

beforeEach(() => {
  tauri = installTauri("update", { update_status: () => available });
});

describe("Update popover", () => {
  it("offers the new version, then asks Rust to show the window once rendered", async () => {
    render(<UpdateNotice />);
    expect(tauri.calls("update_ready")).toHaveLength(0);
    await screen.findByText("Socorin 1.1.0 is available");
    expect(screen.getByText(/You have 1.0.2/)).toBeTruthy();
    await waitFor(() => expect(tauri.calls("update_ready")).toHaveLength(1));
    fireEvent.click(screen.getByText("Update now"));
    expect(tauri.calls("install_update")).toHaveLength(1);
    fireEvent.click(screen.getByText("Later"));
    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.keyDown(window, { key: "a" });
    expect(tauri.calls("dismiss_update")).toHaveLength(2);
    // A later status arrives by event; the window is not shown twice.
    act(() => tauri.emit("update:status", { ...available, available: "1.2.0" }));
    expect(screen.getByText("Socorin 1.2.0 is available")).toBeTruthy();
    expect(tauri.calls("update_ready")).toHaveLength(1);
  });

  it("shows the download progress with and without a known size", async () => {
    tauri.handlers.update_status = () => ({ ...available, phase: { phase: "installing", received: 2_600_000, total: 5_200_000 } });
    render(<UpdateNotice />);
    await screen.findByText("Updating to Socorin 1.1.0…");
    expect(screen.getByText(/50% – 2.6 MB of 5.2 MB/)).toBeTruthy();
    expect((screen.getByRole("progressbar").firstElementChild as HTMLElement).style.width).toBe("50%");
    act(() => tauri.emit("update:status", { ...available, phase: { phase: "installing", received: 700_000, total: 0 } }));
    expect(screen.getByText(/700 KB downloaded/)).toBeTruthy();
    expect(screen.getByRole("progressbar").firstElementChild!.className).toBe("indeterminate");
    expect(screen.queryByText("Later")).toBeNull();
    expect(formatBytes(999)).toBe("999 B");
  });

  it("explains a failure and opens the download page", async () => {
    tauri.handlers.update_status = () => ({ ...available, phase: { phase: "failed", error: "No installer for this platform" } });
    render(<UpdateNotice />);
    await screen.findByText("Could not update Socorin");
    expect(screen.getByText("No installer for this platform")).toBeTruthy();
    fireEvent.click(screen.getByText("Open download page"));
    expect(tauri.calls("plugin:opener|open_url")[0]).toMatchObject({ url: "https://socorin.com/#download" });
    fireEvent.click(screen.getByText("Close"));
    expect(tauri.calls("dismiss_update")).toHaveLength(2);
  });

  it("says so once after an update, and when nothing is newer", async () => {
    tauri.handlers.update_status = () => ({ ...UPDATE_STATUS, justUpdated: true });
    render(<UpdateNotice />);
    await screen.findByText("Socorin was updated to 1.0.2");
    fireEvent.click(screen.getByText("OK"));
    expect(tauri.calls("dismiss_update")).toHaveLength(1);
    act(() => tauri.emit("update:status", UPDATE_STATUS));
    expect(screen.getByText("Socorin is up to date")).toBeTruthy();
  });

  it("renders nothing until the status is known", async () => {
    tauri.handlers.update_status = () => Promise.reject("nope");
    const { container } = render(<UpdateNotice />);
    await act(() => new Promise((r) => setTimeout(r, 0)));
    expect(container.innerHTML).toBe("");
    expect(tauri.calls("update_ready")).toHaveLength(0);
  });
});
