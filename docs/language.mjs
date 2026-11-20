// Language and regional choices match Mouflon's published localization catalog.
export const names = new Map([
  ['de','Deutsch'], ['en','English'], ['es','Español'], ['fr','Français'],
  ['it','Italiano'], ['nl','Nederlands'], ['pt','Português (Brasil)'],
  ['pt-PT','Português (Portugal)'], ['ru','Русский'], ['ja','日本語'],
  ['ko','한국어'], ['zh-Hans','简体中文'], ['zh-Hant','繁體中文'],
]);

export function preferredLanguage(tags) {
  for (const tag of tags) {
    if (typeof tag !== 'string') continue;
    let locale;
    try { locale = new Intl.Locale(tag.replaceAll('_','-')); }
    catch { continue; }
    const {language, script, region} = locale;
    if (language === 'zh') {
      if (script) return script === 'Hant' ? 'zh-Hant' : 'zh-Hans';
      return ['TW','HK','MO'].includes(region) ? 'zh-Hant' : 'zh-Hans';
    }
    if (language === 'pt') {
      return ['PT','AO','MZ','CV','GW','ST','TL','MO'].includes(region) ? 'pt-PT' : 'pt';
    }
    if (names.has(language)) return language;
  }
  return 'en';
}
