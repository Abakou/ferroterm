package com.ferroterm.ssh

import android.util.Log
import androidx.compose.ui.graphics.Color
import com.ferroterm.data.PrivateKeys
import com.ferroterm.data.Protocol
import com.ferroterm.data.SecretStore
import com.ferroterm.data.Session
import com.ferroterm.files.FileBrowser
import com.ferroterm.files.RemoteFiles
import com.ferroterm.monitor.MonitorParser
import com.ferroterm.monitor.MonitorScript
import com.ferroterm.monitor.MonitorStats
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import net.schmizz.sshj.sftp.SFTPClient
import java.io.IOException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.isActive
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.runInterruptible
import net.schmizz.keepalive.KeepAliveProvider
import net.schmizz.sshj.DefaultConfig
import net.schmizz.sshj.SSHClient
import net.schmizz.sshj.connection.channel.direct.Session as SshSession
import net.schmizz.sshj.transport.TransportException
import net.schmizz.sshj.userauth.UserAuthException
import com.hierynomus.sshj.common.KeyDecryptionFailedException
import net.schmizz.sshj.userauth.method.AuthKeyboardInteractive
import net.schmizz.sshj.userauth.method.AuthPassword
import net.schmizz.sshj.userauth.method.PasswordResponseProvider
import net.schmizz.sshj.userauth.password.PasswordFinder
import net.schmizz.sshj.userauth.password.Resource
import org.connectbot.terminal.TerminalDimensions
import org.connectbot.terminal.TerminalEmulator
import org.connectbot.terminal.TerminalEmulatorFactory
import java.io.File
import java.net.ConnectException
import java.net.NoRouteToHostException
import java.net.SocketTimeoutException
import java.net.UnknownHostException

/** L'utilisateur a annulé une question : on arrête la connexion sans message d'erreur. */
private class Cancelled : RuntimeException()

private inline fun <reified T : Throwable> Throwable.hasCause(): Boolean =
    generateSequence(this) { it.cause }.take(10).any { it is T }

/**
 * Une session SSH ouverte avec son terminal (ou SFTP seul). Survit aux changements
 * d'écran : c'est [ConnectionManager] qui la crée et la ferme.
 */
class SshConnection(
    override val id: Long,
    override val session: Session,
    private val scope: CoroutineScope,
    private val secrets: SecretStore,
    private val keys: PrivateKeys,
    private val knownHosts: File,
    private val onStateChanged: () -> Unit,
) : RemoteConnection {
    private val _state = MutableStateFlow<ConnectionState>(ConnectionState.Connecting("Connexion…"))
    override val state: StateFlow<ConnectionState> = _state.asStateFlow()

    private val _prompt = MutableStateFlow<Prompt?>(null)
    override val prompt: StateFlow<Prompt?> = _prompt.asStateFlow()

    /** Frappes du clavier en attente d'envoi au serveur. */
    private val outgoing = Channel<ByteArray>(Channel.UNLIMITED)

    /**
     * Une tentative de connexion et ce qui en dépend. Chaque tentative ferme la sienne :
     * une reconnexion rapide ne risque pas de voir l'ancienne fermer la nouvelle.
     */
    private class Link(val ssh: SSHClient) {
        @Volatile var shell: SshSession.Shell? = null
        @Volatile var sftp: SFTPClient? = null
    }

    @Volatile private var link: Link? = null
    private val sftpLock = Mutex()
    private var job: Job? = null

    /** Terminal pour les sessions SSH ; les sessions « SFTP » n'ont que l'explorateur. */
    override val hasShell: Boolean = session.protocol == Protocol.SSH
    private var resizeJob: Job? = null

    /** Taille (colonnes, lignes) annoncée au serveur pour le PTY en cours. */
    @Volatile private var remoteSize: Pair<Int, Int>? = null
    private var connectedOnce = false

    val emulator: TerminalEmulator = TerminalEmulatorFactory.create(
        initialRows = 24,
        initialCols = 80,
        defaultForeground = TERMINAL_FOREGROUND,
        defaultBackground = TERMINAL_BACKGROUND,
        onKeyboardInput = { data -> outgoing.trySend(data) },
        onResize = { dimensions -> resizeRemote(dimensions) },
    )

    override val files = FileBrowser(this, scope)

    /** Dernières mesures de la supervision ; `null` tant qu'aucune n'est arrivée (ou serveur non Linux). */
    private val _stats = MutableStateFlow<MonitorStats?>(null)
    val stats: StateFlow<MonitorStats?> = _stats.asStateFlow()

    /** Charge CPU des dernières mesures (une toutes les deux secondes), pour la courbe. */
    private val _cpuHistory = MutableStateFlow<List<Float>>(emptyList())
    val cpuHistory: StateFlow<List<Float>> = _cpuHistory.asStateFlow()

    /** Envoie des octets au shell comme s'ils étaient tapés (macros). */
    fun sendInput(data: ByteArray): Boolean =
        _state.value == ConnectionState.Connected && outgoing.trySend(data).isSuccess

    /**
     * Supervision : script exécuté sur un canal dédié de la même connexion. Sur un serveur
     * qui n'est pas un Linux courant (ou qui refuse `exec`), il échoue sans bruit.
     */
    private suspend fun runMonitor(ssh: SSHClient) {
        // Le canal peut se fermer sans que le terminal tombe (serveur occupé, commande
        // interrompue) : on le relance quelques fois avant d'abandonner.
        repeat(MONITOR_ATTEMPTS) { attempt ->
            if (attempt > 0) delay(MONITOR_RETRY_MS)
            try {
                val channel = ssh.startSession()
                val command = channel.exec(MonitorScript.command())
                val parser = MonitorParser()
                runInterruptible {
                    command.inputStream.bufferedReader(Charsets.UTF_8).useLines { lines ->
                        for (line in lines) {
                            for (stats in parser.feed(line + "\n")) {
                                _stats.value = stats
                                stats.cpuPercent?.let { cpu ->
                                    _cpuHistory.value = (_cpuHistory.value + cpu).takeLast(CPU_HISTORY)
                                }
                            }
                        }
                    }
                }
                Log.i(TAG, "Supervision de ${session.host} interrompue (essai ${attempt + 1})")
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                Log.i(TAG, "Supervision indisponible sur ${session.host} : ${e.message}")
            }
        }
    }

    override fun connect() {
        if (job?.isActive == true) return
        _stats.value = null
        _cpuHistory.value = emptyList()
        // Ce qui a été tapé pendant la déconnexion ne doit pas partir vers le serveur.
        while (outgoing.tryReceive().isSuccess) Unit
        if (connectedOnce) startFreshScreen()
        setState(ConnectionState.Connecting("Connexion à ${session.host}:${session.port}…"))
        job = scope.launch(Dispatchers.IO) { run() }
    }

    /** Coupe la connexion ; le terminal reste affiché, et [connect] peut la rouvrir. */
    override fun disconnect() {
        job?.cancel()
        link?.let(::release)
        _prompt.value?.dismiss()
        if (_state.value !is ConnectionState.Closed) {
            setState(ConnectionState.Closed("Déconnecté", error = false))
        }
    }

    /** Libère tout, terminal compris. L'objet n'est plus utilisable ensuite. */
    override fun dispose() {
        files.cancelAll()
        disconnect()
        emulator.close()
    }

    override fun forgetChangedHostKey(key: HostKeyInfo) {
        forgetHostKeys(knownHosts, key.host)
    }

    /**
     * Client SFTP de la connexion en cours, ouvert à la première demande. Il partage la
     * connexion SSH du terminal, comme sur le bureau : pas de seconde authentification.
     */
    override suspend fun remoteFiles(): SftpFiles = sftpLock.withLock {
        val current = link?.takeIf { _state.value == ConnectionState.Connected }
            ?: throw IOException("Session non connectée")
        val sftp = current.sftp ?: withContext(Dispatchers.IO) { current.ssh.newSFTPClient() }.also { current.sftp = it }
        SftpFiles(sftp)
    }

    /** Oublie le client SFTP (après une coupure) : le prochain accès en rouvre un. */
    override fun discard(files: RemoteFiles) {
        val broken = (files as? SftpFiles)?.sftp ?: return
        val current = link ?: return
        if (current.sftp === broken) {
            current.sftp = null
            scope.launch(Dispatchers.IO + NonCancellable) { runCatching { broken.close() } }
        }
    }

    private suspend fun run() {
        val ssh = SSHClient(DefaultConfig().apply { keepAliveProvider = KeepAliveProvider.KEEP_ALIVE })
        val current = Link(ssh)
        link = current
        try {
            val verifier = HostKeyVerifier(knownHosts) { key ->
                runBlocking { ask(Prompt.UnknownHostKey(key)).answer.await() }
            }
            ssh.addHostKeyVerifier(verifier)
            ssh.connectTimeout = CONNECT_TIMEOUT_MS
            // Laisse à l'utilisateur le temps de lire l'empreinte avant que l'échange de clés n'expire.
            ssh.transport.timeoutMs = HOST_KEY_TIMEOUT_MS
            try {
                runInterruptible { ssh.connect(session.host, session.port) }
            } catch (e: TransportException) {
                verifier.changed?.let { throw HostKeyChanged(it) }
                if (verifier.refused) throw Cancelled()
                throw e
            }
            ssh.transport.timeoutMs = TRANSPORT_TIMEOUT_MS
            ssh.connection.keepAlive.keepAliveInterval = KEEPALIVE_SECONDS

            authenticate(ssh)

            if (!hasShell) {
                // Explorateur seul : la connexion vit jusqu'à sa fermeture.
                setState(ConnectionState.Connected)
                runInterruptible { ssh.transport.join() }
                finish(ConnectionState.Closed("Connexion fermée par le serveur", error = false))
                return
            }

            setState(ConnectionState.Connecting("Ouverture du terminal…"))
            val sshSession = ssh.startSession()
            val dimensions = emulator.dimensions
            sshSession.allocatePTY("xterm-256color", dimensions.columns, dimensions.rows, 0, 0, emptyMap())
            remoteSize = dimensions.columns to dimensions.rows
            val opened = sshSession.startShell()
            current.shell = opened
            connectedOnce = true
            setState(ConnectionState.Connected)
            // La taille a pu changer pendant l'ouverture (clavier affiché, rotation).
            withContext(Dispatchers.Main) { resizeRemote(emulator.dimensions) }

            coroutineScope {
                val writer = launch {
                    for (data in outgoing) {
                        runInterruptible {
                            opened.outputStream.write(data)
                            opened.outputStream.flush()
                        }
                    }
                }
                val monitor = launch { runMonitor(ssh) }
                runInterruptible {
                    val buffer = ByteArray(16 * 1024)
                    while (true) {
                        val read = opened.inputStream.read(buffer)
                        if (read < 0) break
                        if (read > 0) emulator.writeInput(buffer, 0, read)
                    }
                }
                writer.cancel()
                monitor.cancel()
            }
            finish(ConnectionState.Closed("Session terminée", error = false))
        } catch (e: Cancelled) {
            finish(ConnectionState.Closed("Connexion annulée", error = false))
        } catch (e: HostKeyChanged) {
            finish(
                ConnectionState.Closed(
                    "La clé du serveur a changé depuis la dernière connexion. Il peut s'agir d'une " +
                        "réinstallation du serveur… ou d'une interception. Connexion refusée.",
                    error = true,
                    changedHostKey = e.key,
                ),
            )
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            Log.w(TAG, "Connexion ${session.host} terminée", e)
            finish(ConnectionState.Closed(describe(e), error = true))
        } finally {
            release(current)
        }
    }

    /**
     * État de fin, sauf si l'utilisateur a lui-même coupé la connexion : la lecture
     * interrompue lève alors une erreur (InterruptedIOException…) qui n'en est pas une,
     * et [disconnect] pose « Déconnecté ».
     */
    private suspend fun finish(state: ConnectionState.Closed) {
        if (currentCoroutineContext().isActive) setState(state)
    }

    private suspend fun authenticate(ssh: SSHClient) {
        var username = session.username
        val keyName = session.keyPath
        if (keyName != null) {
            if (username.isEmpty()) username = askUsername()
            val keyFile = keys.file(keyName)
                ?: throw IllegalStateException("Clé privée introuvable : réimportez-la dans la session.")
            setState(ConnectionState.Connecting("Authentification par clé…"))
            val provider = ssh.loadKeys(keyFile.absolutePath, PassphraseFinder(keyName))
            try {
                runInterruptible { ssh.authPublickey(username, provider) }
            } catch (e: UserAuthException) {
                // sshj enveloppe les erreurs de lecture de la clé dans UserAuthException.
                if (e.hasCause<Cancelled>()) throw Cancelled()
                if (e.hasCause<KeyDecryptionFailedException>()) {
                    throw IllegalStateException("Phrase de passe de la clé incorrecte.", e)
                }
                throw IllegalStateException("Clé refusée par le serveur pour « $username ».", e)
            }
            return
        }

        val storeName = SecretStore.passwordName(session.id)
        var stored = if (session.id != 0L) secrets.get(storeName) else null
        var error: String? = null
        repeat(MAX_PASSWORD_ATTEMPTS) {
            val answer = if (stored != null && username.isNotEmpty()) {
                CredentialsAnswer(username, stored!!, remember = false)
            } else {
                askCredentials(
                    Prompt.Credentials(
                        title = "Connexion à ${session.host}",
                        username = username.ifEmpty { null },
                        askUsername = username.isEmpty(),
                        secretLabel = "Mot de passe",
                        error = error,
                        canRemember = session.id != 0L,
                    ),
                )
            }
            username = answer.username
            setState(ConnectionState.Connecting("Authentification…"))
            val finder = FixedPassword(answer.secret)
            try {
                runInterruptible {
                    ssh.auth(username, AuthPassword(finder), AuthKeyboardInteractive(PasswordResponseProvider(finder)))
                }
                if (answer.remember) secrets.put(storeName, answer.secret)
                return
            } catch (e: UserAuthException) {
                if (stored != null) {
                    // Mot de passe mémorisé périmé : on le redemande, et on l'oublie.
                    secrets.remove(storeName)
                    stored = null
                }
                error = "Mot de passe refusé."
            }
        }
        throw IllegalStateException("Authentification refusée pour « $username ».")
    }

    private suspend fun askUsername(): String = askCredentials(
        Prompt.Credentials(
            title = "Connexion à ${session.host}",
            username = null,
            askUsername = true,
            secretLabel = null,
            error = null,
            canRemember = false,
        ),
    ).username

    private suspend fun askCredentials(prompt: Prompt.Credentials): CredentialsAnswer =
        ask(prompt).answer.await() ?: throw Cancelled()

    /** Affiche [prompt] et attend la réponse ; le dialogue disparaît une fois répondu. */
    private suspend fun <P : Prompt> ask(prompt: P): P {
        _prompt.value = prompt
        try {
            prompt.awaitAnswer()
        } finally {
            _prompt.compareAndSet(prompt, null)
        }
        return prompt
    }

    /** Phrase de passe de la clé : mémorisée, sinon demandée (3 essais). */
    private inner class PassphraseFinder(private val keyName: String) : PasswordFinder {
        private var attempts = 0
        private var fromStore = false

        override fun reqPassword(resource: Resource<*>?): CharArray {
            attempts++
            val storeName = SecretStore.passphraseName(keyName)
            if (attempts == 1) {
                secrets.get(storeName)?.let {
                    fromStore = true
                    return it.toCharArray()
                }
            } else if (fromStore) {
                secrets.remove(storeName)
                fromStore = false
            }
            val answer = runBlocking {
                ask(
                    Prompt.Credentials(
                        title = "Clé ${keys.label(keyName)}",
                        username = null,
                        askUsername = false,
                        secretLabel = "Phrase de passe",
                        error = if (attempts > 1) "Phrase de passe incorrecte." else null,
                        canRemember = true,
                    ),
                ).answer.await()
            } ?: throw Cancelled()
            if (answer.remember) secrets.put(storeName, answer.secret)
            return answer.secret.toCharArray()
        }

        override fun shouldRetry(resource: Resource<*>?): Boolean = attempts < MAX_PASSWORD_ATTEMPTS
    }

    private class FixedPassword(private val password: String) : PasswordFinder {
        override fun reqPassword(resource: Resource<*>?): CharArray = password.toCharArray()
        override fun shouldRetry(resource: Resource<*>?): Boolean = false
    }

    private class HostKeyChanged(val key: HostKeyInfo) : Exception()

    /**
     * Annonce la nouvelle taille au serveur, une fois stabilisée : pendant l'animation
     * du clavier, la taille change à chaque image, et chaque annonce fait redessiner
     * l'écran par le programme distant.
     */
    private fun resizeRemote(dimensions: TerminalDimensions) {
        resizeJob?.cancel()
        resizeJob = scope.launch {
            delay(RESIZE_DEBOUNCE_MS)
            val size = dimensions.columns to dimensions.rows
            val shell = link?.shell ?: return@launch
            if (size == remoteSize) return@launch
            remoteSize = size
            withContext(Dispatchers.IO) {
                runCatching { shell.changeWindowDimensions(size.first, size.second, 0, 0) }
            }
        }
    }

    /**
     * Avant une reconnexion : pousse l'écran précédent dans l'historique de défilement
     * (pour pouvoir le relire) et repart d'un écran vide, que le nouveau shell suppose.
     */
    private fun startFreshScreen() {
        val rows = emulator.dimensions.rows
        val text = "\r\n\u001b[2m── reconnexion ──\u001b[0m" + "\r\n".repeat(rows) + "\u001b[H\u001b[2J"
        val bytes = text.encodeToByteArray()
        runCatching { emulator.writeInput(bytes, 0, bytes.size) }
    }

    /**
     * Ferme une tentative, toujours hors du fil principal : Android y interdit le réseau,
     * et sshj sauterait alors la fermeture du socket (connexion laissée ouverte).
     */
    private fun release(target: Link) {
        if (link === target) link = null
        scope.launch(Dispatchers.IO + NonCancellable) {
            runCatching { target.sftp?.close() }
            runCatching { target.ssh.disconnect() }
        }
    }

    private fun setState(state: ConnectionState) {
        _state.value = state
        onStateChanged()
    }

    private fun describe(e: Exception): String {
        val target = "${session.host}:${session.port}"
        return when (e) {
            is UnknownHostException -> "Hôte introuvable : ${session.host}"
            is ConnectException -> "Connexion refusée par $target"
            is NoRouteToHostException -> "Aucune route vers $target (réseau indisponible ?)"
            is SocketTimeoutException -> "Pas de réponse de $target"
            is TransportException -> "Connexion interrompue : ${e.message ?: e.javaClass.simpleName}"
            // Coupure réseau, serveur arrêté, changement de Wi-Fi…
            is java.io.IOException -> "Connexion perdue avec $target (${e.message ?: e.javaClass.simpleName})"
            else -> e.message ?: e.javaClass.simpleName
        }
    }

    companion object {
        private const val TAG = "SshConnection"
        private const val CONNECT_TIMEOUT_MS = 15_000
        private const val HOST_KEY_TIMEOUT_MS = 5 * 60_000
        private const val TRANSPORT_TIMEOUT_MS = 30_000
        private const val KEEPALIVE_SECONDS = 15
        private const val MAX_PASSWORD_ATTEMPTS = 3
        private const val RESIZE_DEBOUNCE_MS = 200L
        private const val CPU_HISTORY = 60
        private const val MONITOR_ATTEMPTS = 3
        private const val MONITOR_RETRY_MS = 10_000L

        val TERMINAL_BACKGROUND = Color(0xFF141414)
        val TERMINAL_FOREGROUND = Color(0xFFD4D4D4)
    }
}
