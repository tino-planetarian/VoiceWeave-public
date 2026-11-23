// Render the real GPU panel with synthetic state. No browser/Tauri, network or GPU.
const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const ts = require('typescript');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const filename = path.join(__dirname, '../src/GpuPanel.tsx');
const compiled = ts.transpileModule(fs.readFileSync(filename, 'utf8'), {
  compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS },
}).outputText;
const component = {};
new Function('require', 'exports', compiled)(require, component);
const render = (props) => renderToStaticMarkup(React.createElement(component.GpuPanel, {
  status: { installed: false, installing: false, supported: true },
  device: null, progress: null, outcome: null, onInstall() {}, onCancel() {}, ...props,
}));

test('progress percent is bounded and unknown without a total', () => {
  assert.equal(component.progressPercent(null), null);
  assert.equal(component.progressPercent({ phase: 'downloading', completed: 5, total: 0 }), null);
  assert.equal(component.progressPercent({ phase: 'downloading', completed: 50, total: 200 }), 25);
  assert.equal(component.progressPercent({ phase: 'downloading', completed: 999, total: 200 }), 100);
});

test('not installed offers the install button; unsupported builds disable it', () => {
  assert.match(render({}), /GPU高速化を導入する/);
  assert.doesNotMatch(render({}), /disabled/);
  const html = render({ status: { installed: false, installing: false, supported: false } });
  assert.match(html, /disabled/);
  assert.match(html, /まだ導入に対応していません/);
});

test('installing shows the phase, percentage and a cancel button', () => {
  const html = render({
    status: { installed: false, installing: true, supported: true },
    progress: { phase: 'downloading', completed: 50, total: 200 },
  });
  assert.match(html, /ダウンロード中（25%）/);
  assert.match(html, /導入を取り消す/);
  assert.doesNotMatch(html, /GPU高速化を導入する/);
});

test('active device, installed-but-cpu and outcomes are stated plainly', () => {
  const installed = { installed: true, installing: false, supported: true };
  assert.match(render({ status: installed, device: 'cuda' }), /GPUで音声認識しています/);
  assert.match(render({ status: installed, device: 'cpu' }), /導入済みです/);
  assert.match(render({ outcome: 'failed' }), /導入できませんでした/);
  assert.match(render({ outcome: 'cancelled' }), /何も変更していません/);
  assert.match(render({ status: null }), /確認中/);
});
