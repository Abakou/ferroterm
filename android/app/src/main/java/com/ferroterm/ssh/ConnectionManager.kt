package com.ferroterm.ssh

import android.content.Context
import android.content.Intent
import android.util.Log
import com.ferroterm.data.PrivateKeys
import com.ferroterm.data.Protocol
import com.ferroterm.ftp.FtpConnection
import com.ferroterm.ftp.KnownCertificates
import com.ferroterm.data.SecretStore
import com.ferroterm.data.Session
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.getAndUpdate
import kotlinx.coroutines.flow.update
import java.io.File
import java.util.concurrent.atomic.AtomicLong

/**
 * Connexions ouvertes (l'équivalent des onglets de la version bureau). Vit aussi
 * longtemps que le processus ; [ConnectionService] le garde en vie tant qu'au moins
 * une connexion est active.
 */
class ConnectionManager(
    private val context: Context,
    private val secrets: SecretStore,
    private val keys: PrivateKeys,
    private val knownHosts: File,
    private val certificates: KnownCertificates,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val nextId = AtomicLong(1)

    private val _connections = MutableStateFlow<List<RemoteConnection>>(emptyList())
    val connections: StateFlow<List<RemoteConnection>> = _connections.asStateFlow()

    /** Nombre de connexions en cours ou établies : sert à la notification du service. */
    private val _activeCount = MutableStateFlow(0)
    val activeCount: StateFlow<Int> = _activeCount.asStateFlow()

    fun get(id: Long): RemoteConnection? = _connections.value.firstOrNull { it.id == id }

    fun open(session: Session): RemoteConnection {
        val id = nextId.getAndIncrement()
        val connection: RemoteConnection = if (session.protocol == Protocol.FTP) {
            FtpConnection(id, session, scope, secrets, certificates, onStateChanged = ::refresh)
        } else {
            SshConnection(id, session, scope, secrets, keys, knownHosts, onStateChanged = ::refresh)
        }
        _connections.update { it + connection }
        connection.connect()
        return connection
    }

    fun reconnect(connection: RemoteConnection) {
        connection.connect()
    }

    /** Ferme la connexion (et son terminal). */
    fun close(id: Long) {
        val connection = get(id) ?: return
        _connections.update { list -> list.filterNot { it.id == id } }
        connection.dispose()
        refresh()
    }

    private fun refresh() {
        val active = _connections.value.count { it.isActive }
        val previous = _activeCount.getAndUpdate { active }
        if (previous == 0 && active > 0) {
            // Les connexions démarrent toujours sur une action de l'utilisateur, application
            // au premier plan : le démarrage est autorisé. Sinon, elles restent ouvertes tant
            // qu'Android garde le processus.
            runCatching { context.startForegroundService(Intent(context, ConnectionService::class.java)) }
                .onFailure { Log.w("ConnectionManager", "Service de premier plan refusé", it) }
        }
    }
}
