package com.ferroterm.ssh

import net.schmizz.sshj.common.Buffer
import net.schmizz.sshj.common.KeyType
import net.schmizz.sshj.transport.verification.OpenSSHKnownHosts
import java.io.File
import java.security.MessageDigest
import java.security.PublicKey
import java.util.Base64

/** Clé présentée par un serveur, telle que montrée à l'utilisateur. */
data class HostKeyInfo(val host: String, val type: String, val fingerprint: String)

/** Empreinte au format d'OpenSSH (`SHA256:…`), comme la version bureau. */
fun sha256Fingerprint(key: PublicKey): String {
    val blob = Buffer.PlainBuffer().putPublicKey(key).compactData
    val digest = MessageDigest.getInstance("SHA-256").digest(blob)
    return "SHA256:" + Base64.getEncoder().withoutPadding().encodeToString(digest)
}

/**
 * Vérification des clés serveur avec le fichier `known_hosts` de l'application.
 *
 * - clé connue et identique : connexion acceptée sans question ;
 * - hôte inconnu : [ask] montre l'empreinte à l'utilisateur, la clé est enregistrée s'il l'accepte ;
 * - clé différente de celle enregistrée : connexion refusée ([changed] dit pourquoi).
 *
 * [ask] est appelé depuis le fil de sshj et peut bloquer en attendant la réponse.
 */
class HostKeyVerifier(
    file: File,
    private val ask: (HostKeyInfo) -> Boolean,
) : OpenSSHKnownHosts(file) {

    /** Renseigné si la clé du serveur ne correspond plus à celle enregistrée. */
    @Volatile
    var changed: HostKeyInfo? = null
        private set

    /** Renseigné si l'utilisateur a refusé une clé inconnue. */
    @Volatile
    var refused: Boolean = false
        private set

    override fun hostKeyUnverifiableAction(hostname: String, key: PublicKey): Boolean {
        val type = KeyType.fromKey(key)
        if (!ask(HostKeyInfo(hostname, type.toString(), sha256Fingerprint(key)))) {
            refused = true
            return false
        }
        entries().add(HostEntry(null, hostname, type, key))
        write()
        return true
    }

    override fun hostKeyChangedAction(hostname: String, key: PublicKey): Boolean {
        changed = HostKeyInfo(hostname, KeyType.fromKey(key).toString(), sha256Fingerprint(key))
        return false
    }
}

/** Retire de `known_hosts` toutes les clés enregistrées pour [host] (format `[hôte]:port` si port ≠ 22). */
fun forgetHostKeys(file: File, host: String) {
    if (!file.exists()) return
    val knownHosts = OpenSSHKnownHosts(file)
    knownHosts.entries().removeAll { entry ->
        runCatching { entry.appliesTo(host) }.getOrDefault(false)
    }
    knownHosts.write()
}
