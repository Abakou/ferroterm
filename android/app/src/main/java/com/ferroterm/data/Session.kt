package com.ferroterm.data

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/**
 * Session enregistrée. Mêmes noms de champs JSON que la version bureau
 * (`src/session.rs`), pour pouvoir importer les sessions exportées depuis Ferroterm.
 */
@Serializable
data class Session(
    val id: Long = 0,
    val name: String = "",
    /** Dossier de rangement, segments séparés par `/`. Vide = racine. */
    val folder: String = "",
    val protocol: Protocol = Protocol.SSH,
    val ftp: FtpOptions = FtpOptions(),
    val host: String = "",
    val port: Int = 22,
    /** Vide = demandé à la connexion. */
    val username: String = "",
    /**
     * Clé privée. Sur mobile : nom du fichier importé dans le dossier privé de
     * l'application (voir [PrivateKeys]). `null` = authentification par mot de passe.
     */
    @SerialName("key_path") val keyPath: String? = null,
) {
    /** `utilisateur@hôte[:port]`, le port étant omis s'il vaut celui du protocole. */
    val target: String
        get() {
            val user = if (username.isEmpty()) "" else "$username@"
            return if (port == defaultPort(protocol, ftp.tls)) "$user$host" else "$user$host:$port"
        }

    val displayName: String get() = name.ifBlank { target }
}

@Serializable
enum class Protocol {
    @SerialName("ssh") SSH,
    @SerialName("sftp") SFTP,
    @SerialName("ftp") FTP,
}

@Serializable
enum class FtpTls {
    @SerialName("auto") AUTO,
    @SerialName("required") REQUIRED,
    @SerialName("implicit") IMPLICIT,
    @SerialName("off") OFF,
}

@Serializable
data class FtpOptions(
    val tls: FtpTls = FtpTls.AUTO,
    @SerialName("active_mode") val activeMode: Boolean = false,
)

fun defaultPort(protocol: Protocol, tls: FtpTls = FtpTls.AUTO): Int = when (protocol) {
    Protocol.SSH, Protocol.SFTP -> 22
    Protocol.FTP -> if (tls == FtpTls.IMPLICIT) 990 else 21
}

/** Contenu du fichier `sessions.json` de l'application. */
@Serializable
data class SessionFile(val sessions: List<Session> = emptyList())

/**
 * Tolère les champs inconnus (fichiers du bureau ou d'une version plus récente).
 * Écrit tous les champs : le bureau exige `id`, `name`, `host` et `port`.
 */
val sessionJson = Json {
    ignoreUnknownKeys = true
    encodeDefaults = true
    prettyPrint = true
}
