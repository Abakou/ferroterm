package com.ferroterm.ftp

import android.util.Log
import com.ferroterm.data.FtpTls
import com.ferroterm.data.SecretStore
import com.ferroterm.data.Session
import com.ferroterm.files.FileBrowser
import com.ferroterm.files.RemoteFiles
import com.ferroterm.ssh.ConnectionState
import com.ferroterm.ssh.CredentialsAnswer
import com.ferroterm.ssh.HostKeyInfo
import com.ferroterm.ssh.Prompt
import com.ferroterm.ssh.RemoteConnection
import com.ferroterm.ssh.awaitAnswer
import com.ferroterm.ssh.dismiss
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.runInterruptible
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.apache.commons.net.ftp.FTP
import org.apache.commons.net.ftp.FTPClient
import org.apache.commons.net.ftp.FTPReply
import org.apache.commons.net.ftp.FTPSClient
import java.io.IOException
import java.net.ConnectException
import java.net.NoRouteToHostException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import java.time.Duration
import java.util.concurrent.Semaphore
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLException
import javax.net.ssl.SSLHandshakeException

/** L'utilisateur a annulé une question : on arrête la connexion sans message d'erreur. */
private class Cancelled : RuntimeException()

/**
 * Session FTP ou FTPS : un explorateur de fichiers, sans terminal (comme sur le bureau).
 *
 * - FTPS : certificat vérifié avec les autorités du téléphone ; un certificat non reconnu
 *   (auto-signé…) est soumis à l'utilisateur avec son empreinte, puis retenu s'il l'accepte.
 * - « TLS si disponible » : FTP en clair si le serveur refuse `AUTH TLS` (comme FileZilla).
 * - La connexion de contrôle est entretenue par un NOOP régulier, et rétablie sans rien
 *   demander si elle tombe.
 */
class FtpConnection(
    override val id: Long,
    override val session: Session,
    private val scope: CoroutineScope,
    private val secrets: SecretStore,
    private val certificates: KnownCertificates,
    private val onStateChanged: () -> Unit,
) : RemoteConnection {
    private val _state = MutableStateFlow<ConnectionState>(ConnectionState.Connecting("Connexion…"))
    override val state: StateFlow<ConnectionState> = _state.asStateFlow()

    private val _prompt = MutableStateFlow<Prompt?>(null)
    override val prompt: StateFlow<Prompt?> = _prompt.asStateFlow()

    override val files = FileBrowser(this, scope)
    override val hasShell = false

    /** Identifiants retenus après une connexion réussie : transferts et reconnexions sans question. */
    private class Login(val user: String, val password: String)

    @Volatile private var login: Login? = null

    /** Chiffrement effectif : « TLS si disponible » devient « aucun » si le serveur le refuse. */
    @Volatile private var tls: FtpTls = session.ftp.tls

    /** Empreinte du certificat approuvé par l'utilisateur, s'il y en a un. */
    @Volatile private var trusted: String? = null

    @Volatile private var control: FTPClient? = null
    private val controlLock = Mutex()
    private var job: Job? = null
    private val transferSlots = Semaphore(MAX_TRANSFERS)

    /** Dossier de départ, lu après la connexion. */
    @Volatile var homeDir: String = "/"
        private set

    /** MLSD (listes normalisées) si le serveur l'annonce ; sinon LIST. */
    @Volatile var useMlsd: Boolean = false

    override fun connect() {
        if (job?.isActive == true) return
        tls = session.ftp.tls
        setState(ConnectionState.Connecting("Connexion à ${session.host}:${session.port}…"))
        job = scope.launch(Dispatchers.IO) { run() }
    }

    override fun disconnect() {
        job?.cancel()
        control?.let(::release)
        control = null
        _prompt.value?.dismiss()
        if (_state.value !is ConnectionState.Closed) setState(ConnectionState.Closed("Déconnecté", error = false))
    }

    override fun dispose() {
        files.cancelAll()
        disconnect()
    }

    override fun forgetChangedHostKey(key: HostKeyInfo) {
        certificates.remove(session.host, session.port)
    }

    override suspend fun remoteFiles(): RemoteFiles = controlLock.withLock {
        if (_state.value != ConnectionState.Connected) throw IOException("Session non connectée")
        val client = control ?: withContext(Dispatchers.IO) { newClient() }.also { control = it }
        FtpFiles(this, client)
    }

    override fun discard(files: RemoteFiles) {
        val broken = (files as? FtpFiles)?.control ?: return
        if (control === broken) {
            control = null
            release(broken)
        }
    }

    /** Connexion dédiée à un transfert ([MAX_TRANSFERS] au plus, les serveurs limitant les connexions). */
    internal fun openTransferClient(): FTPClient {
        transferSlots.acquire()
        return try {
            newClient()
        } catch (e: Exception) {
            transferSlots.release()
            throw e
        }
    }

    internal fun closeTransferClient(client: FTPClient) {
        try {
            close(client)
        } finally {
            transferSlots.release()
        }
    }

    private suspend fun run() {
        var client: FTPClient? = null
        try {
            trusted = certificates.get(session.host, session.port)
            client = openTrusting()
            authenticate(client)
            configure(client)
            control = client
            setState(ConnectionState.Connected)
            if (session.ftp.tls == FtpTls.AUTO && tls == FtpTls.OFF) {
                files.notify("Le serveur ne propose pas TLS : connexion non chiffrée.")
            }
            keepAlive()
        } catch (e: Cancelled) {
            finish(ConnectionState.Closed("Connexion annulée", error = false))
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            Log.w(TAG, "Connexion FTP ${session.host} terminée", e)
            finish(ConnectionState.Closed(describe(e), error = true))
        } finally {
            val current = control ?: client
            control = null
            current?.let(::release)
        }
    }

    /** NOOP régulier ; connexion de contrôle rétablie sans question si elle tombe. */
    private suspend fun keepAlive() {
        while (currentCoroutineContext().isActive) {
            delay(KEEPALIVE_MS)
            val current = control
            val alive = current != null && runCatching {
                runInterruptible { synchronized(current) { current.sendNoOp() } }
            }.getOrDefault(false)
            if (!alive) {
                controlLock.withLock {
                    if (control === current) {
                        current?.let(::release)
                        control = runInterruptible { newClient() }
                    }
                }
            }
        }
    }

    /** Ouvre la connexion de contrôle ; un certificat non reconnu est soumis à l'utilisateur. */
    private suspend fun openTrusting(): FTPClient {
        while (true) {
            val check = CertificateCheck(trusted)
            try {
                return runInterruptible { open(tls, check) }
            } catch (e: IOException) {
                val rejection = check.rejection
                if (rejection != null) {
                    val prompt = Prompt.Certificate(
                        host = "${session.host}:${session.port}",
                        reason = rejection.reason,
                        fingerprint = rejection.fingerprint,
                        changed = trusted != null,
                    )
                    if (!ask(prompt).answer.await()) throw Cancelled()
                    certificates.put(session.host, session.port, rejection.fingerprint)
                    trusted = rejection.fingerprint
                    continue
                }
                if (isAuthRefused(e)) {
                    // AUTH TLS refusé : le serveur ne propose pas TLS.
                    if (tls == FtpTls.AUTO) {
                        tls = FtpTls.OFF
                        continue
                    }
                    throw IOException("le serveur refuse TLS (« ${e.message?.trim()} »)", e)
                }
                throw e
            }
        }
    }

    /** Connexion authentifiée et prête (transferts, reconnexion), sans rien demander. */
    private fun newClient(): FTPClient {
        val credentials = login ?: throw IOException("Session non authentifiée")
        val check = CertificateCheck(trusted)
        val client = try {
            open(tls, check)
        } catch (e: IOException) {
            check.rejection?.let { throw IOException("certificat du serveur non reconnu : ${it.reason}", e) }
            throw e
        }
        try {
            client.check(client.login(credentials.user, credentials.password))
            configure(client)
        } catch (e: Exception) {
            close(client)
            throw e
        }
        return client
    }

    /** Connexion de contrôle, chiffrée selon [mode] (sans authentification). */
    private fun open(mode: FtpTls, check: CertificateCheck): FTPClient {
        val client = if (mode == FtpTls.OFF) {
            FTPClient()
        } else {
            val base = SSLContext.getInstance("TLS").apply { init(null, arrayOf(check), null) }
            FTPSClient(mode == FtpTls.IMPLICIT, SessionReuseContext(base, session.host, session.port)).apply {
                hostnameVerifier = javax.net.ssl.HostnameVerifier { host, sslSession -> check.verifyHost(host, sslSession) }
            }
        }
        client.controlEncoding = "UTF-8"
        client.connectTimeout = CONNECT_TIMEOUT_MS
        client.defaultTimeout = COMMAND_TIMEOUT_MS
        client.setDataTimeout(Duration.ofMillis(COMMAND_TIMEOUT_MS.toLong()))
        try {
            client.connect(session.host, session.port)
            client.soTimeout = COMMAND_TIMEOUT_MS
            if (!FTPReply.isPositiveCompletion(client.replyCode)) {
                throw FtpReplyException(client.replyCode, client.replyString.orEmpty())
            }
        } catch (e: Exception) {
            runCatching { client.disconnect() }
            throw e
        }
        return client
    }

    /** Binaire, passif (ou actif), données chiffrées en FTPS ; dossier de départ et MLSD. */
    private fun configure(client: FTPClient) {
        client.check(client.setFileType(FTP.BINARY_FILE_TYPE))
        if (client is FTPSClient) {
            client.execPBSZ(0)
            client.execPROT("P")
        }
        if (session.ftp.activeMode) {
            client.enterLocalActiveMode()
        } else {
            client.enterLocalPassiveMode()
            // Adresse de la connexion de contrôle plutôt que celle annoncée par le serveur
            // (souvent privée quand il est derrière un NAT), comme la version bureau.
            val controlAddress = client.remoteAddress.hostAddress
            client.setPassiveNatWorkaroundStrategy { controlAddress }
        }
        if (control == null) {
            homeDir = client.printWorkingDirectory() ?: "/"
            useMlsd = runCatching { client.hasFeature("MLST") }.getOrDefault(false)
        }
    }

    /** Mêmes étapes que pour SSH : mot de passe mémorisé, puis saisie (trois essais). */
    private suspend fun authenticate(client: FTPClient) {
        var user = session.username
        if (user.isEmpty()) {
            user = askCredentials(
                Prompt.Credentials(
                    title = "Connexion à ${session.host}",
                    username = null,
                    askUsername = true,
                    secretLabel = null,
                    error = null,
                    canRemember = false,
                    usernameHint = "anonymous pour une connexion anonyme",
                ),
            ).username
        }
        setState(ConnectionState.Connecting("Authentification…"))
        if (isAnonymous(user)) {
            if (!runInterruptible { client.login(user, ANONYMOUS_PASSWORD) }) {
                throw IOException("Connexion anonyme refusée (${client.replyString?.trim()})")
            }
            login = Login(user, ANONYMOUS_PASSWORD)
            return
        }
        val storeName = SecretStore.passwordName(session.id)
        var stored = if (session.id != 0L) secrets.get(storeName) else null
        var error: String? = null
        repeat(MAX_PASSWORD_ATTEMPTS) {
            val answer = stored?.let { CredentialsAnswer(user, it, remember = false) } ?: askCredentials(
                Prompt.Credentials(
                    title = "Connexion à ${session.host}",
                    username = user,
                    askUsername = false,
                    secretLabel = "Mot de passe",
                    error = error,
                    canRemember = session.id != 0L,
                ),
            )
            setState(ConnectionState.Connecting("Authentification…"))
            if (runInterruptible { client.login(user, answer.secret) }) {
                if (answer.remember) secrets.put(storeName, answer.secret)
                login = Login(user, answer.secret)
                return
            }
            if (!FTPReply.isNegativePermanent(client.replyCode)) {
                throw FtpReplyException(client.replyCode, client.replyString.orEmpty())
            }
            if (stored != null) {
                // Mot de passe mémorisé périmé : on le redemande, et on l'oublie.
                secrets.remove(storeName)
                stored = null
            }
            error = "Mot de passe refusé."
        }
        throw IOException("Authentification refusée pour « $user ».")
    }

    private suspend fun askCredentials(prompt: Prompt.Credentials): CredentialsAnswer =
        ask(prompt).answer.await() ?: throw Cancelled()

    private suspend fun <P : Prompt> ask(prompt: P): P {
        _prompt.value = prompt
        try {
            prompt.awaitAnswer()
        } finally {
            _prompt.compareAndSet(prompt, null)
        }
        return prompt
    }

    private suspend fun finish(state: ConnectionState.Closed) {
        if (currentCoroutineContext().isActive) setState(state)
    }

    private fun setState(state: ConnectionState) {
        _state.value = state
        onStateChanged()
    }

    /** Ferme hors du fil principal : Android y interdit le réseau. */
    private fun release(client: FTPClient) {
        scope.launch(Dispatchers.IO + NonCancellable) { close(client) }
    }

    private fun close(client: FTPClient) {
        runCatching { if (client.isConnected) client.logout() }
        runCatching { client.disconnect() }
    }

    private fun isAuthRefused(e: IOException): Boolean =
        e is SSLException && e !is SSLHandshakeException && e.message?.trim()?.matches(Regex("^[45]\\d\\d\\b.*", RegexOption.DOT_MATCHES_ALL)) == true

    private fun describe(e: Exception): String {
        val target = "${session.host}:${session.port}"
        return when (e) {
            is UnknownHostException -> "Hôte introuvable : ${session.host}"
            is ConnectException -> "Connexion refusée par $target"
            is NoRouteToHostException -> "Aucune route vers $target (réseau indisponible ?)"
            is SocketTimeoutException -> "Pas de réponse de $target"
            is FtpReplyException -> "Refusé par le serveur : ${e.message}"
            is SSLHandshakeException -> "Échec de la négociation TLS : ${e.message}"
            is IOException -> e.message ?: "Connexion perdue avec $target"
            else -> e.message ?: e.javaClass.simpleName
        }
    }

    private companion object {
        const val TAG = "FtpConnection"
        const val CONNECT_TIMEOUT_MS = 15_000
        const val COMMAND_TIMEOUT_MS = 30_000
        const val KEEPALIVE_MS = 60_000L
        const val MAX_TRANSFERS = 2
        const val MAX_PASSWORD_ATTEMPTS = 3
        const val ANONYMOUS_PASSWORD = "anonymous@"

        fun isAnonymous(user: String) = user.equals("anonymous", ignoreCase = true) || user.equals("ftp", ignoreCase = true)
    }
}
