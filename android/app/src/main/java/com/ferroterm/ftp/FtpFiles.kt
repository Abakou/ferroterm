package com.ferroterm.ftp

import com.ferroterm.files.RemoteEntry
import com.ferroterm.files.RemoteFiles
import com.ferroterm.files.RemotePaths
import org.apache.commons.net.ftp.FTPClient
import org.apache.commons.net.ftp.FTPFile
import org.apache.commons.net.ftp.FTPReply
import java.io.FilterInputStream
import java.io.FilterOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream

/** Réponse négative du serveur FTP (fichier introuvable, refus…) : la connexion reste utilisable. */
class FtpReplyException(val code: Int, reply: String) : IOException(reply.trim().ifEmpty { "Erreur $code" })

/** Lève [FtpReplyException] si la dernière commande a échoué. */
internal fun FTPClient.check(ok: Boolean) {
    if (!ok) throw FtpReplyException(replyCode, replyString.orEmpty())
}

/**
 * Fichiers distants par FTP. La navigation passe par la connexion de contrôle (une
 * commande à la fois) ; chaque transfert ouvre sa propre connexion, comme FileZilla,
 * pour que la navigation reste possible pendant les envois et téléchargements.
 */
class FtpFiles(private val connection: FtpConnection, val control: FTPClient) : RemoteFiles {

    override fun home(): String = connection.homeDir

    override fun list(dir: String): List<RemoteEntry> = synchronized(control) {
        control.check(control.changeWorkingDirectory(dir))
        var files: Array<FTPFile>? = null
        if (connection.useMlsd) {
            files = control.mlistDir()
            // MLSD annoncé mais refusé : on se rabat sur LIST pour la suite.
            if (!FTPReply.isPositiveCompletion(control.replyCode)) {
                connection.useMlsd = false
                files = null
            }
        }
        if (files == null) {
            files = control.listFiles()
            if (!FTPReply.isPositiveCompletion(control.replyCode)) {
                throw FtpReplyException(control.replyCode, control.replyString.orEmpty())
            }
        }
        files.filterNotNull().map { toEntry(dir, it) }
    }

    override fun mkdir(path: String) = synchronized(control) { control.check(control.makeDirectory(path)) }

    override fun rename(from: String, to: String) = synchronized(control) { control.check(control.rename(from, to)) }

    override fun deleteFile(path: String) = synchronized(control) { control.check(control.deleteFile(path)) }

    override fun deleteDir(path: String) = synchronized(control) { control.check(control.removeDirectory(path)) }

    override fun openRead(path: String): InputStream {
        val client = connection.openTransferClient()
        val stream = try {
            client.retrieveFileStream(path) ?: throw FtpReplyException(client.replyCode, client.replyString.orEmpty())
        } catch (e: Exception) {
            connection.closeTransferClient(client)
            throw e
        }
        return object : FilterInputStream(stream) {
            private var closed = false
            override fun close() {
                if (closed) return
                closed = true
                try {
                    super.close()
                    client.check(client.completePendingCommand())
                } finally {
                    connection.closeTransferClient(client)
                }
            }
        }
    }

    override fun openWrite(path: String): OutputStream {
        val client = connection.openTransferClient()
        val stream = try {
            client.storeFileStream(path) ?: throw FtpReplyException(client.replyCode, client.replyString.orEmpty())
        } catch (e: Exception) {
            connection.closeTransferClient(client)
            throw e
        }
        return object : FilterOutputStream(stream) {
            private var closed = false
            // FilterOutputStream écrit octet par octet par défaut : on transmet les blocs.
            override fun write(b: ByteArray, off: Int, len: Int) = out.write(b, off, len)
            override fun close() {
                if (closed) return
                closed = true
                try {
                    super.close()
                    client.check(client.completePendingCommand())
                } finally {
                    connection.closeTransferClient(client)
                }
            }
        }
    }

    override fun isServerError(e: Exception): Boolean = e is FtpReplyException

    /** Le texte du serveur est le plus parlant (« 550 Directory not empty »…). */
    override fun describe(e: Exception): String? = (e as? FtpReplyException)?.message

    private fun toEntry(dir: String, file: FTPFile): RemoteEntry {
        val path = RemotePaths.join(dir, file.name)
        val isLink = file.isSymbolicLink
        // Lien : dossier si l'on peut y entrer (puis on revient au dossier listé).
        val isDirectory = if (isLink) {
            control.changeWorkingDirectory(path).also { entered -> if (entered) control.changeWorkingDirectory(dir) }
        } else {
            file.isDirectory
        }
        return RemoteEntry(
            name = file.name,
            path = path,
            isDirectory = isDirectory,
            isLink = isLink,
            size = if (file.size >= 0) file.size else -1,
            modified = file.timestamp?.timeInMillis?.div(1000) ?: 0,
            permissions = permissions(file),
        )
    }

    /** `rwxr-xr-x` d'après la liste du serveur, ou vide s'il ne donne pas les droits. */
    private fun permissions(file: FTPFile): String {
        var mask = 0
        var bit = 8
        for (access in intArrayOf(FTPFile.USER_ACCESS, FTPFile.GROUP_ACCESS, FTPFile.WORLD_ACCESS)) {
            for (permission in intArrayOf(FTPFile.READ_PERMISSION, FTPFile.WRITE_PERMISSION, FTPFile.EXECUTE_PERMISSION)) {
                if (file.hasPermission(access, permission)) mask = mask or (1 shl bit)
                bit--
            }
        }
        return if (mask == 0) "" else RemotePaths.permissionString(mask)
    }
}
