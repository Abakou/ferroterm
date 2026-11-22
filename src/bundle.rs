//! Secrets joints à un export de sessions : clés privées, phrases de passe et
//! mots de passe, chiffrés par un mot de passe choisi à l'export.
//!
//! Chiffrement : clé AES-256 dérivée du mot de passe par PBKDF2-HMAC-SHA256
//! (sel aléatoire de 16 octets), puis AES-256-GCM (nonce de 12 octets, étiquette
//! de 16 octets ajoutée au chiffré). Mêmes primitives que l'application Android
//! (`android/.../data/SessionExport.kt`), qui sait relire ces fichiers.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use base64ct::{Base64, Encoding};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::pbkdf2;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

use crate::session::Session;
use crate::vault::{self, Secret};

/// Itérations PBKDF2 à l'export (recommandation OWASP pour HMAC-SHA256).
const ITERATIONS: u32 = 600_000;
/// Au-delà, un fichier (modifié ou malveillant) bloquerait l'import trop longtemps.
const MAX_ITERATIONS: u32 = 10_000_000;
const KDF: &str = "pbkdf2-sha256";
const CIPHER: &str = "aes-256-gcm";
/// Données associées : lient le chiffré à ce format.
const AAD: &[u8] = b"ferroterm-secrets-v1";
/// Taille maximale d'une clé privée exportée.
pub const MAX_KEY_SIZE: u64 = 64 * 1024;
/// Longueur minimale du mot de passe d'export.
pub const MIN_PASSWORD_LEN: usize = 8;

/// Secrets chiffrés, tels qu'écrits dans le fichier d'export.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Sealed {
    pub kdf: String,
    pub iterations: u32,
    /// Base64.
    pub salt: String,
    pub cipher: String,
    /// Base64.
    pub nonce: String,
    /// Base64 : secrets en JSON chiffrés, suivis de l'étiquette GCM.
    pub data: String,
}

/// Secrets en clair (uniquement en mémoire).
#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Secrets {
    #[serde(default)]
    pub keys: Vec<KeyEntry>,
    #[serde(default)]
    pub passwords: Vec<PasswordEntry>,
}

/// Clé privée, avec sa phrase de passe si elle est enregistrée.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyEntry {
    /// Chemin de la clé sur le poste d'origine, tel qu'il figure dans les
    /// sessions exportées (`key_path`) : c'est par lui qu'on retrouve la clé.
    pub path: String,
    /// Nom du fichier, pour l'enregistrer sur le poste de destination.
    pub name: String,
    /// Base64 : contenu du fichier.
    pub data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
}

/// Mot de passe enregistré d'un profil de session.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PasswordEntry {
    /// Identifiant de la session dans l'export.
    pub session: u64,
    /// Profil de connexion (0 = principal).
    #[serde(default)]
    pub profile: u64,
    pub password: String,
}

/// Résultat de la collecte des secrets à exporter.
pub struct Collected {
    pub secrets: Secrets,
    /// Clés référencées par une session mais illisibles : (chemin, raison).
    pub missing: Vec<(String, String)>,
}

/// Rassemble les clés privées (contenu des fichiers) et les secrets du coffre
/// du système utilisés par ces sessions, tous profils confondus.
pub fn collect(sessions: &[Session]) -> Collected {
    let mut secrets = Secrets::default();
    let mut missing = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for session in sessions {
        for profile in session.profiles() {
            if let Some(password) = vault::get(&Secret::Password(session.id, profile.id)) {
                secrets.passwords.push(PasswordEntry { session: session.id, profile: profile.id, password });
            }
            let Some(path) = profile.key_path.filter(|p| !p.trim().is_empty()) else { continue };
            if seen.contains(&path) {
                continue;
            }
            seen.push(path.clone());
            match read_key(Path::new(&path)) {
                Ok(bytes) => secrets.keys.push(KeyEntry {
                    name: Path::new(&path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "cle".into()),
                    data: Base64::encode_string(&bytes),
                    passphrase: vault::get(&Secret::Passphrase(path.clone())),
                    path,
                }),
                Err(e) => missing.push((path, format!("{e:#}"))),
            }
        }
    }
    Collected { secrets, missing }
}

fn read_key(path: &Path) -> anyhow::Result<Vec<u8>> {
    let size = std::fs::metadata(path)?.len();
    if size > MAX_KEY_SIZE {
        bail!("fichier trop gros pour une clé privée");
    }
    Ok(std::fs::read(path)?)
}

/// Chiffre les secrets avec le mot de passe d'export.
pub fn seal(secrets: &Secrets, password: &str) -> anyhow::Result<Sealed> {
    let rng = SystemRandom::new();
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; NONCE_LEN];
    rng.fill(&mut salt).map_err(|_| anyhow!("générateur aléatoire indisponible"))?;
    rng.fill(&mut nonce).map_err(|_| anyhow!("générateur aléatoire indisponible"))?;
    let key = derive(password, &salt, ITERATIONS)?;
    let mut data = serde_json::to_vec(secrets)?;
    key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(AAD), &mut data)
        .map_err(|_| anyhow!("chiffrement impossible"))?;
    Ok(Sealed {
        kdf: KDF.into(),
        iterations: ITERATIONS,
        salt: Base64::encode_string(&salt),
        cipher: CIPHER.into(),
        nonce: Base64::encode_string(&nonce),
        data: Base64::encode_string(&data),
    })
}

/// Déchiffre les secrets. Erreur explicite si le mot de passe est faux.
pub fn open(sealed: &Sealed, password: &str) -> anyhow::Result<Secrets> {
    if sealed.kdf != KDF || sealed.cipher != CIPHER {
        bail!("chiffrement non pris en charge ({} / {}) : mettez Ferroterm à jour", sealed.kdf, sealed.cipher);
    }
    if sealed.iterations == 0 || sealed.iterations > MAX_ITERATIONS {
        bail!("paramètres de chiffrement invalides");
    }
    let decode = |field: &str| Base64::decode_vec(field).map_err(|_| anyhow!("secrets illisibles (fichier abîmé)"));
    let salt = decode(&sealed.salt)?;
    let nonce: [u8; NONCE_LEN] =
        decode(&sealed.nonce)?.try_into().map_err(|_| anyhow!("secrets illisibles (fichier abîmé)"))?;
    let mut data = decode(&sealed.data)?;
    let key = derive(password, &salt, sealed.iterations)?;
    let plain = key
        .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(AAD), &mut data)
        .map_err(|_| anyhow!("mot de passe incorrect"))?;
    serde_json::from_slice(plain).context("secrets illisibles")
}

fn derive(password: &str, salt: &[u8], iterations: u32) -> anyhow::Result<LessSafeKey> {
    let iterations = NonZeroU32::new(iterations).ok_or_else(|| anyhow!("paramètres de chiffrement invalides"))?;
    let mut key = [0u8; 32];
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, iterations, salt, password.as_bytes(), &mut key);
    let key = UnboundKey::new(&AES_256_GCM, &key).map_err(|_| anyhow!("clé de chiffrement invalide"))?;
    Ok(LessSafeKey::new(key))
}

/// Dossier où sont enregistrées les clés importées : à côté du fichier de sessions.
pub fn keys_dir() -> Option<PathBuf> {
    crate::session::SessionStore::default_path().and_then(|p| p.parent().map(|d| d.join("keys")))
}

/// Écrit les clés importées dans `dir` et renvoie, pour chacune, le chemin
/// d'origine → chemin local. Une clé déjà présente à son chemin d'origine avec
/// le même contenu (réimport sur le même poste) garde ce chemin.
pub fn install_keys(keys: &[KeyEntry], dir: &Path) -> anyhow::Result<HashMap<String, String>> {
    let mut paths = HashMap::new();
    for key in keys {
        let bytes = Base64::decode_vec(&key.data).map_err(|_| anyhow!("clé {} illisible", key.name))?;
        if std::fs::read(&key.path).is_ok_and(|existing| existing == bytes) {
            paths.insert(key.path.clone(), key.path.clone());
            continue;
        }
        std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
        let target = free_name(dir, &key.name, &bytes);
        if !target.exists() {
            write_private(&target, &bytes).with_context(|| target.display().to_string())?;
        }
        paths.insert(key.path.clone(), target.to_string_lossy().into_owned());
    }
    Ok(paths)
}

/// `dir/nom`, ou `dir/nom-2`… si un autre fichier porte déjà ce nom. Un
/// fichier identique est réutilisé.
fn free_name(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let clean: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' })
        .collect();
    let clean = if clean.trim_matches('.').is_empty() { "cle".to_string() } else { clean };
    for n in 1.. {
        let candidate = if n == 1 { dir.join(&clean) } else { dir.join(format!("{clean}-{n}")) };
        match std::fs::read(&candidate) {
            Ok(existing) if existing == bytes => return candidate,
            Ok(_) => continue,
            Err(_) if candidate.exists() => continue,
            Err(_) => return candidate,
        }
    }
    unreachable!()
}

/// Fichier lisible par son seul propriétaire (OpenSSH refuse les clés trop ouvertes).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Secrets {
        Secrets {
            keys: vec![KeyEntry {
                path: r"C:\Users\a\.ssh\id_ed25519".into(),
                name: "id_ed25519".into(),
                data: Base64::encode_string(b"-----BEGIN OPENSSH PRIVATE KEY-----\n"),
                passphrase: Some("phrase".into()),
            }],
            passwords: vec![PasswordEntry { session: 7, profile: 1, password: "secret é".into() }],
        }
    }

    #[test]
    fn seal_then_open_round_trip() {
        let sealed = seal(&sample(), "mot de passe").unwrap();
        assert_eq!((sealed.kdf.as_str(), sealed.cipher.as_str()), (KDF, CIPHER));
        assert_eq!(open(&sealed, "mot de passe").unwrap(), sample());
        let error = open(&sealed, "autre").unwrap_err().to_string();
        assert!(error.contains("incorrect"), "{error}");
    }

    #[test]
    fn rejects_tampered_or_unknown_parameters() {
        let sealed = seal(&sample(), "pw").unwrap();
        let mut tampered = sealed.clone();
        let mut data = Base64::decode_vec(&tampered.data).unwrap();
        data[0] ^= 1;
        tampered.data = Base64::encode_string(&data);
        assert!(open(&tampered, "pw").is_err());
        assert!(open(&Sealed { iterations: u32::MAX, ..sealed.clone() }, "pw").is_err());
        assert!(open(&Sealed { kdf: "argon2id".into(), ..sealed }, "pw").is_err());
    }

    /// Vecteur fixe, vérifié aussi par le test Android (`SessionExportTest`) :
    /// les deux implémentations doivent rester compatibles. Mot de passe « pw »,
    /// 1000 itérations, sel = 16 × 0x01, nonce = 12 × 0x02, texte clair
    /// `{"passwords":[{"session":1,"password":"x"}]}`.
    #[test]
    fn opens_known_vector() {
        let sealed = Sealed {
            kdf: KDF.into(),
            iterations: 1000,
            salt: "AQEBAQEBAQEBAQEBAQEBAQ==".into(),
            cipher: CIPHER.into(),
            nonce: "AgICAgICAgICAgIC".into(),
            data: "PD7LNdnynMd7ZxX30DSHhFeFdIp2SedZp3mwAqVm0bT7+r3AoalXPH64TTxODMmE774vIvNfdZVOovkk".into(),
        };
        assert_eq!(Base64::decode_vec(&sealed.salt).unwrap(), [1u8; 16]);
        assert_eq!(Base64::decode_vec(&sealed.nonce).unwrap(), [2u8; NONCE_LEN]);
        let secrets = open(&sealed, "pw").unwrap();
        assert_eq!(secrets.passwords, vec![PasswordEntry { session: 1, profile: 0, password: "x".into() }]);
        assert!(secrets.keys.is_empty());
    }

    #[test]
    fn install_keys_reuses_identical_files_and_avoids_collisions() {
        let dir = std::env::temp_dir().join(format!("ferroterm-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let entry = |path: &str, data: &[u8]| KeyEntry {
            path: path.into(),
            name: "id_rsa".into(),
            data: Base64::encode_string(data),
            passphrase: None,
        };
        let keys = vec![entry("/a/id_rsa", b"A"), entry("/b/id_rsa", b"B"), entry("/c/id_rsa", b"A")];
        let paths = install_keys(&keys, &dir).unwrap();
        assert_eq!(paths["/a/id_rsa"], dir.join("id_rsa").to_string_lossy());
        assert_eq!(paths["/b/id_rsa"], dir.join("id_rsa-2").to_string_lossy());
        assert_eq!(paths["/c/id_rsa"], paths["/a/id_rsa"]);
        assert_eq!(std::fs::read(dir.join("id_rsa-2")).unwrap(), b"B");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
