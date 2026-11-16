//! Coloration des mots-clés dans les terminaux, à la manière de MobaXterm
//! (« Standard keywords (OK/warning/error) ») : erreurs en rouge,
//! avertissements en jaune, succès en vert.
//!
//! Seuls les mots entiers sont reconnus, sans tenir compte de la casse. La
//! coloration ne s'applique qu'au texte sans couleur propre : la sortie déjà
//! colorée par un programme (`ls --color`, `systemctl`…) est respectée.

use std::ops::Range;

use eframe::egui::Color32;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Couleur d'une liste, prise dans la palette ANSI du thème (lisible en clair
/// comme en sombre).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HighlightColor {
    Red,
    Yellow,
    Green,
    Blue,
    Magenta,
    Cyan,
}

impl HighlightColor {
    pub const ALL: [HighlightColor; 6] = [Self::Red, Self::Yellow, Self::Green, Self::Blue, Self::Magenta, Self::Cyan];

    pub fn label(self) -> &'static str {
        match self {
            Self::Red => "Rouge",
            Self::Yellow => "Jaune",
            Self::Green => "Vert",
            Self::Blue => "Bleu",
            Self::Magenta => "Violet",
            Self::Cyan => "Cyan",
        }
    }

    /// Couleur à l'écran, selon le thème actuel.
    pub fn color(self) -> Color32 {
        let ansi = crate::theme::p().ansi;
        // Variantes vives (9 à 14) : plus lisibles sur fond sombre comme clair.
        ansi[match self {
            Self::Red => 9,
            Self::Green => 10,
            Self::Yellow => 11,
            Self::Blue => 12,
            Self::Magenta => 13,
            Self::Cyan => 14,
        }]
    }
}

/// Une liste de mots-clés et sa couleur.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HighlightRule {
    pub name: String,
    pub color: HighlightColor,
    /// Mots ou expressions (« permission denied »), sans distinction de casse.
    pub words: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// Réglages enregistrés avec les sessions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Highlighting {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "default_rules")]
    pub rules: Vec<HighlightRule>,
}

impl Default for Highlighting {
    fn default() -> Self {
        Self { enabled: true, rules: default_rules() }
    }
}

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|w| w.to_string()).collect()
}

/// Listes par défaut, inspirées du jeu « Standard keywords » de MobaXterm,
/// en anglais et en français. L'ordre compte : en cas de chevauchement, la
/// première liste l'emporte (une erreur passe avant un avertissement).
pub fn default_rules() -> Vec<HighlightRule> {
    vec![
        HighlightRule {
            name: "Erreurs".into(),
            color: HighlightColor::Red,
            enabled: true,
            words: words(&[
                "error", "errors", "erreur", "erreurs", "fail", "failed", "failure", "failures", "fatal",
                "critical", "panic", "exception", "traceback", "denied", "permission denied", "access denied",
                "refused", "connection refused", "invalid", "not found", "no such file or directory",
                "introuvable", "échec", "échoué", "segfault", "segmentation fault", "core dumped", "corrupt",
                "corrupted", "aborted", "killed", "unreachable", "out of memory", "not permitted",
                "unauthorized", "forbidden", "rejected",
            ]),
        },
        HighlightRule {
            name: "Avertissements".into(),
            color: HighlightColor::Yellow,
            enabled: true,
            words: words(&[
                "warn", "warning", "warnings", "avertissement", "attention", "alert", "deprecated", "obsolète",
                "could not", "unable to", "impossible de", "cannot", "unknown", "inconnu", "timeout",
                "timed out", "retrying", "disabled", "désactivé", "disconnected", "déconnecté", "stopped",
                "arrêté", "inactive", "skipped", "ignored", "expired", "expiré", "unexpected", "inattendu",
            ]),
        },
        HighlightRule {
            name: "Succès".into(),
            color: HighlightColor::Green,
            enabled: true,
            words: words(&[
                "ok", "success", "successful", "successfully", "succeeded", "succès", "réussi", "réussie",
                "done", "terminé", "passed", "completed", "enabled", "activé", "connected", "connecté",
                "running", "started", "démarré", "listening", "accepted", "established", "healthy", "ready",
            ]),
        },
    ]
}

/// Listes compilées, prêtes pour l'affichage.
pub struct Highlighter {
    rules: Vec<(Regex, HighlightColor)>,
}

impl Highlighter {
    /// `None` si la coloration est désactivée ou qu'aucune liste n'est utilisable.
    pub fn new(settings: &Highlighting) -> Option<Self> {
        if !settings.enabled {
            return None;
        }
        let rules: Vec<_> = settings
            .rules
            .iter()
            .filter(|rule| rule.enabled)
            .filter_map(|rule| Some((compile(&rule.words)?, rule.color)))
            .collect();
        (!rules.is_empty()).then_some(Self { rules })
    }

    /// Plages (en octets) à colorer dans `text`, sans chevauchement ; la
    /// première liste l'emporte.
    pub fn find(&self, text: &str) -> Vec<(Range<usize>, HighlightColor)> {
        let mut found: Vec<(Range<usize>, HighlightColor)> = Vec::new();
        for (regex, color) in &self.rules {
            for m in regex.find_iter(text) {
                let range = m.range();
                if !found.iter().any(|(r, _)| r.start < range.end && range.start < r.end) {
                    found.push((range, *color));
                }
            }
        }
        found.sort_by_key(|(r, _)| r.start);
        found
    }
}

/// Une expression régulière par liste : les mots entiers, du plus long au plus
/// court (« permission denied » avant « denied »), espaces souples.
fn compile(words: &[String]) -> Option<Regex> {
    let mut words: Vec<&str> = words.iter().map(|w| w.trim()).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return None;
    }
    words.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
    let alternatives: Vec<String> = words
        .iter()
        .map(|w| w.split_whitespace().map(regex::escape).collect::<Vec<_>>().join(r"\s+"))
        .collect();
    // `\b` ne vaut qu'à côté d'un caractère de mot : un mot-clé qui commence ou
    // finit par un signe (« (ee) ») est accepté tel quel de ce côté-là.
    let pattern = alternatives
        .iter()
        .zip(&words)
        .map(|(alt, word)| {
            let is_word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            let start = if is_word(word.chars().next()) { r"\b" } else { "" };
            let end = if is_word(word.chars().last()) { r"\b" } else { "" };
            format!("{start}{alt}{end}")
        })
        .collect::<Vec<_>>()
        .join("|");
    Regex::new(&format!("(?i)(?:{pattern})")).ok()
}

/// Texte d'une liste dans l'éditeur : un mot ou une expression par ligne.
pub fn words_to_text(words: &[String]) -> String {
    words.join("\n")
}

/// Lecture de l'éditeur : un mot ou une expression par ligne (ou séparés par
/// des virgules), lignes vides ignorées.
pub fn text_to_words(text: &str) -> Vec<String> {
    text.split(['\n', ','])
        .map(|w| w.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|w| !w.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(text: &str) -> Vec<(String, HighlightColor)> {
        let highlighter = Highlighter::new(&Highlighting::default()).unwrap();
        highlighter.find(text).into_iter().map(|(r, c)| (text[r].to_string(), c)).collect()
    }

    #[test]
    fn standard_keywords_are_coloured() {
        use HighlightColor::*;
        assert_eq!(
            matches("[OK] nginx started; WARNING: disk almost full; Error: permission denied"),
            vec![
                ("OK".into(), Green),
                ("started".into(), Green),
                ("WARNING".into(), Yellow),
                ("Error".into(), Red),
                ("permission denied".into(), Red),
            ]
        );
    }

    #[test]
    fn only_whole_words_match() {
        // « error_log », « okay », « inactive » ne contiennent pas de mot-clé entier
        // (sauf « inactive », qui est lui-même un avertissement).
        assert_eq!(matches("error_log okay terrors"), vec![]);
        assert_eq!(matches("inactive"), vec![("inactive".into(), HighlightColor::Yellow)]);
        assert_eq!(matches("Échec de la connexion"), vec![("Échec".into(), HighlightColor::Red)]);
    }

    #[test]
    fn phrases_allow_flexible_spaces_and_win_over_words() {
        assert_eq!(matches("No  such file or directory"), vec![("No  such file or directory".into(), HighlightColor::Red)]);
        // Chevauchement « could not » (avertissement) / « not found » (erreur) :
        // la liste des erreurs passe en premier.
        assert_eq!(matches("could not found"), vec![("not found".into(), HighlightColor::Red)]);
    }

    #[test]
    fn disabled_settings_give_no_highlighter() {
        let mut settings = Highlighting::default();
        settings.enabled = false;
        assert!(Highlighter::new(&settings).is_none());
        let mut settings = Highlighting::default();
        settings.rules.iter_mut().for_each(|r| r.words.clear());
        assert!(Highlighter::new(&settings).is_none());
    }

    #[test]
    fn editor_text_round_trip() {
        assert_eq!(text_to_words("error,  fatal\n\n  permission   denied \n"), vec!["error", "fatal", "permission denied"]);
        assert_eq!(words_to_text(&text_to_words("a\nb")), "a\nb");
    }
}
