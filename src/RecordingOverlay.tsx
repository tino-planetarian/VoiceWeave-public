import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import "./RecordingOverlay.css";

const BAR_SHAPES = [0.58, 0.85, 1, 0.75, 0.52];
// "hidden": blank (fully transparent), drawn just before the window hides, so the next
// showing never starts with the previous picture.
type OverlayPhase = "recording" | "transcribing" | "polishing" | "notice" | "hidden";

export default function RecordingOverlay() {
  const [level, setLevel] = useState(0);
  // Start blank: if a notice arrives before this page listens, never show a false "録音中".
  const [phase, setPhase] = useState<OverlayPhase>("hidden");
  const [message, setMessage] = useState("");
  // Text confirmed so far while speaking (only when that option is on).
  const [preview, setPreview] = useState("");
  // A guess of the part still being spoken (speed-first live text); it may still change.
  const [guess, setGuess] = useState("");
  // The recording goes to the cloud (cloud recognition mode): said the whole time.
  const [cloud, setCloud] = useState(false);
  // Recording started with the English shortcut: said the whole time too.
  const [english, setEnglish] = useState(false);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    let unlistenPhase: (() => void) | undefined;
    let unlistenNotice: (() => void) | undefined;
    let unlistenText: (() => void) | undefined;
    let unlistenGuess: (() => void) | undefined;
    let unlistenMode: (() => void) | undefined;
    void listen<string>("overlay_mode", (event) => {
      if (active) {
        setCloud(event.payload.startsWith("cloud"));
        setEnglish(event.payload.endsWith("english"));
      }
    }).then((off) => {
      if (active) unlistenMode = off;
      else off();
    });
    void listen<string>("overlay_notice", (event) => {
      if (active && typeof event.payload === "string") {
        setMessage(event.payload);
        setPhase("notice");
      }
    }).then((off) => {
      if (active) unlistenNotice = off;
      else off();
    });
    void listen<number>("audio_level", (event) => {
      if (active && Number.isFinite(event.payload)) {
        setLevel(Math.min(1, Math.max(0, event.payload)));
      }
    }).then((off) => {
      if (active) unlisten = off;
      else off();
    });
    void listen<string>("overlay_text", (event) => {
      if (active && typeof event.payload === "string") setPreview(event.payload);
    }).then((off) => {
      if (active) unlistenText = off;
      else off();
    });
    void listen<string>("overlay_guess", (event) => {
      if (active && typeof event.payload === "string") setGuess(event.payload);
    }).then((off) => {
      if (active) unlistenGuess = off;
      else off();
    });
    void listen<OverlayPhase>("overlay_phase", (event) => {
      if (active && event.payload === "hidden") {
        setPreview(""); setGuess(""); setMessage("");
        setPhase("hidden");
      } else if (active && (event.payload === "recording" || event.payload === "transcribing" || event.payload === "polishing")) {
        if (event.payload === "recording") { setPreview(""); setGuess(""); }
        // After stop the guess is replaced by the final text, not kept on screen.
        if (event.payload !== "recording") setGuess("");
        setPhase(event.payload);
      }
    }).then((off) => {
      if (active) unlistenPhase = off;
      else off();
    });
    return () => { active = false; unlisten?.(); unlistenPhase?.(); unlistenNotice?.(); unlistenText?.(); unlistenGuess?.(); unlistenMode?.(); };
  }, []);

  const activity = Math.min(1, Math.sqrt(level) * 2.2);
  if (phase === "hidden") return <div className="overlay-stack" />;

  return (
    <div className="overlay-stack">
    {(preview || guess) && phase !== "notice" && <div className="overlay-preview"><span>{preview}<span className="overlay-guess">{guess}</span></span></div>}
    <div className={phase === "notice" ? "recording-pill notice-pill" : "recording-pill"} role="status"
      aria-label={phase === "recording" ? "録音中" : phase === "transcribing" ? "書き起こし中" : phase === "polishing" ? "整形中" : message}>
      {phase === "notice" ? (
        <span className="notice-label">{message}</span>
      ) : phase === "recording" ? (
        <>
          <span className="recording-dot" aria-hidden="true" />
          <span className="recording-label">録音中</span>
          {cloud && <span className="cloud-badge">クラウド</span>}
          {english && <span className="cloud-badge">English</span>}
          <div className="recording-meter" aria-hidden="true">
            {BAR_SHAPES.map((shape, index) => (
              <span
                className="recording-bar"
                key={index}
                style={{ height: `${5 + Math.round(activity * shape * 23)}px` }}
              />
            ))}
          </div>
        </>
      ) : (
        <>
          <span className="transcribing-label">{phase === "polishing" ? "整形中" : "書き起こし中"}</span>
          {cloud && phase === "transcribing" && <span className="cloud-badge">クラウド</span>}
          {english && phase === "transcribing" && <span className="cloud-badge">English</span>}
          <div className="transcribing-dots" aria-hidden="true">
            {[0, 1, 2, 3].map((index) => <span className="transcribing-dot" key={index} />)}
          </div>
        </>
      )}
    </div>
    </div>
  );
}
