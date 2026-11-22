package com.ferroterm

import android.app.Application
import com.ferroterm.data.MacroRepository
import com.ferroterm.data.PrivateKeys
import com.ferroterm.data.SecretStore
import com.ferroterm.data.SessionRepository
import com.ferroterm.ftp.KnownCertificates
import com.ferroterm.ssh.ConnectionManager
import org.bouncycastle.jce.provider.BouncyCastleProvider
import java.io.File
import java.security.Security

class FerrotermApp : Application() {
    lateinit var sessions: SessionRepository
        private set
    lateinit var secrets: SecretStore
        private set
    lateinit var keys: PrivateKeys
        private set
    lateinit var connections: ConnectionManager
        private set
    lateinit var macros: MacroRepository
        private set
    lateinit var knownHosts: File
        private set

    override fun onCreate() {
        super.onCreate()
        installBouncyCastle()
        sessions = SessionRepository(filesDir)
        secrets = SecretStore(filesDir)
        keys = PrivateKeys(filesDir)
        macros = MacroRepository(filesDir)
        knownHosts = File(filesDir, "known_hosts")
        connections = ConnectionManager(this, secrets, keys, knownHosts, KnownCertificates(File(filesDir, "known_certificates")))
    }

    /**
     * Android fournit un BouncyCastle amputé sous le nom « BC » (sans Ed25519, X25519,
     * ChaCha20-Poly1305…). On le remplace par le complet avant le premier appel à sshj,
     * qui l'utilise alors pour toute la cryptographie SSH.
     */
    private fun installBouncyCastle() {
        Security.removeProvider(BouncyCastleProvider.PROVIDER_NAME)
        Security.addProvider(BouncyCastleProvider())
    }
}
