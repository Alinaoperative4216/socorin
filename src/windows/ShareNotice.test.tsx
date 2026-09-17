import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ShareNotice as Notice, SharedLink } from "../lib/ipc";
import { installTauri, type TauriMock } from "../test/tauri";
import { expiresIn, LIFETIME_MS, ShareNotice } from "./ShareNotice";

let tauri: TauriMock;
const DAY = 86_400_000;
const link: SharedLink = {
  id: "med-0123456789abcdefgh",
  shareUrl: "https://socorin.com/s/med-0123456789abcdefgh",
  expiresAt: new Date(Date.now() + 60 * DAY).toISOString(),
  kind: "image",
  mime: "image/png",
  size: 1000,
  createdAt: 0,
};
const shared: Notice = { kind: "shared", link, retentionDays: 60 };

beforeEach(() => {
  tauri = installTauri("share", { share_notice: () => shared });
});

describe("expiresIn", () => {
  it("counts the days from the ISO date, falling back to the retention", () => {
    const now = Date.parse("2026-09-18T10:00:00Z");
    expect(expiresIn("2026-11-17T10:00:00Z", 60, now)).toBe("expires in 60 days");
    expect(expiresIn("2026-11-17T09:00:00Z", 60, now)).toBe("expires in 60 days");
    expect(expiresIn("2026-09-19T09:00:00Z", 60, now)).toBe("expires in 1 day");
    expect(expiresIn("2026-09-18T09:00:00Z", 60, now)).toBe("expires today");
    expect(expiresIn("soon", 60, now)).toBe("expires in 60 days");
    expect(expiresIn("soon", 0, now)).toBe("expires today");
  });
});

describe("Share popover", () => {
  it("shows the copied link, reveals the window once, copies again and deletes", async () => {
    render(<ShareNotice />);
    expect(tauri.calls("share_ready")).toHaveLength(0);
    await screen.findByText("Link copied · expires in 60 days");
    expect(screen.getByText(link.shareUrl)).toBeTruthy();
    expect(screen.getByText(/Paste it anywhere/)).toBeTruthy();
    await waitFor(() => expect(tauri.calls("share_ready")).toHaveLength(1));

    fireEvent.click(screen.getByTitle("Copy the link again"));
    await screen.findByText("Copied again.");
    expect(tauri.calls("copy_share_link")).toEqual([{ id: link.id }]);

    fireEvent.click(screen.getByText("Delete from server"));
    await screen.findByText("Deleted from server");
    expect(tauri.calls("delete_share")).toEqual([{ id: link.id }]);
    fireEvent.click(screen.getByText("Close"));
    expect(tauri.calls("dismiss_share")).toHaveLength(1);

    // The next link arrives by event; the window is not revealed twice.
    act(() => tauri.emit("share:notice", { ...shared, link: { ...link, id: "n", shareUrl: "https://socorin.com/s/n" } }));
    expect(screen.getByText("https://socorin.com/s/n")).toBeTruthy();
    expect(screen.queryByText("Deleted from server")).toBeNull();
    expect(tauri.calls("share_ready")).toHaveLength(1);
  });

  it("reports a refused delete and a failed copy without losing the link", async () => {
    tauri.handlers.delete_share = () => Promise.reject({ code: "forbidden", message: "socorin.com refused this upload." });
    tauri.handlers.copy_share_link = () => Promise.reject("This link is no longer in the list.");
    render(<ShareNotice />);
    await screen.findByText(link.shareUrl);
    fireEvent.click(screen.getByText("Delete from server"));
    await screen.findByText("socorin.com refused this upload.");
    expect(screen.getByText(link.shareUrl)).toBeTruthy();
    fireEvent.click(screen.getByTitle("Copy the link again"));
    await screen.findByText("This link is no longer in the list.");
  });

  it("explains a failed upload", async () => {
    tauri.handlers.share_notice = () => ({
      kind: "failed",
      error: { code: "file_too_large", message: "This capture is 7.3 MB; the share limit is 5 MB.", maxBytes: 5_242_880 },
    });
    render(<ShareNotice />);
    await screen.findByText("Could not upload");
    expect(screen.getByText("This capture is 7.3 MB; the share limit is 5 MB.")).toBeTruthy();
    expect(screen.queryByText("Delete from server")).toBeNull();
    fireEvent.click(screen.getByText("Close"));
    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.keyDown(window, { key: "a" });
    expect(tauri.calls("dismiss_share")).toHaveLength(2);
  });

  it("goes away by itself unless the mouse is over it", async () => {
    vi.useFakeTimers();
    const { container } = render(<ShareNotice />);
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(screen.getByText(link.shareUrl)).toBeTruthy();
    await act(() => vi.advanceTimersByTimeAsync(LIFETIME_MS - 100));
    expect(tauri.calls("dismiss_share")).toHaveLength(0);
    await act(() => vi.advanceTimersByTimeAsync(100));
    expect(tauri.calls("dismiss_share")).toHaveLength(1);

    // A fresh link restarts the clock; hovering holds it.
    act(() => tauri.emit("share:notice", shared));
    fireEvent.mouseEnter(container.querySelector(".share-notice")!);
    await act(() => vi.advanceTimersByTimeAsync(LIFETIME_MS * 2));
    expect(tauri.calls("dismiss_share")).toHaveLength(1);
    fireEvent.mouseLeave(container.querySelector(".share-notice")!);
    await act(() => vi.advanceTimersByTimeAsync(LIFETIME_MS));
    expect(tauri.calls("dismiss_share")).toHaveLength(2);
  });

  it("renders nothing until a notice exists", async () => {
    tauri.handlers.share_notice = () => Promise.reject("nope");
    const { container } = render(<ShareNotice />);
    await act(() => new Promise((r) => setTimeout(r, 0)));
    expect(container.innerHTML).toBe("");
    expect(tauri.calls("share_ready")).toHaveLength(0);
  });
});
