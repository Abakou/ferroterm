//! Connexions SSH : une tâche tokio par onglet, qui pilote le terminal distant
//! et une session SFTP sur un second canal.
//!
//! L'interface communique avec la tâche par trois canaux :
//! - `Command` (interface → connexion) : saisie clavier, redimensionnement, SFTP ;
//! - `Reply` (interface → connexion) : réponses aux questions (clé d'hôte, mot de passe) ;
//! - `Event` (connexion → interface) : état, questions, résultats SFTP.
//!
//! La sortie du terminal est écrite directement dans un `vt100::Parser` partagé,
//! que l'interface lit pour l'affichage.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow, bail};
use eframe::egui;
use russh::client::{self, KeyboardInteractiveAuthResponse};
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect};
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex as AsyncMutex, mpsc};

use crate::session::Session;
use crate::vault;

pub type SharedParser = Arc<Mutex<vt100::Parser>>;

const SCROLLBACK_LINES: usize = 3000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const SFTP_TIMEOUT: Duration = Duration::from_secs(10);

pub enum Command {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Sftp(SftpCommand),
    Disconnect,
}

pub enum SftpCommand {
    List(String),
    Download { remote: String, local: PathBuf },
    Upload { local: PathBuf, remote_dir: String },
    Mkdir(String),
    Remove { path: String, is_dir: bool },
    /// Télécharge un fichier à éditer, puis répond par `Event::EditReady`.
    FetchForEdit { remote: String, local: PathBuf },
    /// Renvoie un fichier édité à son emplacement exact, fusionné (comme git)
    /// avec les modifications faites sur le serveur depuis `base` ; répond par
    /// `Event::EditSaved`.
    SaveEdited { remote: String, base: Vec<u8>, ours: Vec<u8> },
}

pub enum Reply {
    HostKey(bool),
    /// `text` : `None` = annulé par l'utilisateur ; `remember` : case
    /// « Enregistrer » cochée (si elle était proposée).
    Text { text: Option<String>, remember: bool },
}

pub enum Event {
    Status(String),
    AskHostKey { algorithm: String, fingerprint: String },
    /// `remember` : propose d'enregistrer la réponse dans le coffre du système
    /// (valeur initiale de la case), `None` = pas de case.
    AskText { prompt: String, secret: bool, remember: Option<bool> },
    Connected,
    SftpReady { home: String },
    SftpUnavailable(String),
    SftpListing { path: String, entries: Vec<RemoteEntry> },
    SftpError(String),
    Transfer(TransferUpdate),
    /// Fichier à éditer téléchargé (ou erreur).
    EditReady { remote: String, local: PathBuf, error: Option<String> },
    /// Fichier édité traité : envoyé, fusionné, en conflit… (ou erreur).
    EditSaved { remote: String, result: Result<crate::edit::SaveOutcome, String> },
    /// Mesures de supervision du serveur (sessions SSH uniquement).
    Monitor(crate::monitor::Stats),
    /// Fin de la connexion, avec l'erreur éventuelle.
    Closed(Option<String>),
}

#[derive(Clone, Debug)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub mtime: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct TransferUpdate {
    pub id: u64,
    pub label: String,
    pub upload: bool,
    pub done: u64,
    pub total: Option<u64>,
    pub finished: bool,
    pub error: Option<String>,
}

/// Côté interface d'une connexion.
pub struct Connection {
    commands: mpsc::UnboundedSender<Command>,
    replies: mpsc::UnboundedSender<Reply>,
    pub events: std::sync::mpsc::Receiver<Event>,
    pub parser: SharedParser,
}

impl Connection {
    pub fn spawn(rt: &tokio::runtime::Runtime, session: Session, ctx: egui::Context) -> Self {
        let (conn, parts) = Self::new_parts(ctx);
        let ConnectionParts { ui, parser, commands, replies } = parts;
        rt.spawn(async move {
            let replies = Arc::new(AsyncMutex::new(replies));
            let result = run(session, INITIAL_COLS, INITIAL_ROWS, ui.clone(), parser, commands, replies).await;
            ui.send(Event::Closed(result.err().map(|e| describe_error(&e))));
        });
        conn
    }

    /// Crée la connexion côté interface et les extrémités côté tâche.
    /// Sert aussi au terminal local (voir `local.rs`).
    pub(crate) fn new_parts(ctx: egui::Context) -> (Self, ConnectionParts) {
        let parser = Arc::new(Mutex::new(vt100::Parser::new(
            nonzero(INITIAL_ROWS),
            nonzero(INITIAL_COLS),
            SCROLLBACK_LINES,
        )));
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (reply_tx, reply_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let conn = Self {
            commands: cmd_tx,
            replies: reply_tx,
            events: event_rx,
            parser: parser.clone(),
        };
        let parts = ConnectionParts {
            ui: UiLink { tx: event_tx, ctx },
            parser,
            commands: cmd_rx,
            replies: reply_rx,
        };
        (conn, parts)
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn reply(&self, reply: Reply) {
        let _ = self.replies.send(reply);
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Disconnect);
    }
}

pub(crate) const INITIAL_COLS: u16 = 80;
pub(crate) const INITIAL_ROWS: u16 = 24;

/// Extrémités d'une connexion côté tâche de fond.
pub(crate) struct ConnectionParts {
    pub ui: UiLink,
    pub parser: SharedParser,
    pub commands: mpsc::UnboundedReceiver<Command>,
    pub replies: mpsc::UnboundedReceiver<Reply>,
}

fn nonzero(v: u16) -> std::num::NonZeroU16 {
    std::num::NonZeroU16::new(v.max(1)).expect("valeur non nulle")
}

/// Envoie un événement à l'interface et la réveille.
#[derive(Clone)]
pub(crate) struct UiLink {
    tx: std::sync::mpsc::Sender<Event>,
    pub ctx: egui::Context,
}

impl UiLink {
    pub fn send(&self, event: Event) {
        let _ = self.tx.send(event);
        self.ctx.request_repaint();
    }

    fn status(&self, text: impl Into<String>) {
        self.send(Event::Status(text.into()));
    }
}

type Replies = Arc<AsyncMutex<mpsc::UnboundedReceiver<Reply>>>;

async fn ask_text(ui: &UiLink, replies: &Replies, prompt: &str, secret: bool) -> anyhow::Result<Option<String>> {
    Ok(ask(ui, replies, prompt, secret, None).await?.map(|(text, _)| text))
}

/// Demande un secret en proposant de l'enregistrer (case cochée si `remember`).
/// Renvoie le secret et l'état de la case, ou `None` si annulé.
async fn ask_secret(ui: &UiLink, replies: &Replies, prompt: &str, remember: bool) -> anyhow::Result<Option<(String, bool)>> {
    ask(ui, replies, prompt, true, Some(remember)).await
}

async fn ask(
    ui: &UiLink,
    replies: &Replies,
    prompt: &str,
    secret: bool,
    remember: Option<bool>,
) -> anyhow::Result<Option<(String, bool)>> {
    ui.send(Event::AskText {
        prompt: prompt.to_string(),
        secret,
        remember,
    });
    match replies.lock().await.recv().await {
        Some(Reply::Text { text, remember }) => Ok(text.map(|t| (t, remember))),
        Some(Reply::HostKey(_)) => Ok(None),
        None => bail!("fenêtre fermée"),
    }
}

/// Après une saisie réussie : enregistre le secret si la case est cochée,
/// sinon supprime l'ancien s'il venait d'être refusé (il est périmé).
fn update_vault(ui: &UiLink, secret: &vault::Secret, user: &str, value: &str, remember: bool, stale: bool) {
    if remember {
        if let Err(e) = vault::set(secret, user, value) {
            ui.status(format!("Non enregistré : {e:#}"));
        }
    } else if stale {
        vault::delete(secret);
    }
}

struct Handler {
    host: String,
    port: u16,
    ui: UiLink,
    replies: Replies,
}

impl client::Handler for Handler {
    type Error = anyhow::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let key = key.public_key();
        match russh::keys::check_known_hosts(&self.host, self.port, &key) {
            Ok(true) => return Ok(true),
            Err(russh::keys::Error::KeyChanged { line }) => {
                self.ui.status(format!(
                    "ATTENTION : la clé de {} ne correspond pas à celle enregistrée \
                     (known_hosts, ligne {line}). Connexion refusée.",
                    self.host
                ));
                return Ok(false);
            }
            Ok(false) => {}
            Err(e) => self.ui.status(format!("known_hosts illisible ({e})")),
        }

        // Hôte inconnu : on demande à l'utilisateur.
        self.ui.send(Event::AskHostKey {
            algorithm: key.algorithm().to_string(),
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
        });
        let accepted = matches!(self.replies.lock().await.recv().await, Some(Reply::HostKey(true)));
        if accepted {
            if let Err(e) = russh::keys::known_hosts::learn_known_hosts(&self.host, self.port, &key) {
                self.ui.status(format!("Clé acceptée mais non enregistrée dans known_hosts ({e})"));
            }
        }
        Ok(accepted)
    }
}

async fn run(
    session: Session,
    cols: u16,
    rows: u16,
    ui: UiLink,
    parser: SharedParser,
    mut commands: mpsc::UnboundedReceiver<Command>,
    replies: Replies,
) -> anyhow::Result<()> {
    ui.status(format!("Connexion à {}…", session.target()));
    let stream = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect((session.host.as_str(), session.port)),
    )
    .await
    .map_err(|_| anyhow!("délai de connexion dépassé ({} s)", CONNECT_TIMEOUT.as_secs()))?
    .with_context(|| format!("impossible de joindre {}:{}", session.host, session.port))?;
    let _ = stream.set_nodelay(true);

    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(30)),
        ..Default::default()
    });
    let handler = Handler {
        host: session.host.clone(),
        port: session.port,
        ui: ui.clone(),
        replies: replies.clone(),
    };
    let mut handle = client::connect_stream(config, stream, handler).await?;

    let mut user = session.username.clone();
    if user.is_empty() {
        user = ask_text(&ui, &replies, "Nom d'utilisateur", false)
            .await?
            .filter(|u| !u.is_empty())
            .ok_or_else(|| anyhow!("connexion annulée"))?;
    }

    if !authenticate(&mut handle, &user, &session, &ui, &replies).await? {
        bail!("authentification refusée");
    }

    ui.status("Ouverture du terminal…");
    let channel = handle.channel_open_session().await?;
    channel
        .request_pty(false, "xterm-256color", cols.into(), rows.into(), 0, 0, &[])
        .await?;
    channel.request_shell(false).await?;
    ui.send(Event::Connected);

    // SFTP sur un second canal ; le terminal fonctionne même si le serveur le refuse.
    let (sftp_tx, sftp_rx) = mpsc::unbounded_channel();
    match tokio::time::timeout(SFTP_TIMEOUT, open_sftp(&handle)).await {
        Ok(Ok(sftp)) => {
            let home = sftp.canonicalize(".").await.unwrap_or_else(|_| "/".into());
            ui.send(Event::SftpReady { home });
            tokio::spawn(sftp_worker(Arc::new(sftp), sftp_rx, ui.clone()));
        }
        Ok(Err(e)) => ui.send(Event::SftpUnavailable(describe_error(&e))),
        Err(_) => ui.send(Event::SftpUnavailable("le serveur ne répond pas".into())),
    }

    // Supervision sur un troisième canal ; silencieuse si le serveur la refuse.
    if let Ok(monitor) = handle.channel_open_session().await {
        if monitor.exec(true, crate::monitor::command()).await.is_ok() {
            tokio::spawn(monitor_worker(monitor, ui.clone()));
        }
    }

    let (mut reader, writer) = channel.split();
    loop {
        tokio::select! {
            msg = reader.wait() => match msg {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    let replies = crate::terminal::feed(&mut parser.lock().expect("parser"), &data);
                    if !replies.is_empty() {
                        writer.data(&replies[..]).await?;
                    }
                    ui.ctx.request_repaint();
                }
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                Some(_) => {}
            },
            cmd = commands.recv() => match cmd {
                Some(Command::Input(bytes)) => writer.data(&bytes[..]).await?,
                Some(Command::Resize { cols, rows }) => {
                    let _ = writer.window_change(cols.into(), rows.into(), 0, 0).await;
                }
                Some(Command::Sftp(c)) => { let _ = sftp_tx.send(c); }
                Some(Command::Disconnect) | None => {
                    let _ = writer.close().await;
                    break;
                }
            },
        }
    }
    let _ = handle.disconnect(Disconnect::ByApplication, "", "fr").await;
    Ok(())
}

async fn authenticate(
    handle: &mut client::Handle<Handler>,
    user: &str,
    session: &Session,
    ui: &UiLink,
    replies: &Replies,
) -> anyhow::Result<bool> {
    if let Some(path) = &session.key_path {
        ui.status("Authentification par clé…");
        match load_key(path, ui, replies).await {
            Ok(key) => {
                let hash = handle.best_supported_rsa_hash().await?.flatten();
                let result = handle
                    .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                    .await?;
                if result.success() {
                    return Ok(true);
                }
                ui.status("Clé refusée par le serveur, essai par mot de passe…");
            }
            Err(e) => ui.status(format!(
                "Clé inutilisable ({}), essai par mot de passe…",
                describe_error(&e)
            )),
        }
    }

    // Mot de passe enregistré dans le coffre du système : essayé sans rien demander.
    // Une session sans id (non enregistrée) n'a pas de mot de passe mémorisé.
    let saved = (session.id != 0).then_some(vault::Secret::Password(session.id, session.profile));
    let mut stale = false;
    if let Some(password) = saved.as_ref().and_then(vault::get) {
        ui.status("Authentification avec le mot de passe enregistré…");
        if try_password(handle, user, &password).await? {
            return Ok(true);
        }
        stale = true;
    }

    for attempt in 0..3 {
        let prompt = match (attempt, stale) {
            (0, true) => format!("Le mot de passe enregistré a été refusé. Mot de passe pour {user}@{}", session.host),
            (0, false) => format!("Mot de passe pour {user}@{}", session.host),
            _ => format!("Mot de passe refusé. Mot de passe pour {user}@{}", session.host),
        };
        let (password, remember) = match &saved {
            Some(_) => match ask_secret(ui, replies, &prompt, stale).await? {
                Some(answer) => answer,
                None => return Ok(false),
            },
            None => match ask_text(ui, replies, &prompt, true).await? {
                Some(password) => (password, false),
                None => return Ok(false),
            },
        };
        ui.status("Authentification…");
        if try_password(handle, user, &password).await? {
            if let Some(secret) = &saved {
                update_vault(ui, secret, user, &password, remember, stale);
            }
            return Ok(true);
        }
    }
    Ok(false)
}

/// Mot de passe, puis « keyboard-interactive » : beaucoup de serveurs
/// n'acceptent le mot de passe que sous cette forme.
async fn try_password(handle: &mut client::Handle<Handler>, user: &str, password: &str) -> anyhow::Result<bool> {
    if handle.authenticate_password(user, password).await?.success() {
        return Ok(true);
    }
    Ok(keyboard_interactive(handle, user, password).await.unwrap_or(false))
}

async fn keyboard_interactive(
    handle: &mut client::Handle<Handler>,
    user: &str,
    password: &str,
) -> anyhow::Result<bool> {
    let mut response = handle
        .authenticate_keyboard_interactive_start(user, None::<String>)
        .await?;
    for _ in 0..5 {
        match response {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| password.to_string()).collect();
                response = handle.authenticate_keyboard_interactive_respond(answers).await?;
            }
        }
    }
    Ok(false)
}

async fn load_key(path: &str, ui: &UiLink, replies: &Replies) -> anyhow::Result<PrivateKey> {
    match russh::keys::load_secret_key(path, None) {
        Ok(key) => Ok(key),
        Err(russh::keys::Error::KeyIsEncrypted) => {
            // Phrase de passe enregistrée dans le coffre du système : essayée d'abord.
            let saved = vault::Secret::Passphrase(path.to_string());
            let mut stale = false;
            if let Some(passphrase) = vault::get(&saved) {
                if let Ok(key) = russh::keys::load_secret_key(path, Some(&passphrase)) {
                    return Ok(key);
                }
                stale = true;
            }
            for attempt in 0..3 {
                let prompt = match (attempt, stale) {
                    (0, true) => format!("La phrase de passe enregistrée ne convient plus. Phrase de passe de la clé {path}"),
                    (0, false) => format!("Phrase de passe de la clé {path}"),
                    _ => format!("Phrase de passe incorrecte pour {path}"),
                };
                let Some((passphrase, remember)) = ask_secret(ui, replies, &prompt, stale).await? else {
                    bail!("phrase de passe non fournie");
                };
                if let Ok(key) = russh::keys::load_secret_key(path, Some(&passphrase)) {
                    update_vault(ui, &saved, path, &passphrase, remember, stale);
                    return Ok(key);
                }
            }
            bail!("phrase de passe incorrecte")
        }
        Err(e) => Err(anyhow!("{path} : {e}")),
    }
}

async fn open_sftp(handle: &client::Handle<Handler>) -> anyhow::Result<SftpSession> {
    let channel = handle.channel_open_session().await?;
    channel.request_subsystem(true, "sftp").await?;
    Ok(SftpSession::new(channel.into_stream()).await?)
}

async fn sftp_worker(
    sftp: Arc<SftpSession>,
    mut commands: mpsc::UnboundedReceiver<SftpCommand>,
    ui: UiLink,
) {
    let mut next_transfer = 0u64;
    while let Some(command) = commands.recv().await {
        match command {
            SftpCommand::List(path) => match list_dir(&sftp, &path).await {
                Ok((path, entries)) => ui.send(Event::SftpListing { path, entries }),
                Err(e) => ui.send(Event::SftpError(format!("{path} : {}", describe_error(&e)))),
            },
            SftpCommand::Mkdir(path) => {
                if let Err(e) = sftp.create_dir(path.clone()).await {
                    ui.send(Event::SftpError(format!("Création de {path} impossible : {e}")));
                }
            }
            SftpCommand::Remove { path, is_dir } => {
                let result = if is_dir {
                    sftp.remove_dir(path.clone()).await
                } else {
                    sftp.remove_file(path.clone()).await
                };
                if let Err(e) = result {
                    ui.send(Event::SftpError(format!("Suppression de {path} impossible : {e}")));
                }
            }
            // Les transferts tournent en parallèle pour ne pas bloquer la navigation.
            SftpCommand::Download { remote, local } => {
                next_transfer += 1;
                let (sftp, ui, id) = (sftp.clone(), ui.clone(), next_transfer);
                tokio::spawn(async move {
                    let _ = download(sftp, remote, local, ui, id).await;
                });
            }
            SftpCommand::Upload { local, remote_dir } => {
                next_transfer += 1;
                let name = local.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let remote = join_remote(&remote_dir, &name);
                let (sftp, ui, id) = (sftp.clone(), ui.clone(), next_transfer);
                tokio::spawn(async move {
                    let _ = upload(sftp, local, remote, ui, id).await;
                });
            }
            SftpCommand::FetchForEdit { remote, local } => {
                next_transfer += 1;
                let (sftp, ui, id) = (sftp.clone(), ui.clone(), next_transfer);
                tokio::spawn(async move {
                    let error = download(sftp, remote.clone(), local.clone(), ui.clone(), id).await;
                    ui.send(Event::EditReady { remote, local, error });
                });
            }
            SftpCommand::SaveEdited { remote, base, ours } => {
                next_transfer += 1;
                let (sftp, ui, id) = (sftp.clone(), ui.clone(), next_transfer);
                tokio::spawn(async move {
                    let result = save_edited(sftp, &remote, &base, &ours, &ui, id).await;
                    ui.send(Event::EditSaved { remote, result });
                });
            }
        }
    }
}

async fn monitor_worker(mut channel: russh::Channel<client::Msg>, ui: UiLink) {
    let mut parser = crate::monitor::Parser::default();
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } => {
                for stats in parser.feed(&data) {
                    ui.send(Event::Monitor(stats));
                }
            }
            ChannelMsg::Eof | ChannelMsg::Close | ChannelMsg::ExitStatus { .. } => break,
            _ => {}
        }
    }
}

pub fn join_remote(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

async fn list_dir(sftp: &SftpSession, path: &str) -> anyhow::Result<(String, Vec<RemoteEntry>)> {
    let path = sftp.canonicalize(path.to_string()).await?;
    let mut entries = Vec::new();
    for entry in sftp.read_dir(path.clone()).await? {
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let meta = entry.metadata();
        let file_type = entry.file_type();
        let is_symlink = file_type.is_symlink();
        let mut is_dir = file_type.is_dir();
        if is_symlink {
            // Suivre le lien pour savoir s'il pointe vers un dossier.
            if let Ok(target) = sftp.metadata(join_remote(&path, &name)).await {
                is_dir = target.file_type().is_dir();
            }
        }
        entries.push(RemoteEntry {
            name,
            is_dir,
            is_symlink,
            size: meta.size.unwrap_or(0),
            mtime: meta.mtime,
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok((path, entries))
}

/// Télécharge `remote` dans `local` ; renvoie l'erreur éventuelle (déjà signalée
/// dans la liste des transferts).
async fn download(sftp: Arc<SftpSession>, remote: String, local: PathBuf, ui: UiLink, id: u64) -> Option<String> {
    let name = remote.rsplit('/').next().unwrap_or(&remote).to_string();
    let mut update = TransferUpdate {
        id,
        label: format!("⬇ {name}"),
        upload: false,
        done: 0,
        total: None,
        finished: false,
        error: None,
    };
    let result: anyhow::Result<()> = async {
        update.total = sftp.metadata(remote.clone()).await.ok().and_then(|m| m.size);
        let mut source = sftp.open(remote.clone()).await?;
        let mut target = tokio::fs::File::create(&local)
            .await
            .with_context(|| format!("création de {}", local.display()))?;
        copy_with_progress(&mut source, &mut target, &mut update, &ui).await?;
        target.flush().await?;
        Ok(())
    }
    .await;
    finish_transfer(update, result, &ui)
}

/// Envoie `local` à l'emplacement exact `remote` (remplacé s'il existe) ;
/// renvoie l'erreur éventuelle (déjà signalée dans la liste des transferts).
async fn upload(sftp: Arc<SftpSession>, local: PathBuf, remote: String, ui: UiLink, id: u64) -> Option<String> {
    let name = remote.rsplit('/').next().unwrap_or(&remote).to_string();
    let mut update = TransferUpdate {
        id,
        label: format!("⬆ {name}"),
        upload: true,
        done: 0,
        total: None,
        finished: false,
        error: None,
    };
    let result: anyhow::Result<()> = async {
        let meta = tokio::fs::metadata(&local).await?;
        if meta.is_dir() {
            bail!("l'envoi de dossiers n'est pas encore pris en charge");
        }
        update.total = Some(meta.len());
        let mut source = tokio::fs::File::open(&local).await?;
        let mut target = sftp.create(remote.clone()).await?;
        copy_with_progress(&mut source, &mut target, &mut update, &ui).await?;
        target.shutdown().await?;
        Ok(())
    }
    .await;
    finish_transfer(update, result, &ui)
}

/// Renvoi d'un fichier édité : relit la version du serveur, fusionne avec la
/// version locale (base = dernier contenu connu du serveur), puis écrit le
/// résultat s'il y a lieu.
async fn save_edited(
    sftp: Arc<SftpSession>,
    remote: &str,
    base: &[u8],
    ours: &[u8],
    ui: &UiLink,
    id: u64,
) -> Result<crate::edit::SaveOutcome, String> {
    let name = remote.rsplit('/').next().unwrap_or(remote).to_string();
    let mut update = TransferUpdate {
        id,
        label: format!("⬆ {name}"),
        upload: true,
        done: 0,
        total: Some(ours.len() as u64),
        finished: false,
        error: None,
    };
    ui.send(Event::Transfer(update.clone()));
    let result: anyhow::Result<crate::edit::SaveOutcome> = async {
        let theirs = if sftp.try_exists(remote.to_string()).await? {
            Some(sftp.read(remote.to_string()).await?)
        } else {
            None
        };
        let (write, outcome) = crate::edit::plan_save(base, ours, theirs.as_deref());
        if let Some(content) = write {
            // `create` tronque le fichier (`write` de russh-sftp ne le fait pas).
            let mut target = sftp.create(remote.to_string()).await?;
            target.write_all(&content).await?;
            target.shutdown().await?;
            update.done = content.len() as u64;
        }
        Ok(outcome)
    }
    .await;
    match result {
        Ok(outcome) => {
            finish_transfer(update, Ok(()), ui);
            Ok(outcome)
        }
        Err(e) => Err(finish_transfer(update, Err(e), ui).unwrap_or_default()),
    }
}

async fn copy_with_progress<R, W>(
    source: &mut R,
    target: &mut W,
    update: &mut TransferUpdate,
    ui: &UiLink,
) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0u8; 256 * 1024];
    let mut last_report = Instant::now();
    ui.send(Event::Transfer(update.clone()));
    loop {
        let n = source.read(&mut buffer).await?;
        if n == 0 {
            return Ok(());
        }
        target.write_all(&buffer[..n]).await?;
        update.done += n as u64;
        if last_report.elapsed() > Duration::from_millis(200) {
            last_report = Instant::now();
            ui.send(Event::Transfer(update.clone()));
        }
    }
}

fn finish_transfer(mut update: TransferUpdate, result: anyhow::Result<()>, ui: &UiLink) -> Option<String> {
    update.finished = true;
    update.error = result.err().map(|e| describe_error(&e));
    let error = update.error.clone();
    ui.send(Event::Transfer(update));
    error
}

fn describe_error(error: &anyhow::Error) -> String {
    if let Some(russh::Error::UnknownKey) = error.downcast_ref::<russh::Error>() {
        return "clé de l'hôte refusée".into();
    }
    format!("{error:#}")
}
