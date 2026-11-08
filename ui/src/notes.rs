//! The log the user can open from the footer: what happens, in plain words, with the
//! important things marked (why it is slow, why it stopped). The engine sends `note` events
//! with a code; this file words them in English and Spanish. The technical lines are kept too.

use crate::i18n::Lang;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Something went well (signed in, downloaded).
    Good,
    Info,
    /// It is slower than it could be, or a file failed and another one is tried.
    Warn,
    /// Something stops (no connection, not found, blocked).
    Bad,
    /// A technical line (only with "technical details").
    Tech,
}

impl Level {
    pub fn from(s: &str) -> Level {
        match s {
            "good" => Level::Good,
            "warn" => Level::Warn,
            "bad" => Level::Bad,
            "info" => Level::Info,
            _ => Level::Tech,
        }
    }

    /// Shown with a mark and a colour: it helps to understand what is going on.
    pub fn important(self) -> bool {
        matches!(self, Level::Warn | Level::Bad)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// Unix seconds.
    pub at: f64,
    pub level: Level,
    /// For people (empty for technical lines).
    pub text: String,
    /// The technical line.
    pub tech: String,
    /// The track it is about.
    pub i: Option<usize>,
}

/// Keeps the newest notes only.
pub const MAX_NOTES: usize = 600;

/// What the engine's note means, in the user's words. `title` is "Artist – Title" of the track
/// it is about (or empty).
#[allow(clippy::too_many_arguments)]
pub fn word(l: Lang, code: &str, title: &str, peer: &str, message: &str, count: usize, secs: u32) -> String {
    let t = title;
    let p = peer;
    let es = l.es();
    let clock = crate::media::clock(secs as f64);
    match code {
        "signed_in" => l.t("Signed in to Soulseek.", "Sesión iniciada en Soulseek.").into(),
        "no_account" => l.t("No Soulseek account: every song comes from YouTube Music, which is slower.", "Sin cuenta de Soulseek: todas las canciones vienen de YouTube Music, que es más lento.").into(),
        "login_failed" => {
            if message == "INVALIDPASS" {
                l.t("Soulseek did not accept the password, or the name is taken. Check your account in SETTINGS.", "Soulseek no aceptó la contraseña, o el nombre ya existe. Revisá tu cuenta en AJUSTES.").into()
            } else if es {
                format!("Soulseek no te dejó entrar ({message}). Revisá tu cuenta en AJUSTES.")
            } else {
                format!("Soulseek did not let you in ({message}). Check your account in SETTINGS.")
            }
        }
        "offline" => {
            if message == "relogged" {
                l.t(
                    "Soulseek disconnected: your account signed in somewhere else (another Spotseek, Nicotine+ or another computer). Soulseek allows one place at a time.",
                    "Soulseek se desconectó: tu cuenta entró en otro lado (otro Spotseek, Nicotine+ u otra computadora). Soulseek permite un solo lugar a la vez.",
                )
                .into()
            } else {
                l.t("The connection to Soulseek dropped. Downloads wait; nothing is lost.", "Se cortó la conexión con Soulseek. Las descargas esperan; no se pierde nada.").into()
            }
        }
        "reconnecting" => {
            if es {
                format!("Volviendo a entrar a Soulseek en {secs} s…")
            } else {
                format!("Signing in to Soulseek again in {secs} s…")
            }
        }
        "reconnected" => l.t("Back on Soulseek: the downloads go on.", "De vuelta en Soulseek: las descargas siguen.").into(),
        "search_limit" => {
            if es {
                format!("Soulseek permite 34 búsquedas cada 3:40, así que las listas largas hacen pausas. {count} canciones esperan su búsqueda; la próxima en {clock}. Sigue sola.")
            } else {
                format!("Soulseek allows 34 searches every 3:40, so long lists pause. {count} songs wait for their search; the next one in {clock}. It goes on by itself.")
            }
        }
        "found" => {
            if es {
                format!("{t}: {count} archivos en {secs} s.")
            } else {
                format!("{t}: {count} files in {secs} s.")
            }
        }
        "no_files" => {
            if es {
                format!("{t}: nadie en Soulseek la tiene. Se busca en YouTube Music.")
            } else {
                format!("{t}: nobody on Soulseek has it. Trying YouTube Music.")
            }
        }
        "queued" => match (es, count) {
            (false, 0) => format!("{t}: {p} put you in a queue."),
            (false, n) => format!("{t}: {p} put you in a queue (place {n})."),
            (true, 0) => format!("{t}: {p} te puso en una cola."),
            (true, n) => format!("{t}: {p} te puso en una cola (lugar {n})."),
        },
        "peer_failed" => {
            let why = match (message, es) {
                ("queued", false) => "kept you in its queue too long",
                ("queued", true) => "te tuvo en la cola demasiado tiempo",
                ("no_start", false) => "did not start sending",
                ("no_start", true) => "no empezó a mandar",
                ("stalled", false) => "stopped sending",
                ("stalled", true) => "dejó de mandar",
                ("slow", false) => "was much too slow",
                ("slow", true) => "era demasiado lento",
                ("faster", false) => "was slow, and a much faster file came in",
                ("faster", true) => "era lento, y apareció un archivo mucho más rápido",
                ("connect", false) => "could not be reached",
                ("connect", true) => "no se pudo conectar",
                ("denied", false) => "said no",
                ("denied", true) => "dijo que no",
                ("disk", false) => "could not be saved to disk",
                ("disk", true) => "no se pudo guardar en el disco",
                (_, false) => "broke off",
                (_, true) => "se cortó",
            };
            if es {
                format!("{t}: {p} {why}. Se prueba el próximo archivo.")
            } else {
                format!("{t}: {p} {why}. Trying the next file.")
            }
        }
        "gave_up" => {
            if es {
                format!("{t}: fallaron {count} archivos de Soulseek. Se busca en YouTube Music.")
            } else {
                format!("{t}: {count} Soulseek files failed. Trying YouTube Music.")
            }
        }
        "downloaded" => {
            if es {
                format!("{t}: descargada de {p} en {clock}.")
            } else {
                format!("{t}: downloaded from {p} in {clock}.")
            }
        }
        "yt_done" => {
            if es {
                format!("{t}: descargada de YouTube Music.")
            } else {
                format!("{t}: downloaded from YouTube Music.")
            }
        }
        "yt_no_match" => {
            if es {
                format!("{t}: YouTube Music no tiene una versión que coincida.")
            } else {
                format!("{t}: YouTube Music has no version that matches.")
            }
        }
        "yt_blocked" => {
            if es {
                format!("{t}: YouTube frenó las descargas por un rato (pide confirmar que no sos un robot). Probá más tarde.")
            } else {
                format!("{t}: YouTube stopped downloads for a while (it asks to confirm you are not a robot). Try again later.")
            }
        }
        "yt_failed" => {
            if es {
                format!("{t}: falló la descarga de YouTube Music.")
            } else {
                format!("{t}: the YouTube Music download failed.")
            }
        }
        "missing" => {
            if es {
                format!("{t}: no se encontró en ningún lado.")
            } else {
                format!("{t}: not found anywhere.")
            }
        }
        "convert_failed" => {
            if es {
                format!("{t}: no se pudo convertir a mp3.")
            } else {
                format!("{t}: it could not be made into an mp3.")
            }
        }
        "found_free" => {
            let how = match (message, es) {
                ("cache", false) => "from a search made in the last hours",
                ("cache", true) => "de una búsqueda de las últimas horas",
                (_, false) => "in the folder of a peer that has other songs of the list",
                (_, true) => "en la carpeta de alguien que tiene otras canciones de la lista",
            };
            if es {
                format!("{t}: encontrada {how}, sin gastar una búsqueda.")
            } else {
                format!("{t}: found {how}, without using a search.")
            }
        }
        "browsed" => {
            if es {
                format!("La carpeta de {p} tiene {count} canciones más de la lista: no hace falta buscarlas.")
            } else {
                format!("{p}'s folder has {count} more songs of the list: no need to search for them.")
            }
        }
        "memory" => {
            let peers = message;
            if es {
                format!("Se miró en lo que comparten {peers} usuarios que ya te mandaron buenos archivos: {count} canciones encontradas sin buscar.")
            } else {
                format!("Looked in the shares of {peers} peers that sent you good files before: {count} songs found without a search.")
            }
        }
        "stopping" => l.t("Stopping: the finished songs are converted and kept.", "Frenando: las canciones terminadas se convierten y se guardan.").into(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_has_words_in_both_languages() {
        for code in [
            "signed_in", "no_account", "login_failed", "offline", "reconnecting", "reconnected", "search_limit", "found", "no_files", "queued", "peer_failed", "gave_up",
            "downloaded", "yt_done", "yt_no_match", "yt_blocked", "yt_failed", "missing", "convert_failed", "stopping", "found_free", "browsed", "memory",
        ] {
            for l in [Lang::En, Lang::Es] {
                assert!(!word(l, code, "A – B", "peer", "", 3, 40).is_empty(), "{code} {l:?}");
            }
        }
        assert!(word(Lang::En, "offline", "", "", "relogged", 0, 0).contains("somewhere else"));
        assert!(word(Lang::Es, "peer_failed", "A – B", "zwem", "queued", 0, 0).contains("cola"));
        assert!(word(Lang::En, "unknown", "", "", "", 0, 0).is_empty());
    }
}
