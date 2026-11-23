// Render the real model panel with synthetic state. No browser/Tauri, network or model.
const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const ts = require('typescript');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const load = (file) => {
  const compiled = ts.transpileModule(fs.readFileSync(path.join(__dirname, '../src', file), 'utf8'), {
    compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS },
  }).outputText;
  const exports = {};
  new Function('require', 'exports', compiled)((name) => name === './GpuPanel' ? load('GpuPanel.tsx') : require(name), exports);
  return exports;
};
const { ModelPanel } = load('ModelPanel.tsx');
const render = (props) => renderToStaticMarkup(React.createElement(ModelPanel, {
  standard: undefined, installing: false, progress: null, outcome: null,
  onDownload() {}, onImport() {}, onCancel() {}, ...props,
}));

test('missing model offers download and folder copy, with the copy button disabled until a folder is typed', () => {
  const html = render({});
  assert.match(html, /標準モデル（turbo、約1\.6GB）が必要です/);
  assert.match(html, /ダウンロードして導入する/);
  assert.match(html, /このフォルダーからコピーして導入する<\/button>/);
  assert.match(html, /disabled=""[^>]*>このフォルダーからコピーして導入する/);
});

test('a development-folder model is explained and can be imported', () => {
  const html = render({ standard: { preset: 'standard', files_present: true, location: 'development' } });
  assert.match(html, /開発用フォルダーのモデルを使っています/);
  assert.match(html, /ダウンロードして導入する/);
});

test('installed model, progress and outcomes are stated plainly', () => {
  assert.match(render({ standard: { preset: 'standard', files_present: true, location: 'managed' } }), /導入済みです/);
  const busy = render({ installing: true, progress: { phase: 'copying', completed: 50, total: 100 } });
  assert.match(busy, /モデルをコピー中（50%）/);
  assert.match(busy, /導入を取り消す/);
  assert.doesNotMatch(busy, /ダウンロードして導入する/);
  assert.match(render({ outcome: 'failed' }), /導入できませんでした/);
  assert.match(render({ outcome: 'cancelled' }), /何も変更していません/);
});
