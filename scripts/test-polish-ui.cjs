// Render the polishing-mode settings with synthetic values. Nothing is saved or sent.
const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const ts = require('typescript');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');

const load = (file, localRequire) => {
  const compiled = ts.transpileModule(fs.readFileSync(path.join(__dirname, '../src', file), 'utf8'), {
    compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS },
  }).outputText;
  const exports = {};
  new Function('require', 'exports', compiled)(localRequire, exports);
  return exports;
};
const shortcut = load('ShortcutInput.tsx', require);
const panel = load('PolishPanel.tsx', (name) => (name === './ShortcutInput' ? shortcut : require(name)));
const value = { enabled: true, shortcut: 'F9', model: 'gemma3:4b', endpoint: 'http://127.0.0.1:11434', append_original: true };
const render = (props) => renderToStaticMarkup(React.createElement(panel.PolishPanel, {
  value, disabled: false, clash: false, registered: true, onChange() {}, onCapture() {},
  listModels: async () => [], ...props,
}));

test('the saved model is offered before Ollama is checked', () => {
  const html = render({});
  assert.match(html, /<option value="gemma3:4b"[^>]*>gemma3:4b<\/option>/);
  assert.match(html, /いま登録中の F9: 登録済み/);
  assert.match(html, /元の文（話した言葉に忠実な書き起こし）も入力する/);
});

test('a key shared with recording or re-paste is flagged', () => {
  assert.match(render({ clash: true }), /録音・再入力と同じキーは使えません/);
  assert.doesNotMatch(render({}), /録音・再入力と同じキーは使えません/);
});

test('when the mode is off, its fields are disabled and no registration is claimed', () => {
  const html = render({ value: { ...value, enabled: false }, registered: null });
  assert.match(html, /<fieldset class="shortcut-fields" disabled="">/);
  assert.doesNotMatch(html, /いま登録中/);
});

test('compose mode shows its key, clash and clipboard note', () => {
  const html = render({ composeRegistered: true });
  assert.match(html, /作文モード（話した依頼から文章を作る）/);
  assert.match(html, /value="Shift\+F9"/);
  assert.match(html, /いま登録中の Shift\+F9: 登録済み/);
  assert.match(html, /コピーした文章（返信したいメールなど）を参考にする/);
  assert.match(render({ composeClash: true }), /録音・再入力・整形と同じキーは使えません/);
  const noClipboard = render({ value: { ...value, compose_clipboard: false } });
  assert.doesNotMatch(noClipboard, /クリップボードの文章（6,000字まで）/);
});
