//! Édition de fichiers distants : copie locale dans un dossier temporaire,
//! ouverture dans un éditeur de texte, détection des enregistrements.
//!
//! Comme MobaXterm, chaque enregistrement est proposé au renvoi sur le
//! serveur (avec option d'envoi automatique pour la suite).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

/// Taille maximale d'un fichier ouvert en édition.
pub const MAX_EDIT_SIZE: u64 = 10 * 1024 * 1024;

/// Délai pendant lequel le fichier local ne doit plus bouger avant d'être
/// considéré comme enregistré (certains éditeurs écrivent en plusieurs fois).
const SETTLE: Duration = Duration::from_millis(500);

/// État d'un fichier local : date de modification et taille.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    modified: SystemTime,
    len: u64,
}

impl Stamp {
    pub fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self { modified: meta.modified().ok()?, len: meta.len() })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditState {
    /// Téléchargement de la copie locale en cours.
    Downloading,
    /// Copie locale à jour par rapport au serveur.
    Synced,
    /// Modifiée et enregistrée : en attente de la réponse de l'utilisateur.
    AskUpload,
    /// Renvoi en cours, de la version `Stamp`.
    Uploading(Stamp),
    /// La copie locale contient des marqueurs de conflit à résoudre.
    Conflicted,
    /// Dernier renvoi en échec.
    Failed(String),
}

/// Fichier distant ouvert en édition.
pub struct EditedFile {
    /// Onglet (connexion) d'où vient le fichier.
    pub tab_id: u64,
    /// `utilisateur@hôte`, pour les messages.
    pub host: String,
    pub remote: String,
    pub local: PathBuf,
    pub state: EditState,
    /// Renvoi sans confirmation des prochains enregistrements.
    pub auto_upload: bool,
    /// Dernier événement notable (fusion, version du serveur reprise…), affiché
    /// à côté de l'état.
    pub note: Option<String>,
    /// Version locale déjà traitée (téléchargée, envoyée ou refusée).
    synced: Option<Stamp>,
    /// Version vue pour la première fois à cet instant, pas encore stable.
    seen: Option<(Stamp, Instant)>,
    /// Base de la fusion à trois versions : dernier contenu connu du serveur.
    base: Vec<u8>,
    /// Contenu en cours d'envoi.
    sending: Vec<u8>,
}

impl EditedFile {
    pub fn new(tab_id: u64, host: String, remote: String, local: PathBuf) -> Self {
        Self {
            tab_id,
            host,
            remote,
            local,
            state: EditState::Downloading,
            auto_upload: false,
            note: None,
            synced: None,
            seen: None,
            base: Vec::new(),
            sending: Vec::new(),
        }
    }

    /// Nom du fichier (dernier segment du chemin distant).
    pub fn name(&self) -> &str {
        self.remote.rsplit('/').next().unwrap_or(&self.remote)
    }

    /// La copie locale vient d'être téléchargée : c'est la base des fusions.
    pub fn downloaded(&mut self) {
        self.base = std::fs::read(&self.local).unwrap_or_default();
        self.synced = Stamp::of(&self.local);
        self.state = EditState::Synced;
    }

    /// Vrai si la copie locale contient des modifications non envoyées.
    pub fn has_unsent_changes(&self) -> bool {
        Stamp::of(&self.local).is_some_and(|stamp| Some(stamp) != self.synced)
    }

    /// À appeler régulièrement : vrai quand un enregistrement vient d'être
    /// détecté (fichier modifié, puis stable pendant `SETTLE`).
    pub fn poll(&mut self, now: Instant) -> bool {
        if !matches!(self.state, EditState::Synced | EditState::Conflicted | EditState::Failed(_)) {
            return false;
        }
        let Some(stamp) = Stamp::of(&self.local) else { return false };
        if Some(stamp) == self.synced {
            self.seen = None;
            return false;
        }
        match self.seen {
            Some((seen, since)) if seen == stamp => now.duration_since(since) >= SETTLE,
            _ => {
                self.seen = Some((stamp, now));
                false
            }
        }
    }

    /// Le renvoi de la version locale actuelle commence : renvoie la base et
    /// le contenu à envoyer, pour la fusion côté connexion.
    pub fn upload_started(&mut self) -> Option<(Vec<u8>, Vec<u8>)> {
        let stamp = Stamp::of(&self.local)?;
        let ours = std::fs::read(&self.local).ok()?;
        self.seen = None;
        self.note = None;
        self.state = EditState::Uploading(stamp);
        self.sending = ours.clone();
        Some((self.base.clone(), ours))
    }

    /// Fin du renvoi. Renvoie un message à afficher (conflit, échec de fusion).
    ///
    /// Si la copie locale a été réenregistrée pendant l'envoi, elle n'est pas
    /// écrasée : sa version est traitée au tour suivant, avec pour base le
    /// contenu envoyé.
    pub fn upload_finished(&mut self, result: Result<SaveOutcome, String>) -> Option<String> {
        let EditState::Uploading(stamp) = self.state else { return None };
        let sent = std::mem::take(&mut self.sending);
        let untouched = Stamp::of(&self.local) == Some(stamp);
        self.synced = Some(stamp);
        self.state = EditState::Synced;
        // Remplace la copie locale (si elle n'a pas bougé) ; l'éditeur la recharge.
        let mut replace_local = |content: &[u8]| -> bool {
            if untouched && std::fs::write(&self.local, content).is_ok() {
                self.synced = Stamp::of(&self.local);
                true
            } else {
                false
            }
        };
        match result {
            Ok(SaveOutcome::Uploaded | SaveOutcome::AlreadyThere) => {
                self.base = sent;
                None
            }
            Ok(SaveOutcome::Pulled(theirs)) => {
                if replace_local(&theirs) {
                    self.base = theirs;
                    self.note = Some("version du serveur reprise".into());
                }
                None
            }
            Ok(SaveOutcome::Merged(merged)) => {
                // Le serveur a maintenant la fusion ; sans réécriture locale, la
                // copie locale descend de `sent`, qui sert de base au tour suivant.
                if replace_local(&merged) {
                    self.base = merged;
                } else {
                    self.base = sent;
                    self.synced = None;
                }
                self.note = Some("fusionné avec les modifications du serveur".into());
                None
            }
            Ok(SaveOutcome::Conflict { marked, theirs }) => {
                if !replace_local(&marked) {
                    // Réenregistrée entre-temps : nouvelle tentative au tour suivant.
                    self.synced = None;
                    return None;
                }
                self.base = theirs;
                self.state = EditState::Conflicted;
                Some(format!(
                    "« {} » a aussi été modifié sur le serveur, aux mêmes lignes. Rien n'a été envoyé.\n\n\
                     Les deux versions sont maintenant dans votre éditeur, entre les marqueurs\n\
                     <<<<<<< vos modifications / ======= / >>>>>>> version du serveur.\n\
                     Gardez le bon texte, supprimez les marqueurs, puis enregistrez.",
                    self.name()
                ))
            }
            Ok(SaveOutcome::Unresolved) => {
                self.state = EditState::Conflicted;
                Some(format!(
                    "« {} » contient encore des marqueurs de conflit (<<<<<<< … >>>>>>>). \
                     Résolvez-les puis enregistrez : rien n'a été envoyé.",
                    self.name()
                ))
            }
            Ok(SaveOutcome::Diverged) => {
                self.state = EditState::Failed("modifié sur le serveur, fusion impossible (contenu non textuel)".into());
                None
            }
            Err(e) => {
                self.state = EditState::Failed(e);
                None
            }
        }
    }

    /// L'utilisateur refuse d'envoyer cette version : elle n'est plus proposée.
    pub fn skip_version(&mut self) {
        self.synced = Stamp::of(&self.local);
        self.seen = None;
        self.state = EditState::Synced;
    }

    /// Supprime la copie locale et son dossier temporaire.
    pub fn remove_local_copy(&self) {
        let _ = std::fs::remove_file(&self.local);
        if let Some(dir) = self.local.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

/// Résultat d'un renvoi, décidé par une fusion à trois versions (comme git) :
/// la base (version téléchargée), la nôtre (copie locale) et la leur (serveur).
#[derive(Clone, Debug, PartialEq)]
pub enum SaveOutcome {
    /// Le serveur n'avait pas changé : la version locale a été envoyée.
    Uploaded,
    /// Le serveur avait déjà exactement cette version : rien à envoyer.
    AlreadyThere,
    /// Copie locale inchangée mais serveur modifié : la copie locale reprend
    /// la version du serveur (rien n'est envoyé).
    Pulled(Vec<u8>),
    /// Modifications des deux côtés, à des endroits différents : fusionnées,
    /// puis envoyées. La copie locale reprend le résultat.
    Merged(Vec<u8>),
    /// Modifications des deux côtés aux mêmes lignes : rien n'est envoyé ; la
    /// copie locale reçoit les marqueurs de conflit, la nouvelle base est la
    /// version du serveur.
    Conflict { marked: Vec<u8>, theirs: Vec<u8> },
    /// La copie locale contient encore des marqueurs de conflit : rien n'est envoyé.
    Unresolved,
    /// Serveur modifié et contenu non textuel : fusion impossible, rien n'est envoyé.
    Diverged,
}

/// Libellés des marqueurs de conflit (ceux de `diffy` sont « ours » et « theirs »).
const OURS_MARKER: (&str, &str) = ("<<<<<<< ours", "<<<<<<< vos modifications");
const THEIRS_MARKER: (&str, &str) = (">>>>>>> theirs", ">>>>>>> version du serveur");

/// Décide quoi faire d'un enregistrement : contenu à écrire sur le serveur
/// (s'il y en a) et résultat. `theirs` = `None` si le fichier n'existe plus
/// sur le serveur (il est recréé).
pub fn plan_save(base: &[u8], ours: &[u8], theirs: Option<&[u8]>) -> (Option<Vec<u8>>, SaveOutcome) {
    if has_conflict_markers(ours) {
        return (None, SaveOutcome::Unresolved);
    }
    let Some(theirs) = theirs else { return (Some(ours.to_vec()), SaveOutcome::Uploaded) };
    if theirs == base {
        return (Some(ours.to_vec()), SaveOutcome::Uploaded);
    }
    if theirs == ours {
        return (None, SaveOutcome::AlreadyThere);
    }
    if ours == base {
        return (None, SaveOutcome::Pulled(theirs.to_vec()));
    }
    let (Ok(base), Ok(ours_text), Ok(theirs_text)) =
        (std::str::from_utf8(base), std::str::from_utf8(ours), std::str::from_utf8(theirs))
    else {
        return (None, SaveOutcome::Diverged);
    };
    let mut options = diffy::MergeOptions::new();
    options.set_conflict_style(diffy::ConflictStyle::Merge);
    match options.merge(base, ours_text, theirs_text) {
        Ok(merged) => (Some(merged.clone().into_bytes()), SaveOutcome::Merged(merged.into_bytes())),
        Err(conflicted) => {
            let marked = conflicted.replace(OURS_MARKER.0, OURS_MARKER.1).replace(THEIRS_MARKER.0, THEIRS_MARKER.1);
            (None, SaveOutcome::Conflict { marked: marked.into_bytes(), theirs: theirs.to_vec() })
        }
    }
}

/// Vrai si le texte contient un bloc de conflit (ligne d'ouverture et de
/// fermeture) : un simple `=======` (soulignement Markdown) ne compte pas.
pub fn has_conflict_markers(text: &[u8]) -> bool {
    let text = String::from_utf8_lossy(text);
    let mut open = false;
    for line in text.lines() {
        if line.starts_with("<<<<<<< ") {
            open = true;
        } else if open && line.starts_with(">>>>>>> ") {
            return true;
        }
    }
    false
}

/// Emplacement de la copie locale : un dossier temporaire par édition, pour
/// garder le nom du fichier (l'éditeur l'affiche et choisit sa coloration).
pub fn local_path(edit_id: u64, remote: &str) -> PathBuf {
    let name = remote.rsplit('/').next().unwrap_or(remote);
    // Caractères permis sous Linux mais interdits dans un nom de fichier Windows.
    let safe: String = name
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*') || c.is_control() { '_' } else { c })
        .collect();
    let safe = if safe.trim_matches(['.', ' ']).is_empty() { "fichier".to_string() } else { safe };
    std::env::temp_dir()
        .join("ferroterm-edit")
        .join(format!("{}-{edit_id}", std::process::id()))
        .join(safe)
}

/// Vrai si le fichier semble binaire (octet nul dans les premiers 8 Ko).
pub fn looks_binary(path: &Path) -> bool {
    use std::io::Read;
    let mut buffer = [0u8; 8192];
    let Ok(mut file) = std::fs::File::open(path) else { return false };
    let n = file.read(&mut buffer).unwrap_or(0);
    buffer[..n].contains(&0)
}

/// Éditeur de texte : VS Code, sinon Notepad++, sinon le Bloc-notes.
/// L'association Windows de l'extension n'est volontairement pas utilisée :
/// pour un `.sh`, `.bat` ou `.py`, elle exécuterait le script au lieu de l'ouvrir.
pub fn editor() -> &'static (PathBuf, &'static str) {
    static EDITOR: OnceLock<(PathBuf, &'static str)> = OnceLock::new();
    EDITOR.get_or_init(|| {
        let env = |name: &str| std::env::var_os(name).map(PathBuf::from);
        let candidates = [
            (env("LOCALAPPDATA").map(|d| d.join(r"Programs\Microsoft VS Code\Code.exe")), "VS Code"),
            (env("ProgramFiles").map(|d| d.join(r"Microsoft VS Code\Code.exe")), "VS Code"),
            (env("ProgramFiles").map(|d| d.join(r"Notepad++\notepad++.exe")), "Notepad++"),
            (env("ProgramFiles(x86)").map(|d| d.join(r"Notepad++\notepad++.exe")), "Notepad++"),
        ];
        candidates
            .into_iter()
            .find_map(|(path, name)| path.filter(|p| p.is_file()).map(|p| (p, name)))
            .unwrap_or_else(|| (PathBuf::from("notepad.exe"), "Bloc-notes"))
    })
}

/// Ouvre la copie locale dans l'éditeur.
pub fn open_in_editor(path: &Path) -> std::io::Result<()> {
    std::process::Command::new(&editor().0).arg(path).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_path_keeps_name_and_replaces_forbidden_characters() {
        let path = local_path(3, "/etc/nginx/nginx.conf");
        assert_eq!(path.file_name().unwrap(), "nginx.conf");
        let path = local_path(4, "/tmp/a:b?.log");
        assert_eq!(path.file_name().unwrap(), "a_b_.log");
        assert_ne!(local_path(5, "/x/f").parent(), local_path(6, "/x/f").parent());
    }

    #[test]
    fn detects_a_save_once_the_file_is_stable() {
        let dir = std::env::temp_dir().join(format!("ferroterm-edit-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let local = dir.join("f.txt");
        std::fs::write(&local, "avant").unwrap();
        let mut edit = EditedFile::new(1, "moi@hote".into(), "/f.txt".into(), local.clone());
        edit.downloaded();
        let start = Instant::now();
        assert!(!edit.poll(start), "fichier inchangé");

        std::fs::write(&local, "après, plus long").unwrap();
        assert!(!edit.poll(start), "changement pas encore stable");
        assert!(edit.poll(start + SETTLE), "changement stable : enregistrement détecté");
        assert!(edit.has_unsent_changes());

        // Renvoi réussi : plus rien à proposer.
        edit.upload_started().unwrap();
        assert!(!edit.poll(start + SETTLE * 2), "pas de détection pendant l'envoi");
        assert_eq!(edit.upload_finished(Ok(SaveOutcome::Uploaded)), None);
        assert_eq!(edit.state, EditState::Synced);
        assert!(!edit.has_unsent_changes());
        assert!(!edit.poll(start + SETTLE * 3));

        // Version refusée : elle n'est plus proposée.
        std::fs::write(&local, "encore une autre version").unwrap();
        assert!(!edit.poll(start + SETTLE * 4));
        assert!(edit.poll(start + SETTLE * 5));
        edit.skip_version();
        assert!(!edit.poll(start + SETTLE * 6));

        edit.remove_local_copy();
        assert!(!local.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    const BASE: &str = "un\ndeux\ntrois\nquatre\ncinq\n";

    #[test]
    fn server_unchanged_uploads_ours() {
        let ours = "un\nDEUX\ntrois\nquatre\ncinq\n";
        let (write, outcome) = plan_save(BASE.as_bytes(), ours.as_bytes(), Some(BASE.as_bytes()));
        assert_eq!(write.as_deref(), Some(ours.as_bytes()));
        assert_eq!(outcome, SaveOutcome::Uploaded);
        // Fichier supprimé sur le serveur : il est recréé.
        let (write, outcome) = plan_save(BASE.as_bytes(), ours.as_bytes(), None);
        assert_eq!(write.as_deref(), Some(ours.as_bytes()));
        assert_eq!(outcome, SaveOutcome::Uploaded);
    }

    #[test]
    fn changes_on_different_lines_are_merged() {
        let ours = "un\nDEUX\ntrois\nquatre\ncinq\n";
        let theirs = "un\ndeux\ntrois\nquatre\nCINQ\n";
        let (write, outcome) = plan_save(BASE.as_bytes(), ours.as_bytes(), Some(theirs.as_bytes()));
        let merged = "un\nDEUX\ntrois\nquatre\nCINQ\n";
        assert_eq!(write.as_deref(), Some(merged.as_bytes()));
        assert_eq!(outcome, SaveOutcome::Merged(merged.as_bytes().to_vec()));
    }

    #[test]
    fn changes_on_the_same_line_conflict() {
        let ours = "un\ndeux\nTROIS (moi)\nquatre\ncinq\n";
        let theirs = "un\ndeux\nTROIS (serveur)\nquatre\ncinq\n";
        let (write, outcome) = plan_save(BASE.as_bytes(), ours.as_bytes(), Some(theirs.as_bytes()));
        assert_eq!(write, None, "rien n'est envoyé en cas de conflit");
        let SaveOutcome::Conflict { marked, theirs: new_base } = outcome else { panic!("conflit attendu") };
        let marked = String::from_utf8(marked).unwrap();
        assert!(marked.contains("<<<<<<< vos modifications\nTROIS (moi)\n=======\nTROIS (serveur)\n>>>>>>> version du serveur\n"), "{marked}");
        assert_eq!(new_base, theirs.as_bytes());
        // Tant que les marqueurs restent, rien n'est envoyé.
        let (write, outcome) = plan_save(&new_base, marked.as_bytes(), Some(&new_base));
        assert_eq!((write, outcome), (None, SaveOutcome::Unresolved));
        // Conflit résolu : envoi normal (le serveur n'a pas rebougé).
        let resolved = "un\ndeux\nTROIS (les deux)\nquatre\ncinq\n";
        let (write, outcome) = plan_save(&new_base, resolved.as_bytes(), Some(&new_base));
        assert_eq!(write.as_deref(), Some(resolved.as_bytes()));
        assert_eq!(outcome, SaveOutcome::Uploaded);
    }

    #[test]
    fn identical_or_untouched_versions() {
        let theirs = "un\ndeux\ntrois\nquatre\nCINQ\n";
        // Même modification des deux côtés : rien à envoyer.
        let (write, outcome) = plan_save(BASE.as_bytes(), theirs.as_bytes(), Some(theirs.as_bytes()));
        assert_eq!((write, outcome), (None, SaveOutcome::AlreadyThere));
        // Copie locale réenregistrée sans changement : elle reprend la version du serveur.
        let (write, outcome) = plan_save(BASE.as_bytes(), BASE.as_bytes(), Some(theirs.as_bytes()));
        assert_eq!((write, outcome), (None, SaveOutcome::Pulled(theirs.as_bytes().to_vec())));
    }

    #[test]
    fn non_text_cannot_be_merged() {
        let (write, outcome) = plan_save(b"a\xff\n", b"b\xff\n", Some(b"c\xff\n"));
        assert_eq!((write, outcome), (None, SaveOutcome::Diverged));
    }

    /// Cycle complet sur une vraie copie locale : fusion, puis conflit, puis résolution.
    #[test]
    fn merge_and_conflict_update_the_local_copy() {
        let dir = std::env::temp_dir().join(format!("ferroterm-merge-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let local = dir.join("app.conf");
        std::fs::write(&local, BASE).unwrap();
        let mut edit = EditedFile::new(1, "moi@hote".into(), "/etc/app.conf".into(), local.clone());
        edit.downloaded();

        // Fusion : le serveur a changé la ligne 5, nous la ligne 2.
        std::fs::write(&local, "un\nDEUX\ntrois\nquatre\ncinq\n").unwrap();
        let (base, ours) = edit.upload_started().unwrap();
        let (_, outcome) = plan_save(&base, &ours, Some(b"un\ndeux\ntrois\nquatre\nCINQ\n"));
        assert_eq!(edit.upload_finished(Ok(outcome)), None);
        assert_eq!(std::fs::read_to_string(&local).unwrap(), "un\nDEUX\ntrois\nquatre\nCINQ\n", "l'éditeur voit la fusion");
        assert_eq!(edit.state, EditState::Synced);
        assert!(!edit.has_unsent_changes(), "la réécriture locale n'est pas un nouvel enregistrement");

        // Conflit : la ligne 3 modifiée des deux côtés.
        let server = "un\nDEUX\nTROIS (serveur)\nquatre\nCINQ\n";
        std::fs::write(&local, "un\nDEUX\nTROIS (moi)\nquatre\nCINQ\n").unwrap();
        let (base, ours) = edit.upload_started().unwrap();
        let (write, outcome) = plan_save(&base, &ours, Some(server.as_bytes()));
        assert_eq!(write, None);
        assert!(edit.upload_finished(Ok(outcome)).is_some(), "message de conflit");
        assert_eq!(edit.state, EditState::Conflicted);
        assert!(has_conflict_markers(&std::fs::read(&local).unwrap()));

        // Résolution : la base est désormais la version du serveur, l'envoi passe.
        std::fs::write(&local, "un\nDEUX\nTROIS (les deux)\nquatre\nCINQ\n").unwrap();
        let (base, ours) = edit.upload_started().unwrap();
        assert_eq!(base, server.as_bytes());
        let (write, outcome) = plan_save(&base, &ours, Some(server.as_bytes()));
        assert_eq!(write.as_deref(), Some(ours.as_slice()));
        assert_eq!(outcome, SaveOutcome::Uploaded);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn markdown_underline_is_not_a_conflict() {
        assert!(!has_conflict_markers(b"Titre\n=======\ntexte\n"));
        assert!(has_conflict_markers(b"<<<<<<< a\nx\n=======\ny\n>>>>>>> b\n"));
    }

    #[test]
    fn binary_detection() {
        let dir = std::env::temp_dir().join(format!("ferroterm-bin-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = dir.join("t.txt");
        let bin = dir.join("b.bin");
        std::fs::write(&text, "bonjour\n").unwrap();
        std::fs::write(&bin, [0x7f, b'E', b'L', b'F', 0, 1]).unwrap();
        assert!(!looks_binary(&text));
        assert!(looks_binary(&bin));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
