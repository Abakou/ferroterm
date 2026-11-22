package com.ferroterm.files

import java.io.InputStream
import java.io.OutputStream

/** Élément d'un dossier distant. Pour un lien symbolique, les attributs sont ceux de sa cible. */
data class RemoteEntry(
    val name: String,
    val path: String,
    val isDirectory: Boolean,
    val isLink: Boolean,
    /** -1 si inconnue. */
    val size: Long,
    /** Secondes depuis 1970 ; 0 si inconnue. */
    val modified: Long,
    /** `rwxr-xr-x`, ou vide si le serveur ne le dit pas. */
    val permissions: String,
)

/**
 * Opérations sur des fichiers distants, quel que soit le protocole (SFTP, FTP).
 * Méthodes bloquantes, appelées hors du fil principal.
 */
interface RemoteFiles {
    /** Dossier de départ (dossier personnel). */
    fun home(): String
    fun list(dir: String): List<RemoteEntry>
    fun mkdir(path: String)
    fun rename(from: String, to: String)
    fun deleteFile(path: String)

    /** Dossier vide seulement. */
    fun deleteDir(path: String)

    /** Flux de lecture ; le fermer termine le transfert. */
    fun openRead(path: String): InputStream

    /** Flux d'écriture (fichier créé ou remplacé) ; le fermer termine le transfert. */
    fun openWrite(path: String): OutputStream

    /**
     * Erreur répondue par le serveur (fichier introuvable, permission…) : la connexion
     * reste utilisable. Sinon (coupure), elle est abandonnée et rouverte au besoin.
     */
    fun isServerError(e: Exception): Boolean

    /** Explication en français d'une erreur propre au protocole, ou `null`. */
    fun describe(e: Exception): String?
}

/** Donne accès aux fichiers d'une connexion ouverte. */
interface FileSource {
    /** Échoue si la connexion n'est pas établie. */
    suspend fun remoteFiles(): RemoteFiles

    /** Abandonne un accès devenu inutilisable (coupure) : le suivant en rouvre un. */
    fun discard(files: RemoteFiles)
}

/** Échec expliqué à l'utilisateur, sans remettre en cause la connexion. */
class ServerRefusal(message: String) : Exception(message)

object RemotePaths {
    fun join(dir: String, name: String): String = if (dir.endsWith("/")) dir + name else "$dir/$name"

    fun parentOf(path: String): String? {
        if (path == "/" || path.isEmpty()) return null
        val trimmed = path.trimEnd('/')
        val index = trimmed.lastIndexOf('/')
        return if (index <= 0) "/" else trimmed.substring(0, index)
    }

    /** `rwxr-xr-x` d'après les 9 bits de permission Unix. */
    fun permissionString(mask: Int): String {
        val flags = "rwxrwxrwx"
        return buildString {
            for (i in 0 until 9) append(if (mask and (1 shl (8 - i)) != 0) flags[i] else '-')
        }
    }
}
