// Key presses become the shortcut text the app parses. No keyboard hook is installed.
const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const ts = require('typescript');
const compiled = ts.transpileModule(fs.readFileSync(path.join(__dirname, '../src/ShortcutInput.tsx'), 'utf8'), {
  compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.CommonJS },
}).outputText;
const component = {};
new Function('require', 'exports', compiled)(require, component);
const press = (code, mods = {}) => component.shortcutFromKey({
  code, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods,
});

test('modifiers and a key become the parsed form', () => {
  assert.equal(press('KeyR', { ctrlKey: true, altKey: true }), 'Ctrl+Alt+R');
  assert.equal(press('F8'), 'F8');
  assert.equal(press('Space', { ctrlKey: true, shiftKey: true }), 'Ctrl+Shift+Space');
  assert.equal(press('Digit1', { altKey: true }), 'Alt+1');
  assert.equal(press('F12', { metaKey: true }), 'Win+F12');
});

test('modifier-only presses and unsupported keys change nothing', () => {
  assert.equal(press('ControlLeft', { ctrlKey: true }), null);
  assert.equal(press('AltRight', { altKey: true, ctrlKey: true }), null);
  assert.equal(press('Enter'), null);
  assert.equal(press('BracketLeft', { ctrlKey: true }), null);
  assert.equal(press('F25'), null);
});

test('held modifiers are previewed before the key arrives', () => {
  const held = (mods) => component.heldModifiers({ ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods });
  assert.equal(held({ ctrlKey: true }), 'Ctrl+');
  assert.equal(held({ ctrlKey: true, altKey: true }), 'Ctrl+Alt+');
  assert.equal(held({}), '');
});
