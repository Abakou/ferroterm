//! Connexions FTP et FTPS : un explorateur de fichiers, sans terminal.
//!
//! Même interface que les connexions SSH (`Connection`, `SftpCommand`, `Event`) :
//! l'explorateur, les transferts et l'édition de fichiers distants marchent à
//! l'identique.
//!
//! - La connexion de contrôle sert à la navigation (liste, création,
//!   suppression). Un NOOP régulier évite que le serveur la ferme ; si elle tombe
//!   quand même, elle est rétablie sans rien demander.
//! - Chaque transfert ouvre sa propre connexion, comme FileZilla : la navigation
//!   reste possible pendant les envois et téléchargements ([`MAX_TRANSFERS`] au
//!   plus en même temps, les serveurs limitant souvent les connexions par adresse).
//! - FTPS : le certificat est vérifié avec les autorités du système et celles de
//!   Mozilla. Un certificat non reconnu (auto-signé…) est soumis à l'utilisateur
//!   avec son empreinte, comme une clé d'hôte SSH inconnue, et retenu s'il est
//!   accepté (fichier `known_certificates`, à côté des sessions).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context as _, anyhow, bail};
use eframe::egui;
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use suppaftp::list::File;
use suppaftp::tokio::{AsyncRustlsConnector, AsyncRustlsFtpStream};
use suppaftp::types::FileType;
use suppaftp::{FtpError, Mode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex as AsyncMutex, Semaphore, mpsc};

use crate::session::{FtpTls, Session};
use crate::ssh::{
    CONNECT_TIMEOUT, Command, Connection, ConnectionParts, Event, RemoteEntry, Replies, Reply, SftpCommand,
    TransferUpdate, UiLink, ask_secret, ask_text, copy_with_progress, describe_error, finish_transfer, join_remote,
    update_vault,
};
use crate::vault;

/// Le même type sert en clair et en TLS : le chiffrement est choisi à la connexion.
type Ftp = AsyncRustlsFtpStream;

/// Transferts simultanés, chacun sur sa propre connexion.
const MAX_TRANSFERS: usize = 2;
/// Délai entre deux NOOP sur la connexion de contrôle inactive.
const KEEPALIVE: Duration = Duration::from_secs(60);
/// Délai maximal d'une commande de navigation (liste comprise).
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// Mot de passe d'usage pour une connexion anonyme.
const ANONYMOUS_PASSWORD: &str = "anonymous@";

pub fn spawn(rt: &tokio::runtime::Runtime, session: Session, ctx: egui::Context) -> Connection {
    let (conn, parts) = Connection::new_parts(ctx);
    // Pas de terminal : l'analyseur de la connexion reste inutilisé.
    let ConnectionParts { ui, commands, replies, .. } = parts;
    rt.spawn(async move {
        let replies = Arc::new(AsyncMutex::new(replies));
        let result = run(session, ui.clone(), commands, replies).await;
        ui.send(Event::Closed(result.err().map(|e| describe_error(&e))));
    });
    conn
}

async fn run(
    session: Session,
    ui: UiLink,
    mut commands: mpsc::UnboundedReceiver<Command>,
    replies: Replies,
) -> anyhow::Result<()> {
    ui.status(format!("Connexion à {}…", session.target()));
    let mut endpoint = Endpoint {
        host: session.host.clone(),
        port: session.port,
        tls: session.ftp.tls,
        active_mode: session.ftp.active_mode,
        trusted: known_certificate(&session.host, session.port),
    };
    // Le serveur (et son certificat) est vérifié avant de demander quoi que ce soit.
    let pending = connect_trusting(&mut endpoint, &ui, &replies).await?;
    let (login, ftp) = authenticate(&session, endpoint, pending, &ui, &replies).await?;
    let login = Arc::new(login);
    let mut control = Control::new(ftp).await?;
    ui.send(Event::Connected);
    // Après `Connected` : reste affiché dans la barre d'état.
    if login.endpoint.tls == FtpTls::Off {
        ui.status("Connexion FTP non chiffrée");
    }
    let home = control.ftp.pwd().await.unwrap_or_else(|_| "/".into());
    ui.send(Event::SftpReady { home });

    let slots = Arc::new(Semaphore::new(MAX_TRANSFERS));
    let mut next_transfer = 0u64;
    let mut keepalive = tokio::time::interval_at(tokio::time::Instant::now() + KEEPALIVE, KEEPALIVE);
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Disconnect) | None => break,
                // Pas de terminal : saisie et redimensionnement sont sans objet.
                Some(Command::Input(_)) | Some(Command::Resize { .. }) => {}
                Some(Command::Sftp(command)) => {
                    keepalive.reset();
                    match command {
                        SftpCommand::List(_) | SftpCommand::Mkdir(_) | SftpCommand::Remove { .. } => {
                            control_command(&mut control, &login, command, &ui).await?;
                        }
                        transfer => {
                            next_transfer += 1;
                            spawn_transfer(transfer, next_transfer, login.clone(), slots.clone(), ui.clone());
                        }
                    }
                }
            },
            _ = keepalive.tick() => {
                let alive = tokio::time::timeout(COMMAND_TIMEOUT, control.ftp.noop()).await;
                if !matches!(alive, Ok(Ok(()))) {
                    ui.status("Connexion de contrôle perdue, reconnexion…");
                    control = login.connect().await?;
                }
            }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), control.ftp.quit()).await;
    Ok(())
}

// ----------------------------------------------------------------------
// Connexion et authentification
// ----------------------------------------------------------------------

/// Où et comment se connecter, sans les identifiants.
#[derive(Clone)]
struct Endpoint {
    host: String,
    port: u16,
    tls: FtpTls,
    active_mode: bool,
    /// Empreinte du certificat que l'utilisateur a accepté pour ce serveur
    /// (utile seulement si le certificat n'est pas reconnu autrement).
    trusted: Option<String>,
}

enum OpenError {
    /// Certificat non reconnu : à soumettre à l'utilisateur.
    Certificate { reason: String, fingerprint: String },
    Other(anyhow::Error),
}

impl OpenError {
    fn into_anyhow(self) -> anyhow::Error {
        match self {
            OpenError::Certificate { reason, .. } => anyhow!("certificat du serveur non reconnu : {reason}"),
            OpenError::Other(e) => e,
        }
    }
}

impl Endpoint {
    /// Ouvre la connexion de contrôle, chiffrée selon les réglages. Renvoie
    /// aussi si elle est chiffrée (en mode `Auto`, le serveur peut refuser TLS).
    async fn open(&self) -> Result<(Ftp, bool), OpenError> {
        let rejected = Arc::new(Mutex::new(None));
        let result = tokio::time::timeout(CONNECT_TIMEOUT, self.open_with(&rejected))
            .await
            .unwrap_or_else(|_| Err(anyhow!("délai de connexion dépassé ({} s)", CONNECT_TIMEOUT.as_secs())));
        result.map_err(|e| match rejected.lock().expect("certificat refusé").take() {
            Some((reason, fingerprint)) => OpenError::Certificate { reason, fingerprint },
            None => OpenError::Other(e),
        })
    }

    async fn open_with(&self, rejected: &Rejected) -> anyhow::Result<(Ftp, bool)> {
        let address = (self.host.as_str(), self.port);
        let unreachable = || format!("impossible de joindre {}:{}", self.host, self.port);
        let (mut ftp, encrypted) = if self.tls == FtpTls::Implicit {
            let connector = self.tls_connector(rejected)?;
            (Ftp::connect_secure_implicit(address, connector, &self.host).await.ftp().with_context(unreachable)?, true)
        } else {
            let stream = tokio::net::TcpStream::connect(address).await.with_context(unreachable)?;
            let _ = stream.set_nodelay(true);
            let ftp = Ftp::connect_with_stream(stream).await.ftp()?;
            if self.tls == FtpTls::Off {
                (ftp, false)
            } else {
                match ftp.into_secure(self.tls_connector(rejected)?, &self.host).await {
                    Ok(ftp) => (ftp, true),
                    // AUTH TLS refusé : le serveur ne propose pas TLS. En clair si c'est
                    // permis, sur une nouvelle connexion (l'ancienne a été consommée).
                    Err(FtpError::UnexpectedResponse(_)) if self.tls == FtpTls::Auto => {
                        let plain = Endpoint { tls: FtpTls::Off, ..self.clone() };
                        return Box::pin(plain.open_with(rejected)).await;
                    }
                    Err(FtpError::UnexpectedResponse(r)) => {
                        bail!("le serveur refuse TLS (« {} »)", response_text(&r))
                    }
                    Err(e) => return Err(FtpFailure(e).into()),
                }
            }
        };
        // Passif par défaut, avec l'adresse de la connexion de contrôle plutôt
        // que celle annoncée (souvent privée quand le serveur est derrière un NAT).
        ftp.set_mode(Mode::Passive);
        ftp.set_passive_nat_workaround(true);
        if self.active_mode {
            ftp = ftp.active_mode(Duration::from_secs(30));
        }
        Ok((ftp, encrypted))
    }

    fn tls_connector(&self, rejected: &Rejected) -> anyhow::Result<AsyncRustlsConnector> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let webpki = WebPkiServerVerifier::builder_with_provider(roots(), provider.clone())
            .build()
            .context("vérification des certificats")?;
        let verifier = Verifier { webpki, trusted: self.trusted.clone(), rejected: rejected.clone() };
        // Une seule configuration par connexion : la session TLS de la connexion
        // de contrôle est reprise sur les connexions de données, ce qu'exigent
        // certains serveurs (vsftpd, `require_ssl_reuse`).
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .context("configuration TLS")?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        Ok(AsyncRustlsConnector::from(tokio_rustls::TlsConnector::from(Arc::new(config))))
    }
}

/// Ouvre une connexion ; un certificat non reconnu est soumis à l'utilisateur,
/// puis retenu s'il l'accepte.
async fn connect_trusting(endpoint: &mut Endpoint, ui: &UiLink, replies: &Replies) -> anyhow::Result<Ftp> {
    loop {
        match endpoint.open().await {
            Ok((ftp, encrypted)) => {
                if endpoint.tls == FtpTls::Auto && !encrypted {
                    ui.status("Le serveur ne propose pas TLS : connexion non chiffrée.");
                    // Les connexions suivantes (transferts) ne retentent pas TLS.
                    endpoint.tls = FtpTls::Off;
                }
                return Ok(ftp);
            }
            Err(OpenError::Certificate { reason, fingerprint }) => {
                ui.send(Event::AskCertificate {
                    reason,
                    fingerprint: fingerprint.clone(),
                    changed: endpoint.trusted.is_some(),
                });
                let accepted = matches!(replies.lock().await.recv().await, Some(Reply::HostKey(true)));
                if !accepted {
                    bail!("certificat du serveur refusé");
                }
                if let Err(e) = remember_certificate(&endpoint.host, endpoint.port, &fingerprint) {
                    ui.status(format!("Certificat accepté mais non enregistré ({e:#})"));
                }
                endpoint.trusted = Some(fingerprint);
            }
            Err(OpenError::Other(e)) => return Err(e),
        }
    }
}

/// Identifiants retenus après une connexion réussie, pour ouvrir les
/// connexions suivantes (transferts, reconnexion) sans rien redemander.
struct Login {
    endpoint: Endpoint,
    user: String,
    password: String,
}

impl Login {
    /// Nouvelle connexion authentifiée et prête à l'emploi.
    async fn connect(&self) -> anyhow::Result<Control> {
        let (mut ftp, _) = self.endpoint.open().await.map_err(OpenError::into_anyhow)?;
        ftp.login(self.user.as_str(), self.password.as_str()).await.ftp()?;
        Control::new(ftp).await
    }
}

fn is_anonymous(user: &str) -> bool {
    user.eq_ignore_ascii_case("anonymous") || user.eq_ignore_ascii_case("ftp")
}

/// Mêmes étapes que pour SSH : mot de passe enregistré, puis saisie (trois
/// essais), avec la case « Enregistrer ».
async fn authenticate(
    session: &Session,
    mut endpoint: Endpoint,
    pending: Ftp,
    ui: &UiLink,
    replies: &Replies,
) -> anyhow::Result<(Login, Ftp)> {
    let mut pending = Some(pending);
    let mut user = session.username.clone();
    if user.is_empty() {
        user = ask_text(ui, replies, "Nom d'utilisateur (anonymous pour une connexion anonyme)", false)
            .await?
            .map(|u| u.trim().to_string())
            .ok_or_else(|| anyhow!("connexion annulée"))?;
        if user.is_empty() {
            user = "anonymous".into();
        }
    }

    let saved = (session.id != 0).then_some(vault::Secret::Password(session.id, session.profile));
    let saved_password = saved.as_ref().and_then(vault::get);
    if saved_password.is_none() && is_anonymous(&user) {
        ui.status("Connexion anonyme…");
        if let Some(ftp) = try_login(&mut endpoint, &mut pending, &user, ANONYMOUS_PASSWORD, ui, replies).await? {
            return Ok((Login { endpoint, user, password: ANONYMOUS_PASSWORD.into() }, ftp));
        }
    }
    let mut stale = false;
    if let Some(password) = saved_password {
        ui.status("Authentification avec le mot de passe enregistré…");
        if let Some(ftp) = try_login(&mut endpoint, &mut pending, &user, &password, ui, replies).await? {
            return Ok((Login { endpoint, user, password }, ftp));
        }
        stale = true;
    }

    let host = &session.host;
    for attempt in 0..3 {
        let prompt = match (attempt, stale) {
            (0, true) => format!("Le mot de passe enregistré a été refusé. Mot de passe pour {user}@{host}"),
            (0, false) => format!("Mot de passe pour {user}@{host}"),
            _ => format!("Mot de passe refusé. Mot de passe pour {user}@{host}"),
        };
        let (password, remember) = match &saved {
            Some(_) => ask_secret(ui, replies, &prompt, stale).await?,
            None => ask_text(ui, replies, &prompt, true).await?.map(|p| (p, false)),
        }
        .ok_or_else(|| anyhow!("connexion annulée"))?;
        ui.status("Authentification…");
        if let Some(ftp) = try_login(&mut endpoint, &mut pending, &user, &password, ui, replies).await? {
            if let Some(secret) = &saved {
                update_vault(ui, secret, &user, &password, remember, stale);
            }
            return Ok((Login { endpoint, user, password }, ftp));
        }
    }
    bail!("authentification refusée")
}

/// Essaie des identifiants : `None` s'ils sont refusés. Utilise la connexion
/// déjà ouverte s'il y en a une ; sinon (ou si elle a expiré pendant la
/// saisie du mot de passe), en ouvre une nouvelle.
async fn try_login(
    endpoint: &mut Endpoint,
    pending: &mut Option<Ftp>,
    user: &str,
    password: &str,
    ui: &UiLink,
    replies: &Replies,
) -> anyhow::Result<Option<Ftp>> {
    loop {
        let reused = pending.is_some();
        let mut ftp = match pending.take() {
            Some(ftp) => ftp,
            None => connect_trusting(endpoint, ui, replies).await?,
        };
        match ftp.login(user, password).await {
            Ok(()) => return Ok(Some(ftp)),
            // 530 : identifiants refusés ; 430 (non standard) ; 332 : compte exigé.
            Err(FtpError::UnexpectedResponse(r)) if matches!(r.status.code(), 530 | 430 | 332) => return Ok(None),
            Err(e) => {
                let error: anyhow::Error = FtpFailure(e).into();
                if !(reused && is_lost(&error)) {
                    return Err(error);
                }
                // Connexion fermée par le serveur pendant la saisie : on recommence.
            }
        }
    }
}

// ----------------------------------------------------------------------
// Navigation sur la connexion de contrôle
// ----------------------------------------------------------------------

/// Connexion authentifiée, avec ce que le serveur sait faire.
struct Control {
    ftp: Ftp,
    /// Le serveur sait lister au format MLSD (lisible sans ambiguïté).
    mlsd: bool,
}

impl Control {
    /// Réglages de chaque connexion après authentification : transferts
    /// binaires (sans conversion des fins de ligne), noms de fichiers en UTF-8.
    async fn new(mut ftp: Ftp) -> anyhow::Result<Self> {
        ftp.transfer_type(FileType::Binary).await.ftp()?;
        // FEAT est facultatif (serveurs anciens) : sans réponse, LIST et l'encodage par défaut.
        let features = ftp.feat().await.unwrap_or_default();
        let has = |name: &str| features.keys().any(|k| k.eq_ignore_ascii_case(name));
        if has("UTF8") {
            let _ = ftp.opts("UTF8", Some("ON")).await;
        }
        Ok(Self { mlsd: has("MLST"), ftp })
    }
}

/// Exécute une commande de navigation, en rétablissant une fois la connexion
/// si elle a été perdue. Les erreurs de la commande sont signalées à
/// l'explorateur ; seule l'impossibilité de se reconnecter met fin à la session.
async fn control_command(control: &mut Control, login: &Login, command: SftpCommand, ui: &UiLink) -> anyhow::Result<()> {
    let mut result = run_control(control, &command).await;
    if result.as_ref().is_err_and(is_lost) {
        // Une connexion interrompue peut être celle des données (liste) : si la
        // connexion de contrôle répond encore, c'est elle qui a échoué.
        let data_failed = result.as_ref().is_err_and(|e| !is_timeout(e))
            && matches!(tokio::time::timeout(Duration::from_secs(10), control.ftp.noop()).await, Ok(Ok(())));
        if data_failed {
            result = result.context(if login.endpoint.active_mode {
                "connexion de données impossible (pare-feu ? essayez sans le mode actif)"
            } else {
                "connexion de données impossible (pare-feu ? essayez le mode actif dans la session)"
            });
        } else {
            ui.status("Connexion de contrôle perdue, reconnexion…");
            *control = login.connect().await?;
            result = run_control(control, &command).await;
        }
    }
    match (result, command) {
        (Ok(Some(event)), _) => ui.send(event),
        (Ok(None), _) => {}
        (Err(e), SftpCommand::List(path)) => ui.send(Event::SftpError(format!("{path} : {}", describe_error(&e)))),
        (Err(e), SftpCommand::Mkdir(path)) => {
            ui.send(Event::SftpError(format!("Création de {path} impossible : {}", describe_error(&e))))
        }
        (Err(e), SftpCommand::Remove { path, .. }) => {
            ui.send(Event::SftpError(format!("Suppression de {path} impossible : {}", describe_error(&e))))
        }
        (Err(e), _) => ui.send(Event::SftpError(describe_error(&e))),
    }
    Ok(())
}

async fn run_control(control: &mut Control, command: &SftpCommand) -> anyhow::Result<Option<Event>> {
    let work = async {
        match command {
            SftpCommand::List(path) => {
                let (path, entries) = list_dir(control, path).await?;
                Ok(Some(Event::SftpListing { path, entries }))
            }
            SftpCommand::Mkdir(path) => {
                control.ftp.mkdir(path).await.ftp()?;
                Ok(None)
            }
            SftpCommand::Remove { path, is_dir: true } => {
                control.ftp.rmdir(path).await.ftp()?;
                Ok(None)
            }
            SftpCommand::Remove { path, is_dir: false } => {
                control.ftp.rm(path).await.ftp()?;
                Ok(None)
            }
            _ => Ok(None),
        }
    };
    match tokio::time::timeout(COMMAND_TIMEOUT, work).await {
        Ok(result) => result,
        // Connexion dans un état inconnu : considérée comme perdue.
        Err(_) => Err(FtpFailure(FtpError::ConnectionError(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("le serveur ne répond pas ({} s)", COMMAND_TIMEOUT.as_secs()),
        )))
        .into()),
    }
}

async fn list_dir(control: &mut Control, path: &str) -> anyhow::Result<(String, Vec<RemoteEntry>)> {
    let ftp = &mut control.ftp;
    ftp.cwd(path).await.ftp()?;
    let path = ftp.pwd().await.ftp()?;
    let lines = if control.mlsd { ftp.mlsd(None).await } else { ftp.list(None).await }.ftp()?;
    let mut entries: Vec<RemoteEntry> = lines.iter().filter_map(|line| parse_entry(line)).collect();
    // Liens symboliques : un dossier si l'on peut y entrer.
    let mut moved = false;
    for entry in entries.iter_mut().filter(|e| e.is_symlink && !e.is_dir) {
        if ftp.cwd(join_remote(&path, &entry.name)).await.is_ok() {
            entry.is_dir = true;
            moved = true;
        }
    }
    if moved {
        let _ = ftp.cwd(&path).await;
    }
    sort_entries(&mut entries);
    Ok((path, entries))
}

/// Une ligne de MLSD ou de LIST (format Unix ou Windows) ; `None` pour les
/// lignes sans fichier (« total 12 », « . », « .. ») ou illisibles.
fn parse_entry(line: &str) -> Option<RemoteEntry> {
    // En-tête de LIST sous Unix, que suppaftp prendrait pour un fichier « total 12 ».
    if line.strip_prefix("total ").is_some_and(|n| n.trim().chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let file = File::try_from(line).ok()?;
    let name = file.name().to_string();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    let mtime = file
        .modified()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u32::try_from(d.as_secs()).ok())
        .filter(|&secs| secs > 0);
    Some(RemoteEntry {
        name,
        is_dir: file.is_directory(),
        is_symlink: file.is_symlink(),
        size: file.size() as u64,
        mtime,
    })
}

/// Dossiers d'abord, puis par nom (comme l'explorateur SFTP).
fn sort_entries(entries: &mut [RemoteEntry]) {
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
}

// ----------------------------------------------------------------------
// Transferts, chacun sur sa propre connexion
// ----------------------------------------------------------------------

fn spawn_transfer(command: SftpCommand, id: u64, login: Arc<Login>, slots: Arc<Semaphore>, ui: UiLink) {
    tokio::spawn(async move {
        match command {
            SftpCommand::Download { remote, local } => {
                let _ = download(&login, &slots, &remote, &local, &ui, id).await;
            }
            SftpCommand::Upload { local, remote_dir } => {
                let name = local.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let remote = join_remote(&remote_dir, &name);
                let _ = upload(&login, &slots, &local, &remote, &ui, id).await;
            }
            SftpCommand::FetchForEdit { remote, local } => {
                let error = download(&login, &slots, &remote, &local, &ui, id).await;
                ui.send(Event::EditReady { remote, local, error });
            }
            SftpCommand::SaveEdited { remote, base, ours } => {
                let result = save_edited(&login, &slots, &remote, &base, &ours, &ui, id).await;
                ui.send(Event::EditSaved { remote, result });
            }
            SftpCommand::List(_) | SftpCommand::Mkdir(_) | SftpCommand::Remove { .. } => {}
        }
    });
}

fn transfer(id: u64, remote: &str, upload: bool, total: Option<u64>) -> TransferUpdate {
    let name = remote.rsplit('/').next().unwrap_or(remote);
    TransferUpdate {
        id,
        label: format!("{} {name}", if upload { "⬆" } else { "⬇" }),
        upload,
        done: 0,
        total,
        finished: false,
        error: None,
    }
}

/// Télécharge `remote` dans `local` ; renvoie l'erreur éventuelle (déjà
/// signalée dans la liste des transferts).
async fn download(
    login: &Login,
    slots: &Semaphore,
    remote: &str,
    local: &PathBuf,
    ui: &UiLink,
    id: u64,
) -> Option<String> {
    let mut update = transfer(id, remote, false, None);
    // Affiché tout de suite, même en attente d'une place.
    ui.send(Event::Transfer(update.clone()));
    let result: anyhow::Result<()> = async {
        let _slot = slots.acquire().await?;
        let mut ftp = login.connect().await?.ftp;
        update.total = ftp.size(remote).await.ok().map(|n| n as u64);
        let mut source = ftp.retr_as_stream(remote).await.ftp()?;
        let mut target = tokio::fs::File::create(local)
            .await
            .with_context(|| format!("création de {}", local.display()))?;
        copy_with_progress(&mut source, &mut target, &mut update, ui).await?;
        target.flush().await?;
        source.finish().await.ftp()?;
        let _ = ftp.quit().await;
        Ok(())
    }
    .await;
    finish_transfer(update, result, ui)
}

/// Envoie `local` à l'emplacement exact `remote` (remplacé s'il existe).
async fn upload(
    login: &Login,
    slots: &Semaphore,
    local: &PathBuf,
    remote: &str,
    ui: &UiLink,
    id: u64,
) -> Option<String> {
    let mut update = transfer(id, remote, true, None);
    let result: anyhow::Result<()> = async {
        let meta = tokio::fs::metadata(local).await?;
        if meta.is_dir() {
            bail!("l'envoi de dossiers n'est pas encore pris en charge");
        }
        update.total = Some(meta.len());
        ui.send(Event::Transfer(update.clone()));
        let _slot = slots.acquire().await?;
        let mut ftp = login.connect().await?.ftp;
        let mut source = tokio::fs::File::open(local).await?;
        let mut target = ftp.put_with_stream(remote).await.ftp()?;
        copy_with_progress(&mut source, &mut target, &mut update, ui).await?;
        target.finish().await.ftp()?;
        let _ = ftp.quit().await;
        Ok(())
    }
    .await;
    finish_transfer(update, result, ui)
}

/// Renvoi d'un fichier édité : relit la version du serveur, la fusionne avec
/// la version locale (base = dernier contenu connu du serveur), puis écrit le
/// résultat s'il y a lieu. Même logique que pour SFTP.
async fn save_edited(
    login: &Login,
    slots: &Semaphore,
    remote: &str,
    base: &[u8],
    ours: &[u8],
    ui: &UiLink,
    id: u64,
) -> Result<crate::edit::SaveOutcome, String> {
    let mut update = transfer(id, remote, true, Some(ours.len() as u64));
    ui.send(Event::Transfer(update.clone()));
    let result: anyhow::Result<crate::edit::SaveOutcome> = async {
        let _slot = slots.acquire().await?;
        let mut ftp = login.connect().await?.ftp;
        let theirs = read_remote(&mut ftp, remote).await?;
        let (write, outcome) = crate::edit::plan_save(base, ours, theirs.as_deref());
        if let Some(content) = write {
            let mut target = ftp.put_with_stream(remote).await.ftp()?;
            target.write_all(&content).await?;
            target.finish().await.ftp()?;
            update.done = content.len() as u64;
        }
        let _ = ftp.quit().await;
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

/// Contenu actuel d'un fichier distant ; `None` s'il n'existe pas (550).
async fn read_remote(ftp: &mut Ftp, remote: &str) -> anyhow::Result<Option<Vec<u8>>> {
    let mut stream = match ftp.retr_as_stream(remote).await {
        Ok(stream) => stream,
        Err(FtpError::UnexpectedResponse(r)) if r.status.code() == 550 => return Ok(None),
        Err(e) => return Err(FtpFailure(e).into()),
    };
    let mut content = Vec::new();
    stream.read_to_end(&mut content).await?;
    stream.finish().await.ftp()?;
    Ok(Some(content))
}

// ----------------------------------------------------------------------
// Erreurs
// ----------------------------------------------------------------------

/// Erreur FTP, avec un message lisible ; reste accessible par `downcast_ref`
/// pour reconnaître une connexion perdue.
#[derive(Debug)]
struct FtpFailure(FtpError);

impl std::fmt::Display for FtpFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            FtpError::UnexpectedResponse(r) => write!(f, "le serveur a répondu « {} »", response_text(r)),
            FtpError::ConnectionError(e) => write!(f, "connexion interrompue ({e})"),
            FtpError::SecureError(e) => write!(f, "échec de la négociation TLS ({e})"),
            FtpError::BadResponse => write!(f, "réponse du serveur illisible"),
            FtpError::InvalidAddress(e) => write!(f, "adresse invalide ({e})"),
            FtpError::DataConnectionAlreadyOpen => write!(f, "un transfert est déjà en cours"),
        }
    }
}

impl std::error::Error for FtpFailure {}

trait FtpResultExt<T> {
    /// Convertit une erreur FTP en erreur lisible.
    fn ftp(self) -> anyhow::Result<T>;
}

impl<T> FtpResultExt<T> for Result<T, FtpError> {
    fn ftp(self) -> anyhow::Result<T> {
        self.map_err(|e| FtpFailure(e).into())
    }
}

/// Texte d'une réponse du serveur, code compris, sur une ligne.
fn response_text(response: &suppaftp::types::Response) -> String {
    let body = String::from_utf8_lossy(&response.body);
    let text = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let code = response.status.code().to_string();
    if text.starts_with(&code) { text } else { format!("{code} {text}").trim_end().to_string() }
}

/// Commande restée sans réponse (voir `run_control`) : la connexion est dans
/// un état inconnu.
fn is_timeout(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<FtpFailure>(),
        Some(FtpFailure(FtpError::ConnectionError(e))) if e.kind() == std::io::ErrorKind::TimedOut
    )
}

/// Vrai si la connexion de contrôle est inutilisable (fermée, sans réponse,
/// ou « 421 service non disponible »).
fn is_lost(error: &anyhow::Error) -> bool {
    match error.downcast_ref::<FtpFailure>() {
        Some(FtpFailure(FtpError::ConnectionError(_) | FtpError::BadResponse)) => true,
        Some(FtpFailure(FtpError::UnexpectedResponse(r))) => r.status.code() == 421,
        _ => false,
    }
}

// ----------------------------------------------------------------------
// Certificats FTPS
// ----------------------------------------------------------------------

/// Dernier certificat refusé par le vérificateur : (raison, empreinte).
type Rejected = Arc<Mutex<Option<(String, String)>>>;

/// Autorités de certification reconnues : celles de Mozilla et celles du
/// système (dont les autorités d'entreprise). Chargées une fois.
fn roots() -> Arc<RootCertStore> {
    static ROOTS: OnceLock<Arc<RootCertStore>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let mut roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
            roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
            Arc::new(roots)
        })
        .clone()
}

/// Vérification habituelle (autorité, nom, validité) ; en cas d'échec, le
/// certificat est tout de même accepté si l'utilisateur l'a approuvé
/// (même empreinte), sinon refusé et noté pour lui être soumis.
#[derive(Debug)]
struct Verifier {
    webpki: Arc<WebPkiServerVerifier>,
    trusted: Option<String>,
    rejected: Rejected,
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let error = match self.webpki.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now) {
            Ok(verified) => return Ok(verified),
            Err(e) => e,
        };
        let fingerprint = fingerprint(end_entity);
        if self.trusted.as_deref() == Some(fingerprint.as_str()) {
            return Ok(ServerCertVerified::assertion());
        }
        *self.rejected.lock().expect("certificat refusé") = Some((describe_certificate_error(&error), fingerprint));
        Err(error)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.webpki.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.webpki.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.webpki.supported_verify_schemes()
    }
}

/// Empreinte SHA-256 d'un certificat, en hexadécimal séparé par des « : »
/// (comme l'affichent les navigateurs).
fn fingerprint(cert: &CertificateDer<'_>) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, cert.as_ref());
    digest.as_ref().iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// Pourquoi le certificat n'est pas reconnu, en complément de « Le certificat
/// de ce serveur n'est pas reconnu : … ».
fn describe_certificate_error(error: &rustls::Error) -> String {
    match error {
        rustls::Error::InvalidCertificate(e) => match e {
            CertificateError::UnknownIssuer => {
                "il n'est pas signé par une autorité reconnue (certificat auto-signé ?)".into()
            }
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => {
                "il n'a pas été établi pour ce nom d'hôte".into()
            }
            CertificateError::Expired | CertificateError::ExpiredContext { .. } => "il a expiré".into(),
            CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => {
                "il n'est pas encore valable".into()
            }
            CertificateError::Revoked => "il a été révoqué".into(),
            // Certificat auto-signé fait avec `openssl req -x509` : marqué « autorité ».
            other if format!("{other:?}").contains("CaUsedAsEndEntity") => {
                "il n'est pas signé par une autorité reconnue (certificat auto-signé)".into()
            }
            other => format!("il est invalide ({other:?})"),
        },
        other => format!("{other}"),
    }
}

/// Certificats approuvés, un par ligne : `hôte:port empreinte`.
fn known_certificates_path() -> Option<PathBuf> {
    crate::session::SessionStore::default_path().and_then(|p| p.parent().map(|dir| dir.join("known_certificates")))
}

fn certificate_key(host: &str, port: u16) -> String {
    format!("{}:{port}", host.to_lowercase())
}

fn known_certificate(host: &str, port: u16) -> Option<String> {
    let text = std::fs::read_to_string(known_certificates_path()?).ok()?;
    lookup_certificate(&text, &certificate_key(host, port))
}

fn lookup_certificate(text: &str, key: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| line.trim().split_once(' '))
        .filter(|(k, _)| *k == key)
        .map(|(_, fp)| fp.trim().to_string())
        .next_back()
}

fn remember_certificate(host: &str, port: u16, fingerprint: &str) -> anyhow::Result<()> {
    let path = known_certificates_path().ok_or_else(|| anyhow!("aucun dossier de configuration disponible"))?;
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, with_certificate(&text, &certificate_key(host, port), fingerprint))?;
    Ok(())
}

/// Le fichier avec l'empreinte de `key` remplacée (ou ajoutée).
fn with_certificate(text: &str, key: &str, fingerprint: &str) -> String {
    let mut lines: Vec<String> = text
        .lines()
        .filter(|line| line.trim().split_once(' ').map(|(k, _)| k) != Some(key) && !line.trim().is_empty())
        .map(str::to_string)
        .collect();
    lines.push(format!("{key} {fingerprint}"));
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unix_windows_and_mlsd_listings() {
        let lines = [
            "total 12",
            "drwxr-xr-x    2 ftp      ftp          4096 Nov 05  2018 pub",
            "-rw-r--r--    1 ftp      ftp          1234 Nov 05  2018 readme.txt",
            "lrwxrwxrwx    1 ftp      ftp             3 Nov 05  2018 www -> pub",
            "04-08-14  03:09PM       <DIR>          Docs",
            "04-08-14  03:09PM                 8192 omar.txt",
            "type=cdir;modify=20181105163248; .",
            "type=file;size=42;modify=20181105163248; notes.md",
            "type=dir;modify=20181105163248; src",
        ];
        let mut entries: Vec<RemoteEntry> = lines.iter().filter_map(|l| parse_entry(l)).collect();
        sort_entries(&mut entries);
        let summary: Vec<_> = entries.iter().map(|e| (e.name.as_str(), e.is_dir, e.is_symlink, e.size)).collect();
        assert_eq!(
            summary,
            vec![
                ("Docs", true, false, 0),
                ("pub", true, false, 4096),
                ("src", true, false, 0),
                ("notes.md", false, false, 42),
                ("omar.txt", false, false, 8192),
                ("readme.txt", false, false, 1234),
                ("www", false, true, 3),
            ]
        );
        assert!(entries.iter().find(|e| e.name == "notes.md").unwrap().mtime.is_some());
    }

    #[test]
    fn certificates_are_replaced_per_host_and_port() {
        let text = with_certificate("", "ftp.example.org:21", "AA:BB");
        let text = with_certificate(&text, "ftp.example.org:990", "CC:DD");
        let text = with_certificate(&text, "ftp.example.org:21", "EE:FF");
        assert_eq!(lookup_certificate(&text, "ftp.example.org:21").as_deref(), Some("EE:FF"));
        assert_eq!(lookup_certificate(&text, "ftp.example.org:990").as_deref(), Some("CC:DD"));
        assert_eq!(lookup_certificate(&text, "autre:21"), None);
        assert_eq!(text.lines().count(), 2);
        assert_eq!(certificate_key("FTP.Example.org", 21), "ftp.example.org:21");
    }

    /// Essai complet sur un vrai serveur, par la même voie que l'interface
    /// (événements, questions, commandes de l'explorateur) :
    ///
    /// `FERROTERM_FTP_TEST=ftp://utilisateur:motdepasse@hôte:port/dossier cargo test real_server -- --ignored --nocapture`
    ///
    /// `ftp://` : TLS si disponible, `ftpes://` : TLS explicite obligatoire,
    /// `ftps://` : TLS implicite, `sftp://` : session SFTP seule (par SSH ; une
    /// clé d'hôte inconnue est acceptée et ajoutée à `~/.ssh/known_hosts`).
    /// Le dossier doit être accessible en écriture :
    /// un fichier et un dossier y sont créés puis supprimés. Un certificat non
    /// reconnu est accepté (et retenu dans un dossier temporaire, pas dans la
    /// configuration de l'utilisateur).
    #[test]
    #[ignore]
    fn real_server() {
        let url = std::env::var("FERROTERM_FTP_TEST").expect("FERROTERM_FTP_TEST");
        let (session, password, dir) = session_from_url(&url);
        let scratch = std::env::temp_dir().join(format!("ferroterm-ftp-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        // SAFETY : seul test à lire ou modifier cette variable.
        unsafe { std::env::set_var("FERROTERM_SESSIONS", scratch.join("sessions.json")) };
        let rt = tokio::runtime::Runtime::new().unwrap();

        let conn = open(&rt, session.clone());
        let mut asked_certificate = false;
        let mut next = |conn: &Connection| next_event(conn, &password, &mut asked_certificate);
        let home = loop {
            if let Event::SftpReady { home } = next(&conn) {
                break home;
            }
        };
        println!("connecté, dossier de départ : {home}");
        let list = |conn: &Connection, next: &mut dyn FnMut(&Connection) -> Event, path: &str| {
            conn.send(Command::Sftp(SftpCommand::List(path.to_string())));
            loop {
                if let Event::SftpListing { path, entries } = next(conn) {
                    return (path, entries);
                }
            }
        };
        let (cwd, entries) = list(&conn, &mut next, if dir.is_empty() { &home } else { &dir });
        println!("{cwd} : {} entrées", entries.len());
        for e in entries.iter().take(15) {
            println!("  {}{} {} o", e.name, if e.is_dir { "/" } else { "" }, e.size);
        }

        // Envoi puis téléchargement : octets identiques (mode binaire, UTF-8).
        let content: Vec<u8> = [&b"ligne 1\nligne \xc3\xa9t\xc3\xa9 2\r\n"[..], &[0, 255, 13, 10, 26]].concat();
        let local = scratch.join("ferroterm-essai.txt");
        std::fs::write(&local, &content).unwrap();
        conn.send(Command::Sftp(SftpCommand::Upload { local: local.clone(), remote_dir: cwd.clone() }));
        wait_transfer(&conn, &mut next);
        let remote = join_remote(&cwd, "ferroterm-essai.txt");
        let copy = scratch.join("copie.txt");
        conn.send(Command::Sftp(SftpCommand::Download { remote: remote.clone(), local: copy.clone() }));
        wait_transfer(&conn, &mut next);
        assert_eq!(std::fs::read(&copy).unwrap(), content, "contenu téléchargé différent");

        // Édition : renvoi fusionné avec la version du serveur.
        let ours = [&content[..], b"ajout\n"].concat();
        conn.send(Command::Sftp(SftpCommand::SaveEdited { remote: remote.clone(), base: content.clone(), ours: ours.clone() }));
        loop {
            if let Event::EditSaved { result, .. } = next(&conn) {
                println!("renvoi du fichier édité : {result:?}");
                assert!(result.is_ok());
                break;
            }
        }
        conn.send(Command::Sftp(SftpCommand::Download { remote: remote.clone(), local: copy.clone() }));
        wait_transfer(&conn, &mut next);
        assert_eq!(std::fs::read(&copy).unwrap(), ours, "fichier édité mal renvoyé");

        // Dossier créé, listé, supprimé ; fichier supprimé.
        let folder = join_remote(&cwd, "ferroterm-dossier");
        conn.send(Command::Sftp(SftpCommand::Mkdir(folder.clone())));
        let (_, entries) = list(&conn, &mut next, &cwd);
        assert!(entries.iter().any(|e| e.name == "ferroterm-dossier" && e.is_dir));
        assert!(entries.iter().any(|e| e.name == "ferroterm-essai.txt" && e.size == ours.len() as u64));
        conn.send(Command::Sftp(SftpCommand::Remove { path: folder, is_dir: true }));
        conn.send(Command::Sftp(SftpCommand::Remove { path: remote, is_dir: false }));
        let (_, entries) = list(&conn, &mut next, &cwd);
        assert!(!entries.iter().any(|e| e.name.starts_with("ferroterm-")), "fichiers d'essai restés sur le serveur");
        drop(conn);

        // Seconde connexion : un certificat accepté n'est plus redemandé.
        if asked_certificate {
            let conn = open(&rt, session);
            let mut asked_again = false;
            while !matches!(next_event(&conn, &password, &mut asked_again), Event::SftpReady { .. }) {}
            assert!(!asked_again, "certificat redemandé alors qu'il a été accepté");
            println!("seconde connexion : certificat reconnu sans question");
        }
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// Connexion comme l'ouvre un onglet : FTP ici, SFTP par `ssh.rs`.
    fn open(rt: &tokio::runtime::Runtime, session: Session) -> Connection {
        if session.is_ftp() {
            spawn(rt, session, egui::Context::default())
        } else {
            Connection::spawn(rt, session, egui::Context::default())
        }
    }

    /// Prochain événement ; répond aux questions (mot de passe, clé d'hôte, certificat).
    fn next_event(conn: &Connection, password: &str, asked_certificate: &mut bool) -> Event {
        loop {
            let event = conn.events.recv_timeout(Duration::from_secs(60)).expect("le serveur ne répond pas");
            match event {
                Event::Status(text) => println!("état : {text}"),
                Event::AskText { prompt, secret, .. } => {
                    println!("question : {prompt}");
                    assert!(secret, "question inattendue : {prompt}");
                    conn.reply(Reply::Text { text: Some(password.to_string()), remember: false });
                }
                Event::AskHostKey { algorithm, fingerprint } => {
                    println!("clé d'hôte inconnue : {algorithm} {fingerprint}");
                    conn.reply(Reply::HostKey(true));
                }
                Event::AskCertificate { reason, fingerprint, changed } => {
                    println!("certificat non reconnu ({reason}, changé : {changed}) : {fingerprint}");
                    *asked_certificate = true;
                    conn.reply(Reply::HostKey(true));
                }
                Event::SftpError(e) => panic!("erreur de l'explorateur : {e}"),
                Event::Closed(e) => panic!("connexion fermée : {e:?}"),
                other => return other,
            }
        }
    }

    fn wait_transfer(conn: &Connection, next: &mut dyn FnMut(&Connection) -> Event) {
        loop {
            if let Event::Transfer(t) = next(conn)
                && t.finished
            {
                assert_eq!(t.error, None, "{}", t.label);
                println!("{} : {} o", t.label, t.done);
                return;
            }
        }
    }

    /// `schéma://utilisateur:motdepasse@hôte:port/dossier` → (session, mot de passe, dossier).
    fn session_from_url(url: &str) -> (Session, String, String) {
        use crate::session::Protocol;
        let (scheme, rest) = url.split_once("://").expect("schéma");
        let (protocol, tls) = match scheme {
            "ftp" => (Protocol::Ftp, FtpTls::Auto),
            "ftpes" => (Protocol::Ftp, FtpTls::Required),
            "ftps" => (Protocol::Ftp, FtpTls::Implicit),
            "sftp" => (Protocol::Sftp, FtpTls::Auto),
            other => panic!("schéma inconnu : {other}"),
        };
        let (credentials, rest) = rest.rsplit_once('@').expect("utilisateur:motdepasse@");
        let (user, password) = credentials.split_once(':').expect("utilisateur:motdepasse");
        let (address, dir) = rest.split_once('/').map(|(a, d)| (a, format!("/{d}"))).unwrap_or((rest, String::new()));
        let (host, port) = match address.rsplit_once(':') {
            Some((host, port)) => (host, port.parse().expect("port")),
            None => (address, crate::session::default_port(protocol, tls)),
        };
        let mut session = Session {
            name: "essai".into(),
            host: host.into(),
            port,
            username: user.into(),
            protocol,
            ..Default::default()
        };
        session.ftp.tls = tls;
        session.ftp.active_mode = std::env::var_os("FERROTERM_FTP_ACTIVE").is_some();
        (session, password.into(), dir.trim_end_matches('/').to_string())
    }

    #[test]
    fn server_replies_are_readable() {
        use suppaftp::Status;
        let refused = FtpFailure(FtpError::UnexpectedResponse(suppaftp::types::Response {
            status: Status::NotLoggedIn,
            body: b"530 Login incorrect.\r\n".to_vec(),
        }));
        assert_eq!(refused.to_string(), "le serveur a répondu « 530 Login incorrect. »");
        let lost: anyhow::Error = FtpFailure(FtpError::UnexpectedResponse(suppaftp::types::Response {
            status: Status::NotAvailable,
            body: b"Timeout.".to_vec(),
        }))
        .into();
        assert!(is_lost(&lost));
        assert!(lost.to_string().ends_with("« 421 Timeout. »"));
        assert!(!is_lost(&anyhow::Error::from(refused)));
    }
}
