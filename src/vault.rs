//! Mots de passe et phrases de passe enregistrés dans le Gestionnaire
//! d'identification Windows (chiffrés par DPAPI, liés au compte Windows).
//!
//! Chaque secret est une identification « générique » nommée `Ferroterm/…`,
//! visible et supprimable aussi depuis le panneau de configuration Windows.

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
    /// Nom de l'identification dans le coffre Windows.
    fn target(&self) -> String {
        match self {
            Secret::Password(id, 0) => format!("Ferroterm/session/{id}"),
            Secret::Password(id, profile) => format!("Ferroterm/session/{id}/{profile}"),
            Secret::Passphrase(path) => format!("Ferroterm/key/{}", path.replace('\\', "/").to_lowercase()),
        }
    }
}

/// Secret enregistré, ou `None` s'il n'y en a pas (ou si le coffre est illisible).
pub fn get(secret: &Secret) -> Option<String> {
    imp::read(&secret.target())
}

/// Vrai si un secret est enregistré.
pub fn exists(secret: &Secret) -> bool {
    get(secret).is_some()
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

/// Hors Windows : pas de coffre, rien n'est enregistré.
#[cfg(not(windows))]
mod imp {
    pub fn read(_target: &str) -> Option<String> {
        None
    }

    pub fn write(_target: &str, _user: &str, _value: &str) -> anyhow::Result<()> {
        anyhow::bail!("coffre de mots de passe disponible sous Windows uniquement")
    }

    pub fn delete(_target: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_stable() {
        assert_eq!(Secret::Password(42, 0).target(), "Ferroterm/session/42");
        assert_eq!(Secret::Password(42, 3).target(), "Ferroterm/session/42/3");
        assert_eq!(
            Secret::Passphrase(r"C:\Users\Moi\.ssh\ID_ed25519".into()).target(),
            "Ferroterm/key/c:/users/moi/.ssh/id_ed25519"
        );
    }

    /// Écrit, relit et supprime un secret dans le vrai coffre Windows.
    #[cfg(windows)]
    #[test]
    fn round_trip_in_windows_vault() {
        let secret = Secret::Password(u64::MAX - 7, 0);
        delete(&secret);
        assert_eq!(get(&secret), None);
        set(&secret, "test", "mot de passe é ✓").unwrap();
        assert_eq!(get(&secret).as_deref(), Some("mot de passe é ✓"));
        set(&secret, "test", "remplacé").unwrap();
        assert_eq!(get(&secret).as_deref(), Some("remplacé"));
        delete(&secret);
        assert!(!exists(&secret));
    }
}
