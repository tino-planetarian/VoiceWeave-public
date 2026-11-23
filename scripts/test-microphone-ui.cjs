// Render the real microphone selector with synthetic devices. No audio device is opened.
const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const ts = require('typescript');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const compiled = ts.transpileModule(fs.readFileSync(path.join(__dirname, '../src/MicrophoneSelect.tsx'), 'utf8'), {
  compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS },
}).outputText;
const component = {};
new Function('require', 'exports', compiled)(require, component);
const mics = [
  { id: 'wasapi:a', name: 'マイク (MU900)', is_default: true },
  { id: 'wasapi:b', name: 'Headset', is_default: false },
];
const render = (props) => renderToStaticMarkup(React.createElement(component.MicrophoneSelect, {
  value: { kind: 'system_default' }, microphones: mics, disabled: false, onChange() {}, onRefresh() {}, ...props,
}));

test('lists the Windows default with the current device name, then each microphone', () => {
  const html = render({});
  assert.match(html, /Windowsの既定（現在: マイク \(MU900\)）/);
  assert.match(html, /<option value="wasapi:b">Headset<\/option>/);
  assert.doesNotMatch(html, /見つかりません/);
});

test('a saved microphone that is unplugged stays selectable and is explained', () => {
  const html = render({ value: { kind: 'endpoint', id: 'wasapi:gone', display_name: 'USB Mic' } });
  assert.match(html, /USB Mic（見つかりません）/);
  assert.match(html, /Windowsの既定のマイクで録音します/);
});

test('while the list loads the select is disabled and no missing warning is shown', () => {
  const html = render({ microphones: null, value: { kind: 'endpoint', id: 'wasapi:a', display_name: 'x' } });
  assert.match(html, /確認中/);
  assert.match(html, /<select[^>]*disabled/);
  assert.doesNotMatch(html, /見つかりません/);
});
