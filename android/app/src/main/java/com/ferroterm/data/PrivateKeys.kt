package com.ferroterm.data

import android.content.ContentResolver
import android.net.Uri
import android.provider.OpenableColumns
import java.io.File
import java.util.UUID

/**
 * Clés privées importées. Le fichier choisi par l'utilisateur est copié dans le
 * dossier privé de l'application : l'original peut ensuite être supprimé du téléphone.
 */
class PrivateKeys(filesDir: File) {
    private val dir = File(filesDir, "keys").apply { mkdirs() }

    /** Copie la clé désignée par [uri] ; renvoie le nom à enregistrer dans la session. */
    fun import(resolver: ContentResolver, uri: Uri): String {
        val bytes = resolver.openInputStream(uri)?.use { it.readBytes() }
            ?: error("Fichier illisible")
        return importBytes(displayName(resolver, uri) ?: "cle", bytes)
    }

    /** Enregistre une clé (ex. jointe à un export de sessions) ; renvoie son nom. */
    fun importBytes(originalName: String, bytes: ByteArray): String {
        require(bytes.size <= MAX_KEY_SIZE) { "Fichier trop gros pour une clé privée" }
        val text = bytes.decodeToString()
        require("PRIVATE KEY" in text || text.startsWith("PuTTY-User-Key-File")) {
            "Ce fichier ne ressemble pas à une clé privée (OpenSSH, PEM ou PuTTY)"
        }
        val original = originalName.replace(Regex("[^A-Za-z0-9._-]"), "_").ifBlank { "cle" }
        val name = "${UUID.randomUUID().toString().take(8)}-$original"
        File(dir, name).writeBytes(bytes)
        return name
    }

    fun file(name: String): File? = File(dir, name).takeIf { it.isFile && it.parentFile == dir }

    fun delete(name: String) {
        file(name)?.delete()
    }

    /** Nom lisible : sans le préfixe aléatoire ajouté à l'import. */
    fun label(name: String): String = name.substringAfter('-', name)

    private fun displayName(resolver: ContentResolver, uri: Uri): String? =
        resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null
        }

    private companion object {
        const val MAX_KEY_SIZE = 64 * 1024
    }
}
