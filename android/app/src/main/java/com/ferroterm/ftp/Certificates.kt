package com.ferroterm.ftp

import java.io.File
import java.net.Socket
import java.security.KeyStore
import java.security.MessageDigest
import java.security.cert.CertPathValidatorException
import java.security.cert.CertificateException
import java.security.cert.CertificateExpiredException
import java.security.cert.CertificateNotYetValidException
import java.security.cert.X509Certificate
import javax.net.ssl.HttpsURLConnection
import javax.net.ssl.KeyManager
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLContextSpi
import javax.net.ssl.SSLEngine
import javax.net.ssl.SSLParameters
import javax.net.ssl.SSLServerSocketFactory
import javax.net.ssl.SSLSession
import javax.net.ssl.SSLSessionContext
import javax.net.ssl.SSLSocketFactory
import javax.net.ssl.TrustManager
import javax.net.ssl.TrustManagerFactory
import javax.net.ssl.X509TrustManager

/**
 * Certificats FTPS approuvés par l'utilisateur, un par ligne : `hôte:port empreinte`
 * (même format que le fichier `known_certificates` de la version bureau).
 */
class KnownCertificates(private val file: File) {
    @Synchronized
    fun get(host: String, port: Int): String? = read()[key(host, port)]

    @Synchronized
    fun put(host: String, port: Int, fingerprint: String) = write(read() + (key(host, port) to fingerprint))

    @Synchronized
    fun remove(host: String, port: Int) = write(read() - key(host, port))

    private fun key(host: String, port: Int) = "${host.lowercase()}:$port"

    private fun read(): Map<String, String> {
        if (!file.exists()) return emptyMap()
        return file.readLines().mapNotNull { line ->
            val parts = line.trim().split(Regex("\\s+"), limit = 2)
            if (parts.size == 2) parts[0] to parts[1] else null
        }.toMap()
    }

    private fun write(entries: Map<String, String>) {
        file.writeText(entries.entries.joinToString("") { "${it.key} ${it.value}\n" })
    }
}

/** Empreinte SHA-256 en hexadécimal séparé par des « : », comme les navigateurs (et le bureau). */
fun certificateFingerprint(cert: X509Certificate): String =
    MessageDigest.getInstance("SHA-256").digest(cert.encoded).joinToString(":") { "%02X".format(it) }

/** Certificat refusé : pourquoi, et son empreinte (à soumettre à l'utilisateur). */
data class Rejection(val reason: String, val fingerprint: String)

/**
 * Vérification habituelle (autorités du téléphone, validité, nom d'hôte). En cas d'échec,
 * le certificat est tout de même accepté s'il a déjà été approuvé (même empreinte) ;
 * sinon il est refusé et noté dans [rejection] pour être soumis à l'utilisateur.
 */
class CertificateCheck(private val trusted: String?) : X509TrustManager {
    private val system: X509TrustManager = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm())
        .apply { init(null as KeyStore?) }
        .trustManagers.filterIsInstance<X509TrustManager>().first()

    @Volatile var rejection: Rejection? = null
        private set

    override fun checkServerTrusted(chain: Array<out X509Certificate>, authType: String) {
        try {
            system.checkServerTrusted(chain, authType)
        } catch (e: CertificateException) {
            val fingerprint = certificateFingerprint(chain[0])
            if (fingerprint == trusted) return
            rejection = Rejection(describe(e), fingerprint)
            throw e
        }
    }

    /** Nom d'hôte : vérifié après la négociation TLS (voir FTPSClient.setHostnameVerifier). */
    fun verifyHost(host: String, session: SSLSession): Boolean {
        if (HttpsURLConnection.getDefaultHostnameVerifier().verify(host, session)) return true
        val cert = session.peerCertificates.firstOrNull() as? X509Certificate ?: return false
        val fingerprint = certificateFingerprint(cert)
        if (fingerprint == trusted) return true
        rejection = Rejection("il n'a pas été établi pour ce nom d'hôte", fingerprint)
        return false
    }

    override fun checkClientTrusted(chain: Array<out X509Certificate>, authType: String) =
        throw CertificateException("Pas de certificat client")

    override fun getAcceptedIssuers(): Array<X509Certificate> = system.acceptedIssuers

    private fun describe(e: CertificateException): String {
        val causes = generateSequence(e as Throwable) { it.cause }.take(8).toList()
        return when {
            causes.any { it is CertificateExpiredException } -> "il a expiré"
            causes.any { it is CertificateNotYetValidException } -> "il n'est pas encore valable"
            causes.any { it is CertPathValidatorException } ->
                "il n'est pas signé par une autorité reconnue (certificat auto-signé ?)"
            else -> "il est invalide (${e.message})"
        }
    }
}

/**
 * Contexte TLS dont toutes les connexions (contrôle et données) sont ouvertes sous le
 * même nom (hôte et port de contrôle). Le cache de sessions d'Android étant indexé par
 * hôte et port, les connexions de données reprennent ainsi la session TLS de la
 * connexion de contrôle, ce qu'exigent certains serveurs (vsftpd : `require_ssl_reuse`).
 */
class SessionReuseContext(base: SSLContext, host: String, port: Int) :
    SSLContext(ReuseSpi(base, host, port), base.provider, base.protocol) {

    private class ReuseSpi(private val base: SSLContext, private val host: String, private val port: Int) : SSLContextSpi() {
        override fun engineInit(km: Array<out KeyManager>?, tm: Array<out TrustManager>?, sr: java.security.SecureRandom?) =
            base.init(km, tm, sr)

        override fun engineGetSocketFactory(): SSLSocketFactory = ReuseSocketFactory(base.socketFactory, host, port)
        override fun engineGetServerSocketFactory(): SSLServerSocketFactory = base.serverSocketFactory
        override fun engineCreateSSLEngine(): SSLEngine = base.createSSLEngine()
        override fun engineCreateSSLEngine(host: String?, port: Int): SSLEngine = base.createSSLEngine(host, port)
        override fun engineGetServerSessionContext(): SSLSessionContext = base.serverSessionContext
        override fun engineGetClientSessionContext(): SSLSessionContext = base.clientSessionContext
        override fun engineGetDefaultSSLParameters(): SSLParameters = base.defaultSSLParameters
        override fun engineGetSupportedSSLParameters(): SSLParameters = base.supportedSSLParameters
    }

    private class ReuseSocketFactory(
        private val base: SSLSocketFactory,
        private val host: String,
        private val port: Int,
    ) : SSLSocketFactory() {
        override fun createSocket(s: Socket, host: String?, port: Int, autoClose: Boolean): Socket =
            base.createSocket(s, this.host, this.port, autoClose)

        override fun getDefaultCipherSuites(): Array<String> = base.defaultCipherSuites
        override fun getSupportedCipherSuites(): Array<String> = base.supportedCipherSuites
        override fun createSocket(host: String?, port: Int): Socket = base.createSocket(host, port)
        override fun createSocket(host: String?, port: Int, localHost: java.net.InetAddress?, localPort: Int): Socket =
            base.createSocket(host, port, localHost, localPort)
        override fun createSocket(host: java.net.InetAddress?, port: Int): Socket = base.createSocket(host, port)
        override fun createSocket(address: java.net.InetAddress?, port: Int, localAddress: java.net.InetAddress?, localPort: Int): Socket =
            base.createSocket(address, port, localAddress, localPort)
        /**
         * Socket de données (`PROT P`, mode passif) : Commons Net la crée non connectée
         * puis la connecte au port de données. TLS n'est négocié qu'une fois connectée,
         * sous le nom de la connexion de contrôle.
         */
        override fun createSocket(): Socket = DeferredSslSocket(base, host, port)
    }
}

/**
 * Socket TCP ordinaire jusqu'à sa connexion, puis enveloppée dans TLS par
 * `factory.createSocket(socket, host, port)` : c'est ce couple (hôte, port) qui sert de
 * clé au cache de sessions, et donc à la reprise de la session de contrôle.
 */
private class DeferredSslSocket(
    private val factory: SSLSocketFactory,
    private val host: String,
    private val port: Int,
) : javax.net.ssl.SSLSocket() {
    /**
     * Conscrypt (le TLS d'Android) indexe son cache avec le port de la socket sous-jacente,
     * pas avec celui passé à `createSocket` : celle-ci annonce donc le port de contrôle.
     */
    private val plain = object : Socket() {
        override fun getPort(): Int = if (isConnected) this@DeferredSslSocket.port else super.getPort()
    }
    @Volatile private var tls: javax.net.ssl.SSLSocket? = null
    private val ssl: javax.net.ssl.SSLSocket
        get() = tls ?: throw java.net.SocketException("Connexion TLS pas encore établie")

    override fun connect(endpoint: java.net.SocketAddress?, timeout: Int) {
        plain.connect(endpoint, timeout)
        tls = factory.createSocket(plain, host, port, true) as javax.net.ssl.SSLSocket
    }

    override fun connect(endpoint: java.net.SocketAddress?) = connect(endpoint, 0)
    override fun bind(bindpoint: java.net.SocketAddress?) = plain.bind(bindpoint)
    override fun getInputStream(): java.io.InputStream = ssl.inputStream
    override fun getOutputStream(): java.io.OutputStream = ssl.outputStream
    override fun close() = tls?.close() ?: plain.close()
    override fun isClosed(): Boolean = tls?.isClosed ?: plain.isClosed
    override fun isConnected(): Boolean = plain.isConnected
    override fun isBound(): Boolean = plain.isBound
    override fun getInetAddress(): java.net.InetAddress? = plain.inetAddress
    override fun getPort(): Int = plain.port
    override fun getLocalAddress(): java.net.InetAddress? = plain.localAddress
    override fun getLocalPort(): Int = plain.localPort
    override fun getRemoteSocketAddress(): java.net.SocketAddress? = plain.remoteSocketAddress
    override fun getLocalSocketAddress(): java.net.SocketAddress? = plain.localSocketAddress
    override fun setSoTimeout(timeout: Int) { tls?.soTimeout = timeout; plain.soTimeout = timeout }
    override fun getSoTimeout(): Int = plain.soTimeout
    override fun setReceiveBufferSize(size: Int) { plain.receiveBufferSize = size }
    override fun getReceiveBufferSize(): Int = plain.receiveBufferSize
    override fun setSendBufferSize(size: Int) { plain.sendBufferSize = size }
    override fun getSendBufferSize(): Int = plain.sendBufferSize
    override fun setTcpNoDelay(on: Boolean) { plain.tcpNoDelay = on }
    override fun getTcpNoDelay(): Boolean = plain.tcpNoDelay
    override fun setKeepAlive(on: Boolean) { plain.keepAlive = on }
    override fun getKeepAlive(): Boolean = plain.keepAlive
    override fun setSoLinger(on: Boolean, linger: Int) = plain.setSoLinger(on, linger)
    override fun getSoLinger(): Int = plain.soLinger
    override fun shutdownInput() = ssl.shutdownInput()
    override fun shutdownOutput() = ssl.shutdownOutput()
    override fun isInputShutdown(): Boolean = tls?.isInputShutdown ?: plain.isInputShutdown
    override fun isOutputShutdown(): Boolean = tls?.isOutputShutdown ?: plain.isOutputShutdown

    override fun getSupportedCipherSuites(): Array<String> = ssl.supportedCipherSuites
    override fun getEnabledCipherSuites(): Array<String> = ssl.enabledCipherSuites
    override fun setEnabledCipherSuites(suites: Array<out String>?) { ssl.enabledCipherSuites = suites }
    override fun getSupportedProtocols(): Array<String> = ssl.supportedProtocols
    override fun getEnabledProtocols(): Array<String> = ssl.enabledProtocols
    override fun setEnabledProtocols(protocols: Array<out String>?) { ssl.enabledProtocols = protocols }
    override fun getSession(): SSLSession = ssl.session
    override fun getHandshakeSession(): SSLSession? = ssl.handshakeSession
    override fun addHandshakeCompletedListener(listener: javax.net.ssl.HandshakeCompletedListener?) =
        ssl.addHandshakeCompletedListener(listener)
    override fun removeHandshakeCompletedListener(listener: javax.net.ssl.HandshakeCompletedListener?) =
        ssl.removeHandshakeCompletedListener(listener)
    override fun startHandshake() = ssl.startHandshake()
    override fun setUseClientMode(mode: Boolean) { ssl.useClientMode = mode }
    override fun getUseClientMode(): Boolean = ssl.useClientMode
    override fun setNeedClientAuth(need: Boolean) { ssl.needClientAuth = need }
    override fun getNeedClientAuth(): Boolean = ssl.needClientAuth
    override fun setWantClientAuth(want: Boolean) { ssl.wantClientAuth = want }
    override fun getWantClientAuth(): Boolean = ssl.wantClientAuth
    override fun setEnableSessionCreation(flag: Boolean) { ssl.enableSessionCreation = flag }
    override fun getEnableSessionCreation(): Boolean = ssl.enableSessionCreation
    override fun getSSLParameters(): SSLParameters = ssl.sslParameters
    override fun setSSLParameters(params: SSLParameters?) { ssl.sslParameters = params }
}
