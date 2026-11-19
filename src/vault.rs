//! Mots de passe et phrases de passe enregistrés dans le coffre du système :
//! - Windows : Gestionnaire d'identification (chiffré par DPAPI, lié au compte).
//!   Chaque secret est une identification « générique » nommée `Ferroterm/…`,
//!   visible et supprimable aussi depuis le panneau de configuration.
//! - Linux : Secret Service (GNOME Keyring, KWallet, KeePassXC…), via `keyring`.
//!   Chaque secret est l'entrée `Ferroterm/…` du service `ferroterm`, visible
//!   dans « Mots de passe et clés » (Seahorse).
//! - macOS : trousseau de session, via `keyring`.

/// Secret enregistrable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Secret {
    /// Mot de passe SSH d'un profil de session enregistrée : (session, profil).
    /// Le profil principal (0) garde le nom d'origine, sans numéro de profil.
    Password(u64, u64),
    /// Phrase de passe d'une clé privée (par chemin du fichier) : partagée par
    /// toutes les sessions qui utilisent cette clé.
    Passphrase(String),
}

impl Secret {
    /// Nom du secret dans le coffre du système.
    fn target(&self) -> String {
        match self {
            Secret::Password(id, 0) => format!("Ferroterm/session/{id}"),
            Secret::Password(id, profile) => format!("Ferroterm/session/{id}/{profile}"),
            Secret::Passphrase(path) => {
                // Chemins insensibles à la casse sous Windows seulement.
                let path = path.replace('\\', "/");
                let path = if cfg!(windows) { path.to_lowercase() } else { path };
                format!("Ferroterm/key/{path}")
            }
        }
    }
}

/// Secret enregistré, ou `None` s'il n'y en a pas (ou si le coffre est illisible).
pub fn get(secret: &Secret) -> Option<String> {
    imp::read(&secret.target())
}

/// Vrai si un secret est enregistré. Assez rapide pour être appelé à chaque image.
pub fn exists(secret: &Secret) -> bool {
    imp::exists(&secret.target())
}

/// Enregistre (ou remplace) un secret. `user` n'est qu'indicatif (affiché par Windows).
pub fn set(secret: &Secret, user: &str, value: &str) -> anyhow::Result<()> {
    imp::write(&secret.target(), user, value)
}

/// Supprime un secret ; sans effet s'il n'existe pas.
pub fn delete(secret: &Secret) {
    imp::delete(&secret.target());
}

#[cfg(windows)]
mod imp {
    use std::ptr;

    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW, CredWriteW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn read(target: &str) -> Option<String> {
        let target = wide(target);
        let mut cred: *mut CREDENTIALW = ptr::null_mut();
        // SAFETY : `target` est une chaîne UTF-16 terminée par un zéro ; en cas
        // de succès, `cred` pointe sur une structure allouée par Windows, lue
        // puis libérée par `CredFree`.
        unsafe {
            if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) == 0 || cred.is_null() {
                return None;
            }
            let blob = std::slice::from_raw_parts((*cred).CredentialBlob, (*cred).CredentialBlobSize as usize);
            let value = String::from_utf8(blob.to_vec()).ok();
            CredFree(cred.cast());
            value
        }
    }

    pub fn exists(target: &str) -> bool {
        read(target).is_some()
    }

    pub fn write(target: &str, user: &str, value: &str) -> anyhow::Result<()> {
        let mut target = wide(target);
        let mut user = wide(user);
        let mut blob = value.as_bytes().to_vec();
        // SAFETY : toutes les chaînes et le contenu vivent jusqu'à la fin de
        // l'appel ; Windows en fait une copie.
        let ok = unsafe {
            let mut cred: CREDENTIALW = std::mem::zeroed();
            cred.Type = CRED_TYPE_GENERIC;
            cred.TargetName = target.as_mut_ptr();
            cred.UserName = user.as_mut_ptr();
            cred.CredentialBlobSize = blob.len() as u32;
            cred.CredentialBlob = blob.as_mut_ptr();
            cred.Persist = CRED_PERSIST_LOCAL_MACHINE;
            CredWriteW(&cred, 0)
        };
        // Pas de copie du secret laissée en mémoire plus longtemps que nécessaire.
        blob.fill(0);
        if ok == 0 {
            anyhow::bail!("coffre Windows : {}", std::io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn delete(target: &str) {
        let target = wide(target);
        // SAFETY : chaîne UTF-16 terminée par un zéro ; l'échec (secret absent) est ignoré.
        unsafe {
            CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0);
        }
    }
}

/// Hors Windows : coffre du système via `keyring` (Secret Service sous Linux,
/// trousseau sous macOS). Les appels sont bloquants, comme sous Windows.
#[cfg(not(windows))]
mod imp {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};

    use keyring::{Entry, Error};

    /// Service sous lequel sont rangés les secrets ; le nom de l'entrée est la cible.
    const SERVICE: &str = "ferroterm";

    /// Présence connue de chaque secret. `exists` est appelé à chaque image par
    /// l'éditeur de session : sans cette mémoire, ce serait un aller-retour
    /// D-Bus par image. Seule la présence est retenue, jamais le secret.
    /// Mise à jour par nos propres lectures et écritures ; une modification
    /// faite ailleurs (Seahorse) n'est vue qu'à la lecture suivante du secret.
    static KNOWN: LazyLock<Mutex<HashMap<String, bool>>> = LazyLock::new(Default::default);

    fn remember(target: &str, present: bool) {
        KNOWN.lock().expect("vault").insert(target.to_string(), present);
    }

    fn entry(target: &str) -> keyring::Result<Entry> {
        Entry::new(SERVICE, target)
    }

    pub fn read(target: &str) -> Option<String> {
        match entry(target).and_then(|e| e.get_password()) {
            Ok(value) => {
                remember(target, true);
                Some(value)
            }
            Err(Error::NoEntry) => {
                remember(target, false);
                None
            }
            // Coffre absent ou verrouillé : on ne mémorise rien, pour réessayer.
            Err(_) => None,
        }
    }

    pub fn exists(target: &str) -> bool {
        if let Some(&present) = KNOWN.lock().expect("vault").get(target) {
            return present;
        }
        // Première question : on interroge le coffre et la réponse est mémorisée.
        // Un coffre injoignable est mémorisé comme « absent » lui aussi, pour ne
        // pas bloquer l'interface à chaque image ; une vraie lecture (connexion)
        // le réinterrogera.
        let present = read(target).is_some();
        remember(target, present);
        present
    }

    pub fn write(target: &str, _user: &str, value: &str) -> anyhow::Result<()> {
        entry(target)
            .and_then(|e| e.set_password(value))
            .map_err(|e| anyhow::anyhow!("coffre du système : {e}"))?;
        remember(target, true);
        Ok(())
    }

    pub fn delete(target: &str) {
        // Absence du secret ou coffre injoignable : sans effet, comme sous Windows.
        if let Ok(entry) = entry(target) {
            let _ = entry.delete_credential();
        }
        remember(target, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_stable() {
        assert_eq!(Secret::Password(42, 0).target(), "Ferroterm/session/42");
        assert_eq!(Secret::Password(42, 3).target(), "Ferroterm/session/42/3");
        #[cfg(windows)]
        assert_eq!(
            Secret::Passphrase(r"C:\Users\Moi\.ssh\ID_ed25519".into()).target(),
            "Ferroterm/key/c:/users/moi/.ssh/id_ed25519"
        );
        #[cfg(not(windows))]
        assert_eq!(
            Secret::Passphrase("/home/moi/.ssh/ID_ed25519".into()).target(),
            "Ferroterm/key//home/moi/.ssh/ID_ed25519"
        );
    }

    /// Écrit, relit et supprime un secret dans le vrai coffre du système.
    /// Hors Windows, il faut un trousseau déverrouillé : lancer avec `--ignored`.
    #[cfg_attr(not(windows), ignore = "demande un trousseau Secret Service déverrouillé")]
    #[test]
    fn round_trip_in_system_vault() {
        let secret = Secret::Password(u64::MAX - 7, 0);
        delete(&secret);
        assert_eq!(get(&secret), None);
        assert!(!exists(&secret));
        set(&secret, "test", "mot de passe é ✓").unwrap();
        assert_eq!(get(&secret).as_deref(), Some("mot de passe é ✓"));
        assert!(exists(&secret));
        set(&secret, "test", "remplacé").unwrap();
        assert_eq!(get(&secret).as_deref(), Some("remplacé"));
        delete(&secret);
        assert!(!exists(&secret));
    }
}
