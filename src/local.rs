//! Terminal local : un shell Windows (PowerShell, cmd, WSL, Git Bash…) lancé
//! dans la console intégrée de Windows (ConPTY), affiché comme un onglet SSH.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::ssh::{Command, Connection, ConnectionParts, Event, INITIAL_COLS, INITIAL_ROWS};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalShell {
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// Shells disponibles sur cette machine, dans l'ordre de préférence.
pub fn detect_shells() -> Vec<LocalShell> {
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let program_files = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));

    let candidates: Vec<(&str, Option<PathBuf>, &[&str])> = vec![
        (
            "PowerShell 7",
            find_in_path("pwsh.exe").or_else(|| existing(program_files.join(r"PowerShell\7\pwsh.exe"))),
            &["-NoLogo"],
        ),
        (
            "Windows PowerShell",
            existing(system_root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe")),
            &["-NoLogo"],
        ),
        (
            "Invite de commandes",
            std::env::var_os("ComSpec")
                .map(PathBuf::from)
                .and_then(existing)
                .or_else(|| existing(system_root.join(r"System32\cmd.exe"))),
            &[],
        ),
        ("WSL", existing(system_root.join(r"System32\wsl.exe")), &[]),
        (
            "Git Bash",
            existing(program_files.join(r"Git\bin\bash.exe")),
            &["--login", "-i"],
        ),
    ];

    candidates
        .into_iter()
        .filter_map(|(name, program, args)| {
            program.map(|program| LocalShell {
                name: name.to_string(),
                program,
                args: args.iter().map(|a| a.to_string()).collect(),
            })
        })
        .collect()
}

fn existing(path: PathBuf) -> Option<PathBuf> {
    path.is_file().then_some(path)
}

fn find_in_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe))
        .find(|candidate| candidate.is_file())
}

type SharedMaster = Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>;
type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Lance le shell et renvoie une connexion utilisable comme un onglet SSH.
pub fn spawn(shell: &LocalShell, ctx: egui::Context) -> Connection {
    let (conn, parts) = Connection::new_parts(ctx);
    let ui = parts.ui.clone();
    if let Err(e) = start(shell, parts) {
        ui.send(Event::Closed(Some(format!("{} : {e:#}", shell.program.display()))));
    }
    conn
}

fn start(shell: &LocalShell, parts: ConnectionParts) -> anyhow::Result<()> {
    let ConnectionParts { ui, parser, mut commands, .. } = parts;

    let pair = native_pty_system().openpty(PtySize {
        rows: INITIAL_ROWS,
        cols: INITIAL_COLS,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut command = CommandBuilder::new(&shell.program);
    command.args(&shell.args);
    if let Some(home) = std::env::var_os("USERPROFILE").filter(|h| Path::new(h).is_dir()) {
        command.cwd(home);
    }
    command.env("TERM", "xterm-256color");
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let writer: SharedWriter = Arc::new(Mutex::new(pair.master.take_writer()?));
    let master: SharedMaster = Arc::new(Mutex::new(Some(pair.master)));
    let mut killer = child.clone_killer();

    ui.send(Event::Connected);
    ui.send(Event::SftpUnavailable("terminal local".into()));

    // Sortie du shell → terminal (avec réponses aux questions de ConPTY).
    {
        let ui = ui.clone();
        let writer = writer.clone();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let replies = crate::terminal::feed(&mut parser.lock().expect("parser"), &buffer[..n]);
                        if !replies.is_empty() {
                            let mut w = writer.lock().expect("writer");
                            let _ = w.write_all(&replies);
                            let _ = w.flush();
                        }
                        ui.ctx.request_repaint();
                    }
                }
            }
        });
    }

    // Commandes de l'interface → shell.
    {
        let master = master.clone();
        std::thread::spawn(move || {
            while let Some(command) = commands.blocking_recv() {
                match command {
                    Command::Input(bytes) => {
                        let mut w = writer.lock().expect("writer");
                        let _ = w.write_all(&bytes);
                        let _ = w.flush();
                    }
                    Command::Resize { cols, rows } => {
                        if let Some(m) = master.lock().expect("master").as_ref() {
                            let _ = m.resize(PtySize {
                                rows,
                                cols,
                                pixel_width: 0,
                                pixel_height: 0,
                            });
                        }
                    }
                    Command::Sftp(_) => {}
                    Command::Disconnect => {
                        let _ = killer.kill();
                        break;
                    }
                }
            }
        });
    }

    // Fin du shell : on ferme la console, ce qui débloque la lecture.
    std::thread::spawn(move || {
        let status = child.wait();
        master.lock().expect("master").take();
        let error = match status {
            Ok(s) if s.success() => None,
            Ok(s) => Some(format!("le shell s'est terminé avec le code {}", s.exit_code())),
            Err(e) => Some(e.to_string()),
        };
        ui.send(Event::Closed(error));
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_for(conn: &Connection, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let contents = conn.parser.lock().unwrap().screen().contents();
            if contents.contains(needle) || Instant::now() > deadline {
                return contents;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn detects_at_least_cmd() {
        let shells = detect_shells();
        assert!(shells.iter().any(|s| s.name == "Invite de commandes"), "{shells:?}");
    }

    /// Lance vraiment `cmd` dans ConPTY : vérifie que la console démarre
    /// (réponse à la question de position du curseur), que la saisie arrive
    /// et que la sortie est affichée.
    #[test]
    fn cmd_runs_a_command_through_conpty() {
        let shell = detect_shells()
            .into_iter()
            .find(|s| s.name == "Invite de commandes")
            .expect("cmd");
        let conn = spawn(&shell, egui::Context::default());
        let prompt = wait_for(&conn, ">");
        assert!(prompt.contains('>'), "pas d'invite : {prompt:?}");

        conn.send(Command::Input(b"echo ferro-%OS%\r".to_vec()));
        let screen = wait_for(&conn, "ferro-Windows_NT");
        assert!(screen.contains("ferro-Windows_NT"), "sortie absente : {screen:?}");

        conn.send(Command::Input(b"exit\r".to_vec()));
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut closed = None;
        while Instant::now() < deadline && closed.is_none() {
            while let Ok(event) = conn.events.try_recv() {
                if let Event::Closed(error) = event {
                    closed = Some(error);
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(closed, Some(None), "le shell aurait dû se fermer sans erreur");
    }
}
