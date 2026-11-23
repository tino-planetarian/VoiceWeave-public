// Render the actual component with synthetic metadata. No browser/Tauri or mic.
const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const ts = require('typescript');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const filename = path.join(__dirname, '../src/DiagnosticsPanel.tsx');
const compiled = ts.transpileModule(fs.readFileSync(filename, 'utf8'), {
  compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS },
}).outputText;
const component = {};
// The only evaluated input is the checked-in component, never diagnostic data.
new Function('require', 'exports', compiled)(require, component);
const render = (snapshot) => renderToStaticMarkup(React.createElement(component.DiagnosticsPanel, { snapshot }));
const latest = {
  attempt: 2, model: 'standard', outcome: 'sent_unverified',
  timings: { stop_to_complete_ms: 9000, stream_stop_ms: 0, conversion_ms: null },
  startup_xruns: 0,
};

test('zero is a measurement, null is unknown, and sent input is unverified', () => {
  const html = render({ revision: 2, latest, last_error: null });
  assert.match(html, /9\.000 秒/);
  assert.match(html, /0\.000 秒/);
  assert.match(html, /未計測/);
  assert.match(html, /挿入は未確認/);
  assert.doesNotMatch(html, /挿入成功/);
});

test('prior error remains visible alongside a later success and has no speech field', () => {
  const html = render({ revision: 3, latest, last_error: {
    attempt: 1, stage: 'capture_start', code: 'audio_build', backend_kind: 'device_busy',
  } });
  assert.match(html, /試行 1/);
  assert.match(html, /試行 2/);
  assert.match(html, /audio_build \/ device_busy/);
  assert.doesNotMatch(html, /raw_text|dictionary|window_title/);
});

test('cold snapshot does not invent a completed attempt', () => {
  const html = render({ revision: 0, latest: null, last_error: null });
  assert.match(html, /まだ処理時間の記録はありません/);
  assert.doesNotMatch(html, /0\.000 秒/);
});

test('late initial snapshot cannot erase a more recent event', () => {
  const event = { revision: 3, latest, last_error: null };
  const initial = { revision: 1, latest: null, last_error: null };
  assert.equal(component.newerDiagnostics(event, initial), event);
  assert.equal(component.newerDiagnostics(null, initial), initial);
  assert.equal(component.newerDiagnostics(initial, event), event);
});

test('recognition path is shown with a plain label and is optional', () => {
  assert.match(render({ revision: 4, latest: { ...latest, recognition_path: 'short' }, last_error: null }), /高速処理（録音の長さだけを処理）/);
  assert.match(render({ revision: 5, latest: { ...latest, recognition_path: 'short_fallback' }, last_error: null }), /通常処理（30秒分）に切り替え/);
  assert.doesNotMatch(render({ revision: 6, latest, last_error: null }), /処理方式/);
  assert.match(render({ revision: 7, latest: { ...latest, recognition_path: 'short_fallback', fallback_reason: 'no_speech' }, last_error: null }), /理由: 音声がない可能性/);
});

test('a capture fault shows where in the recording it happened', () => {
  const failure = { attempt: 1, stage: 'capture_stream', code: 'audio_stream', backend_kind: 'xrun',
    fault_position: { callback: 12, audio_ms: 1250, xruns: 2 } };
  const html = render({ revision: 8, latest, last_error: failure });
  assert.match(html, /録音開始から 1\.25 秒（12 回目の音声受信の後）/);
  assert.match(html, /欠落通知: 2 回/);
  const before = render({ revision: 9, latest, last_error: { ...failure, fault_position: { callback: 0, audio_ms: 0, xruns: 2 } } });
  assert.match(before, /音声が届く前/);
});

test('summary shows counts, fallback reasons and duration statistics only', () => {
  const summary = { attempts: 12, delivered: 11, failed: 1, fast_path: 10, fallback: 1, full_path: 0,
    fallback_reasons: [{ reason: 'no_speech', count: 1 }],
    stop_to_complete: { count: 11, median_ms: 812, p90_ms: 950, max_ms: 2320 } };
  const html = render({ revision: 10, latest, last_error: null, summary });
  assert.match(html, /試行 12 回（入力・コピー 11 回 · 失敗 1 回）/);
  assert.match(html, /高速処理 10 回 · 通常処理に切り替え 1 回/);
  assert.match(html, /理由: 音声がない可能性 1回/);
  assert.match(html, /中央値 0\.812 秒/);
  assert.doesNotMatch(render({ revision: 11, latest: null, last_error: null,
    summary: { ...summary, attempts: 0 } }), /起動してからの集計/);
});

test('removed hesitations are shown as a count only when the option was on', () => {
  assert.match(render({ revision: 12, latest: { ...latest, fillers_removed: 3 }, last_error: null }), /言いよどみの除去: 3 件/);
  assert.match(render({ revision: 13, latest: { ...latest, fillers_removed: 0 }, last_error: null }), /言いよどみの除去: 0 件/);
  assert.doesNotMatch(render({ revision: 14, latest, last_error: null }), /言いよどみ/);
});
