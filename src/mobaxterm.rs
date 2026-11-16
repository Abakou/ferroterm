//! Import des sessions depuis un fichier `MobaXterm.ini` (interopérabilité).
//!
//! Format observé dans les sections `[Bookmarks]` et `[Bookmarks_N]` :
//!
//! ```text
//! SubRep=<dossier, sous-dossiers séparés par '\'>
//! ImgNum=<icône du dossier>
//! <nom>=#<icône>#<type>%<hôte>%<port>%<utilisateur>%…%<clé privée (champ 14)>%…#<réglages du terminal>#…
//! ```
//!
//! Le type `0` correspond à SSH ; les autres types de session ne sont pas importés.
//! Les chemins de clés peuvent contenir `_ProfileDir_` (dossier du profil Windows)
//! et `_CurrentDrive_` (lecteur où se trouve MobaXterm).

use crate::session::Session;

const SSH_TYPE: &str = "0";
const FIELD_HOST: usize = 1;
const FIELD_PORT: usize = 2;
const FIELD_USER: usize = 3;
const FIELD_KEY: usize = 14;

#[derive(Debug, Default)]
pub struct ImportResult {
    pub sessions: Vec<Session>,
    /// Entrées ignorées, avec la raison.
    pub skipped: Vec<String>,
}

/// Le fichier est normalement en ANSI (Windows-1252) ; on accepte aussi l'UTF-8.
pub fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.trim_start_matches('\u{feff}').to_string(),
        // Approximation Latin-1, suffisante pour les noms de sessions.
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

pub fn parse(text: &str, profile_dir: &str, current_drive: &str) -> ImportResult {
    let mut result = ImportResult::default();
    let mut in_bookmarks = false;
    let mut folder = String::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = &trimmed[1..trimmed.len() - 1];
            in_bookmarks = section == "Bookmarks" || section.starts_with("Bookmarks_");
            folder.clear();
            continue;
        }
        if !in_bookmarks || trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "SubRep" => {
                folder = value.trim().replace('\\', "/").trim_matches('/').to_string();
            }
            "ImgNum" => {}
            name => match parse_entry(name, value, &folder, profile_dir, current_drive) {
                Ok(session) => result.sessions.push(session),
                Err(reason) => result.skipped.push(format!("{name} : {reason}")),
            },
        }
    }
    result
}

fn parse_entry(
    name: &str,
    value: &str,
    folder: &str,
    profile_dir: &str,
    current_drive: &str,
) -> Result<Session, String> {
    let mut parts = value.trim_start().split('#');
    // La valeur commence par '#', la première partie est donc vide.
    if parts.next() != Some("") {
        return Err("format inconnu".into());
    }
    let _icon = parts.next().ok_or("format inconnu")?;
    let connection = parts.next().ok_or("format inconnu")?;
    let fields: Vec<&str> = connection.split('%').map(str::trim).collect();

    let kind = fields.first().copied().unwrap_or_default();
    if kind != SSH_TYPE {
        return Err(format!(
            "type de session {kind} non pris en charge (seul SSH est importé)"
        ));
    }
    let host = fields.get(FIELD_HOST).copied().unwrap_or_default();
    if host.is_empty() {
        return Err("hôte vide".into());
    }
    let port = match fields.get(FIELD_PORT).copied().unwrap_or_default() {
        "" => 22,
        p => p.parse::<u16>().map_err(|_| format!("port invalide : {p}"))?,
    };
    let username = fields.get(FIELD_USER).copied().unwrap_or_default();
    let key_path = fields
        .get(FIELD_KEY)
        .copied()
        .filter(|k| !k.is_empty())
        .map(|k| {
            k.replace("_ProfileDir_", profile_dir)
                .replace("_CurrentDrive_", current_drive)
        });

    Ok(Session {
        id: 0,
        name: name.to_string(),
        folder: folder.to_string(),
        host: host.to_string(),
        port,
        username: username.to_string(),
        key_path,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Lignes au format MobaXterm, avec des hôtes fictifs.
    const SAMPLE: &str = "[Misc]\r\n\
SomeSetting=1\r\n\
[Bookmarks]\r\n\
SubRep=\r\n\
ImgNum=42\r\n\
root-box=#109#0%192.0.2.10%22%root%%-1%-1%%%%%0%0%0%%%-1%0%0%0%%1080%%0%0%1#MobaFont%10%0%0%-1%15#0# #-1\r\n\
[Bookmarks_1]\r\n\
SubRep=Clients\\Acme\r\n\
ImgNum=41\r\n\
web= #109#0%web.example.org%2222%deploy%%-1%0%%%%%0%-1%0%_ProfileDir_\\.ssh\\id_web%%-1%-1%0%0%%1080%%0%0%1#MobaFont%10#0# #-1\r\n\
bureau=#91#4%192.0.2.20%3389%admin%0%-1%-1#MobaFont%10#0# #-1\r\n\
[SSH_Hostkeys]\r\n\
ssh-ed25519@22:192.0.2.10=0x1234\r\n";

    #[test]
    fn parses_ssh_sessions_folders_and_keys() {
        let result = parse(SAMPLE, r"C:\Users\alice", "C:");
        assert_eq!(result.sessions.len(), 2);

        let root = &result.sessions[0];
        assert_eq!(root.name, "root-box");
        assert_eq!(root.folder, "");
        assert_eq!(root.host, "192.0.2.10");
        assert_eq!(root.port, 22);
        assert_eq!(root.username, "root");
        assert_eq!(root.key_path, None);

        let web = &result.sessions[1];
        assert_eq!(web.folder, "Clients/Acme");
        assert_eq!(web.host, "web.example.org");
        assert_eq!(web.port, 2222);
        assert_eq!(web.username, "deploy");
        assert_eq!(web.key_path.as_deref(), Some(r"C:\Users\alice\.ssh\id_web"));
    }

    #[test]
    fn skips_non_ssh_sessions_with_reason() {
        let result = parse(SAMPLE, r"C:\Users\alice", "C:");
        assert_eq!(result.skipped.len(), 1);
        assert!(result.skipped[0].starts_with("bureau"));
    }

    /// Vérification sur un vrai fichier :
    /// `FERROTERM_MOBAXTERM_INI=chemin\MobaXterm.ini cargo test real_file -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_file() {
        let path = std::env::var("FERROTERM_MOBAXTERM_INI").expect("FERROTERM_MOBAXTERM_INI");
        let profile = std::env::var("USERPROFILE").unwrap_or_default();
        let text = decode(&std::fs::read(&path).expect("lecture"));
        let result = parse(&text, &profile, "C:");
        for s in &result.sessions {
            let key = s.key_path.as_deref().unwrap_or("(mot de passe)");
            let key_ok = s.key_path.as_ref().map(|k| std::path::Path::new(k).exists());
            println!("{:<12} {:<28} {:<22} clé={key} existe={key_ok:?}", s.folder, s.name, s.target());
        }
        println!("{} sessions, {} ignorées : {:?}", result.sessions.len(), result.skipped.len(), result.skipped);
    }

    #[test]
    fn decodes_latin1_fallback() {
        assert_eq!(decode(b"caf\xe9"), "café");
        assert_eq!(decode("\u{feff}[Bookmarks]".as_bytes()), "[Bookmarks]");
    }
}
