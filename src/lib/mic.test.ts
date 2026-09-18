import { describe, expect, it } from "vitest";
import { micState, micStateOf, micTitle, NO_MIC } from "./mic";

const inputs = [
  { id: "loop", name: "Microsoft Teams Audio", default: false },
  { id: "builtin", name: "MacBook Pro Microphone", default: true },
  { id: "usb:1", name: "Jabra Speak 710", default: false },
];

describe("micState", () => {
  it("is off when the setting is, whatever is connected", () => {
    const off = { mic: false, micDevice: "usb:1", micDeviceName: "Jabra" };
    expect(micState(off, inputs)).toEqual({ on: false, name: null, issue: null });
    expect(micState(off, null)).toEqual({ on: false, name: null, issue: null });
  });

  it("names the default, or the chosen microphone, once the list is in", () => {
    const dflt = { mic: true, micDevice: "", micDeviceName: "" };
    expect(micState(dflt, null)).toEqual({ on: true, name: null, issue: null });
    expect(micState(dflt, inputs)).toEqual({ on: true, name: "MacBook Pro Microphone", issue: null });
    // No default flagged: the first one, as in Rust.
    expect(micState(dflt, inputs.map((i) => ({ ...i, default: false }))).name).toBe("Microsoft Teams Audio");
    expect(micState({ mic: true, micDevice: "usb:1", micDeviceName: "Jabra Speak 710" }, inputs)).toEqual({
      on: true,
      name: "Jabra Speak 710",
      issue: null,
    });
    // No settings at all: Rust records with the default, so say so.
    expect(micState(null, inputs)).toEqual({ on: true, name: "MacBook Pro Microphone", issue: null });
  });

  it("says when the chosen microphone is unplugged or none is there", () => {
    const chosen = { mic: true, micDevice: "usb:1", micDeviceName: "Jabra Speak 710" };
    expect(micState(chosen, inputs.slice(0, 2))).toEqual({
      on: true,
      name: "MacBook Pro Microphone",
      issue: "Jabra Speak 710 is not connected; recording with MacBook Pro Microphone.",
    });
    expect(micState({ mic: true, micDevice: "gone", micDeviceName: "" }, inputs).issue).toBe(
      "The chosen microphone is not connected; recording with MacBook Pro Microphone.",
    );
    expect(micState(chosen, [])).toEqual({ on: true, name: null, issue: NO_MIC });
  });

  it("reads a running recording's sound from its status", () => {
    expect(micStateOf({ audio: "Jabra Speak 710", audioIssue: null })).toEqual({ on: true, name: "Jabra Speak 710", issue: null });
    expect(micStateOf({ audio: null, audioIssue: "Microphone access is off." })).toEqual({
      on: false,
      name: null,
      issue: "Microphone access is off.",
    });
    expect(micStateOf({})).toEqual({ on: false, name: null, issue: null });
  });

  it("words the tooltip for the button and for the indicator", () => {
    expect(micTitle({ on: true, name: "Jabra", issue: null }, true)).toBe("Microphone: Jabra — click to record without sound");
    expect(micTitle({ on: true, name: null, issue: null }, false)).toBe("Microphone: system default");
    expect(micTitle({ on: false, name: null, issue: null }, true)).toBe("Microphone off — click to record sound");
    expect(micTitle({ on: false, name: null, issue: "Access is off." }, false)).toBe("Microphone off: Access is off.");
    expect(micTitle({ on: true, name: "Built-in", issue: "Jabra is not connected; recording with Built-in." }, true)).toBe(
      "Jabra is not connected; recording with Built-in. — click to record without sound",
    );
  });
});
