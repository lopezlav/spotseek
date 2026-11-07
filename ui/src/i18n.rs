//! English and Spanish. Spanish is neutral, with "vos" (pegá, buscá, apretá). The language
//! follows the system unless the settings pick one.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Es,
}

impl Lang {
    /// "en", "es", or anything else for the system language.
    pub fn from_setting(s: &str) -> Lang {
        match s {
            "en" => Lang::En,
            "es" => Lang::Es,
            _ => Lang::system(),
        }
    }

    pub fn system() -> Lang {
        let loc = std::env::var("SPOTSEEK_LANG").ok().or_else(sys_locale::get_locale).unwrap_or_default();
        if loc.to_lowercase().starts_with("es") { Lang::Es } else { Lang::En }
    }

    pub fn es(self) -> bool {
        self == Lang::Es
    }

    /// The text for this language.
    pub fn t(self, en: &'static str, es: &'static str) -> &'static str {
        if self.es() { es } else { en }
    }

    /// "1 TRACK" / "3 TRACKS", "1 CANCIÓN" / "3 CANCIONES".
    pub fn tracks(self, n: usize) -> String {
        match (self, n) {
            (Lang::En, 1) => "1 TRACK".into(),
            (Lang::En, n) => format!("{n} TRACKS"),
            (Lang::Es, 1) => "1 CANCIÓN".into(),
            (Lang::Es, n) => format!("{n} CANCIONES"),
        }
    }

    /// The kind of link: playlist, album or track.
    pub fn kind(self, kind: &str) -> String {
        match (self, kind) {
            (Lang::Es, "album") => "ÁLBUM".into(),
            (Lang::Es, "track") => "CANCIÓN".into(),
            (_, k) => k.to_uppercase(),
        }
    }

    pub fn month(self, m: u32) -> &'static str {
        const EN: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
        const ES: [&str; 12] = ["ENE", "FEB", "MAR", "ABR", "MAY", "JUN", "JUL", "AGO", "SEP", "OCT", "NOV", "DIC"];
        let i = (m as usize).saturating_sub(1).min(11);
        if self.es() { ES[i] } else { EN[i] }
    }

    /// The paste shortcut of this system.
    pub fn paste_keys(self) -> &'static str {
        if cfg!(target_os = "macos") { "⌘V" } else { "CTRL+V" }
    }
}

/// A clearer message for errors people can fix themselves.
pub fn hint(lang: Lang, error: &str) -> Option<&'static str> {
    if error.contains("certificate has expired or is not yet valid") || error.contains("certificate is not yet valid") {
        return Some(lang.t(
            "Your computer's clock looks wrong. Turn on \"Set time automatically\" in the system settings and try again.",
            "El reloj de tu computadora parece estar mal. Activá \"Ajustar la hora automáticamente\" en la configuración del sistema y probá de nuevo.",
        ));
    }
    if error.contains("no such host") || error.contains("network is unreachable") || error.contains("dial tcp") {
        return Some(lang.t("Spotseek cannot reach the internet. Check your connection and try again.", "Spotseek no se puede conectar a internet. Revisá tu conexión y probá de nuevo."));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spanish_plurals_and_kinds() {
        assert_eq!(Lang::Es.tracks(1), "1 CANCIÓN");
        assert_eq!(Lang::Es.tracks(28), "28 CANCIONES");
        assert_eq!(Lang::Es.kind("album"), "ÁLBUM");
        assert_eq!(Lang::En.kind("playlist"), "PLAYLIST");
        assert!(hint(Lang::Es, "tls: failed to verify certificate: x509: certificate has expired or is not yet valid").unwrap().contains("reloj"));
    }
}
