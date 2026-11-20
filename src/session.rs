//! Sessions SSH enregistrées et leur stockage sur disque (JSON).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub id: u64,
    pub name: String,
    /// Dossier de rangement, segments séparés par `/`. Vide = racine.
    #[serde(default)]
    pub folder: String,
    pub host: String,
    pub port: u16,
    /// Vide = demandé à la connexion.
    #[serde(default)]
    pub username: String,
    /// Clé privée OpenSSH. `None` = authentification par mot de passe.
    #[serde(default)]
    pub key_path: Option<String>,
    /// Profils de connexion, dans l'ordre d'affichage (vide = le seul profil
    /// principal). Les champs ci-dessus sont ceux du profil actif, et font foi
    /// pour lui : son entrée ici ne sert qu'à garder son nom et sa place.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<Profile>,
    /// Profil actif (0 = profil principal).
    #[serde(default)]
    pub profile: u64,
}

/// Profil de connexion d'une session : où et comment se connecter. Une même
/// session peut en avoir plusieurs (ex. « Principal » et « Prod »), un seul
/// étant actif à la fois.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Profile {
    /// Unique dans la session ; 0 = profil principal.
    pub id: u64,
    pub name: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub key_path: Option<String>,
}

impl Profile {
    /// `utilisateur@hôte[:port]`, pour l'affichage.
    pub fn target(&self) -> String {
        target(&self.username, &self.host, self.port)
    }
}

fn target(username: &str, host: &str, port: u16) -> String {
    let user = if username.is_empty() { String::new() } else { format!("{username}@") };
    if port == 22 { format!("{user}{host}") } else { format!("{user}{host}:{port}") }
}

/// Nom du profil créé avec chaque session.
pub const MAIN_PROFILE: &str = "Principal";

impl Session {
    /// Profil actif, d'après les champs de connexion de la session.
    pub fn active_profile(&self) -> Profile {
        Profile {
            id: self.profile,
            name: self.profile_name().to_string(),
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            key_path: self.key_path.clone(),
        }
    }

    /// Nom du profil actif.
    pub fn profile_name(&self) -> &str {
        self.profiles
            .iter()
            .find(|p| p.id == self.profile)
            .map(|p| p.name.as_str())
            .unwrap_or(MAIN_PROFILE)
    }

    /// Vrai si la session a plusieurs profils (le nom du profil actif est alors affiché).
    pub fn has_profiles(&self) -> bool {
        self.profiles.len() > 1
    }

    /// Tous les profils, dans l'ordre, le profil actif à jour.
    pub fn profiles(&self) -> Vec<Profile> {
        if !self.profiles.iter().any(|p| p.id == self.profile) {
            let mut all = self.profiles.clone();
            all.insert(0, self.active_profile());
            return all;
        }
        self.profiles
            .iter()
            .map(|p| if p.id == self.profile { self.active_profile() } else { p.clone() })
            .collect()
    }

    /// Remplace les profils et active `active` (le premier s'il n'existe pas).
    pub fn set_profiles(&mut self, profiles: Vec<Profile>, active: u64) {
        let Some(current) = profiles.iter().find(|p| p.id == active).or(profiles.first()).cloned() else {
            return;
        };
        self.profile = current.id;
        self.host = current.host;
        self.port = current.port;
        self.username = current.username;
        self.key_path = current.key_path;
        // Un seul profil, principal et non renommé : rien à mémoriser.
        let plain = profiles.len() == 1 && current.id == 0 && current.name == MAIN_PROFILE;
        self.profiles = if plain { Vec::new() } else { profiles };
    }

    /// Active un autre profil. Renvoie `false` s'il n'existe pas.
    pub fn switch_profile(&mut self, id: u64) -> bool {
        let profiles = self.profiles();
        if !profiles.iter().any(|p| p.id == id) {
            return false;
        }
        self.set_profiles(profiles, id);
        true
    }

    /// Identifiant libre pour un nouveau profil.
    pub fn next_profile_id(profiles: &[Profile]) -> u64 {
        profiles.iter().map(|p| p.id + 1).max().unwrap_or(1).max(1)
    }

    /// `utilisateur@hôte[:port]` du profil actif, pour l'affichage.
    pub fn target(&self) -> String {
        target(&self.username, &self.host, self.port)
    }
}

/// Macro : suite de commandes envoyée au terminal actif, une ligne à la fois.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Macro {
    pub id: u64,
    pub name: String,
    /// Une commande par ligne ; chaque ligne est suivie d'Entrée à l'exécution.
    pub commands: String,
    /// Lancée (double-clic ou Entrée), la macro est exécutée ; sinon elle est
    /// seulement posée sur la ligne de commande.
    #[serde(default, alias = "run_on_click")]
    pub auto_run: bool,
}

impl Macro {
    /// Lignes à envoyer : les lignes vides sont ignorées.
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.commands.lines().map(|l| l.trim_end()).filter(|l| !l.trim().is_empty())
    }

    /// Octets à écrire dans le terminal : chaque ligne suivie d'un retour chariot.
    pub fn input(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for line in self.lines() {
            bytes.extend_from_slice(line.as_bytes());
            bytes.push(b'\r');
        }
        bytes
    }

    /// Texte à poser sur la ligne de commande sans l'exécuter : les lignes
    /// sont jointes par « ; » pour qu'aucun retour chariot ne les lance.
    pub fn command_line(&self) -> String {
        self.lines().map(str::trim).collect::<Vec<_>>().join("; ")
    }
}

/// Onglet mémorisé pour être rouvert au prochain démarrage.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SavedTab {
    /// Groupe d'onglets (scission), de gauche à droite à partir de 0.
    pub group: usize,
    /// Session SSH (par id) ; `None` = terminal local.
    #[serde(default)]
    pub session: Option<u64>,
    /// Profil de connexion utilisé par l'onglet ; `None` = profil actif.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<u64>,
    /// Nom du shell local, si ce n'est pas une session SSH.
    #[serde(default)]
    pub shell: Option<String>,
    /// Onglet affiché dans son groupe.
    #[serde(default)]
    pub active: bool,
    /// Nom donné à l'onglet ; `None` = nom de la session ou du shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Disposition des groupes d'onglets à l'écran.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Arrangement {
    /// Côte à côte, de gauche à droite.
    #[default]
    Columns,
    /// Empilés, de haut en bas.
    Rows,
    /// En grille : des rangées empilées, chacune avec ses propres colonnes.
    Grid,
}

/// Disposition des terminaux à la fermeture : onglets, scissions, largeurs.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Workspace {
    #[serde(default)]
    pub tabs: Vec<SavedTab>,
    #[serde(default)]
    pub groups: usize,
    #[serde(default)]
    pub active_group: usize,
    /// Largeur (hauteur pour des groupes empilés) en points de chaque groupe
    /// sauf le dernier, qui prend le reste. Vide pour une grille.
    #[serde(default)]
    pub widths: Vec<f32>,
    #[serde(default)]
    pub arrangement: Arrangement,
    /// Nombre de groupes de chaque rangée, de haut en bas (groupes numérotés
    /// rangée par rangée). Vide = déduit de `arrangement`.
    #[serde(default)]
    pub rows: Vec<usize>,
}

/// Fichier d'export des sessions, relisible par « Importer des sessions ».
/// Les mots de passe (coffre du système) n'y figurent pas.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionExport {
    /// Toujours [`EXPORT_FORMAT`] : distingue ce fichier d'un JSON quelconque.
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub sessions: Vec<Session>,
    /// Groupes, y compris ceux qui n'ont aucune session.
    #[serde(default)]
    pub folders: Vec<String>,
}

pub const EXPORT_FORMAT: &str = "ferroterm-sessions";

impl SessionExport {
    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn from_json(text: &str) -> anyhow::Result<Self> {
        let export: Self = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| anyhow::anyhow!("ce fichier n'est pas un export de sessions Ferroterm ({e})"))?;
        if export.format != EXPORT_FORMAT {
            anyhow::bail!("ce fichier n'est pas un export de sessions Ferroterm");
        }
        Ok(export)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SessionStore {
    /// Terminaux ouverts lors de la dernière utilisation.
    #[serde(default)]
    pub workspace: Workspace,
    #[serde(default)]
    pub sessions: Vec<Session>,
    /// Sessions ouvertes récemment (par id), la plus récente en premier.
    #[serde(default)]
    pub recent: Vec<u64>,
    /// Groupes créés explicitement (chemins séparés par `/`) : ils restent
    /// affichés même sans session.
    #[serde(default)]
    pub folders: Vec<String>,
    #[serde(default)]
    pub macros: Vec<Macro>,
    /// Nom du thème choisi (vide = thème par défaut).
    #[serde(default)]
    pub theme: String,
    /// Coloration des mots-clés dans les terminaux.
    #[serde(default)]
    pub highlighting: crate::highlight::Highlighting,
    #[serde(default)]
    next_id: u64,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl SessionStore {
    /// Emplacement du fichier de sessions. La variable `FERROTERM_SESSIONS`
    /// permet d'en choisir un autre (mode portable, essais).
    pub fn default_path() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("FERROTERM_SESSIONS").filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(path));
        }
        directories::ProjectDirs::from("", "", "Ferroterm")
            .map(|d| d.config_dir().join("sessions.json"))
    }

    /// Charge les sessions. En cas de fichier illisible, il est mis de côté
    /// (`sessions.json.bak`) pour ne pas être écrasé, et un avertissement est renvoyé.
    pub fn load() -> (Self, Option<String>) {
        let path = Self::default_path();
        let mut warning = None;
        let mut store = match &path {
            Some(p) if p.exists() => match std::fs::read_to_string(p)
                .map_err(anyhow::Error::from)
                .and_then(|t| Ok(serde_json::from_str::<SessionStore>(&t)?))
            {
                Ok(store) => store,
                Err(e) => {
                    let backup = p.with_extension("json.bak");
                    let _ = std::fs::copy(p, &backup);
                    warning = Some(format!(
                        "Le fichier de sessions {} est illisible ({e}). Une copie a été gardée dans {}.",
                        p.display(),
                        backup.display()
                    ));
                    Self::default()
                }
            },
            _ => Self::default(),
        };
        store.path = path;
        // Sessions et macros partagent le même compteur d'identifiants.
        let max_id = store
            .sessions
            .iter()
            .map(|s| s.id)
            .chain(store.macros.iter().map(|m| m.id))
            .max()
            .unwrap_or(0);
        store.next_id = store.next_id.max(max_id + 1);
        (store, warning)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            anyhow::bail!("aucun dossier de configuration disponible");
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Écriture atomique : fichier temporaire puis renommage.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn get(&self, id: u64) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    /// Place une session en tête des sessions récentes (10 au plus).
    pub fn touch_recent(&mut self, id: u64) {
        self.recent.retain(|other| *other != id);
        self.recent.insert(0, id);
        self.recent.truncate(10);
    }

    /// Sessions récentes encore existantes, la plus récente en premier.
    pub fn recent_sessions(&self) -> Vec<&Session> {
        self.recent.iter().filter_map(|id| self.get(*id)).collect()
    }

    pub fn insert(&mut self, mut session: Session) -> u64 {
        session.id = self.next_id;
        self.next_id += 1;
        let id = session.id;
        self.sessions.push(session);
        id
    }

    pub fn update(&mut self, session: Session) {
        if let Some(existing) = self.sessions.iter_mut().find(|s| s.id == session.id) {
            *existing = session;
        }
    }

    pub fn remove(&mut self, id: u64) {
        self.sessions.retain(|s| s.id != id);
    }

    /// Vrai si le groupe existe : créé explicitement, ou contenant (lui ou un
    /// sous-groupe) au moins une session.
    pub fn has_folder(&self, path: &str) -> bool {
        let inside = |folder: &str| {
            folder.strip_prefix(path).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        };
        self.folders.iter().any(|f| inside(f)) || self.sessions.iter().any(|s| inside(&s.folder))
    }

    /// Crée un groupe vide. Renvoie `false` s'il existe déjà.
    pub fn add_folder(&mut self, path: &str) -> bool {
        if path.is_empty() || self.has_folder(path) {
            return false;
        }
        self.folders.push(path.to_string());
        true
    }

    /// Supprime un groupe et ses sous-groupes ; leurs sessions remontent
    /// dans le groupe parent.
    pub fn remove_folder(&mut self, path: &str) {
        let parent = path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        let inside = |folder: &str| {
            folder.strip_prefix(path).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        };
        self.folders.retain(|f| !inside(f));
        for session in self.sessions.iter_mut().filter(|s| inside(&s.folder)) {
            session.folder = parent.to_string();
        }
        // Le parent ne doit pas disparaître s'il se retrouve vide.
        if !parent.is_empty() && !self.has_folder(parent) {
            self.folders.push(parent.to_string());
        }
    }

    pub fn get_macro(&self, id: u64) -> Option<&Macro> {
        self.macros.iter().find(|m| m.id == id)
    }

    /// Enregistre une macro : création si son id est inconnu (ou nul), mise à jour sinon.
    pub fn save_macro(&mut self, mut item: Macro) -> u64 {
        if let Some(existing) = self.macros.iter_mut().find(|m| m.id == item.id && item.id != 0) {
            *existing = item;
            return existing.id;
        }
        item.id = self.next_id.max(1);
        self.next_id = item.id + 1;
        let id = item.id;
        self.macros.push(item);
        id
    }

    pub fn remove_macro(&mut self, id: u64) {
        self.macros.retain(|m| m.id != id);
    }

    /// Sessions et groupes à exporter : tout (`folder` vide), ou un groupe et
    /// ses sous-groupes. Les chemins complets des groupes sont gardés.
    pub fn export(&self, folder: &str) -> SessionExport {
        let inside = |path: &str| {
            folder.is_empty()
                || path.strip_prefix(folder).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        };
        let mut sessions: Vec<Session> = self.sessions.iter().filter(|s| inside(&s.folder)).cloned().collect();
        sessions.sort_by(|a, b| (&a.folder, a.name.to_lowercase()).cmp(&(&b.folder, b.name.to_lowercase())));
        let mut folders: Vec<String> = self.folders.iter().filter(|f| inside(f)).cloned().collect();
        if !folder.is_empty() && !folders.iter().any(|f| f == folder) {
            folders.push(folder.to_string());
        }
        folders.sort();
        SessionExport { format: EXPORT_FORMAT.into(), version: 1, sessions, folders }
    }

    /// Importe un export : les groupes manquants sont créés, les sessions
    /// déjà présentes (même dossier et même nom) ignorées. Renvoie
    /// (ajoutées, déjà présentes).
    pub fn import(&mut self, export: SessionExport) -> (usize, usize) {
        for folder in &export.folders {
            self.add_folder(folder);
        }
        let sessions = export
            .sessions
            .into_iter()
            .filter(|s| !s.host.trim().is_empty())
            .map(|mut s| {
                // Profils incohérents (fichier modifié à la main) : remis d'aplomb.
                let profiles = s.profiles();
                let active = s.profile;
                s.set_profiles(profiles, active);
                s
            })
            .collect();
        self.merge(sessions)
    }

    /// Ajoute les sessions qui n'existent pas déjà (même dossier et même nom).
    /// Renvoie (ajoutées, déjà présentes).
    pub fn merge(&mut self, imported: Vec<Session>) -> (usize, usize) {
        let (mut added, mut duplicates) = (0, 0);
        for session in imported {
            let exists = self
                .sessions
                .iter()
                .any(|s| s.folder == session.folder && s.name == session.name);
            if exists {
                duplicates += 1;
            } else {
                self.insert(session);
                added += 1;
            }
        }
        (added, duplicates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(folder: &str, name: &str) -> Session {
        Session {
            name: name.into(),
            folder: folder.into(),
            host: "example.org".into(),
            port: 22,
            ..Default::default()
        }
    }

    #[test]
    fn merge_skips_duplicates_and_assigns_ids() {
        let mut store = SessionStore::default();
        store.insert(session("prod", "web"));
        let (added, dupes) = store.merge(vec![
            session("prod", "web"),
            session("prod", "db"),
            session("dev", "web"),
        ]);
        assert_eq!((added, dupes), (2, 1));
        let mut ids: Vec<u64> = store.sessions.iter().map(|s| s.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn folders_can_be_created_empty_and_removed() {
        let mut store = SessionStore::default();
        assert!(store.add_folder("EFB"));
        assert!(!store.add_folder("EFB"));
        assert!(store.add_folder("EFB/Prod"));
        let id = store.insert(session("EFB/Prod", "web"));
        // « EF » n'est pas un groupe : seul un segment entier compte.
        assert!(!store.has_folder("EF"));
        store.remove_folder("EFB/Prod");
        assert_eq!(store.get(id).map(|s| s.folder.as_str()), Some("EFB"));
        assert!(store.has_folder("EFB"));
        assert!(!store.has_folder("EFB/Prod"));
    }

    #[test]
    fn macro_input_sends_each_line_with_enter() {
        let item = Macro { id: 1, name: "m".into(), commands: "cd /tmp\r\n\n  ls -la  \n".into(), ..Default::default() };
        assert_eq!(item.input(), b"cd /tmp\r  ls -la\r");
    }

    #[test]
    fn macro_command_line_has_no_enter() {
        let item = Macro { id: 1, name: "m".into(), commands: "cd /tmp\r\n\n  ls -la  \n".into(), ..Default::default() };
        assert_eq!(item.command_line(), "cd /tmp; ls -la");
    }

    #[test]
    fn save_macro_creates_then_updates() {
        let mut store = SessionStore::default();
        let id = store.save_macro(Macro { name: "a".into(), commands: "ls".into(), ..Default::default() });
        assert_ne!(id, 0);
        store.save_macro(Macro { id, name: "b".into(), commands: "pwd".into(), ..Default::default() });
        assert_eq!(store.macros.len(), 1);
        assert_eq!(store.get_macro(id).map(|m| m.name.as_str()), Some("b"));
        store.remove_macro(id);
        assert!(store.macros.is_empty());
    }

    #[test]
    fn export_then_import_round_trip() {
        let mut store = SessionStore::default();
        store.add_folder("EFB/Vide");
        let web = store.insert(session("EFB/PROD", "web"));
        store.insert(session("EFB", "dev"));
        store.insert(session("Autre", "x"));
        let mut prod = store.get(web).unwrap().profiles();
        prod.push(Profile { id: 1, name: "Prod".into(), host: "prod".into(), port: 22, ..Default::default() });
        store.sessions.iter_mut().find(|s| s.id == web).unwrap().set_profiles(prod, 1);

        // Un groupe : ses sessions et sous-groupes seulement, chemins complets.
        let export = store.export("EFB");
        let names: Vec<_> = export.sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["dev", "web"]);
        assert_eq!(export.folders, vec!["EFB", "EFB/Vide"]);
        assert_eq!(store.export("EF").sessions.len(), 0);

        let text = store.export("").to_json().unwrap();
        let mut other = SessionStore::default();
        other.insert(session("Autre", "x"));
        assert_eq!(other.import(SessionExport::from_json(&text).unwrap()), (2, 1));
        assert!(other.has_folder("EFB/Vide"));
        let imported = other.sessions.iter().find(|s| s.name == "web").unwrap();
        assert_eq!((imported.host.as_str(), imported.profile_name()), ("prod", "Prod"));

        assert!(SessionExport::from_json("{\"sessions\": []}").is_err());
        assert!(SessionExport::from_json("{\"format\": \"autre\", \"version\": 1}").is_err());
    }

    #[test]
    fn profiles_switch_and_keep_their_order() {
        let mut s = session("", "x");
        // Ancienne session : un seul profil, principal.
        assert_eq!(s.profile_name(), MAIN_PROFILE);
        assert!(!s.has_profiles());
        let mut all = s.profiles();
        assert_eq!(all.len(), 1);
        all.push(Profile {
            id: Session::next_profile_id(&all),
            name: "Prod".into(),
            host: "prod.example.org".into(),
            port: 2222,
            username: "deploy".into(),
            key_path: None,
        });
        s.set_profiles(all, 1);
        assert_eq!((s.profile, s.host.as_str(), s.port, s.profile_name()), (1, "prod.example.org", 2222, "Prod"));
        assert!(s.has_profiles());

        // Les champs de la session font foi pour le profil actif.
        s.username = "admin".into();
        assert!(s.switch_profile(0));
        assert_eq!((s.host.as_str(), s.profile_name()), ("example.org", MAIN_PROFILE));
        let names: Vec<_> = s.profiles().into_iter().map(|p| (p.name, p.username)).collect();
        assert_eq!(names, vec![(MAIN_PROFILE.into(), String::new()), ("Prod".into(), "admin".into())]);
        assert!(!s.switch_profile(9));

        // Retour à un seul profil principal : rien à mémoriser.
        let main = s.profiles().remove(0);
        s.set_profiles(vec![main], 0);
        assert!(s.profiles.is_empty());
    }

    #[test]
    fn target_hides_default_port() {
        let mut s = session("", "x");
        s.username = "alice".into();
        assert_eq!(s.target(), "alice@example.org");
        s.port = 2222;
        assert_eq!(s.target(), "alice@example.org:2222");
    }
}
