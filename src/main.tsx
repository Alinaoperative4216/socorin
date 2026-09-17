import { lazy, Suspense } from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import "./styles.css";
import { Overlay } from "./windows/Overlay";

// The editor pulls in Konva; keep it out of the overlay's critical path.
const Editor = lazy(() => import("./windows/Editor").then((m) => ({ default: m.Editor })));
const Settings = lazy(() => import("./windows/Settings").then((m) => ({ default: m.Settings })));
const Welcome = lazy(() => import("./windows/Welcome").then((m) => ({ default: m.Welcome })));
const Recorder = lazy(() => import("./windows/Recorder").then((m) => ({ default: m.Recorder })));
const UpdateNotice = lazy(() => import("./windows/UpdateNotice").then((m) => ({ default: m.UpdateNotice })));
const ShareNotice = lazy(() => import("./windows/ShareNotice").then((m) => ({ default: m.ShareNotice })));

const OVERLAY_PREFIX = "overlay-";

function Root({ label }: { label: string }) {
  const kind = label.startsWith(OVERLAY_PREFIX) ? "overlay" : label;
  document.documentElement.dataset.window = kind;
  if (kind === "overlay") {
    return <Overlay monitorId={Number(label.slice(OVERLAY_PREFIX.length))} />;
  }
  if (kind === "editor") {
    return (
      <Suspense fallback={null}>
        <Editor />
      </Suspense>
    );
  }
  if (kind === "welcome") {
    return (
      <Suspense fallback={null}>
        <Welcome />
      </Suspense>
    );
  }
  if (kind === "recorder") {
    return (
      <Suspense fallback={null}>
        <Recorder />
      </Suspense>
    );
  }
  if (kind === "update") {
    return (
      <Suspense fallback={null}>
        <UpdateNotice />
      </Suspense>
    );
  }
  if (kind === "share") {
    return (
      <Suspense fallback={null}>
        <ShareNotice />
      </Suspense>
    );
  }
  return (
    <Suspense fallback={null}>
      <Settings />
    </Suspense>
  );
}

async function boot() {
  // Dev-only: `?mock=<window label>` runs the UI in a plain browser (see devmock.ts).
  const mock = import.meta.env.DEV ? new URLSearchParams(location.search).get("mock") : null;
  if (mock) {
    (await import("./devmock")).installMock(mock);
  }
  const label = getCurrentWebviewWindow().label;
  ReactDOM.createRoot(document.getElementById("root")!).render(<Root label={label} />);
}

void boot();
