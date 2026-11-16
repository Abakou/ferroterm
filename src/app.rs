//! Interface principale : sessions à gauche, onglets de terminal au centre,
//! explorateur SFTP de l'onglet actif.

use std::collections::BTreeMap;
use std::path::PathBuf;

use eframe::egui::{self, Color32, Id, Key, RichText};

use crate::local::{self, LocalShell};
use crate::mobaxterm;
use crate::session::{Arrangement, Macro, Session, SessionStore};

/// Nombre de colonnes d'une grille de `count` groupes.
fn grid_columns(count: usize) -> usize {
    if count <= 4 { 2 } else { (count as f32).sqrt().ceil() as usize }
}

/// Rangées (nombre de groupes par rangée) d'une disposition sans rangées
/// explicites : anciens espaces de travail, grille de quatre.
fn default_rows(count: usize, arrangement: Arrangement) -> Vec<usize> {
    let count = count.max(1);
    match arrangement {
        Arrangement::Columns => vec![count],
        Arrangement::Rows => vec![1; count],
        Arrangement::Grid => {
            let columns = grid_columns(count);
            (0..count).step_by(columns).map(|start| columns.min(count - start)).collect()
        }
    }
}

/// Disposition correspondant à des rangées : une seule rangée = côte à côte,
/// un groupe par rangée = empilés, sinon grille.
fn arrangement_of(rows: &[usize]) -> Arrangement {
    if rows.len() <= 1 {
        Arrangement::Columns
    } else if rows.iter().all(|&n| n == 1) {
        Arrangement::Rows
    } else {
        Arrangement::Grid
    }
}

/// Taille minimale d'un groupe d'onglets.
const MIN_GROUP_WIDTH: f32 = 160.0;
const MIN_GROUP_HEIGHT: f32 = 110.0;
use crate::ssh::{Command, Connection, Event, RemoteEntry, Reply, SftpCommand, TransferUpdate, join_remote};
use crate::terminal::TerminalView;
use crate::theme;
use crate::win_frame::{self, CaptionButton};

pub struct FerroApp {
    rt: tokio::runtime::Runtime,
    store: SessionStore,
    tabs: Vec<Tab>,
    active: usize,
    /// Panneau gauche visible (sessions ou macros, selon `side_view`).
    sessions_visible: bool,
    /// Vue affichée dans le panneau gauche, choisie dans la barre d'activité.
    side_view: SideView,
    selected_macro: Option<u64>,
    macro_editor: Option<MacroEditor>,
    filter: String,
    selected_session: Option<u64>,
    editor: Option<Editor>,
    group_editor: Option<GroupEditor>,
    notice: Option<(String, String)>,
    confirm: Option<Confirm>,
    mkdir_name: Option<String>,
    tab_rename: Option<TabRename>,
    /// Fichiers distants ouverts dans un éditeur local, renvoyés à l'enregistrement.
    edits: Vec<crate::edit::EditedFile>,
    next_edit: u64,
    /// Coloration des mots-clés, compilée depuis les réglages (`None` = désactivée).
    highlighter: Option<std::sync::Arc<crate::highlight::Highlighter>>,
    highlight_editor: Option<HighlightEditor>,
    /// Donne le focus au champ de recherche à la prochaine image.
    focus_filter: bool,
    /// Panneau droit : explorateur de fichiers distants.
    files_visible: bool,
    /// Panneau bas : supervision du serveur distant.
    monitor_visible: bool,
    /// Nombre de groupes d'onglets affichés côte à côte (au moins 1).
    groups: usize,
    /// Disposition des groupes : côte à côte, empilés ou en grille (déduite de `rows`).
    arrangement: Arrangement,
    /// Nombre de groupes de chaque rangée, de haut en bas ; les groupes sont
    /// numérotés rangée par rangée. La somme vaut `groups`.
    rows: Vec<usize>,
    active_group: usize,
    /// Onglet actif (id) de chaque groupe.
    group_active: Vec<Option<u64>>,
    /// Groupes vierges (créés par une scission, sans onglet) : ils restent
    /// affichés, avec la liste des sessions récentes, jusqu'à leur premier onglet.
    blank: Vec<bool>,
    /// Dernier onglet amené dans le champ de la barre, par groupe.
    scrolled_to: Vec<Option<u64>>,
    next_tab_id: u64,
    shells: Vec<LocalShell>,
    /// Icône de l'application, affichée dans la barre de titre.
    logo: egui::TextureHandle,
    /// Largeur mesurée de chaque groupe d'onglets sauf le dernier.
    group_widths: Vec<f32>,
    /// Largeurs de la dernière utilisation, appliquées à la restauration.
    restored_widths: Vec<f32>,
    /// Mémorise les terminaux ouverts pour les rouvrir au prochain démarrage
    /// (désactivé avec `--local`, réservé aux essais et captures).
    persist_workspace: bool,
}

/// Ce qu'un onglet affiche : une session SSH ou un shell local.
#[derive(Clone)]
enum Target {
    Ssh(Session),
    Local(LocalShell),
}

impl Target {
    fn title(&self) -> &str {
        match self {
            Target::Ssh(s) => &s.name,
            Target::Local(s) => &s.name,
        }
    }

    fn subtitle(&self) -> String {
        match self {
            Target::Ssh(s) if s.has_profiles() => format!("{} (profil {})", s.target(), s.profile_name()),
            Target::Ssh(s) => s.target(),
            Target::Local(s) => s.program.display().to_string(),
        }
    }

    fn session_id(&self) -> Option<u64> {
        match self {
            Target::Ssh(s) => Some(s.id),
            Target::Local(_) => None,
        }
    }

    fn connect(&self, rt: &tokio::runtime::Runtime, ctx: &egui::Context) -> Connection {
        match self {
            Target::Ssh(s) => Connection::spawn(rt, s.clone(), ctx.clone()),
            Target::Local(s) => local::spawn(s, ctx.clone()),
        }
    }
}

/// Vue du panneau gauche.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SideView {
    Sessions,
    Macros,
}

/// Macro en cours de création ou de modification (id nul = nouvelle).
struct MacroEditor {
    item: Macro,
    error: Option<String>,
}

/// Réglages de coloration des mots-clés en cours de modification.
struct HighlightEditor {
    enabled: bool,
    /// Listes, avec le texte de leurs mots dans l'éditeur (un par ligne).
    rules: Vec<(crate::highlight::HighlightRule, String)>,
}

impl HighlightEditor {
    fn new(settings: &crate::highlight::Highlighting) -> Self {
        Self {
            enabled: settings.enabled,
            rules: settings
                .rules
                .iter()
                .map(|rule| (rule.clone(), crate::highlight::words_to_text(&rule.words)))
                .collect(),
        }
    }

    fn settings(&self) -> crate::highlight::Highlighting {
        crate::highlight::Highlighting {
            enabled: self.enabled,
            rules: self
                .rules
                .iter()
                .map(|(rule, text)| crate::highlight::HighlightRule {
                    words: crate::highlight::text_to_words(text),
                    ..rule.clone()
                })
                .collect(),
        }
    }
}

/// Groupe de sessions en cours de création.
struct GroupEditor {
    /// Groupe parent (vide = racine).
    parent: String,
    name: String,
    error: Option<String>,
}

enum Confirm {
    DeleteSession(u64),
    DeleteGroup(String),
    DeleteMacro(u64),
    DeleteRemote { path: String, is_dir: bool },
}

struct Tab {
    id: u64,
    /// Groupe d'onglets, numéroté de gauche à droite à partir de 0.
    group: usize,
    target: Target,
    /// Nom donné par l'utilisateur ; `None` = nom de la session ou du shell.
    title: Option<String>,
    conn: Connection,
    view: TerminalView,
    state: TabState,
    last_status: Option<String>,
    prompt: Option<Prompt>,
    sftp: SftpState,
    monitor: Option<crate::monitor::Stats>,
    history: MonitorHistory,
}

impl Tab {
    /// Nom affiché : celui donné par l'utilisateur, sinon celui de la cible.
    fn title(&self) -> &str {
        self.title.as_deref().unwrap_or_else(|| self.target.title())
    }
}

/// Onglet en cours de renommage, directement dans la barre d'onglets.
struct TabRename {
    id: u64,
    /// Nom en cours de saisie.
    name: String,
    /// Le champ a déjà reçu le focus (sinon : première image).
    started: bool,
}

enum TabState {
    Connecting(String),
    Connected,
    Closed(String),
}

enum Prompt {
    HostKey { algorithm: String, fingerprint: String },
    /// `remember` : case « Enregistrer dans le coffre Windows », si proposée.
    Text { prompt: String, secret: bool, value: String, remember: Option<bool> },
}

struct SftpState {
    available: Option<Result<(), String>>,
    cwd: String,
    path_edit: String,
    entries: Vec<RemoteEntry>,
    selected: Option<usize>,
    loading: bool,
    error: Option<String>,
    transfers: Vec<TransferUpdate>,
    /// L'explorateur suit le dossier courant du shell du terminal.
    follow_terminal: bool,
    last_terminal_cwd: Option<String>,
}

impl Default for SftpState {
    fn default() -> Self {
        Self {
            available: None,
            cwd: String::new(),
            path_edit: String::new(),
            entries: Vec::new(),
            selected: None,
            loading: false,
            error: None,
            transfers: Vec::new(),
            follow_terminal: true,
            last_terminal_cwd: None,
        }
    }
}

/// Session en cours de création ou de modification, avec ses profils.
struct Editor {
    /// Nom, dossier et id ; les champs de connexion viennent des profils.
    session: Session,
    is_new: bool,
    profiles: Vec<ProfileForm>,
    /// Profil affiché à droite (index dans `profiles`).
    selected: usize,
    /// Profil actif (id) : celui utilisé pour se connecter.
    active: u64,
    /// Profils supprimés (ids), dont le mot de passe enregistré sera effacé.
    removed: Vec<u64>,
    /// Le nom du profil affiché doit prendre le focus (profil tout juste créé).
    focus_profile_name: bool,
    error: Option<String>,
}

/// Profil de connexion en cours de modification, avec ses secrets.
struct ProfileForm {
    profile: crate::session::Profile,
    use_key: bool,
    port: String,
    /// Mot de passe saisi ici, enregistré dans le coffre Windows ; vide = inchangé.
    password: String,
    /// Un mot de passe est déjà enregistré pour ce profil.
    saved_password: bool,
    /// Le mot de passe enregistré sera oublié à l'enregistrement.
    forget_password: bool,
    /// Phrase de passe enregistrée pour la clé : (chemin vérifié, présente).
    saved_passphrase: Option<(String, bool)>,
    /// La phrase de passe enregistrée de la clé sera oubliée.
    forget_passphrase: bool,
}

/// Actions décidées pendant le dessin, appliquées ensuite (évite les conflits d'emprunt).
enum Action {
    Connect(u64),
    Edit(u64),
    Duplicate(u64),
    /// Active un autre profil de connexion de la session.
    SwitchProfile { session: u64, profile: u64 },
    /// Ouvre l'éditeur de la session sur un nouveau profil.
    NewProfile(u64),
    AskDelete(u64),
    NewSession,
    /// Nouvelle session rangée dans ce groupe.
    NewSessionIn(String),
    /// Nouveau groupe de sessions, dans ce groupe parent (vide = racine).
    NewGroup(String),
    AskDeleteGroup(String),
    Import,
    /// Exporte les sessions dans un fichier : toutes (vide), ou ce groupe.
    ExportSessions(String),
    /// Importe un fichier exporté par Ferroterm.
    ImportSessions,
    OpenLocal(usize),
    ToggleSessions,
    /// Clic dans la barre d'activité : affiche cette vue, ou replie le
    /// panneau si elle est déjà affichée.
    ShowSide(SideView),
    NewMacro,
    EditMacro(u64),
    DuplicateMacro(u64),
    AskDeleteMacro(u64),
    /// Envoie les commandes de la macro au terminal actif.
    RunMacro(u64),
    ToggleFiles,
    ToggleMonitor,
    SetTheme(String),
    /// Ouvre un second groupe d'onglets à droite, avec un terminal local.
    SplitRight,
    /// Ouvre un second groupe d'onglets en dessous, avec un terminal local.
    SplitDown,
    /// Dispose quatre groupes d'onglets en grille, complétés par des terminaux locaux.
    GridOfFour,
    /// Ouvre les réglages de coloration des mots-clés.
    EditHighlighting,
    /// Ouvre la recherche dans le terminal de l'onglet actif.
    FindInTerminal,
    /// Déplace une session dans un autre dossier (glisser-déposer).
    MoveToFolder { session: u64, folder: String },
}

/// Charge utile du glisser-déposer d'un onglet (réorganisation, changement de groupe).
struct DragTab {
    id: u64,
}

/// Charge utile du glisser-déposer d'un groupe vierge (son onglet « Nouvel onglet »).
struct DragBlank {
    group: usize,
}

/// Charge utile du glisser-déposer d'une session entre groupes.
struct DragSession {
    id: u64,
    name: String,
}

/// Icône d'une action : glyphe de police, ou toggle dessiné (contour / rempli).
enum TitleIcon {
    Font(&'static theme::Icon),
    Toggle(theme::ToggleGlyph),
    /// Menu déroulant des shells locaux.
    Shells,
}

/// Bouton d'action de la barre de titre (à gauche des boutons de fenêtre).
struct TitleAction {
    icon: TitleIcon,
    tooltip: &'static str,
    /// Affiché en surbrillance (ex. panneau actuellement visible).
    active: bool,
    action: Action,
}

impl FerroApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        // Pas de zoom global de l’interface (Ctrl + / - / 0) : seuls les terminaux zooment.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let (store, warning) = SessionStore::load();
        let highlighter = crate::highlight::Highlighter::new(&store.highlighting).map(std::sync::Arc::new);
        if !store.theme.is_empty() {
            theme::set_theme(&cc.egui_ctx, &store.theme);
        }
        win_frame::install(cc);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime tokio");
        let mut app = Self {
            rt,
            store,
            tabs: Vec::new(),
            active: 0,
            sessions_visible: true,
            side_view: SideView::Sessions,
            selected_macro: None,
            macro_editor: None,
            filter: String::new(),
            selected_session: None,
            editor: None,
            group_editor: None,
            notice: warning.map(|w| ("Sessions".to_string(), w)),
            confirm: None,
            mkdir_name: None,
            tab_rename: None,
            edits: Vec::new(),
            next_edit: 0,
            highlighter,
            highlight_editor: None,
            focus_filter: false,
            files_visible: true,
            monitor_visible: true,
            groups: 1,
            arrangement: Arrangement::Columns,
            rows: vec![1],
            active_group: 0,
            group_active: vec![None],
            blank: vec![false],
            scrolled_to: vec![None],
            next_tab_id: 0,
            shells: local::detect_shells(),
            logo: cc.egui_ctx.load_texture(
                "logo",
                egui::ColorImage::from_rgba_unmultiplied([64, 64], include_bytes!("../assets/ferroterm-64.rgba")),
                egui::TextureOptions::LINEAR,
            ),
            group_widths: Vec::new(),
            restored_widths: Vec::new(),
            persist_workspace: true,
        };
        // `ferroterm --local [--local …]` : un terminal local par occurrence ;
        // `--split` ajoute un second groupe d'onglets à droite (essais, captures).
        // `--macros` ouvre le panneau gauche sur la vue des macros.
        if std::env::args().any(|a| a == "--macros") {
            app.side_view = SideView::Macros;
        }
        let locals = std::env::args().filter(|a| a == "--local").count();
        if locals > 0 {
            app.persist_workspace = false;
        } else {
            app.restore_workspace(&cc.egui_ctx);
        }
        if let Some(shell) = app.shells.first().cloned() {
            for _ in 0..locals {
                app.open_target(Target::Local(shell.clone()), &cc.egui_ctx);
            }
            if locals > 0 {
                // Un groupe supplémentaire par `--split`.
                for _ in std::env::args().filter(|a| a == "--split") {
                    app.split(false);
                    app.open_target(Target::Local(shell.clone()), &cc.egui_ctx);
                }
            }
        }
        app
    }

    /// Bouton déroulant listant les shells locaux détectés.
    fn local_menu(&self, ui: &mut egui::Ui, label: impl Into<egui::WidgetText>, actions: &mut Vec<Action>) {
        ui.menu_button(label.into(), |ui| {
            // Le menu n'hérite pas du remplissage (parfois serré) du bouton.
            ui.spacing_mut().button_padding = egui::vec2(10.0, 4.0);
            if self.shells.is_empty() {
                ui.label("Aucun shell détecté");
            }
            for (index, shell) in self.shells.iter().enumerate() {
                if ui.button(&shell.name).on_hover_text(shell.program.display().to_string()).clicked() {
                    actions.push(Action::OpenLocal(index));
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text("Ouvrir un terminal sur cette machine");
    }

    fn save_store(&mut self) {
        if let Err(e) = self.store.save() {
            self.notice = Some(("Enregistrement impossible".into(), format!("{e:#}")));
        }
    }

    /// Rouvre les terminaux de la dernière utilisation, dans leurs groupes.
    /// Les sessions supprimées depuis sont ignorées.
    fn restore_workspace(&mut self, ctx: &egui::Context) {
        let saved = self.store.workspace.clone();
        if saved.tabs.is_empty() {
            return;
        }
        self.groups = saved.groups.max(1);
        self.rows = if saved.rows.iter().sum::<usize>() == self.groups && !saved.rows.contains(&0) {
            saved.rows.clone()
        } else {
            default_rows(self.groups, saved.arrangement)
        };
        self.arrangement = arrangement_of(&self.rows);
        self.group_active = vec![None; self.groups];
        self.scrolled_to = vec![None; self.groups];
        let mut active_ids = vec![None; self.groups];
        for tab in &saved.tabs {
            let target = match (tab.session, &tab.shell) {
                (Some(id), _) => self.store.get(id).cloned().map(|mut session| {
                    // L'onglet se rouvre avec son profil, même si un autre est actif depuis.
                    if let Some(profile) = tab.profile {
                        session.switch_profile(profile);
                    }
                    Target::Ssh(session)
                }),
                (None, name) => self
                    .shells
                    .iter()
                    .find(|s| Some(&s.name) == name.as_ref())
                    .or(self.shells.first())
                    .cloned()
                    .map(Target::Local),
            };
            let Some(target) = target else { continue };
            self.active_group = tab.group.min(self.groups - 1);
            self.open_target(target, ctx);
            if let Some(opened) = self.tabs.last_mut() {
                opened.title = tab.title.clone();
            }
            if tab.active {
                active_ids[self.active_group] = Some(self.next_tab_id);
            }
        }
        for (group, id) in active_ids.into_iter().enumerate() {
            if id.is_some() {
                self.group_active[group] = id;
            }
        }
        self.active_group = saved.active_group.min(self.groups - 1);
        self.normalize_groups();
        // Les largeurs ne valent que si aucun groupe n'a disparu.
        if self.groups == saved.groups {
            self.restored_widths = saved.widths;
        }
    }

    /// Enregistre la disposition des terminaux dès qu'elle change.
    fn sync_workspace(&mut self, ctx: &egui::Context) {
        // Pas pendant un glissement (redimensionnement d'un groupe, onglet déplacé).
        if !self.persist_workspace || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        let tabs = self
            .tabs
            .iter()
            .map(|tab| crate::session::SavedTab {
                group: tab.group,
                session: tab.target.session_id(),
                profile: match &tab.target {
                    Target::Ssh(session) if session.has_profiles() => Some(session.profile),
                    _ => None,
                },
                shell: match &tab.target {
                    Target::Local(shell) => Some(shell.name.clone()),
                    Target::Ssh(_) => None,
                },
                active: self.group_active.get(tab.group).copied().flatten() == Some(tab.id),
                title: tab.title.clone(),
            })
            .collect::<Vec<_>>();
        let split = !tabs.is_empty() && self.groups > 1;
        let workspace = crate::session::Workspace {
            groups: if tabs.is_empty() { 0 } else { self.groups },
            active_group: if split { self.active_group } else { 0 },
            widths: if split { self.group_widths.clone() } else { Vec::new() },
            arrangement: if split { self.arrangement } else { Arrangement::Columns },
            rows: if split { self.rows.clone() } else { Vec::new() },
            tabs,
        };
        if workspace != self.store.workspace {
            self.store.workspace = workspace;
            self.save_store();
        }
    }

    fn open_target(&mut self, target: Target, ctx: &egui::Context) {
        let group = self.active_group.min(self.groups.saturating_sub(1));
        let conn = target.connect(&self.rt, ctx);
        self.next_tab_id += 1;
        self.tabs.push(Tab {
            id: self.next_tab_id,
            group,
            target,
            title: None,
            conn,
            view: TerminalView::default(),
            state: TabState::Connecting("Connexion…".into()),
            last_status: None,
            prompt: None,
            sftp: SftpState::default(),
            monitor: None,
            history: MonitorHistory::default(),
        });
        self.active = self.tabs.len() - 1;
        self.group_active[group] = Some(self.next_tab_id);
        self.active_group = group;
    }

    /// Ouvre un nouveau groupe d'onglets, vierge. Scinder en bas ajoute
    /// toujours une nouvelle rangée, en bas ; scinder à droite ajoute un
    /// groupe dans la rangée du groupe actif, juste après lui.
    fn split(&mut self, down: bool) {
        self.keep_first_group();
        let focused = self.active_group.min(self.groups - 1);
        let row = self.row_of(focused);
        let new = if down { self.groups } else { focused + 1 };
        self.insert_group(new);
        if down {
            self.rows.push(1);
        } else {
            self.rows[row] += 1;
        }
        self.arrangement = arrangement_of(&self.rows);
        self.blank[new] = true;
        self.active_group = new;
    }

    /// Passe en grille de quatre groupes : les groupes manquants sont créés vierges.
    fn grid_of_four(&mut self) {
        self.keep_first_group();
        let first_new = self.groups;
        while self.groups < 4 {
            let new = self.groups;
            self.insert_group(new);
            self.blank[new] = true;
        }
        if first_new < self.groups {
            self.active_group = first_new;
        }
        self.rows = default_rows(self.groups, Arrangement::Grid);
        self.arrangement = arrangement_of(&self.rows);
    }

    /// Sans aucun onglet ouvert, l'unique groupe devient vierge pour pouvoir
    /// être scindé.
    fn keep_first_group(&mut self) {
        self.groups = self.groups.max(1);
        self.blank.resize(self.groups, false);
        if self.tabs.is_empty() && self.groups == 1 {
            self.blank[0] = true;
        }
        self.fix_rows();
    }

    /// Rangées incohérentes avec le nombre de groupes : on les recalcule.
    fn fix_rows(&mut self) {
        let count = self.groups.max(1);
        if self.rows.iter().sum::<usize>() != count || self.rows.contains(&0) {
            self.rows = default_rows(count, self.arrangement);
        }
        self.arrangement = arrangement_of(&self.rows);
    }

    /// Rangée contenant le groupe `group`.
    fn row_of(&self, group: usize) -> usize {
        let mut end = 0;
        for (row, &len) in self.rows.iter().enumerate() {
            end += len;
            if group < end {
                return row;
            }
        }
        self.rows.len().saturating_sub(1)
    }

    /// Insère un groupe vide à la position `at` ; les groupes suivants se décalent.
    fn insert_group(&mut self, at: usize) {
        for tab in &mut self.tabs {
            if tab.group >= at {
                tab.group += 1;
            }
        }
        self.blank.resize(self.groups, false);
        self.groups += 1;
        self.blank.insert(at.min(self.blank.len()), false);
        self.group_active.insert(at.min(self.group_active.len()), None);
        self.scrolled_to.insert(at.min(self.scrolled_to.len()), None);
        if self.active_group >= at {
            self.active_group += 1;
        }
    }

    /// Déplace un onglet dans le groupe `group` (créé à droite s'il n'existe
    /// pas encore). `anchor` : onglet de référence et côté (vrai = après lui) ;
    /// sans ancre, l'onglet est placé en fin de groupe.
    fn move_tab(&mut self, id: u64, group: usize, anchor: Option<(u64, bool)>) {
        let Some(from) = self.tabs.iter().position(|t| t.id == id) else { return };
        if anchor.is_some_and(|(other, _)| other == id) {
            return;
        }
        if group >= self.groups {
            self.fix_rows();
            self.insert_group(self.groups);
            // Nouveau groupe en bas si les groupes sont empilés, sinon au bout
            // de la dernière rangée.
            if self.arrangement == Arrangement::Rows {
                self.rows.push(1);
            } else if let Some(last) = self.rows.last_mut() {
                *last += 1;
            }
            self.arrangement = arrangement_of(&self.rows);
        }
        let group = group.min(self.groups - 1);
        let mut tab = self.tabs.remove(from);
        tab.group = group;
        let position = anchor
            .and_then(|(other, after)| {
                self.tabs.iter().position(|t| t.id == other).map(|index| if after { index + 1 } else { index })
            })
            .unwrap_or_else(|| {
                // Juste après le dernier onglet du groupe.
                self.tabs.iter().rposition(|t| t.group == group).map(|index| index + 1).unwrap_or(self.tabs.len())
            });
        self.tabs.insert(position, tab);
        self.group_active[group] = Some(id);
        self.active_group = group;
        self.normalize_groups();
    }

    /// Groupe vierge glissé au-dessus du groupe `group` (zone `rect`) : trait
    /// du côté où il sera placé, puis déplacement au relâchement. Vrai si le
    /// groupe a été déplacé (la disposition a changé).
    fn drop_blank_group(&mut self, ui: &egui::Ui, group: usize, rect: egui::Rect) -> bool {
        let Some(payload) = egui::DragAndDrop::payload::<DragBlank>(ui.ctx()) else { return false };
        let Some(pos) = ui.ctx().pointer_latest_pos().filter(|pos| rect.contains(*pos)) else { return false };
        if payload.group == group {
            return false;
        }
        let after = pos.x > rect.center().x;
        // Au premier plan : le contenu du groupe est dessiné après.
        let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("blank_drop")));
        let half = if after {
            egui::Rect::from_min_max(egui::pos2(rect.center().x, rect.top()), rect.max)
        } else {
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.center().x, rect.bottom()))
        };
        painter.rect_filled(half, 0.0, theme::p().accent.gamma_multiply(0.08));
        let x = if after { rect.right() - 1.5 } else { rect.left() + 1.5 };
        painter.vline(x, rect.y_range(), egui::Stroke::new(3.0, theme::p().accent));
        if !ui.input(|i| i.pointer.any_released()) {
            return false;
        }
        egui::DragAndDrop::clear_payload(ui.ctx());
        self.move_group(payload.group, group, after);
        true
    }

    /// Déplace le groupe vierge `from` juste avant (ou après) le groupe
    /// `target`, dans la rangée de celui-ci.
    fn move_group(&mut self, from: usize, target: usize, after: bool) {
        self.fix_rows();
        if from == target || from >= self.groups || target >= self.groups || !self.blank[from] {
            return;
        }
        // Retrait : la rangée d'origine perd un groupe, et disparaît si elle est vide.
        let from_row = self.row_of(from);
        self.rows[from_row] -= 1;
        if self.rows[from_row] == 0 {
            self.rows.remove(from_row);
        }
        for tab in &mut self.tabs {
            if tab.group > from {
                tab.group -= 1;
            }
        }
        self.blank.remove(from);
        self.group_active.remove(from);
        self.scrolled_to.remove(from);
        self.groups -= 1;
        if self.active_group > from {
            self.active_group -= 1;
        }
        // Insertion à côté de la cible, dans sa rangée.
        let target = if target > from { target - 1 } else { target };
        let row = self.row_of(target);
        let at = if after { target + 1 } else { target };
        self.insert_group(at);
        self.rows[row] += 1;
        self.arrangement = arrangement_of(&self.rows);
        self.blank[at] = true;
        self.active_group = at;
        self.normalize_groups();
    }

    fn reconnect(&mut self, index: usize, ctx: &egui::Context) {
        let tab = &mut self.tabs[index];
        tab.conn = tab.target.connect(&self.rt, ctx);
        // Le zoom du terminal survit à la reconnexion.
        let font_size = tab.view.font_size;
        tab.view = TerminalView::default();
        tab.view.font_size = font_size;
        tab.state = TabState::Connecting("Connexion…".into());
        tab.last_status = None;
        tab.prompt = None;
        tab.sftp = SftpState::default();
        tab.monitor = None;
        tab.history = MonitorHistory::default();
    }

    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Connect(id) => {
                if let Some(session) = self.store.get(id).cloned() {
                    self.open_target(Target::Ssh(session), ctx);
                    if self.store.recent.first() != Some(&id) {
                        self.store.touch_recent(id);
                        self.save_store();
                    }
                }
            }
            Action::Edit(id) => {
                if let Some(session) = self.store.get(id).cloned() {
                    self.editor = Some(Editor::new(session, false));
                }
            }
            Action::Duplicate(id) => {
                if let Some(mut session) = self.store.get(id).cloned() {
                    session.name = format!("{} (copie)", session.name);
                    self.editor = Some(Editor::new(session, true));
                }
            }
            Action::SwitchProfile { session, profile } => {
                // Les onglets déjà ouverts gardent leur profil ; le nouveau
                // profil sert aux prochaines connexions.
                if let Some(s) = self.store.sessions.iter_mut().find(|s| s.id == session) {
                    if s.profile != profile && s.switch_profile(profile) {
                        self.save_store();
                    }
                }
            }
            Action::NewProfile(id) => {
                if let Some(session) = self.store.get(id).cloned() {
                    let mut editor = Editor::new(session, false);
                    editor.add_profile();
                    self.editor = Some(editor);
                }
            }
            Action::AskDelete(id) => self.confirm = Some(Confirm::DeleteSession(id)),
            Action::NewSession => {
                let folder = self
                    .selected_session
                    .and_then(|id| self.store.get(id))
                    .map(|s| s.folder.clone())
                    .unwrap_or_default();
                self.editor = Some(Editor::new(
                    Session {
                        port: 22,
                        folder,
                        ..Default::default()
                    },
                    true,
                ));
            }
            Action::NewSessionIn(folder) => {
                self.editor = Some(Editor::new(Session { port: 22, folder, ..Default::default() }, true));
            }
            Action::NewGroup(parent) => {
                self.group_editor = Some(GroupEditor { parent, name: String::new(), error: None });
            }
            Action::AskDeleteGroup(path) => self.confirm = Some(Confirm::DeleteGroup(path)),
            Action::Import => self.import_mobaxterm(),
            Action::ExportSessions(folder) => self.export_sessions(&folder),
            Action::ImportSessions => self.import_sessions(),
            Action::ToggleSessions => self.sessions_visible = !self.sessions_visible,
            Action::ShowSide(view) => {
                if self.sessions_visible && self.side_view == view {
                    self.sessions_visible = false;
                } else {
                    self.side_view = view;
                    self.sessions_visible = true;
                }
            }
            Action::NewMacro => self.macro_editor = Some(MacroEditor { item: Macro::default(), error: None }),
            Action::EditMacro(id) => {
                if let Some(item) = self.store.get_macro(id).cloned() {
                    self.macro_editor = Some(MacroEditor { item, error: None });
                }
            }
            Action::DuplicateMacro(id) => {
                if let Some(mut item) = self.store.get_macro(id).cloned() {
                    item.id = 0;
                    item.name = format!("{} (copie)", item.name);
                    self.macro_editor = Some(MacroEditor { item, error: None });
                }
            }
            Action::AskDeleteMacro(id) => self.confirm = Some(Confirm::DeleteMacro(id)),
            Action::RunMacro(id) => self.run_macro(id),
            Action::ToggleFiles => self.files_visible = !self.files_visible,
            Action::ToggleMonitor => self.monitor_visible = !self.monitor_visible,
            Action::SplitRight => self.split(false),
            Action::SplitDown => self.split(true),
            Action::GridOfFour => self.grid_of_four(),
            Action::EditHighlighting => {
                self.highlight_editor = Some(HighlightEditor::new(&self.store.highlighting));
            }
            Action::SetTheme(name) => {
                if theme::set_theme(ctx, &name) {
                    let pal = theme::p();
                    win_frame::set_frame_theme(pal.dark, pal.border);
                    self.store.theme = name;
                    self.save_store();
                }
            }
            Action::MoveToFolder { session, folder } => {
                if let Some(mut s) = self.store.get(session).cloned() {
                    if s.folder != folder {
                        s.folder = folder;
                        self.store.update(s);
                        self.save_store();
                    }
                }
            }
            Action::OpenLocal(index) => {
                if let Some(shell) = self.shells.get(index).cloned() {
                    self.open_target(Target::Local(shell), ctx);
                }
            }
            Action::FindInTerminal => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.view.open_search();
                }
            }
        }
    }

    /// Envoie les commandes d'une macro au terminal de l'onglet actif.
    fn run_macro(&mut self, id: u64) {
        let Some(item) = self.store.get_macro(id) else { return };
        let input = item.input();
        let name = item.name.clone();
        match self.tabs.get_mut(self.active) {
            Some(tab) if matches!(tab.state, TabState::Connected) => {
                tab.conn.send(Command::Input(input));
                tab.view.wants_focus = true;
            }
            _ => {
                self.notice = Some((
                    "Macro".into(),
                    format!("Aucun terminal connecté : ouvrez une session avant d'exécuter « {name} »."),
                ));
            }
        }
    }

    /// Enregistre les sessions (toutes, ou celles d'un groupe) dans un fichier
    /// JSON, relisible par « Importer des sessions ».
    fn export_sessions(&mut self, folder: &str) {
        let export = self.store.export(folder);
        if export.sessions.is_empty() && export.folders.is_empty() {
            self.notice = Some(("Export".into(), "Aucune session à exporter.".into()));
            return;
        }
        let stem = if folder.is_empty() {
            "sessions-ferroterm".to_string()
        } else {
            let name = folder.replace('/', "-");
            let name: String = name.chars().map(|c| if r#"\:*?"<>|"#.contains(c) { '_' } else { c }).collect();
            format!("sessions-ferroterm-{name}")
        };
        let Some(path) = rfd::FileDialog::new()
            .set_title(if folder.is_empty() { "Exporter les sessions".to_string() } else { format!("Exporter le groupe {folder}") })
            .set_file_name(format!("{stem}.json"))
            .add_filter("Sessions Ferroterm", &["json"])
            .save_file()
        else {
            return;
        };
        let result = export.to_json().and_then(|text| Ok(std::fs::write(&path, text)?));
        self.notice = Some(match result {
            Ok(()) => (
                "Sessions exportées".into(),
                format!(
                    "{} session(s) et {} groupe(s) enregistrés dans {}\n\nLes mots de passe ne sont pas exportés : ils restent dans le coffre Windows de ce poste. Les chemins des clés privées sont exportés tels quels.",
                    export.sessions.len(),
                    export.folders.len(),
                    path.display()
                ),
            ),
            Err(e) => ("Export impossible".into(), format!("{}: {e:#}", path.display())),
        });
    }

    /// Recharge un fichier créé par « Exporter les sessions ».
    fn import_sessions(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Importer des sessions Ferroterm")
            .add_filter("Sessions Ferroterm", &["json"])
            .pick_file()
        else {
            return;
        };
        let export = std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|text| crate::session::SessionExport::from_json(&text));
        let export = match export {
            Ok(export) => export,
            Err(e) => {
                self.notice = Some(("Import impossible".into(), format!("{}: {e:#}", path.display())));
                return;
            }
        };
        let found = export.sessions.len();
        let (added, duplicates) = self.store.import(export);
        self.save_store();
        self.notice = Some((
            "Import de sessions".into(),
            format!(
                "{found} session(s) trouvée(s) dans {}\n\n• {added} importée(s)\n• {duplicates} déjà présente(s) (même groupe et même nom)\n\nLes mots de passe ne font pas partie de l'export : ils seront demandés à la connexion.",
                path.display()
            ),
        ));
    }

    fn import_mobaxterm(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Importer les sessions de MobaXterm")
            .add_filter("MobaXterm.ini", &["ini"])
            .pick_file()
        else {
            return;
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                self.notice = Some(("Import impossible".into(), format!("{}: {e}", path.display())));
                return;
            }
        };
        let profile = std::env::var("USERPROFILE").unwrap_or_default();
        let path_text = path.to_string_lossy();
        let drive = if path_text.as_bytes().get(1) == Some(&b':') {
            path_text[..2].to_string()
        } else {
            String::new()
        };
        let result = mobaxterm::parse(&mobaxterm::decode(&bytes), &profile, &drive);
        let found = result.sessions.len();
        let (added, duplicates) = self.store.merge(result.sessions);
        self.save_store();

        let mut body = format!(
            "{found} session(s) SSH trouvée(s) dans {}\n\n• {added} importée(s)\n• {duplicates} déjà présente(s)",
            path.display()
        );
        if !result.skipped.is_empty() {
            body.push_str(&format!("\n• {} ignorée(s) :", result.skipped.len()));
            for line in &result.skipped {
                body.push_str(&format!("\n    – {line}"));
            }
        }
        body.push_str("\n\nLes mots de passe ne sont pas importés : ils seront demandés à la connexion.");
        self.notice = Some(("Import MobaXterm".into(), body));
    }

    // ------------------------------------------------------------------
    // Événements des connexions
    // ------------------------------------------------------------------

    /// Surveille les fichiers en édition : un enregistrement est renvoyé
    /// directement (envoi automatique) ou proposé à l'utilisateur.
    fn poll_edits(&mut self, ctx: &egui::Context) {
        use crate::edit::EditState;
        if self.edits.is_empty() {
            return;
        }
        // La copie locale est relue régulièrement, même sans activité dans la fenêtre.
        ctx.request_repaint_after(std::time::Duration::from_millis(400));
        let now = std::time::Instant::now();
        let mut ask = false;
        let mut index = 0;
        while index < self.edits.len() {
            let file = &mut self.edits[index];
            // Onglet fermé : fin du suivi.
            let Some(tab) = self.tabs.iter().find(|t| t.id == file.tab_id) else {
                let file = self.edits.remove(index);
                if let Some(notice) = stop_editing(&file) {
                    self.notice = Some(notice);
                }
                continue;
            };
            if file.poll(now) {
                if file.auto_upload {
                    start_upload(file, tab);
                } else {
                    file.state = EditState::AskUpload;
                    ask = true;
                }
            }
            index += 1;
        }
        if ask {
            // La question s'affiche ici alors que l'utilisateur est dans son éditeur.
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        }
    }

    /// Question posée à chaque enregistrement d'un fichier édité (comme MobaXterm).
    fn edit_upload_dialog(&mut self, ctx: &egui::Context) {
        use crate::edit::EditState;
        let Some(index) = self.edits.iter().position(|e| e.state == EditState::AskUpload) else { return };
        let file = &mut self.edits[index];
        let mut decision = None;
        theme::modal(ctx, Id::unique(("edit_upload", file.tab_id, file.remote.as_str()))).show(ctx, |ui| {
            ui.set_max_width(480.0);
            ui.label(RichText::new("Fichier modifié").size(15.0).family(theme::semibold()));
            ui.add_space(6.0);
            ui.label(format!("« {} » a été enregistré dans l'éditeur.", file.name()));
            ui.label(format!("L'envoyer sur {} ?", file.host));
            ui.add_space(4.0);
            ui.label(RichText::new(&file.remote).monospace().color(theme::p().text_dim));
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Si le fichier a changé sur le serveur depuis son ouverture, les deux versions sont \
                     fusionnées (comme git) ; en cas de conflit, rien n'est envoyé.",
                )
                .small()
                .color(theme::p().text_dim),
            );
            ui.add_space(8.0);
            ui.checkbox(&mut file.auto_upload, "Envoyer automatiquement les prochains enregistrements de ce fichier");
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if theme::primary_button(ui, "Envoyer").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                    decision = Some(true);
                }
                if ui.button("Ne pas envoyer").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    decision = Some(false);
                }
            });
        });
        match decision {
            Some(true) => match self.tabs.iter().find(|t| t.id == file.tab_id) {
                Some(tab) => start_upload(file, tab),
                None => file.state = EditState::Synced,
            },
            Some(false) => {
                file.auto_upload = false;
                file.skip_version();
            }
            None => {}
        }
    }

    fn poll_connections(&mut self) {
        let mut focus_tab = None;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            while let Ok(event) = tab.conn.events.try_recv() {
                match event {
                    Event::Status(text) => match &mut tab.state {
                        TabState::Connecting(s) => *s = text,
                        _ => tab.last_status = Some(text),
                    },
                    Event::AskHostKey { algorithm, fingerprint } => {
                        tab.prompt = Some(Prompt::HostKey { algorithm, fingerprint });
                        focus_tab = Some(index);
                    }
                    Event::AskText { prompt, secret, remember } => {
                        tab.prompt = Some(Prompt::Text {
                            prompt,
                            secret,
                            value: String::new(),
                            remember,
                        });
                        focus_tab = Some(index);
                    }
                    Event::Connected => {
                        tab.state = TabState::Connected;
                        tab.view.wants_focus = true;
                    }
                    Event::SftpReady { home } => {
                        tab.sftp.available = Some(Ok(()));
                        tab.sftp.loading = true;
                        // Si le dossier du terminal est déjà connu, on y va directement.
                        let start = tab
                            .sftp
                            .last_terminal_cwd
                            .clone()
                            .filter(|_| tab.sftp.follow_terminal)
                            .unwrap_or(home);
                        tab.conn.send(Command::Sftp(SftpCommand::List(start)));
                    }
                    Event::Monitor(stats) => {
                        if let Some(cwd) = &stats.cwd {
                            if tab.sftp.last_terminal_cwd.as_deref() != Some(cwd.as_str()) {
                                tab.sftp.last_terminal_cwd = Some(cwd.clone());
                                let ready = matches!(tab.sftp.available, Some(Ok(())));
                                if tab.sftp.follow_terminal && ready && tab.sftp.cwd != *cwd {
                                    tab.sftp.loading = true;
                                    tab.conn.send(Command::Sftp(SftpCommand::List(cwd.clone())));
                                }
                            }
                        }
                        tab.history.push(&stats);
                        tab.monitor = Some(stats);
                    }
                    Event::SftpUnavailable(reason) => tab.sftp.available = Some(Err(reason)),
                    Event::SftpListing { path, entries } => {
                        tab.sftp.path_edit = path.clone();
                        tab.sftp.cwd = path;
                        tab.sftp.entries = entries;
                        tab.sftp.selected = None;
                        tab.sftp.loading = false;
                        tab.sftp.error = None;
                    }
                    Event::SftpError(e) => {
                        tab.sftp.error = Some(e);
                        tab.sftp.loading = false;
                    }
                    Event::Transfer(update) => {
                        let refresh = update.finished && update.upload && update.error.is_none();
                        match tab.sftp.transfers.iter_mut().find(|t| t.id == update.id) {
                            Some(existing) => *existing = update,
                            None => tab.sftp.transfers.push(update),
                        }
                        if refresh && !tab.sftp.cwd.is_empty() {
                            tab.conn.send(Command::Sftp(SftpCommand::List(tab.sftp.cwd.clone())));
                        }
                    }
                    Event::EditReady { remote, local, error } => {
                        let Some(index) = self.edits.iter().position(|e| {
                            e.tab_id == tab.id && e.local == local && e.state == crate::edit::EditState::Downloading
                        }) else {
                            continue;
                        };
                        let problem = match error {
                            Some(e) => Some(("Édition impossible", format!("{remote} : {e}"))),
                            None if crate::edit::looks_binary(&local) => Some((
                                "Fichier binaire",
                                format!("{remote} n'est pas un fichier texte. Utilisez « Télécharger… »."),
                            )),
                            None => None,
                        };
                        if let Some((title, body)) = problem {
                            self.edits.remove(index).remove_local_copy();
                            self.notice = Some((title.into(), body));
                            continue;
                        }
                        let file = &mut self.edits[index];
                        file.downloaded();
                        if let Err(e) = crate::edit::open_in_editor(&file.local) {
                            self.notice = Some((
                                "Éditeur introuvable".into(),
                                format!("{} : {e}", crate::edit::editor().0.display()),
                            ));
                        }
                    }
                    Event::EditSaved { remote, result } => {
                        if let Some(file) = self.edits.iter_mut().find(|e| {
                            e.tab_id == tab.id && e.remote == remote && matches!(e.state, crate::edit::EditState::Uploading(_))
                        }) {
                            if let Some(message) = file.upload_finished(result) {
                                self.notice = Some(("Conflit à résoudre".into(), message));
                            }
                        }
                    }
                    Event::Closed(error) => {
                        tab.state = TabState::Closed(match error {
                            Some(e) => format!("Connexion interrompue : {e}"),
                            None => "Session terminée.".into(),
                        });
                        tab.prompt = None;
                        if tab.sftp.available.is_none() || matches!(tab.sftp.available, Some(Ok(()))) {
                            tab.sftp.available = Some(Err("session fermée".into()));
                        }
                        // Fichiers édités : les transferts en cours n'aboutiront pas.
                        self.edits.retain_mut(|file| {
                            if file.tab_id != tab.id {
                                return true;
                            }
                            match file.state {
                                crate::edit::EditState::Downloading => {
                                    file.remove_local_copy();
                                    false
                                }
                                crate::edit::EditState::Uploading(_) => {
                                    file.upload_finished(Err("connexion interrompue".into()));
                                    true
                                }
                                _ => true,
                            }
                        });
                    }
                }
            }
        }
        if let Some(index) = focus_tab {
            self.active = index;
        }
    }

    // ------------------------------------------------------------------
    // Panneau des sessions
    // ------------------------------------------------------------------

    /// Liste des macros : double-clic pour exécuter dans le terminal actif.
    fn macros_panel(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        if self.store.macros.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("Aucune macro pour l'instant.").color(theme::p().text_dim));
            ui.label(
                RichText::new("Une macro est une suite de commandes envoyée au terminal actif, une ligne à la fois.")
                    .small()
                    .color(theme::p().text_dim),
            );
            return;
        }
        egui::ScrollArea::vertical().id_salt("macros").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for item in &self.store.macros {
                let count = item.lines().count();
                let first = item.lines().next().unwrap_or("");
                let subtitle = if count > 1 { format!("{first}  (+{})", count - 1) } else { first.to_string() };
                let selected = self.selected_macro == Some(item.id);
                let response = theme::list_row(ui, selected, &theme::ICON_MACRO, &item.name, Some(&subtitle))
                    .on_hover_text(item.commands.trim_end());
                if response.clicked() {
                    self.selected_macro = Some(item.id);
                }
                if response.double_clicked() {
                    actions.push(Action::RunMacro(item.id));
                }
                response.context_menu(|ui| {
                    if ui.button(theme::label(&theme::ICON_PLAY, "Exécuter")).clicked() {
                        actions.push(Action::RunMacro(item.id));
                        ui.close();
                    }
                    if ui.button(theme::label(&theme::ICON_EDIT, "Modifier")).clicked() {
                        actions.push(Action::EditMacro(item.id));
                        ui.close();
                    }
                    if ui.button(theme::label(&theme::ICON_COPY, "Dupliquer")).clicked() {
                        actions.push(Action::DuplicateMacro(item.id));
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .button(RichText::new(theme::label(&theme::ICON_DELETE, "Supprimer")).color(theme::p().red))
                        .clicked()
                    {
                        actions.push(Action::AskDeleteMacro(item.id));
                        ui.close();
                    }
                });
            }
        });
    }

    fn sessions_panel(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text(theme::label(&theme::ICON_SEARCH, "Rechercher une session"))
                .margin(egui::Margin::symmetric(10, 6))
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut self.focus_filter) {
            search.request_focus();
        }
        ui.add_space(6.0);

        let filter = self.filter.to_lowercase();
        let mut root = FolderNode::default();
        let mut shown = 0;
        for session in &self.store.sessions {
            let matches = filter.is_empty()
                || session.name.to_lowercase().contains(&filter)
                || session.host.to_lowercase().contains(&filter)
                || session.folder.to_lowercase().contains(&filter);
            if matches {
                root.insert(session);
                shown += 1;
            }
        }
        // Groupes créés explicitement : affichés même vides.
        for folder in &self.store.folders {
            if filter.is_empty() || folder.to_lowercase().contains(&filter) {
                root.insert_folder(folder);
                shown += 1;
            }
        }

        if self.store.sessions.is_empty() && self.store.folders.is_empty() {
            ui.add_space(12.0);
            ui.label(RichText::new("Aucune session pour l'instant.").color(theme::p().text_dim));
            ui.label(
                RichText::new("Créez-en une, ou importez celles de MobaXterm avec le bouton d'import ci-dessus.")
                    .small()
                    .color(theme::p().text_dim),
            );
            return;
        }
        if shown == 0 {
            ui.label(RichText::new("Aucun résultat.").color(theme::p().text_dim));
            return;
        }

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let force_open = !filter.is_empty();
            self.draw_folder(ui, &root, "", force_open, actions);
            // Zone de dépôt « hors groupe », visible pendant un glissement.
            if let Some(payload) = egui::DragAndDrop::payload::<DragSession>(ui.ctx()) {
                ui.add_space(8.0);
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::hover());
                let hot = response.contains_pointer();
                if hot {
                    ui.painter().rect_filled(rect, 7.0, theme::p().accent_soft);
                }
                ui.painter().rect_stroke(
                    rect,
                    7.0,
                    egui::Stroke::new(1.0, if hot { theme::p().accent } else { theme::p().border }),
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("Déposer « {} » ici : hors de tout groupe", payload.name),
                    egui::FontId::proportional(12.5),
                    theme::p().text_dim,
                );
                if let Some(payload) = response.dnd_release_payload::<DragSession>() {
                    actions.push(Action::MoveToFolder { session: payload.id, folder: String::new() });
                }
            }
        });
    }

    fn draw_folder(
        &mut self,
        ui: &mut egui::Ui,
        node: &FolderNode,
        path: &str,
        force_open: bool,
        actions: &mut Vec<Action>,
    ) {
        let dragging = egui::DragAndDrop::payload::<DragSession>(ui.ctx()).is_some();
        for (name, child) in &node.children {
            let child_path = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}/{name}")
            };
            ui.add_space(2.0);
            // État déplié mémorisé par dossier ; une recherche force l'ouverture.
            let id = ui.make_persistent_id(("folder", &child_path));
            let stored = ui.data_mut(|d| *d.get_persisted_mut_or(id, true));
            let open = stored || force_open;
            let header = theme::group_header(ui, name, child.count(), open, dragging);
            if header.clicked() && !force_open {
                ui.data_mut(|d| d.insert_persisted(id, !stored));
            }
            if let Some(payload) = header.dnd_release_payload::<DragSession>() {
                actions.push(Action::MoveToFolder { session: payload.id, folder: child_path.clone() });
            }
            header.context_menu(|ui| {
                if ui.button(theme::label(&theme::ICON_ADD, "Nouvelle session ici…")).clicked() {
                    actions.push(Action::NewSessionIn(child_path.clone()));
                    ui.close();
                }
                if ui.button(theme::label(&theme::ICON_NEW_FOLDER, "Nouveau sous-groupe…")).clicked() {
                    actions.push(Action::NewGroup(child_path.clone()));
                    ui.close();
                }
                if ui.button(theme::label(&theme::ICON_DOWNLOAD, "Exporter le groupe…")).clicked() {
                    actions.push(Action::ExportSessions(child_path.clone()));
                    ui.close();
                }
                ui.separator();
                if ui
                    .button(RichText::new(theme::label(&theme::ICON_DELETE, "Supprimer le groupe")).color(theme::p().red))
                    .clicked()
                {
                    actions.push(Action::AskDeleteGroup(child_path.clone()));
                    ui.close();
                }
            });
            if open {
                ui.scope(|ui| {
                    ui.spacing_mut().indent = 12.0;
                    ui.visuals_mut().indent_has_left_vline = false;
                    ui.indent(id, |ui| {
                        self.draw_folder(ui, child, &child_path, force_open, actions);
                    });
                });
            }
        }
        for session in &node.sessions {
            let selected = self.selected_session == Some(session.id);
            let response = theme::list_row_badge(
                ui,
                selected,
                &theme::ICON_SERVER,
                &session.name,
                session.has_profiles().then(|| session.profile_name()),
                Some(&session.target()),
            );
            // Pastille d'état si la session est ouverte dans au moins un onglet,
            // avec les mêmes couleurs que les onglets (le meilleur état l'emporte).
            let open_state = self
                .tabs
                .iter()
                .filter(|t| t.target.session_id() == Some(session.id))
                .map(|t| match t.state {
                    TabState::Connected => 2,
                    TabState::Connecting(_) => 1,
                    TabState::Closed(_) => 0,
                })
                .max();
            if let Some(state) = open_state {
                if ui.is_rect_visible(response.rect) {
                    let color = match state {
                        2 => theme::p().green,
                        1 => theme::p().yellow,
                        _ => theme::p().text_dim,
                    };
                    // Insigne au coin bas-droit de l'icône, détouré par la couleur du fond.
                    let ring = if selected {
                        theme::p().accent_soft
                    } else if response.hovered() {
                        theme::p().surface_hover
                    } else {
                        theme::p().sidebar
                    };
                    let center = response.rect.left_center() + egui::vec2(24.0, 5.0);
                    ui.painter().circle_filled(center, 5.0, ring);
                    ui.painter().circle_filled(center, 3.5, color);
                }
            }
            response.dnd_set_drag_payload(DragSession { id: session.id, name: session.name.clone() });
            if response.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                if let Some(pos) = ui.ctx().pointer_latest_pos() {
                    egui::Area::new(Id::unique("drag_preview"))
                        .order(egui::Order::Tooltip)
                        .interactable(false)
                        .fixed_pos(pos + egui::vec2(14.0, 10.0))
                        .show(ui.ctx(), |ui| {
                            egui::Frame::popup(ui.style()).show(ui, |ui| {
                                ui.label(theme::label(&theme::ICON_SERVER, &session.name));
                            });
                        });
                }
            }
            // Déposer sur une session la range dans le groupe de celle-ci.
            if let Some(payload) = response.dnd_release_payload::<DragSession>() {
                if payload.id != session.id {
                    actions.push(Action::MoveToFolder { session: payload.id, folder: session.folder.clone() });
                }
            }
            if response.clicked() {
                self.selected_session = Some(session.id);
            }
            if response.double_clicked() {
                actions.push(Action::Connect(session.id));
            }
            response.context_menu(|ui| {
                if ui.button(theme::label(&theme::ICON_PLAY, "Se connecter")).clicked() {
                    actions.push(Action::Connect(session.id));
                    ui.close();
                }
                ui.menu_button(theme::label(&theme::ICON_SERVER, "Changer de profil"), |ui| {
                    for profile in session.profiles() {
                        let active = profile.id == session.profile;
                        if ui.selectable_label(active, &profile.name).on_hover_text(profile.target()).clicked() {
                            actions.push(Action::SwitchProfile { session: session.id, profile: profile.id });
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button(theme::label(&theme::ICON_ADD, "Nouveau profil…")).clicked() {
                        actions.push(Action::NewProfile(session.id));
                        ui.close();
                    }
                });
                ui.separator();
                if ui.button(theme::label(&theme::ICON_EDIT, "Modifier")).clicked() {
                    actions.push(Action::Edit(session.id));
                    ui.close();
                }
                if ui.button(theme::label(&theme::ICON_COPY, "Dupliquer")).clicked() {
                    actions.push(Action::Duplicate(session.id));
                    ui.close();
                }
                ui.separator();
                if ui
                    .button(RichText::new(theme::label(&theme::ICON_DELETE, "Supprimer")).color(theme::p().red))
                    .clicked()
                {
                    actions.push(Action::AskDelete(session.id));
                    ui.close();
                }
            });
        }
    }

    // ------------------------------------------------------------------
    // Panneau SFTP
    // ------------------------------------------------------------------

    fn sftp_panel(&mut self, ui: &mut egui::Ui) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            ui.label(RichText::new("Ouvrez une session SSH pour parcourir ses fichiers.").weak());
            return;
        };
        ui.label(RichText::new(tab.title()).family(theme::semibold()));
        if matches!(tab.target, Target::Local(_)) {
            ui.label(RichText::new("L'explorateur SFTP est disponible pour les sessions SSH.").weak());
            return;
        }
        match &tab.sftp.available {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Connexion SFTP…");
                });
                return;
            }
            Some(Err(reason)) => {
                ui.label(RichText::new(format!("SFTP indisponible : {reason}")).weak());
                return;
            }
            Some(Ok(())) => {}
        }

        let tab_id = tab.id;
        let host = tab.target.subtitle();
        let sftp = &mut tab.sftp;
        let conn = &tab.conn;
        let mut navigate: Option<String> = None;

        ui.horizontal(|ui| {
            if theme::icon_button(ui, &theme::ICON_UP, "Dossier parent").clicked() {
                navigate = Some(parent_dir(&sftp.cwd));
            }
            if theme::icon_button(ui, &theme::ICON_REFRESH, "Actualiser").clicked() {
                navigate = Some(sftp.cwd.clone());
            }
            if theme::icon_button(ui, &theme::ICON_UPLOAD, "Envoyer des fichiers…").clicked() {
                if let Some(files) = rfd::FileDialog::new().set_title("Envoyer vers le serveur").pick_files() {
                    for local in files {
                        conn.send(Command::Sftp(SftpCommand::Upload {
                            local,
                            remote_dir: sftp.cwd.clone(),
                        }));
                    }
                }
            }
            if theme::icon_button(ui, &theme::ICON_NEW_FOLDER, "Nouveau dossier").clicked() {
                self.mkdir_name = Some(String::new());
            }
            let follow = theme::title_action(
                ui,
                &theme::ICON_TERMINAL,
                "Suivre le dossier courant du terminal",
                sftp.follow_terminal,
            );
            if follow.clicked() {
                sftp.follow_terminal = !sftp.follow_terminal;
                if sftp.follow_terminal {
                    if let Some(cwd) = sftp.last_terminal_cwd.clone() {
                        navigate = Some(cwd);
                    }
                }
            }
        });
        let path_response = ui.add(
            egui::TextEdit::singleline(&mut sftp.path_edit)
                .margin(egui::Margin::symmetric(10, 6))
                .desired_width(f32::INFINITY),
        );
        if path_response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            navigate = Some(sftp.path_edit.clone());
        }
        if let Some(error) = &sftp.error {
            ui.colored_label(theme::p().red, error);
        }
        ui.add_space(4.0);

        // Fichiers de cet onglet ouverts en édition : section sous la liste.
        let edit_count = self.edits.iter().filter(|e| e.tab_id == tab_id).count();
        let edits_height = if edit_count == 0 { 0.0 } else { 34.0 + 26.0 * edit_count.min(4) as f32 };
        let transfers_height = edits_height + if sftp.transfers.is_empty() { 0.0 } else { 110.0 };
        let mut download: Option<RemoteEntry> = None;
        let mut edit: Option<RemoteEntry> = None;
        let mut delete: Option<(String, bool)> = None;
        egui::ScrollArea::vertical()
            .id_salt("sftp_list")
            .auto_shrink(false)
            .max_height(ui.available_height() - transfers_height)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                if sftp.loading {
                    // Pas d'indicateur de chargement : la liste reste visible, grisée.
                    ui.disable();
                }
                for (index, entry) in sftp.entries.iter().enumerate() {
                    let icon = if entry.is_dir {
                        &theme::ICON_FOLDER
                    } else if entry.is_symlink {
                        &theme::ICON_LINK
                    } else {
                        &theme::ICON_FILE
                    };
                    let response = theme::list_row(ui, sftp.selected == Some(index), icon, &entry.name, None)
                        .on_hover_text(describe_entry(entry));
                    if response.clicked() {
                        sftp.selected = Some(index);
                    }
                    if response.double_clicked() {
                        if entry.is_dir {
                            navigate = Some(join_remote(&sftp.cwd, &entry.name));
                        } else {
                            edit = Some(entry.clone());
                        }
                    }
                    response.context_menu(|ui| {
                        if entry.is_dir {
                            if ui.button(theme::label(&theme::ICON_FOLDER, "Ouvrir")).clicked() {
                                navigate = Some(join_remote(&sftp.cwd, &entry.name));
                                ui.close();
                            }
                        } else {
                            let editor = crate::edit::editor().1;
                            if ui
                                .button(theme::label(&theme::ICON_EDIT, "Éditer"))
                                .on_hover_text(format!("Ouvre une copie dans {editor} ; chaque enregistrement est proposé au renvoi sur le serveur."))
                                .clicked()
                            {
                                edit = Some(entry.clone());
                                ui.close();
                            }
                            if ui.button(theme::label(&theme::ICON_DOWNLOAD, "Télécharger…")).clicked() {
                                download = Some(entry.clone());
                                ui.close();
                            }
                        }
                        if ui.button(theme::label(&theme::ICON_COPY, "Copier le chemin")).clicked() {
                            ui.ctx().copy_text(join_remote(&sftp.cwd, &entry.name));
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .button(RichText::new(theme::label(&theme::ICON_DELETE, "Supprimer")).color(theme::p().red))
                            .clicked()
                        {
                            delete = Some((join_remote(&sftp.cwd, &entry.name), entry.is_dir));
                            ui.close();
                        }
                    });
                }
                if sftp.entries.is_empty() && !sftp.loading {
                    ui.label(RichText::new("Dossier vide").weak());
                }
            });

        if edit_count > 0 {
            use crate::edit::EditState;
            ui.separator();
            ui.label(theme::section("En cours d'édition"));
            let mut reopen = None;
            let mut resend = None;
            let mut stop = None;
            egui::ScrollArea::vertical().id_salt("edits").max_height(26.0 * 4.0).show(ui, |ui| {
                for (index, file) in self.edits.iter().enumerate().filter(|(_, e)| e.tab_id == tab_id) {
                    ui.horizontal(|ui| {
                        let pal = theme::p();
                        ui.label(theme::label(&theme::ICON_EDIT, file.name()))
                            .on_hover_text(format!("{}\nCopie locale : {}", file.remote, file.local.display()));
                        let (text, color) = match &file.state {
                            EditState::Downloading => ("téléchargement…".to_string(), pal.text_dim),
                            EditState::Synced => (file.note.clone().unwrap_or_else(|| "à jour".into()), pal.text_dim),
                            EditState::Conflicted => ("conflit à résoudre".to_string(), pal.red),
                            EditState::AskUpload => ("modifié".to_string(), pal.yellow),
                            EditState::Uploading(_) => ("envoi…".to_string(), pal.text_dim),
                            EditState::Failed(e) => (format!("échec : {e}"), pal.red),
                        };
                        ui.label(RichText::new(text).small().color(color));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_button(ui, &theme::ICON_CLOSE, "Arrêter le suivi").clicked() {
                                stop = Some(index);
                            }
                            if matches!(file.state, EditState::Failed(_))
                                && theme::icon_button(ui, &theme::ICON_UPLOAD, "Renvoyer sur le serveur").clicked()
                            {
                                resend = Some(index);
                            }
                            if file.state != EditState::Downloading
                                && theme::icon_button(ui, &theme::ICON_FILE, "Rouvrir dans l'éditeur").clicked()
                            {
                                reopen = Some(index);
                            }
                        });
                    });
                }
            });
            if let Some(file) = reopen.and_then(|i| self.edits.get(i)) {
                if let Err(e) = crate::edit::open_in_editor(&file.local) {
                    self.notice = Some(("Éditeur introuvable".into(), format!("{} : {e}", crate::edit::editor().0.display())));
                }
            }
            if let Some(file) = resend.and_then(|i| self.edits.get_mut(i)) {
                if let Some((base, ours)) = file.upload_started() {
                    conn.send(Command::Sftp(SftpCommand::SaveEdited { remote: file.remote.clone(), base, ours }));
                }
            }
            if let Some(index) = stop {
                let file = self.edits.remove(index);
                if let Some(notice) = stop_editing(&file) {
                    self.notice = Some(notice);
                }
            }
        }

        if !sftp.transfers.is_empty() {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(theme::section("Transferts"));
                if ui.small_button("Effacer les terminés").clicked() {
                    sftp.transfers.retain(|t| !t.finished);
                }
            });
            egui::ScrollArea::vertical().id_salt("transfers").show(ui, |ui| {
                for t in sftp.transfers.iter().rev() {
                    if let Some(error) = &t.error {
                        ui.colored_label(theme::p().red, format!("{} : {error}", t.label));
                    } else if t.finished {
                        ui.label(format!("{} ✔ {}", t.label, human_size(t.done)));
                    } else {
                        let fraction = t.total.filter(|&n| n > 0).map(|n| t.done as f32 / n as f32);
                        let bar = egui::ProgressBar::new(fraction.unwrap_or(0.0))
                            .text(format!("{} — {}", t.label, human_size(t.done)));
                        ui.add(if fraction.is_none() { bar.animate(true) } else { bar });
                    }
                }
            });
        }

        if let Some(entry) = download {
            if let Some(local) = rfd::FileDialog::new().set_file_name(&entry.name).save_file() {
                conn.send(Command::Sftp(SftpCommand::Download {
                    remote: join_remote(&sftp.cwd, &entry.name),
                    local,
                }));
            }
        }
        if let Some(entry) = edit {
            let remote = join_remote(&sftp.cwd, &entry.name);
            if let Some(file) = self.edits.iter().find(|e| e.tab_id == tab_id && e.remote == remote) {
                // Déjà ouvert : on rouvre la copie locale (sans écraser ses modifications).
                if file.state != crate::edit::EditState::Downloading {
                    if let Err(e) = crate::edit::open_in_editor(&file.local) {
                        self.notice = Some(("Éditeur introuvable".into(), format!("{} : {e}", crate::edit::editor().0.display())));
                    }
                }
            } else if entry.size > crate::edit::MAX_EDIT_SIZE {
                self.notice = Some((
                    "Fichier trop volumineux".into(),
                    format!(
                        "{} fait {} : l'édition est limitée à {}. Utilisez « Télécharger… ».",
                        entry.name,
                        human_size(entry.size),
                        human_size(crate::edit::MAX_EDIT_SIZE)
                    ),
                ));
            } else {
                self.next_edit += 1;
                let local = crate::edit::local_path(self.next_edit, &remote);
                match local.parent().map(std::fs::create_dir_all).transpose() {
                    Ok(_) => {
                        conn.send(Command::Sftp(SftpCommand::FetchForEdit { remote: remote.clone(), local: local.clone() }));
                        self.edits.push(crate::edit::EditedFile::new(tab_id, host, remote, local));
                    }
                    Err(e) => {
                        self.notice = Some(("Édition impossible".into(), format!("{} : {e}", local.display())));
                    }
                }
            }
        }
        if let Some((path, is_dir)) = delete {
            self.confirm = Some(Confirm::DeleteRemote { path, is_dir });
        }
        if let Some(path) = navigate {
            sftp.loading = true;
            conn.send(Command::Sftp(SftpCommand::List(path)));
        }
    }

    // ------------------------------------------------------------------
    // Onglets et terminal
    // ------------------------------------------------------------------

    fn welcome(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.28).max(20.0));
            ui.label(RichText::new(theme::ICON_TERMINAL.get()).size(46.0).color(theme::p().accent));
            ui.add_space(4.0);
            ui.label(RichText::new("Ferroterm").size(30.0).family(theme::semibold()));
            ui.label(RichText::new("Terminal SSH, fichiers distants et shells locaux").color(theme::p().text_dim));
            ui.add_space(20.0);
            // Rangée centrée : sa largeur réelle est mesurée d'une image sur l'autre.
            // Si elle ne tient plus dans la largeur disponible, les boutons s'empilent.
            let buttons = |ui: &mut egui::Ui, actions: &mut Vec<Action>| {
                if theme::primary_button(ui, &theme::label(&theme::ICON_ADD, "Nouvelle session")).clicked() {
                    actions.push(Action::NewSession);
                }
                self.local_menu(ui, &theme::label(&theme::ICON_TERMINAL, "Terminal local"), actions);
                if ui.button(theme::label(&theme::ICON_IMPORT, "Importer depuis MobaXterm")).clicked() {
                    actions.push(Action::Import);
                }
            };
            let width_id = ui.scope_id().with("welcome_buttons_width");
            let known: Option<f32> = ui.data(|d| d.get_temp(width_id));
            let available = ui.available_width();
            if known.is_some_and(|width| width > available - 16.0) {
                buttons(ui, actions);
            } else {
                let row = ui.horizontal(|ui| {
                    ui.add_space(((available - known.unwrap_or(0.0)) / 2.0).max(0.0));
                    let start = ui.cursor().left();
                    buttons(ui, actions);
                    ui.min_rect().right() - start
                });
                if known != Some(row.inner) {
                    ui.data_mut(|d| d.insert_temp(width_id, row.inner));
                    ui.ctx().request_repaint();
                }
            }
            ui.add_space(22.0);
            ui.label(
                RichText::new("Double-clic sur une session pour se connecter  ·  sélection = copier  ·  clic droit = coller  ·  Ctrl+F = rechercher")
                    .small()
                    .color(theme::p().text_dim),
            );
        });
    }

    fn central(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        self.normalize_groups();
        if self.tabs.is_empty() && self.groups == 1 {
            self.welcome(ui, actions);
            return;
        }
        // Chaque groupe sauf le dernier est un panneau redimensionnable ; le
        // dernier occupe le reste. L'identifiant dépend du nombre de groupes :
        // la largeur est répartie à parts égales à chaque nouvelle scission.
        let count = self.groups;
        if self.arrangement == Arrangement::Grid {
            // Grille : des rangées empilées, chacune découpée en colonnes. Les
            // tailles d'une grille ne sont pas mémorisées.
            let layout = self.rows.clone();
            let rows = layout.len();
            let mut start = 0;
            for (row, &len) in layout.iter().enumerate() {
                let range = start..(start + len).min(count);
                start += len;
                if range.is_empty() {
                    break;
                }
                if row + 1 == rows {
                    self.group_row(ui, count, range);
                    break;
                }
                let remaining = (rows - row) as f32;
                let available = ui.available_height();
                let max = (available - MIN_GROUP_HEIGHT * (remaining - 1.0)).max(MIN_GROUP_HEIGHT);
                egui::Panel::top(("grid_row", rows, row))
                    .resizable(true)
                    .default_size((available / remaining).clamp(MIN_GROUP_HEIGHT, max))
                    .size_range(MIN_GROUP_HEIGHT..=max)
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| self.group_row(ui, count, range));
            }
            if self.groups == count {
                self.group_widths = Vec::new();
            }
            return;
        }
        let stacked = self.arrangement == Arrangement::Rows;
        let min = if stacked { MIN_GROUP_HEIGHT } else { MIN_GROUP_WIDTH };
        // Tailles de la dernière utilisation, si le nombre de groupes est le même.
        let restored = (self.restored_widths.len() + 1 == count).then(|| self.restored_widths.clone());
        let mut sizes = Vec::with_capacity(count.saturating_sub(1));
        for group in 0..count.saturating_sub(1) {
            let remaining = (count - group) as f32;
            let available = if stacked { ui.available_height() } else { ui.available_width() };
            let max = (available - min * (remaining - 1.0)).max(min);
            let default = restored.as_ref().map(|w| w[group]).unwrap_or(available / remaining);
            let id = ("group", stacked, count, group);
            let panel = if stacked { egui::Panel::top(id) } else { egui::Panel::left(id) }
                .resizable(true)
                .default_size(default.clamp(min, max))
                .size_range(min..=max)
                .frame(egui::Frame::NONE)
                .show(ui, |ui| self.group_view(ui, group));
            let rect = panel.response.rect;
            sizes.push(if stacked { rect.height() } else { rect.width() }.round());
        }
        // Un groupe a pu être ajouté ou retiré pendant le dessin : mesures à refaire.
        if self.groups == count {
            self.group_widths = sizes;
        }
        self.group_view(ui, count - 1);
    }

    /// Groupe affiché en haut à droite : il porte les boutons de scission.
    fn corner_group(&self) -> usize {
        // Dernier groupe de la première rangée.
        self.rows.first().copied().unwrap_or(1).min(self.groups).saturating_sub(1)
    }

    /// Une ligne de la grille : ses groupes côte à côte, le dernier prenant le reste.
    fn group_row(&mut self, ui: &mut egui::Ui, count: usize, range: std::ops::Range<usize>) {
        for group in range.start..range.end - 1 {
            let remaining = (range.end - group) as f32;
            let available = ui.available_width();
            let max = (available - MIN_GROUP_WIDTH * (remaining - 1.0)).max(MIN_GROUP_WIDTH);
            egui::Panel::left(("grid_column", count, group))
                .resizable(true)
                .default_size((available / remaining).clamp(MIN_GROUP_WIDTH, max))
                .size_range(MIN_GROUP_WIDTH..=max)
                .frame(egui::Frame::NONE)
                .show(ui, |ui| self.group_view(ui, group));
        }
        self.group_view(ui, range.end - 1);
    }

    /// Remet les groupes dans un état cohérent : pas de groupe vide (sauf les
    /// groupes vierges issus d'une scission), un onglet actif valide par groupe,
    /// onglet global = onglet actif du groupe actif.
    fn normalize_groups(&mut self) {
        let count = self.groups.max(1);
        self.groups = count;
        self.fix_rows();
        self.blank.resize(count, false);
        for tab in &mut self.tabs {
            tab.group = tab.group.min(count - 1);
        }
        // Supprime les groupes vides : les suivants se décalent vers la gauche.
        // Un groupe vierge cesse de l'être dès qu'il reçoit un onglet.
        let mut map: Vec<Option<usize>> = Vec::with_capacity(count);
        let mut kept = 0;
        for group in 0..count {
            if self.tabs.iter().any(|t| t.group == group) {
                self.blank[group] = false;
            }
            if self.tabs.iter().any(|t| t.group == group) || self.blank[group] {
                map.push(Some(kept));
                kept += 1;
            } else {
                map.push(None);
            }
        }
        // Plus rien d'ouvert (ou un seul groupe vierge) : retour à l'accueil.
        if kept == 0 || (kept == 1 && self.tabs.is_empty()) {
            self.groups = 1;
            self.active_group = 0;
            self.group_active = vec![None];
            self.scrolled_to = vec![None];
            self.blank = vec![false];
            self.rows = vec![1];
            self.arrangement = Arrangement::Columns;
            return;
        }
        if kept != count {
            // Chaque rangée perd ses groupes supprimés ; une rangée vide disparaît.
            let mut start = 0;
            let mut rows = Vec::with_capacity(self.rows.len());
            for &len in &self.rows {
                let left = (start..start + len).filter(|g| map[*g].is_some()).count();
                if left > 0 {
                    rows.push(left);
                }
                start += len;
            }
            self.rows = rows;
            self.arrangement = arrangement_of(&self.rows);
            self.blank = (0..count).filter(|g| map[*g].is_some()).map(|g| self.blank[g]).collect();
            for tab in &mut self.tabs {
                tab.group = map[tab.group].unwrap_or(0);
            }
            let keep = |values: &[Option<u64>]| -> Vec<Option<u64>> {
                (0..count).filter(|g| map[*g].is_some()).map(|g| values.get(g).copied().flatten()).collect()
            };
            self.group_active = keep(&self.group_active);
            self.scrolled_to = keep(&self.scrolled_to);
            // Groupe actif supprimé : on prend son voisin de gauche.
            self.active_group = (0..=self.active_group.min(count - 1)).rev().find_map(|g| map[g]).unwrap_or(0);
            self.groups = kept;
        }
        self.group_active.resize(self.groups, None);
        self.scrolled_to.resize(self.groups, None);
        self.active_group = self.active_group.min(self.groups - 1);
        for group in 0..self.groups {
            let valid = self.group_active[group]
                .map(|id| self.tabs.iter().any(|t| t.id == id && t.group == group))
                .unwrap_or(false);
            if !valid {
                self.group_active[group] = self.tabs.iter().find(|t| t.group == group).map(|t| t.id);
            }
        }
        if let Some(index) = self.group_active[self.active_group].and_then(|id| self.tabs.iter().position(|t| t.id == id)) {
            self.active = index;
        }
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
    }

    /// Groupe vierge, issu d'une scission : sessions récentes à ouvrir ici.
    fn blank_view(&mut self, ui: &mut egui::Ui, group: usize) {
        let rect = ui.available_rect_before_wrap();
        let pal = theme::p();
        ui.painter().rect_filled(rect, 0.0, pal.bg);
        // Un clic dans le vide rend ce groupe actif : les sessions ouvertes
        // depuis le panneau de gauche s'y placent.
        if ui.interact(rect, Id::unique(("blank_group", group)), egui::Sense::click()).clicked() {
            self.active_group = group;
        }
        let recent: Vec<(u64, String, String)> = {
            let mut sessions = self.store.recent_sessions();
            sessions.truncate(8);
            sessions.iter().map(|s| (s.id, s.name.clone(), s.target())).collect()
        };
        let mut actions = Vec::new();
        let mut close = false;
        let mut split_request: Option<bool> = None;
        let corner = group == self.corner_group();
        // Même barre que les autres groupes : un onglet « Nouvel onglet » (sa croix
        // ferme le groupe), puis les scissions en haut à droite.
        let mut tab_rect: Option<egui::Rect> = None;
        let focused = self.groups <= 1 || self.active_group == group;
        let bar = egui::Frame::NONE
            .fill(pal.sidebar)
            .inner_margin(egui::Margin { left: 0, right: 6, top: 0, bottom: 0 })
            .show(ui, |ui| {
                // Hauteur fixe (celle de la barre d'onglets) : sinon la mise en page
                // occupe tout le groupe et centre les boutons verticalement.
                let size = egui::vec2(ui.available_width(), 35.0);
                ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_size(size);
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let tab = theme::tab(ui, true, focused, pal.text_dim, None, "Nouvel onglet", 260.0);
                    tab_rect = Some(tab.response.rect);
                    // Glisser l'onglet déplace le groupe vierge entier.
                    tab.response.dnd_set_drag_payload(DragBlank { group });
                    if tab.response.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    }
                    if tab.clicked {
                        self.active_group = group;
                    }
                    close = tab.close;
                    if corner {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::split_button(ui, "Scinder en bas", true).clicked() {
                                split_request = Some(true);
                            }
                            if theme::split_button(ui, "Scinder à droite", false).clicked() {
                                split_request = Some(false);
                            }
                        });
                    }
                });
            });
        // Bordure sous la barre, interrompue sous l'onglet (comme les autres groupes).
        let strip = bar.response.rect;
        let y = strip.bottom() - 0.5;
        ui.painter().hline(strip.x_range(), y, egui::Stroke::new(1.0, pal.border));
        if let Some(rect) = tab_rect {
            ui.painter().hline(rect.x_range(), y, egui::Stroke::new(1.0, pal.bg));
        }
        // Le contenu est dessiné dans une zone détachée, rognée à la place
        // disponible : un panneau redimensionnable reprend sinon la hauteur de
        // son contenu, et le groupe refuserait de rétrécir sous celle-ci.
        let body = ui.available_rect_before_wrap();
        let mut content = ui.new_child(
            egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Center)),
        );
        content.shrink_clip_rect(body);
        ui.advance_cursor_after_rect(body);
        content.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.18).max(8.0));
            ui.label(RichText::new("Sessions récentes").size(16.0).family(theme::semibold()));
            ui.add_space(8.0);
            if recent.is_empty() {
                ui.label(RichText::new("Aucune session ouverte récemment").color(pal.text_dim));
            }
            for (id, name, target) in &recent {
                let text = theme::label(&theme::ICON_SERVER, name);
                if ui.add(egui::Button::new(text).frame_when_inactive(false)).on_hover_text(target).clicked() {
                    actions.push(Action::Connect(*id));
                }
            }
            ui.add_space(14.0);
            self.local_menu(ui, &theme::label(&theme::ICON_TERMINAL, "Terminal local"), &mut actions);
            ui.add_space(4.0);
            ui.label(
                RichText::new("ou ouvrez une session depuis le panneau de gauche").small().color(pal.text_dim),
            );
        });
        if self.drop_blank_group(ui, group, rect) {
            return;
        }
        // Déposer un onglet ici le place dans ce groupe.
        if let Some(payload) = egui::DragAndDrop::payload::<DragTab>(ui.ctx()) {
            if ui.ctx().pointer_latest_pos().is_some_and(|pos| rect.contains(pos)) {
                ui.painter().rect_filled(rect, 0.0, pal.accent.gamma_multiply(0.08));
                if ui.input(|i| i.pointer.any_released()) {
                    egui::DragAndDrop::clear_payload(ui.ctx());
                    self.move_tab(payload.id, group, None);
                    return;
                }
            }
        }
        if let Some(down) = split_request {
            // Les boutons sont dans le coin, mais la scission vise le groupe actif.
            self.split(down);
            return;
        }
        if close {
            self.blank[group] = false;
            self.normalize_groups();
            return;
        }
        if !actions.is_empty() {
            self.active_group = group;
            let ctx = ui.ctx().clone();
            for action in actions {
                self.apply(action, &ctx);
            }
        }
    }

    /// Un groupe d'onglets : sa barre d'onglets, les bandeaux d'état et le terminal actif.
    fn group_view(&mut self, ui: &mut egui::Ui, group: usize) {
        // Un groupe a pu disparaître pendant le dessin des précédents.
        let Some(mut active_index) =
            self.group_active.get(group).copied().flatten().and_then(|id| self.tabs.iter().position(|t| t.id == id))
        else {
            if self.blank.get(group) == Some(&true) {
                self.blank_view(ui, group);
            }
            return;
        };
        if self.drop_blank_group(ui, group, ui.available_rect_before_wrap()) {
            return;
        }
        let split = self.groups > 1;
        let is_active_group = !split || self.active_group == group;
        // Glisser-déposer d'onglets : (onglet, groupe cible, ancre).
        let mut drop: Option<(u64, usize, Option<(u64, bool)>)> = None;
        let pointer = ui.ctx().pointer_latest_pos();

        let mut close: Option<usize> = None;
        let mut activate: Option<usize> = None;
        // Onglet à renommer (menu contextuel ou double-clic).
        let mut rename: Option<usize> = None;
        // Renommage en cours, modifié directement dans l'onglet ; sa fin :
        // vrai = nom validé, faux = annulé.
        let mut editing = self.tab_rename.take();
        let mut rename_end: Option<bool> = None;
        // Scission demandée : vrai = vers le bas, faux = à droite.
        let mut split_request: Option<bool> = None;
        // Libellés du menu des onglets, selon la disposition des groupes.
        let (to_previous, to_next, to_new) = match self.arrangement {
            Arrangement::Columns => (
                "Déplacer vers le groupe de gauche",
                "Déplacer vers le groupe de droite",
                "Déplacer dans un nouveau groupe à droite",
            ),
            Arrangement::Rows => (
                "Déplacer vers le groupe du haut",
                "Déplacer vers le groupe du bas",
                "Déplacer dans un nouveau groupe en bas",
            ),
            Arrangement::Grid => (
                "Déplacer vers le groupe précédent",
                "Déplacer vers le groupe suivant",
                "Déplacer dans un nouveau groupe",
            ),
        };
        let mut active_rect: Option<egui::Rect> = None;
        let mut scrolled: Option<u64> = None;
        let margin = egui::Margin { left: 0, right: 6, top: 0, bottom: 0 };
        let bar = egui::Frame::NONE
            .fill(theme::p().sidebar)
            .inner_margin(margin)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    // Boutons « scinder » : uniquement dans le groupe en haut à droite.
                    let corner = group == self.corner_group();
                    let actions_width = if corner { 66.0 } else { 0.0 };
                    // Largeur naturelle des onglets (style VS Code) : la barre défile si ça déborde.
                    let tab_max_width = 260.0;
                    egui::ScrollArea::horizontal()
                        .id_salt(("tabs", group))
                        .max_width((ui.available_width() - actions_width).max(60.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                for (index, tab) in self.tabs.iter().enumerate().filter(|(_, t)| t.group == group) {
                                    let dot = match tab.state {
                                        TabState::Connecting(_) => theme::p().yellow,
                                        TabState::Connected => theme::p().green,
                                        TabState::Closed(_) => theme::p().text_dim,
                                    };
                                    let icon = match tab.target {
                                        Target::Ssh(_) => &theme::ICON_SERVER,
                                        Target::Local(_) => &theme::ICON_TERMINAL,
                                    };
                                    // Onglet en cours de renommage : il s'élargit avec le nom saisi.
                                    let edit = editing.as_mut().filter(|e| e.id == tab.id);
                                    let title = match &edit {
                                        Some(edit) => format!("{}  ", edit.name),
                                        None => tab.title().to_string(),
                                    };
                                    let response = theme::tab(ui, index == active_index, is_active_group, dot, Some(icon), &title, tab_max_width);
                                    if let Some(edit) = edit {
                                        // Champ de saisie posé sur le titre, entre l'icône et la croix.
                                        let r = response.response.rect;
                                        let field = egui::Rect::from_min_max(
                                            egui::pos2(r.left() + 30.0, r.center().y - 11.0),
                                            egui::pos2(r.right() - 30.0, r.center().y + 11.0),
                                        );
                                        let output = ui
                                            .scope_builder(egui::UiBuilder::new().max_rect(field), |ui| {
                                                egui::TextEdit::singleline(&mut edit.name)
                                                    .font(egui::FontId::proportional(13.0))
                                                    .margin(egui::Margin::symmetric(4, 2))
                                                    .desired_width(field.width())
                                                    .show(ui)
                                            })
                                            .inner;
                                        if !edit.started {
                                            // Première image : focus et texte sélectionné.
                                            edit.started = true;
                                            let mut state = output.state;
                                            let end = egui::text::CCursor::new(edit.name.chars().count());
                                            state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), end)));
                                            state.store(ui.ctx(), output.response.id);
                                            output.response.request_focus();
                                        } else if ui.input(|i| i.key_pressed(Key::Escape)) {
                                            rename_end = Some(false);
                                        } else if output.response.lost_focus() || !output.response.has_focus() {
                                            // Entrée, ou clic ailleurs : le nom est validé.
                                            rename_end = Some(true);
                                        }
                                        if rename_end.is_some() {
                                            // Le terminal reprend le clavier dans cette même image :
                                            // Entrée et Échap ne doivent pas lui parvenir.
                                            ui.input_mut(|i| {
                                                i.events.retain(|e| {
                                                    !matches!(e, egui::Event::Key { key: Key::Enter | Key::Escape, .. })
                                                })
                                            });
                                        }
                                    } else if response.response.double_clicked() && !response.close {
                                        rename = Some(index);
                                    }
                                    if index == active_index {
                                        active_rect = Some(response.response.rect);
                                        if self.scrolled_to[group] != Some(tab.id) {
                                            response.response.scroll_to_me(None);
                                            scrolled = Some(tab.id);
                                        }
                                    }
                                    if response.clicked {
                                        activate = Some(index);
                                    }
                                    if response.close {
                                        close = Some(index);
                                    }
                                    // Glisser un onglet : le déposer sur un autre le place
                                    // avant ou après lui, selon la moitié survolée.
                                    let rect = response.response.rect;
                                    response.response.dnd_set_drag_payload(DragTab { id: tab.id });
                                    if response.response.dragged() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                                    }
                                    let after = pointer.is_some_and(|pos| pos.x > rect.center().x);
                                    if response.response.dnd_hover_payload::<DragTab>().is_some_and(|p| p.id != tab.id) {
                                        let x = if after { rect.right() - 1.0 } else { rect.left() + 1.0 };
                                        ui.painter().vline(x, rect.y_range(), egui::Stroke::new(2.0, theme::p().accent));
                                    }
                                    if let Some(payload) = response.response.dnd_release_payload::<DragTab>() {
                                        drop = Some((payload.id, group, Some((tab.id, after))));
                                    }
                                    let group_size = self.tabs.iter().filter(|t| t.group == group).count();
                                    response.response.context_menu(|ui| {
                                        if group > 0 && ui.button(to_previous).clicked() {
                                            drop = Some((tab.id, group - 1, None));
                                            ui.close();
                                        }
                                        if group + 1 < self.groups {
                                            if ui.button(to_next).clicked() {
                                                drop = Some((tab.id, group + 1, None));
                                                ui.close();
                                            }
                                        } else if group_size > 1
                                            && ui.button(to_new).clicked()
                                        {
                                            drop = Some((tab.id, group + 1, None));
                                            ui.close();
                                        }
                                        ui.separator();
                                        if ui.button(theme::label(&theme::ICON_EDIT, "Renommer…")).clicked() {
                                            rename = Some(index);
                                            ui.close();
                                        }
                                        if ui.button(theme::label(&theme::ICON_CLOSE, "Fermer")).clicked() {
                                            close = Some(index);
                                            ui.close();
                                        }
                                    });
                                }
                            });
                        });
                    if corner {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            if theme::split_button(ui, "Scinder en bas", true).clicked() {
                                split_request = Some(true);
                            }
                            if theme::split_button(ui, "Scinder à droite", false).clicked() {
                                split_request = Some(false);
                            }
                        });
                    }
                });
            });
        // Bordure sous la barre d'onglets, interrompue sous l'onglet actif
        // pour qu'il se raccorde au terminal (comme VS Code).
        let strip = bar.response.rect;
        let y = strip.bottom() - 0.5;
        ui.painter().hline(strip.x_range(), y, egui::Stroke::new(1.0, theme::p().border));
        if let Some(rect) = active_rect {
            ui.painter().hline(rect.x_range(), y, egui::Stroke::new(1.0, theme::p().bg));
        }
        // Dépôt dans la partie libre de la barre : l'onglet va en fin de groupe.
        if drop.is_none() {
            if let Some(payload) = egui::DragAndDrop::payload::<DragTab>(ui.ctx()) {
                let over = pointer.is_some_and(|pos| strip.contains(pos));
                if over {
                    ui.painter().rect_filled(strip, 0.0, theme::p().accent.gamma_multiply(0.08));
                    if ui.input(|i| i.pointer.any_released()) {
                        drop = Some((payload.id, group, None));
                    }
                }
            }
        }
        if let Some(id) = scrolled {
            self.scrolled_to[group] = Some(id);
        }
        match (rename_end, editing) {
            (Some(commit), Some(edit)) => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == edit.id) {
                    if commit {
                        // Nom vide (ou inchangé) : retour au nom de la session ou du shell.
                        let name = edit.name.trim();
                        tab.title = (!name.is_empty() && name != tab.target.title()).then(|| name.to_string());
                    }
                    // Le clavier revient au terminal.
                    tab.view.wants_focus = true;
                }
            }
            (_, edit) => self.tab_rename = edit,
        }
        if let Some(tab) = rename.and_then(|index| self.tabs.get(index)) {
            self.tab_rename = Some(TabRename { id: tab.id, name: tab.title().to_string(), started: false });
        }
        if let Some(index) = activate {
            self.active = index;
            self.active_group = group;
            self.group_active[group] = Some(self.tabs[index].id);
            active_index = index;
        }
        if let Some(down) = split_request {
            // Les boutons sont dans le coin, mais la scission vise le groupe actif.
            self.split(down);
            return;
        }
        if let Some((id, target, anchor)) = drop {
            egui::DragAndDrop::clear_payload(ui.ctx());
            self.move_tab(id, target, anchor);
            return;
        }
        if let Some(index) = close {
            self.tabs.remove(index);
            self.normalize_groups();
            return;
        }

        let mut reconnect = false;
        let mut close_active = false;
        let highlighter = self.highlighter.clone();
        let tab = &mut self.tabs[active_index];

        if self.monitor_visible && matches!(tab.target, Target::Ssh(_)) {
            let stats = tab.monitor.clone();
            egui::Panel::bottom(("monitor", group))
                .exact_size(30.0)
                .frame(egui::Frame::NONE.fill(theme::p().sidebar).inner_margin(egui::Margin::symmetric(10, 0)))
                .show(ui, |ui| monitor_bar(ui, stats.as_ref()));
        }

        let terminal = egui::Frame::NONE.show(ui, |ui| {
            ui.push_id(tab.id, |ui| tab.view.show(ui, &tab.conn.parser, highlighter.as_deref())).inner
        });
        let output = terminal.inner;
        // État de la connexion : pastille flottante, centrée en bas du terminal.
        let area = terminal.response.rect;
        let pill = Id::unique(("status", tab.id));
        match &tab.state {
            TabState::Connecting(message) => {
                theme::status_pill(ui, pill, area, |ui| {
                    ui.spinner();
                    ui.label(message);
                });
            }
            TabState::Closed(message) => {
                theme::status_pill(ui, pill, area, |ui| {
                    theme::status_dot(ui, theme::p().red);
                    ui.label(message);
                    ui.add_space(6.0);
                    if theme::primary_button(ui, &theme::label(&theme::ICON_REFRESH, "Reconnecter")).clicked() {
                        reconnect = true;
                    }
                    if theme::header_button(ui, &theme::ICON_CLOSE, "Fermer l'onglet").clicked() {
                        close_active = true;
                    }
                });
            }
            TabState::Connected => {}
        }
        if matches!(tab.state, TabState::Connected) {
            if !output.input.is_empty() {
                tab.conn.send(Command::Input(output.input));
            }
            if let Some((cols, rows)) = output.resize {
                tab.conn.send(Command::Resize { cols, rows });
            }
        } else if let Some((cols, rows)) = output.resize {
            // Taille mémorisée par le terminal ; elle sera envoyée dès l'ouverture.
            tab.conn.send(Command::Resize { cols, rows });
        }
        if output.focused && (self.active != active_index || self.active_group != group) {
            self.active = active_index;
            self.active_group = group;
        }

        let ctx = ui.ctx().clone();
        if reconnect {
            self.reconnect(active_index, &ctx);
        } else if close_active {
            self.tabs.remove(active_index);
            self.normalize_groups();
        }
    }

    // ------------------------------------------------------------------
    // Fenêtres de dialogue
    // ------------------------------------------------------------------

    fn dialogs(&mut self, ctx: &egui::Context) {
        self.prompt_dialog(ctx);
        self.editor_dialog(ctx);
        self.group_editor_dialog(ctx);
        self.macro_editor_dialog(ctx);
        self.edit_upload_dialog(ctx);
        self.highlight_editor_dialog(ctx);

        if let Some((title, body)) = &self.notice {
            let mut open = true;
            let mut ok = false;
            egui::Window::new(title.as_str())
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                        ui.label(body.as_str());
                    });
                    ui.add_space(8.0);
                    ok = theme::primary_button(ui, "OK").clicked();
                });
            if !open || ok {
                self.notice = None;
            }
        }

        if let Some(confirm) = &self.confirm {
            let text = match confirm {
                Confirm::DeleteSession(id) => format!(
                    "Supprimer la session « {} » ?",
                    self.store.get(*id).map(|s| s.name.as_str()).unwrap_or("?")
                ),
                Confirm::DeleteGroup(path) => {
                    let inside = |folder: &str| {
                        folder.strip_prefix(path.as_str()).is_some_and(|r| r.is_empty() || r.starts_with('/'))
                    };
                    if self.store.sessions.iter().any(|s| inside(&s.folder)) {
                        format!(
                            "Supprimer le groupe « {path} » ?\nSes sessions ne sont pas supprimées : elles remontent dans le groupe parent."
                        )
                    } else {
                        format!("Supprimer le groupe « {path} » ?")
                    }
                }
                Confirm::DeleteMacro(id) => format!(
                    "Supprimer la macro « {} » ?",
                    self.store.get_macro(*id).map(|m| m.name.as_str()).unwrap_or("?")
                ),
                Confirm::DeleteRemote { path, is_dir } => {
                    if *is_dir {
                        format!("Supprimer le dossier distant {path} ?\n(il doit être vide)")
                    } else {
                        format!("Supprimer le fichier distant {path} ?")
                    }
                }
            };
            let mut decision = None;
            theme::modal(ctx, Id::unique("confirm")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(text);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Supprimer").color(theme::p().red)).clicked() {
                        decision = Some(true);
                    }
                    if ui.button("Annuler").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        decision = Some(false);
                    }
                });
            });
            if let Some(yes) = decision {
                if let Some(confirm) = self.confirm.take() {
                    if yes {
                        match confirm {
                            Confirm::DeleteSession(id) => {
                                // Les mots de passe enregistrés de ses profils n'ont plus d'usage.
                                if let Some(session) = self.store.get(id) {
                                    for profile in session.profiles() {
                                        crate::vault::delete(&crate::vault::Secret::Password(id, profile.id));
                                    }
                                }
                                self.store.remove(id);
                                self.save_store();
                            }
                            Confirm::DeleteGroup(path) => {
                                self.store.remove_folder(&path);
                                self.save_store();
                            }
                            Confirm::DeleteMacro(id) => {
                                self.store.remove_macro(id);
                                self.save_store();
                            }
                            Confirm::DeleteRemote { path, is_dir } => {
                                if let Some(tab) = self.tabs.get_mut(self.active) {
                                    tab.conn.send(Command::Sftp(SftpCommand::Remove { path, is_dir }));
                                    tab.sftp.loading = true;
                                    tab.conn.send(Command::Sftp(SftpCommand::List(tab.sftp.cwd.clone())));
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(name) = &mut self.mkdir_name {
            let mut decision = None;
            theme::modal(ctx, Id::unique("mkdir")).show(ctx, |ui| {
                ui.label("Nom du nouveau dossier :");
                let response = ui.text_edit_singleline(name);
                response.request_focus();
                ui.horizontal(|ui| {
                    if theme::primary_button(ui, "Créer").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                        decision = Some(true);
                    }
                    if ui.button("Annuler").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        decision = Some(false);
                    }
                });
            });
            if let Some(create) = decision {
                let name = self.mkdir_name.take().unwrap_or_default();
                let name = name.trim();
                if create && !name.is_empty() && !name.contains('/') {
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        let path = join_remote(&tab.sftp.cwd, name);
                        tab.conn.send(Command::Sftp(SftpCommand::Mkdir(path)));
                        tab.sftp.loading = true;
                        tab.conn.send(Command::Sftp(SftpCommand::List(tab.sftp.cwd.clone())));
                    }
                }
            }
        }
    }

    fn prompt_dialog(&mut self, ctx: &egui::Context) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let Some(prompt) = &mut tab.prompt else { return };
        let title = tab.target.subtitle();
        let mut reply: Option<Reply> = None;

        theme::modal(ctx, Id::unique(("prompt", tab.id))).show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.heading(title);
            ui.add_space(6.0);
            match prompt {
                Prompt::HostKey { algorithm, fingerprint } => {
                    ui.label("Ce serveur n'est pas encore connu (absent de ~/.ssh/known_hosts).");
                    ui.label("Vérifiez son empreinte avant de continuer :");
                    ui.add_space(4.0);
                    ui.monospace(format!("{algorithm}\n{fingerprint}"));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if theme::primary_button(ui, "Faire confiance et se connecter").clicked() {
                            reply = Some(Reply::HostKey(true));
                        }
                        if ui.button("Annuler").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                            reply = Some(Reply::HostKey(false));
                        }
                    });
                }
                Prompt::Text { prompt, secret, value, remember } => {
                    ui.label(prompt.as_str());
                    let response = ui.add(
                        egui::TextEdit::singleline(value)
                            .password(*secret)
                            .desired_width(f32::INFINITY),
                    );
                    // Le focus revient toujours au champ (même après un clic sur la case).
                    response.request_focus();
                    if let Some(remember) = remember {
                        ui.add_space(4.0);
                        ui.checkbox(remember, "Enregistrer dans le coffre Windows")
                            .on_hover_text("Chiffré par Windows et lié à votre compte : plus de saisie aux prochaines connexions.");
                    }
                    ui.add_space(6.0);
                    let remember = remember.unwrap_or(false);
                    ui.horizontal(|ui| {
                        if theme::primary_button(ui, "OK").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                            reply = Some(Reply::Text { text: Some(std::mem::take(value)), remember });
                        }
                        if ui.button("Annuler").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                            reply = Some(Reply::Text { text: None, remember: false });
                        }
                    });
                }
            }
        });

        if let Some(reply) = reply {
            tab.conn.reply(reply);
            tab.prompt = None;
            tab.view.wants_focus = true;
        }
    }

    /// Réglages de la coloration des mots-clés : listes, couleurs, aperçu.
    fn highlight_editor_dialog(&mut self, ctx: &egui::Context) {
        use crate::highlight::{HighlightColor, HighlightRule, Highlighter, Highlighting};
        let Some(editor) = &mut self.highlight_editor else { return };
        let mut save = false;
        let mut cancel = false;
        theme::modal(ctx, Id::unique("highlight_editor")).show(ctx, |ui| {
            let pal = theme::p();
            ui.set_width(680.0);
            ui.heading("Coloration des mots-clés");
            ui.label(
                RichText::new(
                    "Dans les terminaux, ces mots sont colorés (mots entiers, sans tenir compte des majuscules). \
                     Le texte déjà coloré par un programme n'est pas modifié. En cas de chevauchement, \
                     la liste la plus à gauche l'emporte.",
                )
                .small()
                .color(pal.text_dim),
            );
            ui.add_space(6.0);
            ui.checkbox(&mut editor.enabled, "Colorer les mots-clés");
            ui.add_space(6.0);

            let mut remove = None;
            ui.add_enabled_ui(editor.enabled, |ui| {
                egui::ScrollArea::horizontal().id_salt("highlight_rules").show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        for (index, (rule, text)) in editor.rules.iter_mut().enumerate() {
                            ui.vertical(|ui| {
                                ui.set_width(205.0);
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut rule.enabled, "");
                                    ui.add(egui::TextEdit::singleline(&mut rule.name).desired_width(170.0));
                                });
                                ui.horizontal(|ui| {
                                    let (swatch, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                                    ui.painter().rect_filled(swatch, 3.0, rule.color.color());
                                    egui::ComboBox::from_id_salt(("highlight_color", index))
                                        .selected_text(rule.color.label())
                                        .width(110.0)
                                        .show_ui(ui, |ui| {
                                            for color in HighlightColor::ALL {
                                                ui.selectable_value(&mut rule.color, color, color.label());
                                            }
                                        });
                                    if theme::icon_button(ui, &theme::ICON_DELETE, "Supprimer cette liste").clicked() {
                                        remove = Some(index);
                                    }
                                });
                                // Hauteur bornée : une longue liste défile au lieu
                                // d'agrandir la fenêtre.
                                egui::ScrollArea::vertical()
                                    .id_salt(("highlight_words", index))
                                    .max_height(260.0)
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::TextEdit::multiline(text)
                                                .hint_text("un mot ou une expression par ligne")
                                                .desired_rows(12)
                                                .desired_width(190.0),
                                        );
                                    });
                            });
                        }
                    });
                });
            });
            if let Some(index) = remove {
                editor.rules.remove(index);
            }
            ui.horizontal(|ui| {
                if ui.button(theme::label(&theme::ICON_ADD, "Ajouter une liste")).clicked() {
                    let rule = HighlightRule {
                        name: format!("Liste {}", editor.rules.len() + 1),
                        color: HighlightColor::Cyan,
                        words: Vec::new(),
                        enabled: true,
                    };
                    editor.rules.push((rule, String::new()));
                }
                if ui.button("Rétablir les listes par défaut").clicked() {
                    *editor = HighlightEditor::new(&Highlighting::default());
                }
            });

            // Aperçu, avec les réglages en cours.
            ui.add_space(8.0);
            ui.label(theme::section("Aperçu"));
            let sample = "nginx.service: Started — OK\n\
                          warning: disk usage at 92 %, could not rotate logs\n\
                          Error: Permission denied (publickey)\n\
                          Connexion établie, synchronisation réussie, aucun échec";
            let font = egui::FontId::monospace(13.0);
            let mut job = egui::text::LayoutJob::default();
            let plain = |job: &mut egui::text::LayoutJob, text: &str| {
                job.append(text, 0.0, egui::TextFormat::simple(font.clone(), pal.terminal_fg));
            };
            match Highlighter::new(&editor.settings()) {
                Some(highlighter) => {
                    let mut last = 0;
                    for (range, color) in highlighter.find(sample) {
                        plain(&mut job, &sample[last..range.start]);
                        job.append(&sample[range.clone()], 0.0, egui::TextFormat::simple(font.clone(), color.color()));
                        last = range.end;
                    }
                    plain(&mut job, &sample[last..]);
                }
                None => plain(&mut job, sample),
            }
            egui::Frame::NONE
                .fill(pal.terminal_bg)
                .stroke(egui::Stroke::new(1.0, pal.border))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(job);
                });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                save = theme::primary_button(ui, "Enregistrer").clicked();
                cancel = ui.button("Annuler").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
            });
        });
        if save {
            self.store.highlighting = editor.settings();
            self.highlighter = Highlighter::new(&self.store.highlighting).map(std::sync::Arc::new);
            self.highlight_editor = None;
            self.save_store();
        } else if cancel {
            self.highlight_editor = None;
        }
    }

    fn group_editor_dialog(&mut self, ctx: &egui::Context) {
        let Some(editor) = &mut self.group_editor else { return };
        let mut save = false;
        let mut cancel = false;

        theme::modal(ctx, Id::unique("group_editor")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.heading("Nouveau groupe");
            if !editor.parent.is_empty() {
                ui.label(RichText::new(format!("Dans « {} »", editor.parent)).small().color(theme::p().text_dim));
            }
            ui.add_space(6.0);
            ui.label("Nom");
            ui.add(
                egui::TextEdit::singleline(&mut editor.name)
                    .hint_text("ex. Clients ou Clients/Prod")
                    .desired_width(f32::INFINITY),
            )
            .request_focus();
            if let Some(error) = &editor.error {
                ui.colored_label(theme::p().red, error);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                save = theme::primary_button(ui, "Créer").clicked() || ui.input(|i| i.key_pressed(Key::Enter));
                cancel = ui.button("Annuler").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
            });
        });

        if cancel {
            self.group_editor = None;
            return;
        }
        if save {
            // Même normalisation que le champ « Dossier » d'une session.
            let path = editor
                .parent
                .split('/')
                .chain(editor.name.split(['/', '\\']))
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("/");
            if editor.name.trim().is_empty() || path == editor.parent {
                editor.error = Some("Donnez un nom au groupe.".into());
            } else if !self.store.add_folder(&path) {
                editor.error = Some("Ce groupe existe déjà.".into());
            } else {
                self.group_editor = None;
                self.save_store();
            }
        }
    }

    fn macro_editor_dialog(&mut self, ctx: &egui::Context) {
        let Some(editor) = &mut self.macro_editor else { return };
        let mut save = false;
        let mut cancel = false;

        theme::modal(ctx, Id::unique("macro_editor")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading(if editor.item.id == 0 { "Nouvelle macro" } else { "Modifier la macro" });
            ui.add_space(6.0);
            ui.label("Nom");
            ui.add(egui::TextEdit::singleline(&mut editor.item.name).desired_width(f32::INFINITY));
            ui.add_space(6.0);
            ui.label("Commandes");
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut editor.item.commands)
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(8)
                        .desired_width(f32::INFINITY)
                        .hint_text("cd /var/log\ntail -n 50 syslog"),
                );
            });
            ui.label(
                RichText::new("Une commande par ligne ; chaque ligne est suivie d'Entrée dans le terminal actif.")
                    .small()
                    .color(theme::p().text_dim),
            );
            if let Some(error) = &editor.error {
                ui.colored_label(theme::p().red, error);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                save = theme::primary_button(ui, "Enregistrer").clicked();
                cancel = ui.button("Annuler").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
            });
        });

        if cancel {
            self.macro_editor = None;
            return;
        }
        if save {
            let mut item = editor.item.clone();
            item.name = item.name.trim().to_string();
            if item.name.is_empty() {
                editor.error = Some("Donnez un nom à la macro.".into());
            } else if item.lines().next().is_none() {
                editor.error = Some("Saisissez au moins une commande.".into());
            } else {
                self.macro_editor = None;
                self.selected_macro = Some(self.store.save_macro(item));
                self.save_store();
            }
        }
    }

    fn editor_dialog(&mut self, ctx: &egui::Context) {
        let Some(editor) = &mut self.editor else { return };
        let mut save = false;
        let mut cancel = false;
        let mut add = false;
        let mut activate = false;
        let mut remove = None;
        let mut select = None;

        theme::modal(ctx, Id::unique("editor")).show(ctx, |ui| {
            ui.set_width(660.0);
            ui.heading(if editor.is_new { "Nouvelle session SSH" } else { "Modifier la session" });
            ui.add_space(6.0);
            editor.selected = editor.selected.min(editor.profiles.len().saturating_sub(1));
            let active = editor.active;
            let multiple = editor.profiles.len() > 1;
            let mut divider_x = 0.0;
            let columns = ui.horizontal_top(|ui| {
                // Profils de connexion, à gauche (comme dans Navicat).
                let left = ui.vertical(|ui| {
                    ui.set_width(150.0);
                    ui.set_min_height(280.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Profils").color(theme::p().text_dim));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            add = ui
                                .small_button(theme::ICON_ADD.get())
                                .on_hover_text("Nouveau profil, copie du profil affiché")
                                .clicked();
                        });
                    });
                    ui.add_space(4.0);
                    ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                        for (index, form) in editor.profiles.iter().enumerate() {
                            let mut job = egui::text::LayoutJob::default();
                            let name = form.profile.name.trim();
                            job.append(
                                if name.is_empty() { "(sans nom)" } else { name },
                                0.0,
                                egui::TextFormat::simple(egui::FontId::proportional(13.0), theme::p().text),
                            );
                            if form.profile.id == active {
                                job.append(
                                    "(actif)",
                                    6.0,
                                    egui::TextFormat::simple(egui::FontId::proportional(12.0), theme::p().text_dim),
                                );
                            }
                            let response = ui
                                .selectable_label(index == editor.selected, job)
                                .on_hover_text(form.profile.target());
                            if response.clicked() {
                                select = Some(index);
                            }
                            if response.double_clicked() {
                                select = Some(index);
                                activate = true;
                            }
                            response.context_menu(|ui| {
                                if ui.add_enabled(form.profile.id != active, egui::Button::new("Utiliser ce profil")).clicked() {
                                    select = Some(index);
                                    activate = true;
                                    ui.close();
                                }
                                if ui
                                    .add_enabled(multiple, egui::Button::new(RichText::new("Supprimer").color(theme::p().red)))
                                    .clicked()
                                {
                                    remove = Some(index);
                                    ui.close();
                                }
                            });
                        }
                    });
                })
                .response
                .rect;
                // Trait vertical dessiné après coup, à la hauteur du contenu :
                // `ui.separator()` prendrait toute la hauteur disponible, et la
                // fenêtre modale grandirait à chaque image.
                divider_x = left.right() + 10.0;
                ui.add_space(20.0);

                // Session (commune à tous les profils), puis profil affiché.
                ui.vertical(|ui| {
                    egui::Grid::new("editor_session").num_columns(2).spacing([10.0, 8.0]).min_col_width(110.0).show(ui, |ui| {
                        ui.label("Nom");
                        ui.add(
                            egui::TextEdit::singleline(&mut editor.session.name)
                                .hint_text("par défaut : l'hôte")
                                .desired_width(300.0),
                        );
                        ui.end_row();
                        ui.label("Dossier");
                        ui.add(
                            egui::TextEdit::singleline(&mut editor.session.folder)
                                .hint_text("ex. Clients/Prod")
                                .desired_width(300.0),
                        );
                        ui.end_row();
                    });
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);

                    let focus_name = std::mem::take(&mut editor.focus_profile_name);
                    let selected = editor.selected;
                    let Some(form) = editor.profiles.get_mut(selected) else { return };
                    let passphrase_saved = form.use_key && form.passphrase_saved();
                    egui::Grid::new("editor_profile").num_columns(2).spacing([10.0, 8.0]).min_col_width(110.0).show(ui, |ui| {
                        ui.label("Profil");
                        ui.horizontal(|ui| {
                            let mut output = egui::TextEdit::singleline(&mut form.profile.name)
                                .hint_text("ex. Prod")
                                .desired_width(160.0)
                                .show(ui);
                            if focus_name {
                                // Profil tout juste créé : son nom est sélectionné, prêt à être remplacé.
                                let end = egui::text::CCursor::new(form.profile.name.chars().count());
                                output
                                    .state
                                    .cursor
                                    .set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), end)));
                                output.state.store(ui.ctx(), output.response.id);
                                output.response.request_focus();
                            }
                            if form.profile.id == active {
                                ui.label(RichText::new("actif").color(theme::p().green))
                                    .on_hover_text("Profil utilisé pour se connecter à cette session.");
                            } else if ui
                                .small_button("Utiliser ce profil")
                                .on_hover_text("Les prochaines connexions à cette session utiliseront ce profil.")
                                .clicked()
                            {
                                activate = true;
                            }
                            if multiple && ui.small_button(RichText::new("Supprimer").color(theme::p().red)).clicked() {
                                remove = Some(selected);
                            }
                        });
                        ui.end_row();
                        ui.label("Hôte");
                        ui.add(
                            egui::TextEdit::singleline(&mut form.profile.host)
                                .hint_text("serveur.exemple.fr ou 10.0.0.5")
                                .desired_width(300.0),
                        );
                        ui.end_row();
                        ui.label("Port");
                        ui.add(egui::TextEdit::singleline(&mut form.port).desired_width(60.0));
                        ui.end_row();
                        ui.label("Utilisateur");
                        ui.add(
                            egui::TextEdit::singleline(&mut form.profile.username)
                                .hint_text("demandé à la connexion si vide")
                                .desired_width(300.0),
                        );
                        ui.end_row();
                        ui.label("Authentification");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut form.use_key, false, "Mot de passe");
                            ui.radio_value(&mut form.use_key, true, "Clé privée");
                        });
                        ui.end_row();
                        if form.use_key {
                            ui.label("Clé");
                            ui.horizontal(|ui| {
                                let key = form.profile.key_path.get_or_insert_with(String::new);
                                ui.add(egui::TextEdit::singleline(key).desired_width(220.0));
                                if ui.button("Parcourir…").clicked() {
                                    let mut dialog = rfd::FileDialog::new().set_title("Choisir la clé privée");
                                    if let Some(home) = std::env::var_os("USERPROFILE") {
                                        dialog = dialog.set_directory(PathBuf::from(home).join(".ssh"));
                                    }
                                    if let Some(path) = dialog.pick_file() {
                                        *key = path.to_string_lossy().into_owned();
                                    }
                                }
                            });
                            ui.end_row();
                            ui.label("Phrase de passe");
                            ui.horizontal(|ui| {
                                let dim = theme::p().text_dim;
                                if passphrase_saved && !form.forget_passphrase {
                                    ui.label("Enregistrée dans le coffre Windows");
                                    if ui.small_button("Oublier").clicked() {
                                        form.forget_passphrase = true;
                                    }
                                } else if passphrase_saved {
                                    ui.label(RichText::new("Sera oubliée à l'enregistrement").color(dim));
                                    if ui.small_button("Garder").clicked() {
                                        form.forget_passphrase = false;
                                    }
                                } else {
                                    ui.label(RichText::new("Demandée à la connexion si la clé est chiffrée").color(dim));
                                }
                            });
                            ui.end_row();
                        } else {
                            ui.label("Mot de passe");
                            ui.horizontal(|ui| {
                                let kept = form.saved_password && !form.forget_password;
                                let hint = if kept { "enregistré (vide = inchangé)" } else { "demandé à la connexion si vide" };
                                ui.add(
                                    egui::TextEdit::singleline(&mut form.password)
                                        .password(true)
                                        .hint_text(hint)
                                        .desired_width(220.0),
                                )
                                .on_hover_text("Enregistré dans le coffre Windows (chiffré, lié à votre compte).");
                                if kept && ui.small_button("Oublier").clicked() {
                                    form.forget_password = true;
                                    form.password.clear();
                                } else if form.saved_password && form.forget_password {
                                    ui.label(RichText::new("sera oublié").color(theme::p().text_dim));
                                    if ui.small_button("Garder").clicked() {
                                        form.forget_password = false;
                                    }
                                }
                            });
                            ui.end_row();
                        }
                    });
                });
            });
            let rows = columns.response.rect;
            ui.painter().vline(
                divider_x,
                rows.y_range(),
                egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
            );
            if let Some(error) = &editor.error {
                ui.colored_label(theme::p().red, error);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                save = theme::primary_button(ui, "Enregistrer").clicked();
                cancel = ui.button("Annuler").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
            });
        });

        if cancel {
            self.editor = None;
            return;
        }
        if let Some(index) = select {
            editor.selected = index;
        }
        if activate {
            if let Some(form) = editor.profiles.get(editor.selected) {
                editor.active = form.profile.id;
            }
        }
        if let Some(index) = remove {
            editor.remove_profile(index);
        }
        if add {
            editor.add_profile();
        }
        if !save {
            return;
        }
        match editor.validate() {
            Ok(mut session) => {
                let is_new = editor.is_new;
                // Secrets saisis, par profil : (id, mot de passe, oublier le mot de passe, oublier la phrase de passe).
                let secrets: Vec<_> = editor
                    .profiles
                    .iter_mut()
                    .map(|f| (f.profile.id, std::mem::take(&mut f.password), f.forget_password, f.forget_passphrase))
                    .collect();
                let removed = std::mem::take(&mut editor.removed);
                self.editor = None;
                if is_new {
                    let id = self.store.insert(session.clone());
                    session.id = id;
                    self.selected_session = Some(id);
                } else {
                    self.store.update(session.clone());
                    // Les onglets ouverts reprennent les nouveaux réglages de leur
                    // profil à la reconnexion (le profil actif si le leur a disparu).
                    for tab in self.tabs.iter_mut().filter(|t| t.target.session_id() == Some(session.id)) {
                        let mut updated = session.clone();
                        if let Target::Ssh(old) = &tab.target {
                            updated.switch_profile(old.profile);
                        }
                        tab.target = Target::Ssh(updated);
                    }
                }
                self.save_store();
                // Secrets dans le coffre Windows.
                use crate::vault::{self, Secret};
                for profile in removed {
                    vault::delete(&Secret::Password(session.id, profile));
                }
                let profiles = session.profiles();
                for (id, password, forget_password, forget_passphrase) in secrets {
                    let Some(profile) = profiles.iter().find(|p| p.id == id) else { continue };
                    if let Some(path) = &profile.key_path {
                        if forget_passphrase {
                            vault::delete(&Secret::Passphrase(path.clone()));
                        }
                        continue;
                    }
                    let secret = Secret::Password(session.id, id);
                    if forget_password {
                        vault::delete(&secret);
                    }
                    if !password.is_empty() {
                        if let Err(e) = vault::set(&secret, &profile.username, &password) {
                            self.notice = Some(("Mot de passe non enregistré".into(), format!("{e:#}")));
                        }
                    }
                }
            }
            Err(e) => editor.error = Some(e),
        }
    }
}

impl Drop for FerroApp {
    /// À la fermeture : les copies locales des fichiers édités sont supprimées,
    /// sauf celles qui ont des modifications non envoyées.
    fn drop(&mut self) {
        for file in &self.edits {
            stop_editing(file);
        }
    }
}

impl eframe::App for FerroApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_connections();
        self.poll_edits(&ctx);
        let mut actions = Vec::new();

        // Fond du plan de travail : il apparaît entre les zones arrondies.
        ui.painter().rect_filled(ui.max_rect(), 0.0, theme::p().title_bar);

        egui::Panel::top("title_bar")
            .exact_size(theme::TITLE_BAR_HEIGHT)
            .frame(egui::Frame::NONE.fill(theme::p().title_bar))
            .show_separator_line(false)
            .show(ui, |ui| self.title_bar(ui, &mut actions));

        egui::Panel::bottom("status")
            .exact_size(22.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::p().title_bar)
                    .inner_margin(egui::Margin::symmetric(12, 0)),
            )
            .show_separator_line(false)
            .show(ui, |ui| self.status_bar(ui));

        self.activity_bar(ui, &mut actions);
        if self.sessions_visible {
            self.sessions_sidebar(ui, &mut actions);
        }
        if self.files_visible {
            self.files_sidebar(ui);
        }

        // Zone centrale : l'espace avec une barre latérale est porté par celle-ci.
        let gap = theme::CARD_GAP;
        let outer = egui::Margin {
            left: 0,
            right: if self.files_visible { 0 } else { gap },
            top: 0,
            bottom: gap,
        };
        egui::CentralPanel::default()
            .frame(theme::card(theme::p().bg, egui::Margin::ZERO, outer))
            .show(ui, |ui| {
                let rect = theme::card_rect(ui, egui::Margin::ZERO);
                self.central(ui, &mut actions);
                theme::card_corners(ui, rect);
            });

        self.handle_dropped_files(&ctx);
        self.dialogs(&ctx);

        for action in actions {
            self.apply(action, &ctx);
        }
        self.sync_workspace(&ctx);
    }
}

/// Historique court des mesures, pour les courbes du panneau de supervision.
#[derive(Default)]
struct MonitorHistory {
    cpu: std::collections::VecDeque<f32>,
    mem: std::collections::VecDeque<f32>,
}

impl MonitorHistory {
    const CAPACITY: usize = 90;

    fn push(&mut self, stats: &crate::monitor::Stats) {
        if let Some(cpu) = stats.cpu_percent {
            Self::push_capped(&mut self.cpu, cpu);
        }
        if stats.mem_total > 0 {
            Self::push_capped(&mut self.mem, stats.mem_used as f32 / stats.mem_total as f32 * 100.0);
        }
    }

    fn push_capped(queue: &mut std::collections::VecDeque<f32>, value: f32) {
        if queue.len() >= Self::CAPACITY {
            queue.pop_front();
        }
        queue.push_back(value);
    }
}

/// Courbe simple (0–`max`) sur toute la largeur, avec aplat optionnel sous la courbe.
fn sparkline(ui: &mut egui::Ui, values: &std::collections::VecDeque<f32>, max: f32, color: Color32, fill: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::hover());
    let painter = ui.painter();
    if values.len() < 2 {
        painter.hline(rect.x_range(), rect.bottom() - 1.0, egui::Stroke::new(1.0, theme::p().border));
        return;
    }
    let n = values.len();
    let step = rect.width() / (MonitorHistory::CAPACITY - 1) as f32;
    let x0 = rect.right() - step * (n - 1) as f32;
    let points: Vec<egui::Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let y = rect.bottom() - 1.0 - (v / max).clamp(0.0, 1.0) * (rect.height() - 2.0);
            egui::pos2(x0 + step * i as f32, y)
        })
        .collect();
    if fill {
        let mut mesh = egui::Mesh::default();
        let fill_color = color.gamma_multiply(0.25);
        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let base = rect.bottom();
            let i = mesh.vertices.len() as u32;
            mesh.colored_vertex(a, fill_color);
            mesh.colored_vertex(b, fill_color);
            mesh.colored_vertex(egui::pos2(b.x, base), fill_color);
            mesh.colored_vertex(egui::pos2(a.x, base), fill_color);
            mesh.add_triangle(i, i + 1, i + 2);
            mesh.add_triangle(i, i + 2, i + 3);
        }
        painter.add(egui::Shape::mesh(mesh));
    }
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, color)));
}

/// Ligne « libellé … valeur » d'une métrique, la valeur en grand à droite.
fn metric_row(ui: &mut egui::Ui, label: &str, value: &str, value_color: Color32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(theme::p().text_dim));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(value).size(20.0).family(theme::semibold()).color(value_color));
        });
    });
}

/// Jauge horizontale fine.
fn gauge(ui: &mut egui::Ui, fraction: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 5.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.5, theme::p().surface_hover);
    let filled = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * fraction.clamp(0.0, 1.0), rect.height()));
    ui.painter().rect_filled(filled, 2.5, color);
}

/// Petite carte « titre / valeur » (réseau).
fn stat_card(ui: &mut egui::Ui, title: &str, value: &str) {
    egui::Frame::NONE
        .stroke(egui::Stroke::new(1.0, theme::p().border))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).small().color(theme::p().text_dim));
            ui.label(RichText::new(value).monospace().size(15.0));
        });
}

impl FerroApp {
    /// Panneau de supervision du serveur de l'onglet actif (panneau droit).
    fn monitor_panel(&self, ui: &mut egui::Ui) {
        use crate::monitor::{format_bytes_rate, format_mem, format_uptime};
        ui.label(theme::section("Supervision"));
        ui.add_space(6.0);
        let dim = |text: &str| RichText::new(text).color(theme::p().text_dim);
        let Some(tab) = self.tabs.get(self.active) else {
            ui.label(dim("Ouvrez une session SSH pour superviser le serveur."));
            return;
        };
        if !matches!(tab.target, Target::Ssh(_)) {
            ui.label(dim("La supervision concerne les sessions SSH."));
            return;
        }
        let Some(s) = &tab.monitor else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(dim("En attente des premières mesures…"));
            });
            return;
        };
        let level = |pct: f32| {
            if pct >= 90.0 {
                theme::p().red
            } else if pct >= 75.0 {
                theme::p().yellow
            } else {
                theme::p().text
            }
        };

        egui::ScrollArea::vertical().id_salt("monitor_side_scroll").auto_shrink(false).show(ui, |ui| {
            // En-tête : hôte, distribution, uptime.
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(dot.center(), 4.0, theme::p().green);
                ui.label(RichText::new(if s.host.is_empty() { tab.title() } else { &s.host }).size(15.0).family(theme::semibold()));
            });
            let mut subtitle = Vec::new();
            if !s.os.is_empty() {
                subtitle.push(s.os.clone());
            }
            if s.uptime_secs > 0 {
                subtitle.push(format!("up {}", format_uptime(s.uptime_secs)));
            }
            if !subtitle.is_empty() {
                ui.label(RichText::new(subtitle.join("  ·  ")).small().color(theme::p().text_dim));
            }
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(8.0);

            // CPU
            let cpu_label = if s.cpus > 0 { format!("CPU  ·  {} vCPU", s.cpus) } else { "CPU".to_string() };
            let cpu_value = s.cpu_percent.map(|c| format!("{c:.0}%")).unwrap_or_else(|| "…".into());
            metric_row(ui, &cpu_label, &cpu_value, s.cpu_percent.map(level).map(|c| if c == theme::p().text { theme::p().teal } else { c }).unwrap_or(theme::p().text_dim));
            sparkline(ui, &tab.history.cpu, 100.0, theme::p().teal, true);
            ui.add_space(14.0);

            // Mémoire
            if s.mem_total > 0 {
                let pct = s.mem_used as f32 / s.mem_total as f32 * 100.0;
                metric_row(ui, &format!("Mémoire  ·  {}", format_mem(s.mem_total)), &format!("{pct:.0}%"), level(pct));
                sparkline(ui, &tab.history.mem, 100.0, theme::p().text_dim, false);
                ui.add_space(14.0);
            }

            // Disque racine
            if s.root_total > 0 {
                let fraction = s.root_used as f32 / s.root_total as f32;
                ui.horizontal(|ui| {
                    ui.label(dim("Disque /"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{} de {}", format_mem(s.root_used), format_mem(s.root_total)))
                                .monospace()
                                .color(theme::p().text_dim),
                        );
                    });
                });
                ui.add_space(4.0);
                gauge(ui, fraction, if fraction >= 0.9 { theme::p().red } else { theme::p().text_dim });
                ui.add_space(14.0);
            }

            // Réseau
            if let (Some(rx), Some(tx)) = (s.rx_rate, s.tx_rate) {
                ui.columns(2, |cols| {
                    stat_card(&mut cols[0], &theme::label(&theme::ICON_ARROW_DOWN, "Net"), &format_bytes_rate(rx));
                    stat_card(&mut cols[1], &theme::label(&theme::ICON_ARROW_UP, "Net"), &format_bytes_rate(tx));
                });
                ui.add_space(14.0);
            }

            // Processus
            if !s.processes.is_empty() {
                ui.label(theme::section("Processus"));
                ui.add_space(4.0);
                egui::Grid::new("monitor_processes").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
                    for p in &s.processes {
                        let name: String = p.name.chars().take(22).collect();
                        ui.label(RichText::new(name).monospace().size(13.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(format!("{:.1}%", p.cpu_percent)).monospace().size(13.0));
                        });
                        ui.label(RichText::new(format_mem(p.rss)).monospace().size(13.0).color(theme::p().text_dim));
                        ui.end_row();
                    }
                });
            }
        });
    }
}

/// Barre de supervision du serveur, sous le terminal : hôte, CPU, mémoire,
/// réseau, uptime, utilisateur et disques.
fn monitor_bar(ui: &mut egui::Ui, stats: Option<&crate::monitor::Stats>) {
    use crate::monitor::{format_gb, format_rate, format_uptime};
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let Some(s) = stats else {
            ui.spinner();
            ui.label(RichText::new("Supervision : en attente des premières mesures…").small().color(theme::p().text_dim));
            return;
        };
        fn stat(ui: &mut egui::Ui, icon: Option<&theme::Icon>, label: Option<&str>, value: &str, color: Color32) {
            if let Some(icon) = icon {
                ui.label(RichText::new(icon.get()).size(12.0).color(theme::p().text_dim));
            }
            if let Some(label) = label {
                ui.label(RichText::new(label).size(11.0).color(theme::p().text_dim).family(theme::semibold()));
            }
            ui.label(RichText::new(value).small().color(color));
            ui.add_space(10.0);
        }
        let level = |pct: f32| {
            if pct >= 90.0 {
                theme::p().red
            } else if pct >= 75.0 {
                theme::p().yellow
            } else {
                theme::p().text
            }
        };
        if !s.host.is_empty() {
            stat(ui, Some(&theme::ICON_SERVER), None, &s.host, theme::p().text);
        }
        match s.cpu_percent {
            Some(cpu) => stat(ui, None, Some("CPU"), &format!("{cpu:.0} %"), level(cpu)),
            None => stat(ui, None, Some("CPU"), "…", theme::p().text_dim),
        }
        if s.mem_total > 0 {
            let pct = s.mem_used as f32 / s.mem_total as f32 * 100.0;
            stat(ui, None, Some("RAM"), &format!("{} / {}", format_gb(s.mem_used), format_gb(s.mem_total)), level(pct));
        }
        if let (Some(tx), Some(rx)) = (s.tx_rate, s.rx_rate) {
            stat(ui, Some(&theme::ICON_ARROW_UP), None, &format_rate(tx), theme::p().text);
            stat(ui, Some(&theme::ICON_ARROW_DOWN), None, &format_rate(rx), theme::p().text);
        }
        if s.uptime_secs > 0 {
            stat(ui, Some(&theme::ICON_CLOCK), None, &format_uptime(s.uptime_secs), theme::p().text);
        }
        if !s.user.is_empty() {
            let text = if s.sessions > 1 { format!("{} (x{})", s.user, s.sessions) } else { s.user.clone() };
            stat(ui, Some(&theme::ICON_USER), None, &text, theme::p().text);
        }
        for (index, (mount, pct)) in s.disks.iter().enumerate() {
            let icon = if index == 0 { Some(&theme::ICON_DISK) } else { None };
            stat(ui, icon, None, &format!("{mount}: {pct} %"), level(*pct as f32));
        }
    });
}

impl FerroApp {
    /// Actions de la barre de titre, de gauche à droite.
    /// Pour en ajouter une : une entrée ici, et son traitement dans `apply`.
    fn title_actions(&self) -> Vec<TitleAction> {
        let mut list = vec![
            TitleAction {
                icon: TitleIcon::Font(&theme::ICON_ADD),
                tooltip: "Nouvelle session SSH",
                active: false,
                action: Action::NewSession,
            },
        ];
        if !self.shells.is_empty() {
            // Menu : choix du shell (PowerShell, cmd, WSL, Git Bash…).
            list.push(TitleAction {
                icon: TitleIcon::Shells,
                tooltip: "Nouveau terminal local",
                active: false,
                action: Action::OpenLocal(0),
            });
        }
        list.push(TitleAction {
            icon: TitleIcon::Toggle(theme::ToggleGlyph::PanelLeft),
            tooltip: "Afficher ou masquer les sessions (panneau gauche)",
            active: self.sessions_visible,
            action: Action::ToggleSessions,
        });
        list.push(TitleAction {
            icon: TitleIcon::Toggle(theme::ToggleGlyph::PanelRight),
            tooltip: "Afficher ou masquer les fichiers distants (panneau droit)",
            active: self.files_visible,
            action: Action::ToggleFiles,
        });
        list.push(TitleAction {
            icon: TitleIcon::Toggle(theme::ToggleGlyph::PanelBottom),
            tooltip: "Afficher ou masquer la supervision du serveur (panneau bas)",
            active: self.monitor_visible,
            action: Action::ToggleMonitor,
        });
        list
    }

    /// Barre d'activité : colonne d'icônes à gauche, comme dans VS Code.
    fn activity_bar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::left("activity")
            .exact_size(48.0)
            .resizable(false)
            .frame(egui::Frame::NONE.fill(theme::p().title_bar).inner_margin(egui::Margin::symmetric(6, 8)))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    // Fichiers et supervision ont leurs toggles dans la barre de titre.
                    let items = [
                        (&theme::ICON_TERMINAL, "Sessions", SideView::Sessions),
                        (&theme::ICON_MACRO, "Macros", SideView::Macros),
                    ];
                    for (icon, tooltip, view) in items {
                        let active = self.sessions_visible && self.side_view == view;
                        if theme::activity_item(ui, icon, tooltip, active).clicked() {
                            actions.push(Action::ShowSide(view));
                        }
                    }
                });
            });
    }

    fn sessions_sidebar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let gap = theme::CARD_GAP;
        let inner = egui::Margin::same(10);
        let outer = egui::Margin { left: 0, right: gap, top: 0, bottom: gap };
        let style = theme::hide_panel_resize_line(ui);
        let rect = egui::Panel::left("sessions")
            .resizable(true)
            .default_size(290.0)
            .frame(theme::card(theme::p().sidebar, inner, outer))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.set_style(style.clone());
                let rect = theme::card_rect(ui, inner);
                match self.side_view {
                    SideView::Sessions => {
                        theme::panel_header(ui, "Sessions", |ui| {
                            // Ajoutés de droite à gauche : la session s'affiche avant le groupe.
                            if theme::header_button(ui, &theme::ICON_NEW_FOLDER, "Nouveau groupe").clicked() {
                                actions.push(Action::NewGroup(String::new()));
                            }
                            if theme::header_button_badged(ui, &theme::ICON_SERVER, "Nouvelle session").clicked() {
                                actions.push(Action::NewSession);
                            }
                        });
                        ui.add_space(4.0);
                        self.sessions_panel(ui, actions);
                    }
                    SideView::Macros => {
                        theme::panel_header(ui, "Macros", |ui| {
                            if theme::header_button(ui, &theme::ICON_ADD, "Nouvelle macro").clicked() {
                                actions.push(Action::NewMacro);
                            }
                        });
                        ui.add_space(4.0);
                        self.macros_panel(ui, actions);
                    }
                }
                theme::card_corners(ui, rect);
                rect
            })
            .inner;
        ui.set_style(style);
        theme::card_sash(ui, "sessions", rect, true);
    }

    fn files_sidebar(&mut self, ui: &mut egui::Ui) {
        let gap = theme::CARD_GAP;
        let inner = egui::Margin::same(10);
        let outer = egui::Margin { left: gap, right: gap, top: 0, bottom: gap };
        let style = theme::hide_panel_resize_line(ui);
        let rect = egui::Panel::right("files")
            .resizable(true)
            .default_size(320.0)
            .frame(theme::card(theme::p().sidebar, inner, outer))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.set_style(style.clone());
                let rect = theme::card_rect(ui, inner);
                // Supervision en bas du panneau droit, redimensionnable.
                egui::Panel::bottom("monitor_side")
                    .resizable(true)
                    .default_size(460.0)
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 0, right: 0, top: 10, bottom: 0 }))
                    .show(ui, |ui| self.monitor_panel(ui));
                ui.label(theme::section("Fichiers distants"));
                ui.add_space(4.0);
                self.sftp_panel(ui);
                theme::card_corners(ui, rect);
                rect
            })
            .inner;
        ui.set_style(style);
        theme::card_sash(ui, "files", rect, false);
    }
}

impl FerroApp {
    /// Barre de titre intégrée : menus à gauche, actions et boutons de fenêtre à droite.
    /// Sous Windows, chaque zone est déclarée au système (voir `win_frame`) : le
    /// déplacement, le menu système, la disposition Snap et les boutons sont natifs.
    fn title_bar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        use egui::ViewportCommand;
        let ctx = ui.ctx().clone();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let native = win_frame::is_active();
        let bar = ui.max_rect();
        if native && !maximized {
            // Fine bordure haute, dans la continuité du cadre système.
            ui.painter().hline(bar.x_range(), bar.top() + 0.5, egui::Stroke::new(1.0, theme::p().border));
        }
        let mut regions = win_frame::Regions { height: theme::TITLE_BAR_HEIGHT, ..Default::default() };

        if !native {
            // Sans intégration système : déplacement et agrandissement gérés ici.
            let drag = ui.interact(bar, Id::unique("title_drag"), egui::Sense::click_and_drag());
            if drag.double_clicked() {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
            } else if drag.drag_started_by(egui::PointerButton::Primary) {
                ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            }
        }

        ui.horizontal_centered(|ui| {
            ui.add_space(12.0);
            ui.add(egui::Image::new(&self.logo).fit_to_exact_size(egui::vec2(18.0, 18.0)));
            ui.add_space(6.0);
            let menus = ui.scope(|ui| {
                let widgets = &mut ui.visuals_mut().widgets;
                widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                widgets.inactive.bg_stroke = egui::Stroke::NONE;
                widgets.hovered.bg_stroke = egui::Stroke::NONE;
                widgets.open.bg_stroke = egui::Stroke::NONE;
                ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.menu_button(RichText::new("Session").size(13.0), |ui| {
                    if ui.button(theme::label(&theme::ICON_ADD, "Nouvelle session…")).clicked() {
                        actions.push(Action::NewSession);
                        ui.close();
                    }
                    if ui.button(theme::label(&theme::ICON_NEW_FOLDER, "Nouveau groupe…")).clicked() {
                        actions.push(Action::NewGroup(String::new()));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(theme::label(&theme::ICON_DOWNLOAD, "Exporter les sessions…")).clicked() {
                        actions.push(Action::ExportSessions(String::new()));
                        ui.close();
                    }
                    if ui.button(theme::label(&theme::ICON_UPLOAD, "Importer des sessions…")).clicked() {
                        actions.push(Action::ImportSessions);
                        ui.close();
                    }
                    if ui.button(theme::label(&theme::ICON_IMPORT, "Importer depuis MobaXterm…")).clicked() {
                        actions.push(Action::Import);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Quitter").clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                });
                ui.menu_button(RichText::new("Terminal").size(13.0), |ui| {
                    if self.shells.is_empty() {
                        ui.label("Aucun shell détecté");
                    }
                    for (index, shell) in self.shells.iter().enumerate() {
                        if ui.button(theme::label(&theme::ICON_TERMINAL, &shell.name)).clicked() {
                            actions.push(Action::OpenLocal(index));
                            ui.close();
                        }
                    }
                    ui.separator();
                    let find = egui::Button::new(theme::label(&theme::ICON_SEARCH, "Rechercher dans le terminal"))
                        .shortcut_text("Ctrl+F");
                    if ui.add_enabled(!self.tabs.is_empty(), find).clicked() {
                        actions.push(Action::FindInTerminal);
                        ui.close();
                    }
                });
                ui.menu_button(RichText::new("Affichage").size(13.0), |ui| {
                    if ui.button(theme::label(&theme::ICON_SERVER, "Sessions")).clicked() {
                        actions.push(Action::ShowSide(SideView::Sessions));
                        ui.close();
                    }
                    if ui.button(theme::label(&theme::ICON_MACRO, "Macros")).clicked() {
                        actions.push(Action::ShowSide(SideView::Macros));
                        ui.close();
                    }
                    if ui.button(theme::label(&theme::ICON_FOLDER, "Fichiers distants")).clicked() {
                        actions.push(Action::ToggleFiles);
                        ui.close();
                    }
                    if ui.button(theme::label(&theme::ICON_CLOCK, "Supervision du serveur")).clicked() {
                        actions.push(Action::ToggleMonitor);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Scinder à droite").clicked() {
                        actions.push(Action::SplitRight);
                        ui.close();
                    }
                    if ui.button("Scinder en bas").clicked() {
                        actions.push(Action::SplitDown);
                        ui.close();
                    }
                    if ui.button("Grille de 4 terminaux").clicked() {
                        actions.push(Action::GridOfFour);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(theme::label(&theme::ICON_COLOR, "Coloration des mots-clés…")).clicked() {
                        actions.push(Action::EditHighlighting);
                        ui.close();
                    }
                });
            });
            regions.client.push(menus.response.rect);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let (hovered, pressed) = win_frame::caption_state();
                let maximize_icon = if maximized { &theme::ICON_WIN_RESTORE } else { &theme::ICON_WIN_MAXIMIZE };
                let buttons = [
                    (CaptionButton::Close, &theme::ICON_WIN_CLOSE),
                    (CaptionButton::Maximize, maximize_icon),
                    (CaptionButton::Minimize, &theme::ICON_WIN_MINIMIZE),
                ];
                for (kind, icon) in buttons {
                    let response = theme::window_button(
                        ui,
                        icon,
                        kind == CaptionButton::Close,
                        hovered == Some(kind),
                        pressed == Some(kind),
                    );
                    regions.buttons.push((kind, response.rect));
                    if response.clicked() && !native {
                        ctx.send_viewport_cmd(match kind {
                            CaptionButton::Close => ViewportCommand::Close,
                            CaptionButton::Maximize => ViewportCommand::Maximized(!maximized),
                            CaptionButton::Minimize => ViewportCommand::Minimized(true),
                        });
                    }
                }
                // Zone d'actions, comme les boutons de disposition de VS Code.
                ui.add_space(10.0);
                let zone = ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for item in self.title_actions().into_iter().rev() {
                        let response = match item.icon {
                            TitleIcon::Font(icon) => theme::title_action(ui, icon, item.tooltip, item.active),
                            TitleIcon::Toggle(glyph) => theme::title_toggle(ui, glyph, item.tooltip, item.active),
                            TitleIcon::Shells => {
                                // Le menu pousse lui-même l'action du shell choisi.
                                ui.scope(|ui| {
                                    let widgets = &mut ui.visuals_mut().widgets;
                                    widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                                    widgets.inactive.bg_stroke = egui::Stroke::NONE;
                                    widgets.hovered.bg_stroke = egui::Stroke::NONE;
                                    widgets.open.bg_stroke = egui::Stroke::NONE;
                                    ui.spacing_mut().button_padding = egui::vec2(6.0, 3.0);
                                    let icon = RichText::new(theme::ICON_TERMINAL.get()).size(14.0).color(theme::p().text_dim);
                                    self.local_menu(ui, icon, actions);
                                });
                                continue;
                            }
                        };
                        if response.clicked() {
                            actions.push(item.action);
                        }
                    }
                    // Sélecteur de thème, à gauche des actions.
                    ui.add_space(6.0);
                    ui.scope(|ui| {
                        let widgets = &mut ui.visuals_mut().widgets;
                        widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                        widgets.inactive.bg_stroke = egui::Stroke::NONE;
                        widgets.hovered.bg_stroke = egui::Stroke::NONE;
                        widgets.open.bg_stroke = egui::Stroke::NONE;
                        ui.spacing_mut().button_padding = egui::vec2(6.0, 3.0);
                        let menu = ui.menu_button(
                            RichText::new(theme::ICON_COLOR.get()).size(14.0).color(theme::p().text_dim),
                            |ui| {
                                // Le menu n'hérite pas du remplissage serré du bouton.
                                ui.set_min_width(110.0);
                                ui.spacing_mut().button_padding = egui::vec2(10.0, 6.0);
                                ui.spacing_mut().item_spacing.y = 4.0;
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    ui.add_space(10.0);
                                    ui.label(theme::section("Thème"));
                                });
                                ui.add_space(4.0);
                                ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                                    for palette in &theme::THEMES {
                                        let selected = theme::current_name() == palette.name;
                                        if ui.selectable_label(selected, palette.name).clicked() {
                                            actions.push(Action::SetTheme(palette.name.to_string()));
                                            ui.close();
                                        }
                                    }
                                });
                            },
                        );
                        menu.response.on_hover_text("Thème de l'interface");
                    });
                });
                regions.client.push(zone.response.rect);
            });
        });

        win_frame::update(regions, ctx.pixels_per_point());
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            match self.tabs.get(self.active) {
                Some(tab) => {
                    let (color, state) = match &tab.state {
                        TabState::Connecting(_) => (theme::p().yellow, "Connexion"),
                        TabState::Connected => (theme::p().green, "Connecté"),
                        TabState::Closed(_) => (theme::p().text_dim, "Fermé"),
                    };
                    let (dot, _) = ui.allocate_exact_size(egui::Vec2::splat(8.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 4.0, color);
                    ui.label(
                        RichText::new(format!("{state}  ·  {}", tab.target.subtitle()))
                            .small()
                            .color(theme::p().text_dim),
                    );
                    if let Some(status) = &tab.last_status {
                        ui.label(RichText::new(status).small().color(theme::p().yellow));
                    }
                }
                None => {
                    ui.label(RichText::new("Prêt").small().color(theme::p().text_dim));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{} session(s) enregistrée(s)", self.store.sessions.len()))
                        .small()
                        .color(theme::p().text_dim),
                );
            });
        });
    }

    /// Glisser-déposer de fichiers : envoi dans le dossier SFTP courant.
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if !matches!(tab.sftp.available, Some(Ok(()))) || tab.sftp.cwd.is_empty() {
            self.notice = Some((
                "Envoi impossible".into(),
                "Ouvrez une session avec SFTP disponible avant de déposer des fichiers.".into(),
            ));
            return;
        }
        for local in dropped {
            tab.conn.send(Command::Sftp(SftpCommand::Upload {
                local,
                remote_dir: tab.sftp.cwd.clone(),
            }));
        }
        self.files_visible = true;
    }
}

impl ProfileForm {
    /// `session` : id de la session, pour retrouver le mot de passe enregistré
    /// (`None` pour une nouvelle session, qui n'hérite d'aucun mot de passe).
    fn new(profile: crate::session::Profile, session: Option<u64>) -> Self {
        let saved_password =
            session.is_some_and(|id| crate::vault::exists(&crate::vault::Secret::Password(id, profile.id)));
        Self {
            use_key: profile.key_path.is_some(),
            port: profile.port.to_string(),
            profile,
            password: String::new(),
            saved_password,
            forget_password: false,
            saved_passphrase: None,
            forget_passphrase: false,
        }
    }

    /// Vrai si une phrase de passe est enregistrée pour la clé saisie
    /// (vérifié dans le coffre seulement quand le chemin change).
    fn passphrase_saved(&mut self) -> bool {
        let path = self.profile.key_path.clone().unwrap_or_default().trim().to_string();
        if path.is_empty() {
            return false;
        }
        match &self.saved_passphrase {
            Some((checked, saved)) if *checked == path => *saved,
            _ => {
                let saved = crate::vault::exists(&crate::vault::Secret::Passphrase(path.clone()));
                self.saved_passphrase = Some((path, saved));
                self.forget_passphrase = false;
                saved
            }
        }
    }

    /// Profil vérifié, prêt à enregistrer.
    fn validate(&self) -> Result<crate::session::Profile, String> {
        let mut p = self.profile.clone();
        p.name = p.name.trim().to_string();
        p.host = p.host.trim().to_string();
        p.username = p.username.trim().to_string();
        if p.name.is_empty() {
            return Err("Donnez un nom au profil.".into());
        }
        if p.host.is_empty() {
            return Err("L'hôte est obligatoire.".into());
        }
        p.port = match self.port.trim().parse() {
            Ok(port) if port != 0 => port,
            _ => return Err("Le port doit être un nombre entre 1 et 65535.".into()),
        };
        p.key_path = if self.use_key {
            let key = p.key_path.unwrap_or_default().trim().to_string();
            if key.is_empty() {
                return Err("Choisissez une clé privée, ou l'authentification par mot de passe.".into());
            }
            Some(key)
        } else {
            None
        };
        Ok(p)
    }
}

impl Editor {
    fn new(session: Session, is_new: bool) -> Self {
        // Une copie (nouvelle session) n'hérite pas des mots de passe de l'original.
        let vault_id = (!is_new).then_some(session.id);
        let profiles: Vec<_> = session.profiles().into_iter().map(|p| ProfileForm::new(p, vault_id)).collect();
        let active = session.profile;
        Self {
            selected: profiles.iter().position(|f| f.profile.id == active).unwrap_or(0),
            profiles,
            active,
            session,
            is_new,
            removed: Vec::new(),
            focus_profile_name: false,
            error: None,
        }
    }

    /// Ajoute un profil, copie de celui affiché (même utilisateur, même clé :
    /// souvent seul l'hôte change), et l'affiche.
    fn add_profile(&mut self) {
        let all: Vec<_> = self.profiles.iter().map(|f| f.profile.clone()).collect();
        let mut profile = all.get(self.selected).cloned().unwrap_or_default();
        profile.id = Session::next_profile_id(&all);
        let taken = |name: &str| all.iter().any(|p| p.name.trim().eq_ignore_ascii_case(name));
        profile.name = (1..)
            .map(|n| if n == 1 { "Nouveau profil".to_string() } else { format!("Nouveau profil {n}") })
            .find(|name| !taken(name))
            .unwrap_or_default();
        let mut form = ProfileForm::new(profile, None);
        if let Some(source) = self.profiles.get(self.selected) {
            form.port = source.port.clone();
            form.use_key = source.use_key;
        }
        self.profiles.push(form);
        self.selected = self.profiles.len() - 1;
        self.focus_profile_name = true;
        self.error = None;
    }

    /// Supprime un profil (jamais le dernier). Le profil actif supprimé est
    /// remplacé par le premier restant.
    fn remove_profile(&mut self, index: usize) {
        if self.profiles.len() < 2 || index >= self.profiles.len() {
            return;
        }
        let removed = self.profiles.remove(index);
        if !self.is_new {
            self.removed.push(removed.profile.id);
        }
        if removed.profile.id == self.active {
            self.active = self.profiles[0].profile.id;
        }
        if self.selected >= index && self.selected > 0 {
            self.selected -= 1;
        }
        self.error = None;
    }

    /// Session vérifiée, prête à enregistrer. En cas d'erreur dans un profil,
    /// celui-ci est affiché.
    fn validate(&mut self) -> Result<Session, String> {
        let mut profiles = Vec::new();
        for (index, form) in self.profiles.iter().enumerate() {
            match form.validate() {
                Ok(profile) => profiles.push(profile),
                Err(e) => {
                    self.selected = index;
                    let name = form.profile.name.trim();
                    return Err(if self.profiles.len() > 1 && !name.is_empty() {
                        format!("Profil « {name} » : {e}")
                    } else {
                        e
                    });
                }
            }
        }
        for (index, profile) in profiles.iter().enumerate() {
            if profiles[..index].iter().any(|p| p.name.eq_ignore_ascii_case(&profile.name)) {
                self.selected = index;
                return Err(format!("Deux profils s'appellent « {} ».", profile.name));
            }
        }
        let mut s = self.session.clone();
        s.set_profiles(profiles, self.active);
        s.folder = s
            .folder
            .split(['/', '\\'])
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        s.name = s.name.trim().to_string();
        if s.name.is_empty() {
            s.name = s.host.clone();
        }
        Ok(s)
    }
}

#[derive(Default)]
struct FolderNode {
    children: BTreeMap<String, FolderNode>,
    sessions: Vec<Session>,
}

impl FolderNode {
    fn count(&self) -> usize {
        self.sessions.len() + self.children.values().map(FolderNode::count).sum::<usize>()
    }

    /// Fait exister un groupe, même sans session.
    fn insert_folder(&mut self, path: &str) {
        let mut node = self;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            node = node.children.entry(part.to_string()).or_default();
        }
    }

    fn insert(&mut self, session: &Session) {
        let mut node = self;
        for part in session.folder.split('/').filter(|p| !p.is_empty()) {
            node = node.children.entry(part.to_string()).or_default();
        }
        node.sessions.push(session.clone());
        node.sessions.sort_by_key(|s| s.name.to_lowercase());
    }
}

fn parent_dir(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => trimmed[..i].to_string(),
    }
}

/// Fin du suivi d'un fichier édité : la copie locale est supprimée, sauf si
/// elle contient des modifications non envoyées (message à afficher).
fn stop_editing(file: &crate::edit::EditedFile) -> Option<(String, String)> {
    if file.has_unsent_changes() {
        return Some((
            "Modifications non envoyées".into(),
            format!(
                "Les dernières modifications de {} n'ont pas été envoyées sur le serveur.\nLa copie locale est conservée : {}",
                file.remote,
                file.local.display()
            ),
        ));
    }
    file.remove_local_copy();
    None
}

/// Renvoie la version locale d'un fichier édité (fusionnée avec celle du
/// serveur si elle a changé), si sa connexion le permet.
fn start_upload(file: &mut crate::edit::EditedFile, tab: &Tab) {
    let Some((base, ours)) = file.upload_started() else { return };
    if !matches!(tab.state, TabState::Connected) || !matches!(tab.sftp.available, Some(Ok(()))) {
        file.upload_finished(Err("connexion indisponible".into()));
        return;
    }
    tab.conn.send(Command::Sftp(SftpCommand::SaveEdited { remote: file.remote.clone(), base, ours }));
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["o", "Ko", "Mo", "Go", "To"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} o")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn describe_entry(entry: &RemoteEntry) -> String {
    let mut text = entry.name.clone();
    if !entry.is_dir {
        text.push_str(&format!("\nTaille : {}", human_size(entry.size)));
    }
    if let Some(mtime) = entry.mtime {
        text.push_str(&format!("\nModifié : {} (UTC)", format_unix(mtime as i64)));
    }
    text
}

/// Date AAAA-MM-JJ HH:MM en UTC, sans dépendance supplémentaire.
fn format_unix(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    // Algorithme « civil from days » (H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", secs / 3600, (secs % 3600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_dir_handles_root_and_trailing_slash() {
        assert_eq!(parent_dir("/home/alice"), "/home");
        assert_eq!(parent_dir("/home/alice/"), "/home");
        assert_eq!(parent_dir("/home"), "/");
        assert_eq!(parent_dir("/"), "/");
    }

    #[test]
    fn formats_dates_and_sizes() {
        assert_eq!(format_unix(0), "1970-01-01 00:00");
        assert_eq!(format_unix(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(human_size(512), "512 o");
        assert_eq!(human_size(1536), "1.5 Ko");
    }

    #[test]
    fn editor_validation_normalises_fields() {
        let mut editor = Editor::new(
            Session {
                host: " 10.0.0.5 ".into(),
                folder: "\\Clients\\ Prod /".into(),
                ..Default::default()
            },
            true,
        );
        editor.profiles[0].port = "2222".into();
        let s = editor.validate().unwrap();
        assert_eq!(s.host, "10.0.0.5");
        assert_eq!(s.name, "10.0.0.5");
        assert_eq!(s.folder, "Clients/Prod");
        assert_eq!(s.port, 2222);
        assert_eq!(s.key_path, None);
        assert!(s.profiles.is_empty());
    }

    #[test]
    fn editor_profiles_add_activate_and_remove() {
        let mut editor = Editor::new(Session { host: "dev.local".into(), port: 22, ..Default::default() }, true);
        editor.add_profile();
        assert_eq!(editor.selected, 1);
        let form = &mut editor.profiles[1];
        assert_eq!((form.profile.name.as_str(), form.profile.host.as_str()), ("Nouveau profil", "dev.local"));
        form.profile.name = " Prod ".into();
        form.profile.host = "prod.example.org".into();
        editor.active = editor.profiles[1].profile.id;
        let s = editor.validate().unwrap();
        assert_eq!((s.host.as_str(), s.profile_name()), ("prod.example.org", "Prod"));
        // Le nom par défaut vient du profil actif.
        assert_eq!(s.name, "prod.example.org");

        // Deux profils du même nom sont refusés, et le second est affiché.
        editor.add_profile();
        editor.profiles[2].profile.name = "prod".into();
        editor.selected = 0;
        assert!(editor.validate().is_err());
        assert_eq!(editor.selected, 2);

        // Supprimer le profil actif active le premier restant.
        editor.remove_profile(2);
        editor.remove_profile(1);
        assert_eq!(editor.active, 0);
        assert_eq!(editor.validate().unwrap().host, "dev.local");
        editor.remove_profile(0);
        assert_eq!(editor.profiles.len(), 1);
    }
}
