package com.ferroterm.data

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull
import org.bouncycastle.crypto.digests.SHA256Digest
import org.bouncycastle.crypto.generators.PKCS5S2ParametersGenerator
import org.bouncycastle.crypto.params.KeyParameter
import java.util.Base64
import javax.crypto.AEADBadTagException
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/**
 * Fichier créé par « Exporter les sessions » de la version bureau (`src/session.rs`,
 * `src/bundle.rs`). Les sessions sont en clair ; les secrets, s'ils ont été joints,
 * sont chiffrés par un mot de passe.
 */
@Serializable
data class SessionExportFile(
    val format: String = "",
    val version: Int = 0,
    /** Gardées brutes : le profil actif (`profile`) sert à retrouver les mots de passe. */
    val sessions: List<JsonElement> = emptyList(),
    val folders: List<String> = emptyList(),
    val secrets: Sealed? = null,
)

/** Secrets chiffrés : PBKDF2-HMAC-SHA256 puis AES-256-GCM. */
@Serializable
data class Sealed(
    val kdf: String,
    val iterations: Int,
    val salt: String,
    val cipher: String,
    val nonce: String,
    val data: String,
)

@Serializable
data class ExportSecrets(
    val keys: List<KeyEntry> = emptyList(),
    val passwords: List<PasswordEntry> = emptyList(),
)

@Serializable
data class KeyEntry(
    /** Chemin de la clé sur le poste d'origine, tel qu'il figure dans `key_path`. */
    val path: String,
    val name: String,
    /** Base64 : contenu du fichier. */
    val data: String,
    val passphrase: String? = null,
)

@Serializable
data class PasswordEntry(
    /** Identifiant de la session dans l'export. */
    val session: Long,
    /** Profil de connexion (0 = principal). */
    val profile: Long = 0,
    val password: String,
)

class WrongPasswordException : Exception("Mot de passe incorrect")

object SessionExport {
    const val FORMAT = "ferroterm-sessions"
    private const val KDF = "pbkdf2-sha256"
    private const val CIPHER = "aes-256-gcm"
    private const val MAX_ITERATIONS = 10_000_000
    private val AAD = "ferroterm-secrets-v1".encodeToByteArray()

    /** Lit un fichier d'export ; erreur explicite si ce n'en est pas un. */
    fun parse(text: String): SessionExportFile {
        val export = try {
            sessionJson.decodeFromString<SessionExportFile>(text.removePrefix("﻿"))
        } catch (e: Exception) {
            throw IllegalArgumentException("Ce fichier n'est pas un export de sessions Ferroterm.", e)
        }
        require(export.format == FORMAT) { "Ce fichier n'est pas un export de sessions Ferroterm." }
        return export
    }

    /**
     * Déchiffre les secrets. Lent (PBKDF2, 600 000 itérations) : à appeler hors du
     * fil principal. [WrongPasswordException] si le mot de passe est faux.
     */
    fun open(sealed: Sealed, password: String): ExportSecrets {
        require(sealed.kdf == KDF && sealed.cipher == CIPHER) {
            "Chiffrement non pris en charge (${sealed.kdf} / ${sealed.cipher}) : mettez Ferroterm à jour."
        }
        require(sealed.iterations in 1..MAX_ITERATIONS) { "Paramètres de chiffrement invalides." }
        val decoder = Base64.getDecoder()
        val (salt, nonce, data) = try {
            Triple(decoder.decode(sealed.salt), decoder.decode(sealed.nonce), decoder.decode(sealed.data))
        } catch (e: IllegalArgumentException) {
            throw IllegalArgumentException("Secrets illisibles (fichier abîmé).", e)
        }
        // Même dérivation que `ring` côté bureau : mot de passe en UTF-8.
        val generator = PKCS5S2ParametersGenerator(SHA256Digest())
        generator.init(password.encodeToByteArray(), salt, sealed.iterations)
        val key = (generator.generateDerivedParameters(256) as KeyParameter).key
        val plain = try {
            Cipher.getInstance("AES/GCM/NoPadding").run {
                init(Cipher.DECRYPT_MODE, SecretKeySpec(key, "AES"), GCMParameterSpec(128, nonce))
                updateAAD(AAD)
                doFinal(data)
            }
        } catch (e: AEADBadTagException) {
            throw WrongPasswordException()
        } finally {
            key.fill(0)
        }
        return sessionJson.decodeFromString<ExportSecrets>(plain.decodeToString())
    }

    /** Profil actif d'une session exportée (0 si absent). */
    fun activeProfile(session: JsonElement): Long =
        runCatching { session.jsonObject["profile"]?.jsonPrimitive?.longOrNull }.getOrNull() ?: 0L
}

/** Bilan d'un import. */
data class ImportReport(
    val found: Int,
    val added: Int,
    val duplicates: Int,
    val keys: Int = 0,
    val passwords: Int = 0,
    /** Clés jointes mais refusées : (nom, raison). */
    val badKeys: List<Pair<String, String>> = emptyList(),
)

/**
 * Ajoute les sessions d'un export, et leurs secrets s'ils ont été déchiffrés :
 * clés copiées dans le dossier privé de l'application, mots de passe et phrases
 * de passe dans [SecretStore]. Comme sur le bureau, une session déjà présente
 * (même groupe et même nom) est ignorée, avec ses secrets.
 */
fun importSessions(
    export: SessionExportFile,
    secrets: ExportSecrets?,
    repository: SessionRepository,
    keys: PrivateKeys,
    store: SecretStore,
): ImportReport {
    class Pending(val originalId: Long, val profile: Long, val session: Session)

    val existing = repository.sessions.value
    val pending = mutableListOf<Pending>()
    var duplicates = 0
    for (element in export.sessions) {
        val session = runCatching { sessionJson.decodeFromJsonElement(Session.serializer(), element) }.getOrNull()
        if (session == null || session.host.isBlank()) continue
        val same = { other: Session -> other.folder == session.folder && other.name == session.name }
        if (existing.any(same) || pending.any { same(it.session) }) {
            duplicates++
            continue
        }
        pending += Pending(session.id, SessionExport.activeProfile(element), session)
    }

    // Clés utilisées par les sessions ajoutées : chemin d'origine → nom local.
    val keyNames = mutableMapOf<String, String>()
    val badKeys = mutableListOf<Pair<String, String>>()
    secrets?.keys?.forEach { entry ->
        if (pending.none { it.session.keyPath == entry.path }) return@forEach
        try {
            val name = keys.importBytes(entry.name, Base64.getDecoder().decode(entry.data))
            keyNames[entry.path] = name
            entry.passphrase?.let { store.put(SecretStore.passphraseName(name), it) }
        } catch (e: Exception) {
            badKeys += entry.name to (e.message ?: e.javaClass.simpleName)
        }
    }

    val ids = repository.addAll(
        pending.map { p -> p.session.copy(id = 0, keyPath = p.session.keyPath?.let { keyNames[it] ?: it }) },
    )

    var passwords = 0
    secrets?.passwords?.forEach { entry ->
        // Sur mobile, seul le profil actif d'une session est gardé.
        val index = pending.indexOfFirst { it.originalId == entry.session && it.profile == entry.profile }
        if (index >= 0) {
            store.put(SecretStore.passwordName(ids[index]), entry.password)
            passwords++
        }
    }
    return ImportReport(
        found = export.sessions.size,
        added = ids.size,
        duplicates = duplicates,
        keys = keyNames.values.toSet().size,
        passwords = passwords,
        badKeys = badKeys,
    )
}
