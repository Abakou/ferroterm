package com.ferroterm.ssh

import com.ferroterm.files.RemoteEntry
import com.ferroterm.files.RemoteFiles
import com.ferroterm.files.RemotePaths
import com.ferroterm.files.ServerRefusal
import net.schmizz.sshj.sftp.FileAttributes
import net.schmizz.sshj.sftp.FileMode
import net.schmizz.sshj.sftp.OpenMode
import net.schmizz.sshj.sftp.Response
import net.schmizz.sshj.sftp.SFTPClient
import net.schmizz.sshj.sftp.SFTPException
import java.io.FilterInputStream
import java.io.FilterOutputStream
import java.io.InputStream
import java.io.OutputStream
import java.util.EnumSet

/** Fichiers distants par SFTP, sur la connexion SSH de la session. */
class SftpFiles(val sftp: SFTPClient) : RemoteFiles {

    override fun home(): String = sftp.canonicalize(".")

    override fun list(dir: String): List<RemoteEntry> = sftp.ls(dir).map { info ->
        toEntry(dir, info.name, info.attributes)
    }

    override fun mkdir(path: String) = sftp.mkdir(path)

    override fun rename(from: String, to: String) = sftp.rename(from, to)

    override fun deleteFile(path: String) = sftp.rm(path)

    override fun deleteDir(path: String) {
        try {
            sftp.rmdir(path)
        } catch (e: SFTPException) {
            // SFTP v3 (OpenSSH) ne dit pas pourquoi : « échec » générique.
            if (e.statusCode == Response.StatusCode.FAILURE) {
                throw ServerRefusal("le dossier n'est pas vide (ou accès refusé)")
            }
            throw e
        }
    }

    override fun openRead(path: String): InputStream {
        val remote = sftp.open(path, EnumSet.of(OpenMode.READ))
        return object : FilterInputStream(remote.ReadAheadRemoteFileInputStream(READ_AHEAD)) {
            override fun close() {
                try { super.close() } finally { remote.close() }
            }
        }
    }

    override fun openWrite(path: String): OutputStream {
        val remote = sftp.open(path, EnumSet.of(OpenMode.WRITE, OpenMode.CREAT, OpenMode.TRUNC))
        return object : FilterOutputStream(remote.RemoteFileOutputStream(0, WRITE_AHEAD)) {
            // FilterOutputStream écrit octet par octet par défaut : on transmet les blocs.
            override fun write(b: ByteArray, off: Int, len: Int) = out.write(b, off, len)
            override fun close() {
                try { super.close() } finally { remote.close() }
            }
        }
    }

    override fun isServerError(e: Exception): Boolean = e is SFTPException

    override fun describe(e: Exception): String? = (e as? SFTPException)?.let {
        when (it.statusCode) {
            Response.StatusCode.NO_SUCH_FILE -> "introuvable"
            Response.StatusCode.PERMISSION_DENIED -> "permission refusée"
            Response.StatusCode.FILE_ALREADY_EXISTS -> "existe déjà"
            Response.StatusCode.DIR_NOT_EMPTY -> "le dossier n'est pas vide"
            Response.StatusCode.NO_SPACE_ON_FILESYSTEM -> "plus de place sur le serveur"
            else -> it.message ?: it.statusCode.toString()
        }
    }

    private fun toEntry(dir: String, name: String, attrs: FileAttributes): RemoteEntry {
        val path = RemotePaths.join(dir, name)
        val isLink = attrs.type == FileMode.Type.SYMLINK
        // Lien : on suit la cible pour savoir si c'est un dossier. Lien cassé : on garde le lien.
        val effective = if (isLink) runCatching { sftp.stat(path) }.getOrNull() ?: attrs else attrs
        return RemoteEntry(
            name = name,
            path = path,
            isDirectory = effective.type == FileMode.Type.DIRECTORY,
            isLink = isLink,
            size = effective.size,
            modified = effective.mtime,
            permissions = RemotePaths.permissionString(effective.mode.permissionsMask),
        )
    }

    private companion object {
        const val READ_AHEAD = 16
        const val WRITE_AHEAD = 16
    }
}
