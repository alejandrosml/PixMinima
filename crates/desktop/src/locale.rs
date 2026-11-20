#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lang {
    Fr,
    En,
    Es,
}

impl Lang {
    pub fn from_tag(tag: &str) -> Self {
        match tag
            .split(['-', '_', '.'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "fr" => Self::Fr,
            "es" => Self::Es,
            _ => Self::En,
        }
    }
    pub fn tag(self) -> &'static str {
        self.pick("fr", "en", "es")
    }
    pub fn name(self) -> &'static str {
        self.pick("Français", "English", "Español")
    }
    pub fn pick<'a>(self, fr: &'a str, en: &'a str, es: &'a str) -> &'a str {
        match self {
            Self::Fr => fr,
            Self::En => en,
            Self::Es => es,
        }
    }
    pub fn bytes(self, bytes: u64) -> String {
        let units = match self {
            Self::Fr => ["o", "ko", "Mo", "Go", "To"],
            _ => ["B", "kB", "MB", "GB", "TB"],
        };
        let mut n = bytes as f64;
        let mut i = 0;
        while n >= 1000.0 && i < units.len() - 1 {
            n /= 1000.0;
            i += 1;
        }
        if i == 0 {
            return format!("{bytes} {}", units[0]);
        }
        let s = if n < 10.0 {
            format!("{n:.2}")
        } else if n < 100.0 {
            format!("{n:.1}")
        } else {
            format!("{n:.0}")
        };
        let number = if self == Self::En {
            s
        } else {
            s.replace('.', ",")
        };
        format!("{number} {}", units[i])
    }
    pub fn saved_percent(self, before: u64, after: u64) -> String {
        let delta = before.abs_diff(after) as u128;
        let tenths = if before == 0 {
            0
        } else {
            (delta * 1000 + (before / 2) as u128) / before as u128
        };
        let decimal = if self == Self::En { '.' } else { ',' };
        let space = if self == Self::En { "" } else { " " };
        let sign = if after > before { "−" } else { "" };
        format!("{sign}{}{decimal}{}{space}%", tenths / 10, tenths % 10)
    }
    pub fn exact_bytes(self, n: u64) -> String {
        let digits = n.to_string();
        let sep = self.pick(" ", ",", ".");
        let mut grouped = String::new();
        for (i, c) in digits.chars().enumerate() {
            if i != 0 && (digits.len() - i).is_multiple_of(3) {
                grouped.push_str(sep);
            }
            grouped.push(c);
        }
        let unit = if n == 1 {
            self.pick("octet", "byte", "byte")
        } else {
            self.pick("octets", "bytes", "bytes")
        };
        format!("{grouped} {unit}")
    }

    pub fn items(self, n: usize) -> String {
        let noun = if n == 1 {
            self.pick("élément", "item", "elemento")
        } else {
            self.pick("éléments", "items", "elementos")
        };
        format!("{n} {noun}")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LanguageChoice {
    #[default]
    System,
    Explicit(Lang),
}

impl LanguageChoice {
    pub fn from_saved(tag: Option<&str>) -> Self {
        match tag {
            Some("fr") => Self::Explicit(Lang::Fr),
            Some("en") => Self::Explicit(Lang::En),
            Some("es") => Self::Explicit(Lang::Es),
            _ => Self::System,
        }
    }
    pub fn saved(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Explicit(l) => l.tag(),
        }
    }
    pub fn resolve(self, system: Option<&str>) -> Lang {
        match self {
            Self::System => Lang::from_tag(system.unwrap_or("en")),
            Self::Explicit(l) => l,
        }
    }
    pub fn label(self, ui: Lang) -> &'static str {
        match self {
            Self::System => ui.pick("Langue du système", "System language", "Idioma del sistema"),
            Self::Explicit(l) => l.name(),
        }
    }
}

impl Lang {
    pub fn sizes(self, before: u64, after: u64) -> String {
        format!(
            "{} {}  ·  {} {}",
            self.bytes(before),
            self.pick("avant", "before", "antes"),
            self.bytes(after),
            self.pick("après", "after", "después")
        )
    }
}

impl Lang {
    pub fn error(self, detail: &str) -> &'static str {
        let e = detail.to_ascii_lowercase();
        if e.contains("changed after optimization") {
            self.pick("Le fichier a été modifié depuis l’optimisation. Sa version actuelle et sa sauvegarde sont conservées.","The file was edited after optimization. Both the current file and its backup were kept.","El archivo cambió después de optimizarlo. Se conservan el archivo actual y su copia de seguridad.")
        } else if e.contains("would not be smaller") {
            self.pick(
                "L’archive réversible serait aussi grande ou plus grande que le JPEG d’origine.",
                "The reversible archive would be as large as or larger than the original JPEG.",
                "El archivo reversible sería igual o mayor que el JPEG original.",
            )
        } else if e.contains("backup already") {
            self.pick(
                "Une sauvegarde existe déjà pour ce fichier.",
                "A backup already exists for this file.",
                "Ya existe una copia de seguridad de este archivo.",
            )
        } else if e.contains("hard links") {
            self.pick(
                "Ce fichier possède plusieurs liens physiques. Utilise le mode copies séparées.",
                "This file has multiple hard links. Use separate copies.",
                "Este archivo tiene varios enlaces físicos. Utiliza copias separadas.",
            )
        } else if e.contains("unsupported")
            || e.contains("recognized")
            || e.contains("unknown format")
        {
            self.pick(
                "Cette variante de fichier n’est pas prise en charge.",
                "This file variant is unsupported.",
                "Esta variante de archivo no es compatible.",
            )
        } else if e.contains("limit") || e.contains("too large") || e.contains("exceed") {
            self.pick(
                "Ce fichier dépasse la limite de traitement.",
                "This file exceeds the processing limit.",
                "El archivo supera el límite de procesamiento.",
            )
        } else if e.contains("permission") || e.contains("denied") {
            self.pick(
                "L’accès au fichier a été refusé.",
                "Access to the file was denied.",
                "Se ha denegado el acceso al archivo.",
            )
        } else if e.contains("exists") {
            self.pick(
                "Un fichier existe déjà à cet emplacement.",
                "A file already exists at this location.",
                "Ya existe un archivo en esta ubicación.",
            )
        } else if e.contains("no such file") || e.contains("not found") {
            self.pick(
                "Le fichier est introuvable.",
                "The file could not be found.",
                "No se ha encontrado el archivo.",
            )
        } else {
            self.pick(
                "L’opération a échoué. Le détail technique est disponible.",
                "The operation failed. Technical details are available.",
                "La operación ha fallado. El detalle técnico está disponible.",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn percentages_round_from_the_exact_sizes_and_follow_the_ui_language() {
        assert_eq!(Lang::Fr.saved_percent(1000, 947), "5,3 %");
        assert_eq!(Lang::Es.saved_percent(1000, 947), "5,3 %");
        assert_eq!(Lang::En.saved_percent(1000, 947), "5.3%");
        assert_eq!(Lang::En.saved_percent(1, 1), "0.0%");
        assert_eq!(Lang::Fr.saved_percent(0, 0), "0,0 %");
        assert_eq!(Lang::Fr.saved_percent(100, 125), "−25,0 %");
        assert_eq!(Lang::En.saved_percent(u64::MAX, 0), "100.0%");
    }
    #[test]
    fn system_choice_tracks_the_actual_locale_and_returns_to_it() {
        let system = LanguageChoice::from_saved(None);
        for (tag, language) in [
            ("fr-FR", Lang::Fr),
            ("fr_CA", Lang::Fr),
            ("es-MX", Lang::Es),
            ("en-US", Lang::En),
            ("ja-JP", Lang::En),
        ] {
            assert_eq!(system.resolve(Some(tag)), language);
        }
        assert_eq!(
            LanguageChoice::from_saved(Some("es")).resolve(Some("fr-CA")),
            Lang::Es
        );
        assert_eq!(
            LanguageChoice::from_saved(Some("system")).resolve(Some("fr-CA")),
            Lang::Fr
        );
        assert_eq!(LanguageChoice::default().saved(), "system");
    }
}
