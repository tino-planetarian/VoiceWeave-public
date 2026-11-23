import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

// Shown after several recordings made while the PC was short of memory (see
// src-tauri/src/memory_gate.rs). Not answering counts as "leave as it is".
const ASK_SECONDS = 40;

export default function MemoryPrompt() {
  const [problem, setProblem] = useState("");
  const answer = (switchToRules: boolean) => {
    // The switch is refused while a recording is being processed: say so and stay open,
    // so the click can be repeated (the window closes by itself after the time is up).
    invoke("answer_memory_prompt", { switchToRules }).catch((error) =>
      setProblem(`切り替えられませんでした（${String(error)}）。少し待って、もう一度押してください。`));
  };
  useEffect(() => {
    document.body.style.margin = "0";
    document.body.style.background = "#14202b";
    const timer = window.setTimeout(() => answer(false), ASK_SECONDS * 1000);
    return () => window.clearTimeout(timer);
  }, []);
  const button = {
    border: "1px solid #5b7a96", borderRadius: 6, padding: "6px 10px", color: "#e8f0f7",
    background: "#1f3447", cursor: "pointer", font: "inherit",
  } as const;
  return <div role="alertdialog" aria-label="メモリ不足の確認" style={{
    boxSizing: "border-box", height: "100vh", padding: 14, color: "#e8f0f7", background: "#14202b",
    font: "13px/1.5 'Segoe UI', 'Yu Gothic UI', sans-serif", display: "flex", flexDirection: "column", gap: 10,
  }}>
    <div>
      <strong>パソコンのメモリが足りない状態が続いています。</strong>
      <div style={{ opacity: 0.85 }}>句読点のAIをやめて、「規則」に切り替えますか？（メモリの使用を抑えます）</div>
      {problem && <div role="status" style={{ marginTop: 4, color: "#ffb4a8" }}>{problem}</div>}
    </div>
    <div style={{ display: "flex", gap: 8, marginTop: "auto" }}>
      <button type="button" style={button} onClick={() => answer(true)}>規則にする</button>
      <button type="button" style={button} onClick={() => answer(false)}>そのままにする</button>
    </div>
  </div>;
}
