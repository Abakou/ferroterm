package com.ferroterm.files

import android.content.ContentResolver
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.util.Log
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.atomic.AtomicLong
import kotlin.coroutines.coroutineContext

/** Transfert en cours. [total] vaut -1 si la taille est inconnue. */
data class Transfer(
    val id: Long,
    val name: String,
    val upload: Boolean,
    val total: Long,
    val done: Long,
)

/**
 * Explorateur de fichiers d'une connexion (SFTP ou FTP) : dossier courant, contenu,
 * transferts. Il vit avec la connexion (pas avec l'écran) : quitter l'écran ne coupe
 * pas un transfert, et on retrouve le même dossier en y revenant.
 */
class FileBrowser(private val source: FileSource, private val scope: CoroutineScope) {
    private val _path = MutableStateFlow<String?>(null)
    val path: StateFlow<String?> = _path.asStateFlow()

    private val _entries = MutableStateFlow<List<RemoteEntry>>(emptyList())
    val entries: StateFlow<List<RemoteEntry>> = _entries.asStateFlow()

    private val _loading = MutableStateFlow(false)
    val loading: StateFlow<Boolean> = _loading.asStateFlow()

    /** Échec du dernier listage (le contenu affiché est alors celui d'avant). */
    private val _error = MutableStateFlow<String?>(null)
    val error: StateFlow<String?> = _error.asStateFlow()

    private val _transfers = MutableStateFlow<List<Transfer>>(emptyList())
    val transfers: StateFlow<List<Transfer>> = _transfers.asStateFlow()

    /** Messages ponctuels pour l'utilisateur (fin de transfert, erreur d'une action). */
    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 16)
    val messages: SharedFlow<String> = _messages.asSharedFlow()

    /** Dossier personnel, point de départ ; connu après le premier listage. */
    @Volatile var home: String? = null
        private set

    private var loadJob: Job? = null
    private val transferJobs = mutableMapOf<Long, Job>()
    private val nextTransfer = AtomicLong(1)

    /** Premier affichage, ou retour de connexion : recharge le dossier courant. */
    fun refreshOrStart() = load(_path.value)

    fun refresh() = load(_path.value)

    fun open(path: String) = load(path)

    fun goHome() = load(home)

    fun up() {
        RemotePaths.parentOf(_path.value ?: return)?.let(::load)
    }

    fun mkdir(name: String) = action("Création de « $name »") { files ->
        files.mkdir(child(name))
        "Dossier « $name » créé"
    }

    fun rename(entry: RemoteEntry, newName: String) = action("Renommage de « ${entry.name} »") { files ->
        files.rename(entry.path, RemotePaths.join(RemotePaths.parentOf(entry.path) ?: "/", newName))
        "« ${entry.name} » renommé en « $newName »"
    }

    /** Fichier, lien, ou dossier vide (comme la version bureau : pas de suppression récursive). */
    fun delete(entry: RemoteEntry) = action("Suppression de « ${entry.name} »") { files ->
        if (entry.isDirectory && !entry.isLink) files.deleteDir(entry.path) else files.deleteFile(entry.path)
        "« ${entry.name} » supprimé"
    }

    /** Télécharge [entry] vers le document [target] choisi par l'utilisateur. */
    fun download(resolver: ContentResolver, entry: RemoteEntry, target: Uri) {
        transfer(entry.name, upload = false, total = entry.size, onFailure = {
            runCatching { DocumentsContract.deleteDocument(resolver, target) }
        }) { files, progress ->
            val output = resolver.openOutputStream(target, "wt") ?: throw IOException("Destination illisible")
            output.use { out -> files.openRead(entry.path).use { copy(it, out, progress) } }
            "« ${entry.name} » téléchargé"
        }
    }

    /** Télécharge [entry] dans le cache de l'application, puis appelle [onReady] (pour l'ouvrir). */
    fun downloadToCache(cacheDir: File, entry: RemoteEntry, onReady: (File) -> Unit) {
        val dir = File(cacheDir, "sftp").apply { mkdirs() }
        // Nom conservé (pour que l'application choisie reconnaisse le type), un fichier à la fois.
        dir.listFiles()?.forEach { it.delete() }
        val file = File(dir, entry.name.replace('/', '_'))
        transfer(entry.name, upload = false, total = entry.size, onFailure = { file.delete() }) { files, progress ->
            file.outputStream().use { out -> files.openRead(entry.path).use { copy(it, out, progress) } }
            withContext(Dispatchers.Main) { onReady(file) }
            null
        }
    }

    /** Envoie les documents [sources] dans le dossier courant (fichiers existants remplacés). */
    fun upload(resolver: ContentResolver, sources: List<Uri>) {
        val dir = _path.value ?: return
        for (uri in sources) {
            val (name, size) = describeDocument(resolver, uri)
            val remotePath = RemotePaths.join(dir, name)
            transfer(name, upload = true, total = size, onFailure = { files ->
                runCatching { files?.deleteFile(remotePath) }
            }) { files, progress ->
                val input = resolver.openInputStream(uri) ?: throw IOException("Fichier illisible")
                input.use { files.openWrite(remotePath).use { out -> copy(input, out, progress) } }
                if (_path.value == dir) load(dir)
                "« $name » envoyé"
            }
        }
    }

    /** Message de l'écran (ex. : aucune application pour ouvrir un fichier). */
    fun notify(message: String) {
        _messages.tryEmit(message)
    }

    fun cancelTransfer(id: Long) {
        synchronized(transferJobs) { transferJobs[id] }?.cancel()
    }

    fun cancelAll() {
        synchronized(transferJobs) { transferJobs.values.toList() }.forEach { it.cancel() }
        loadJob?.cancel()
    }

    /** Noms du dossier courant (pour prévenir avant d'écraser un fichier). */
    fun existingNames(): Set<String> = _entries.value.mapTo(HashSet()) { it.name }

    private fun load(target: String?) {
        loadJob?.cancel()
        loadJob = scope.launch {
            _loading.value = true
            try {
                val (dir, list) = withFiles { files ->
                    val start = home ?: files.home().also { home = it }
                    val dir = target ?: start
                    dir to files.list(dir).filter { it.name != "." && it.name != ".." }
                }
                _entries.value = list.sortedWith(compareBy<RemoteEntry>({ !it.isDirectory }, { it.name.lowercase() }))
                _path.value = dir
                _error.value = null
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _error.value = "${target ?: "Dossier personnel"} : ${describe(e)}"
            } finally {
                _loading.value = false
            }
        }
    }

    /** Action ponctuelle sur le dossier courant, suivie d'un rechargement. */
    private fun action(what: String, block: (RemoteFiles) -> String) {
        scope.launch {
            try {
                _messages.tryEmit(withFiles(block))
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _messages.tryEmit("$what impossible : ${describe(e)}")
            }
            refresh()
        }
    }

    private fun transfer(
        name: String,
        upload: Boolean,
        total: Long,
        onFailure: (RemoteFiles?) -> Unit,
        block: suspend (RemoteFiles, (Long) -> Unit) -> String?,
    ) {
        val id = nextTransfer.getAndIncrement()
        _transfers.update { it + Transfer(id, name, upload, total, 0) }
        val job = scope.launch(Dispatchers.IO) {
            var files: RemoteFiles? = null
            try {
                files = source.remoteFiles()
                val message = block(files) { done ->
                    _transfers.update { list -> list.map { if (it.id == id) it.copy(done = done) else it } }
                }
                message?.let(_messages::tryEmit)
            } catch (e: CancellationException) {
                withContext(NonCancellable) { onFailure(files) }
                _messages.tryEmit("Transfert de « $name » annulé")
                throw e
            } catch (e: Exception) {
                Log.w(TAG, "Transfert de $name échoué", e)
                withContext(NonCancellable) { onFailure(files) }
                if (files != null && !files.isServerError(e)) source.discard(files)
                _messages.tryEmit("Transfert de « $name » échoué : ${describe(e, files)}")
            } finally {
                _transfers.update { list -> list.filterNot { it.id == id } }
                synchronized(transferJobs) { transferJobs.remove(id) }
            }
        }
        synchronized(transferJobs) { transferJobs[id] = job }
    }

    /** Copie par blocs ; s'interrompt dès que le transfert est annulé. */
    private suspend fun copy(input: InputStream, output: OutputStream, progress: (Long) -> Unit) {
        val buffer = ByteArray(COPY_BUFFER)
        var done = 0L
        var reported = 0L
        while (true) {
            coroutineContext.ensureActive()
            val read = input.read(buffer)
            if (read < 0) break
            output.write(buffer, 0, read)
            done += read
            if (done - reported >= PROGRESS_STEP) {
                progress(done)
                reported = done
            }
        }
        output.flush()
        progress(done)
    }

    /** Dernier accès obtenu, pour traduire ses erreurs. */
    @Volatile private var lastFiles: RemoteFiles? = null

    /**
     * Exécute [block] hors du fil principal. Une coupure (et non une simple réponse
     * d'erreur du serveur) fait abandonner l'accès : le suivant en rouvre un.
     */
    private suspend fun <T> withFiles(block: (RemoteFiles) -> T): T {
        val files = source.remoteFiles()
        lastFiles = files
        return try {
            withContext(Dispatchers.IO) { block(files) }
        } catch (e: IOException) {
            if (!files.isServerError(e)) source.discard(files)
            throw e
        }
    }

    private fun describe(e: Exception, files: RemoteFiles? = lastFiles): String =
        (e as? ServerRefusal)?.message ?: files?.describe(e) ?: e.message ?: e.javaClass.simpleName

    private fun child(name: String): String = RemotePaths.join(_path.value ?: "/", name)

    private fun describeDocument(resolver: ContentResolver, uri: Uri): Pair<String, Long> {
        resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
            if (c.moveToFirst()) {
                val name = c.getString(0)?.replace('/', '_') ?: "fichier"
                val size = if (c.isNull(1)) -1L else c.getLong(1)
                return name to size
            }
        }
        return (uri.lastPathSegment?.substringAfterLast('/') ?: "fichier") to -1L
    }

    private companion object {
        const val TAG = "FileBrowser"
        const val COPY_BUFFER = 32 * 1024
        const val PROGRESS_STEP = 256 * 1024L
    }
}
