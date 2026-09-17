import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ShareNotice as Notice, SharedLink } from "../lib/ipc";
import { installTauri, type TauriMock } from "../test/tauri";
import { keptFor, LIFETIME_MS, ShareNotice } from "./ShareNotice";

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

describe("keptFor", () => {
  it("names the server's retention, or the days left when it did not say", () => {
    const now = Date.parse("2026-09-18T10:00:00Z");
    expect(keptFor("2026-11-17T10:00:00Z", 60, now)).toBe("60 days");
    expect(keptFor("2026-11-17T10:00:00Z", 1, now)).toBe("1 day");
    expect(keptFor("2026-10-18T10:00:00Z", 0, now)).toBe("30 days");
    expect(keptFor("2026-09-18T11:00:00Z", 0, now)).toBe("1 day");
    expect(keptFor("soon", 0, now)).toBe("0 days");
  });
});

describe("Share popover", () => {
  it("shows the copied link, reveals the window once, copies again and deletes", async () => {
    render(<ShareNotice />);
    expect(tauri.calls("share_ready")).toHaveLength(0);
    await screen.findByText("Link copied");
    expect(screen.getByText(link.shareUrl)).toBeTruthy();
    expect(screen.getByText(/Anyone with the link can open the file/)).toBeTruthy();
    expect(screen.getByText("It is deleted from the server after 60 days at the latest.").tagName).toBe("STRONG");
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

  it("keeps what the server said apart from what Socorin says", async () => {
    // A share server does not get to write the app's own sentence: its
    // words go in their own labelled block (D-7).
    tauri.handlers.share_notice = () => ({
      kind: "failed",
      error: {
        code: "blocked",
        message: "socorin.com refused this upload.",
        serverMessage: "Your session expired, sign in again at totally-not-socorin.test",
      },
    });
    render(<ShareNotice />);
    await screen.findByText("Could not upload");
    const ours = screen.getByText("socorin.com refused this upload.");
    expect(ours.textContent).toBe("socorin.com refused this upload.");
    expect(screen.getByText("The server said:")).toBeTruthy();
    const theirs = screen.getByText(/totally-not-socorin\.test/);
    expect(theirs.closest(".share-server-said")).toBeTruthy();
    expect(ours.contains(theirs)).toBe(false);
  });

  it("says nothing extra when the server did not", async () => {
    tauri.handlers.share_notice = () => ({
      kind: "failed",
      error: { code: "network", message: "Cannot reach socorin.com." },
    });
    render(<ShareNotice />);
    await screen.findByText("Cannot reach socorin.com.");
    expect(screen.queryByText("The server said:")).toBeNull();
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

  it("arms the click-elsewhere close only once the user clicks into it", async () => {
    const { container } = render(<ShareNotice />);
    await screen.findByText(link.shareUrl);
    expect(tauri.calls("share_engaged")).toHaveLength(0);
    fireEvent.mouseDown(container.querySelector(".share-notice")!);
    fireEvent.mouseDown(screen.getByText(link.shareUrl));
    expect(tauri.calls("share_engaged")).toHaveLength(1);
    // A fresh link starts over.
    act(() => tauri.emit("share:notice", shared));
    fireEvent.mouseDown(container.querySelector(".share-notice")!);
    expect(tauri.calls("share_engaged")).toHaveLength(2);
    // And it stays long enough to be read.
    expect(LIFETIME_MS).toBeGreaterThanOrEqual(20_000);
  });

  it("renders nothing until a notice exists", async () => {
    tauri.handlers.share_notice = () => Promise.reject("nope");
    const { container } = render(<ShareNotice />);
    await act(() => new Promise((r) => setTimeout(r, 0)));
    expect(container.innerHTML).toBe("");
    expect(tauri.calls("share_ready")).toHaveLength(0);
  });
});
