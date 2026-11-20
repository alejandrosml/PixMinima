import { names, preferredLanguage } from './language.mjs';

const select = document.getElementById('language');
const screenshot = document.getElementById('screenshot');
const comparison = document.getElementById('compare');
const description = document.querySelector('meta[name="description"]');
const nodes = [...document.querySelectorAll('[data-key]')];
const french = Object.fromEntries(nodes.map(n => [n.dataset.key, n.textContent]));
french.alt = screenshot.alt;
french.description = description.content;
const systemOption = select.options[0];
const catalogs = new Map([['fr', french]]);
const key = 'pixminima-language';
let chosen, revision = 0;
try { chosen = localStorage.getItem(key); } catch { /* Private browser session */ }
if (!names.has(chosen)) chosen = 'auto';

for (const [tag, name] of names) {
  const option = new Option(name, tag);
  option.lang = tag === 'pt' ? 'pt-BR' : tag;
  select.add(option);
}

async function display() {
  const request = ++revision;
  const browserLanguages = [...(navigator.languages || []), navigator.language];
  const desired = chosen === 'auto' ? preferredLanguage(browserLanguages) : chosen;
  select.value = chosen;
  let lang = desired;
  if (!catalogs.has(lang)) {
    try {
      const result = await fetch(new URL('./i18n/' + lang + '.json', import.meta.url));
      if (!result.ok) throw new Error('Language unavailable');
      const words = await result.json();
      if (Object.keys(french).some(k => typeof words[k] !== 'string' || !words[k])) throw new Error('Incomplete language');
      catalogs.set(lang, words);
    } catch { lang = 'fr'; }
  }
  if (request !== revision) return;
  const words = catalogs.get(lang);
  const appLang = ['fr','en','es'].includes(lang) ? lang : 'en';
  document.documentElement.lang = lang === 'pt' ? 'pt-BR' : lang;
  document.title = 'PixMinima · ' + words.title;
  description.content = words.description;
  nodes.forEach(n => { n.textContent = words[n.dataset.key]; });
  systemOption.textContent = words.system;
  screenshot.src = 'images/pixminima-' + appLang + '.png';
  screenshot.alt = words.alt;
  comparison.href = 'https://github.com/alejandrosml/PixMinima/blob/main/docs/comparison.' + (lang === 'fr' ? 'fr' : 'en') + '.md';
}

select.addEventListener('change', () => {
  chosen = names.has(select.value) ? select.value : 'auto';
  try { if (chosen === 'auto') localStorage.removeItem(key); else localStorage.setItem(key, chosen); } catch {}
  display();
});
window.addEventListener('languagechange', () => { if (chosen === 'auto') display(); });
display();
