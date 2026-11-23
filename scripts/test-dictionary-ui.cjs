// Render the real dictionary editor with synthetic entries. No settings are written.
const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const ts = require('typescript');
const React = require('react');
const { renderToStaticMarkup } = require('react-dom/server');
const compiled = ts.transpileModule(fs.readFileSync(path.join(__dirname, '../src/DictionaryEditor.tsx'), 'utf8'), {
  compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS },
}).outputText;
const component = {};
new Function('require', 'exports', compiled)(require, component);
const render = (props) => renderToStaticMarkup(React.createElement(component.DictionaryEditor, {
  entries: [], disabled: false, onChange() {}, preview: async () => ({ text: '', replacements: 0, hints: [] }),
  newId: () => 'id', ...props,
}));

test('aliases split on Japanese and ASCII commas and drop blanks', () => {
  assert.deepEqual(component.splitAliases('ぼいすうぃーぶ、 ボイス ウィーブ ,,VW，'), ['ぼいすうぃーぶ', 'ボイス ウィーブ', 'VW']);
  assert.deepEqual(component.splitAliases('   '), []);
});

test('adding aliases skips duplicates, blanks and the term itself', () => {
  assert.deepEqual(component.addAliases(['クロード'], 'クラウド、クロード', 'Claude'), ['クロード', 'クラウド']);
  assert.deepEqual(component.addAliases([], 'Claude、 ', 'Claude'), []);
});

test('registered words are compact table rows with their aliases', () => {
  const html = render({ entries: [
    { id: 'a', term: 'Claude', aliases: ['クロード', 'クラウド'], enabled: true },
    { id: 'b', term: 'VoiceWeave', aliases: [], enabled: false },
  ] });
  assert.match(html, /<table class="dictionary-list">/);
  assert.match(html, /<td>Claude<\/td><td>クロード、クラウド<\/td>/);
  assert.match(html, /（置き換えなし）/);
  assert.match(html, /class="disabled-entry"/);
  assert.match(html, /2 \/ 200 件/);
  assert.doesNotMatch(html, /登録はまだありません/);
});

test('an empty dictionary explains itself and the form starts empty with registration disabled', () => {
  const html = render({});
  assert.match(html, /登録はまだありません/);
  assert.match(html, /disabled=""[^>]*>登録<\/button>/);
});
