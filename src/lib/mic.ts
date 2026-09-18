import type { AudioInput, RecordingStatus, Settings } from "./ipc";

/**
 * What the microphone button / indicator on the Record bar shows: whether
 * sound is recorded, which microphone, and what is wrong when something is
 * (the chosen one is unplugged, there is none, access is off).
 */
export interface MicState {
  on: boolean;
  /** The microphone's name; null while unknown (the list has not loaded yet). */
  name: string | null;
  issue: string | null;
}

/** Said when the microphone is wanted but none is there (same words as Rust). */
export const NO_MIC = "No microphone is connected; recording without sound.";

/**
 * The state before a recording, from the settings (`mic`, `micDevice`,
 * `micDeviceName`) and the microphones present (`inputs`; null while they
 * are still being listed). Mirrors `audio::choose` in Rust, so the bar
 * says beforehand what the recording will do.
 */
export function micState(
  settings: Pick<Settings, "mic" | "micDevice" | "micDeviceName"> | null,
  inputs: AudioInput[] | null,
): MicState {
  // No settings (the overlay could not load them): Rust records with the
  // default, so say so.
  if (settings && !settings.mic) return { on: false, name: null, issue: null };
  if (!inputs) return { on: true, name: null, issue: null };
  const fallback = inputs.find((i) => i.default) ?? inputs[0];
  if (!fallback) return { on: true, name: null, issue: NO_MIC };
  const wanted = settings?.micDevice ?? "";
  if (!wanted) return { on: true, name: fallback.name, issue: null };
  const chosen = inputs.find((i) => i.id === wanted);
  if (chosen) return { on: true, name: chosen.name, issue: null };
  const label = settings?.micDeviceName || "The chosen microphone";
  return { on: true, name: fallback.name, issue: `${label} is not connected; recording with ${fallback.name}.` };
}

/** The state of a running recording, from what Rust reported when it started. */
export function micStateOf(status: Pick<RecordingStatus, "audio" | "audioIssue">): MicState {
  return { on: !!status.audio, name: status.audio ?? null, issue: status.audioIssue ?? null };
}

/** The tooltip of the mic button (before) or indicator (during a recording). */
export function micTitle(state: MicState, clickable: boolean): string {
  const hint = clickable ? (state.on ? " — click to record without sound" : " — click to record sound") : "";
  if (!state.on) return `Microphone off${state.issue ? `: ${state.issue}` : ""}${hint}`;
  if (state.issue) return `${state.issue}${hint}`;
  return `Microphone: ${state.name ?? "system default"}${hint}`;
}
