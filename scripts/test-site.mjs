import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import test from 'node:test';
import {names, preferredLanguage} from '../docs/language.mjs';

test('browser preferences select Mouflon languages and regional variants', () => {
  const cases = [
    [['fr-CA'], 'fr'], [['de-CH'], 'de'], [['es-MX'], 'es'], [['it-IT'], 'it'],
    [['nl-BE'], 'nl'], [['pt_BR'], 'pt'], [['pt'], 'pt'], [['pt-PT'], 'pt-PT'],
    [['pt-AO'], 'pt-PT'], [['ru-RU'], 'ru'], [['ja-JP'], 'ja'], [['ko-KR'], 'ko'],
    [['zh-CN'], 'zh-Hans'], [['zh-SG'], 'zh-Hans'], [['zh-TW'], 'zh-Hant'],
    [['zh-MO'], 'zh-Hant'], [['zh-Hant-CN'], 'zh-Hant'], [['zh-Hans-HK'], 'zh-Hans'],
    [['ar-EG','nb-NO','de-DE','en'], 'de'], [['%%%','fr-CH'], 'fr'], [['pl'], 'en'],
  ];
  for (const [tags, expected] of cases) assert.equal(preferredLanguage(tags), expected);
});

test('each language covers the visible page, controls, screenshot and metadata', async () => {
  const html = await readFile(new URL('../docs/index.html', import.meta.url), 'utf8');
  const keys = [...html.matchAll(/data-key="([^"]+)"/g)].map(m => m[1]);
  keys.push('alt','description');
  assert.equal(names.size, 13);
  for (const tag of names.keys()) {
    const words = JSON.parse(await readFile(new URL('../docs/i18n/'+tag+'.json', import.meta.url), 'utf8'));
    assert.deepEqual(Object.keys(words).sort(), [...keys].sort(), tag);
    assert.ok(Object.values(words).every(s => typeof s === 'string' && s.trim()), tag);
  }
});

test('the public format list matches the extensions accepted by the Mac app and engine', async () => {
  const read = path => readFile(new URL(path, import.meta.url), 'utf8');
  const [html, rust, plist] = await Promise.all([
    read('../docs/index.html'), read('../crates/core/src/lib.rs'), read('../packaging/macos/Info.plist'),
  ]);
  const section = html.match(/<section id="formats"[\s\S]*?<\/section>/)[0];
  const visible = [...section.matchAll(/<code>\.([a-z0-9]+)<\/code>/g)].map(m => m[1]).sort();
  const engine = [...rust.match(/pub const EXTENSIONS:[\s\S]*?= &\[([\s\S]*?)\];/)[1].matchAll(/"(\w+)"/g)].map(m => m[1]).sort();
  const registration = plist.match(/<key>CFBundleTypeExtensions<\/key>\s*<array>([\s\S]*?)<\/array>/)[1];
  const mac = [...registration.matchAll(/<string>(\w+)<\/string>/g)].map(m => m[1]).sort();
  assert.deepEqual(visible, engine);
  assert.deepEqual(visible, mac);
  assert.equal(visible.length, new Set(visible).size);
  assert.ok(html.includes('href="#formats"'));
});
