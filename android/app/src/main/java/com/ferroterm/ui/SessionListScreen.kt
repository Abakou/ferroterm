package com.ferroterm.ui

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.ferroterm.FerrotermApp
import com.ferroterm.data.Protocol
import com.ferroterm.data.SecretStore
import com.ferroterm.data.Session
import com.ferroterm.ssh.ConnectionState
import com.ferroterm.ssh.RemoteConnection

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SessionListScreen(
    app: FerrotermApp,
    onEdit: (Long) -> Unit,
    onOpenConnection: (Long) -> Unit,
) {
    val sessions by app.sessions.sessions.collectAsStateWithLifecycle()
    val connections by app.connections.connections.collectAsStateWithLifecycle()
    var toDelete by remember { mutableStateOf<Session?>(null) }
    var notice by rememberSaveable { mutableStateOf<String?>(null) }

    // Sans cette permission, la notification du service reste invisible ; la connexion
    // fonctionne quand même. On la demande une fois, à la première connexion.
    val context = LocalContext.current
    var pendingSession by remember { mutableStateOf<Session?>(null) }
    val connect: (Session) -> Unit = { session ->
        onOpenConnection(app.connections.open(session).id)
    }
    val askNotifications = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        pendingSession?.let(connect)
        pendingSession = null
    }
    val connectWithPermission: (Session) -> Unit = { session ->
        val granted = context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED
        if (granted) {
            connect(session)
        } else {
            pendingSession = session
            askNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    val importSessions = rememberSessionImport(app) { notice = it }
    var menu by remember { mutableStateOf(false) }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Ferroterm") },
                actions = {
                    Box {
                        IconButton(onClick = { menu = true }) { Icon(Icons.Default.MoreVert, contentDescription = "Menu") }
                        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                            DropdownMenuItem(
                                text = { Text("Importer des sessions…") },
                                onClick = {
                                    menu = false
                                    importSessions()
                                },
                            )
                        }
                    }
                },
            )
        },
        floatingActionButton = {
            FloatingActionButton(onClick = { onEdit(0L) }) {
                Icon(Icons.Default.Add, contentDescription = "Nouvelle session")
            }
        },
    ) { padding ->
        if (sessions.isEmpty() && connections.isEmpty()) {
            Box(Modifier.fillMaxSize().padding(padding).padding(32.dp), contentAlignment = Alignment.Center) {
                Text(
                    "Aucune session.\nTouchez + pour ajouter un serveur SSH, ou importez " +
                        "celles de Ferroterm sur PC depuis le menu ⋮.",
                    textAlign = TextAlign.Center,
                    style = MaterialTheme.typography.bodyLarge,
                )
            }
            return@Scaffold
        }
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(
                top = padding.calculateTopPadding(),
                bottom = padding.calculateBottomPadding() + 88.dp,
            ),
        ) {
            if (connections.isNotEmpty()) {
                item { SectionHeader("Ouvertes") }
                items(connections, key = { "c${it.id}" }) { connection ->
                    OpenConnectionItem(
                        connection = connection,
                        onOpen = { onOpenConnection(connection.id) },
                        onClose = { app.connections.close(connection.id) },
                    )
                }
                item { HorizontalDivider(Modifier.padding(vertical = 8.dp)) }
            }
            val byFolder = sessions.sortedWith(compareBy({ it.folder.lowercase() }, { it.displayName.lowercase() }))
                .groupBy { it.folder }
            byFolder.forEach { (folder, list) ->
                item(key = "f:$folder") { SectionHeader(folder.ifEmpty { "Sessions" }) }
                items(list, key = { "s${it.id}" }) { session ->
                    SessionItem(
                        session = session,
                        onConnect = { connectWithPermission(session) },
                        onEdit = { onEdit(session.id) },
                        onDelete = { toDelete = session },
                    )
                }
            }
        }
    }

    toDelete?.let { session ->
        AlertDialog(
            onDismissRequest = { toDelete = null },
            title = { Text("Supprimer « ${session.displayName} » ?") },
            text = { Text("Le mot de passe mémorisé et la clé importée pour cette session seront aussi effacés.") },
            confirmButton = {
                TextButton(onClick = {
                    deleteSession(app, session)
                    toDelete = null
                }) { Text("Supprimer") }
            },
            dismissButton = { TextButton(onClick = { toDelete = null }) { Text("Annuler") } },
        )
    }
    notice?.let { message ->
        AlertDialog(
            onDismissRequest = { notice = null },
            text = { Text(message) },
            confirmButton = { TextButton(onClick = { notice = null }) { Text("OK") } },
        )
    }
}

/** Supprime la session, son mot de passe, et sa clé si aucune autre session ne l'utilise. */
private fun deleteSession(app: FerrotermApp, session: Session) {
    app.sessions.remove(session.id)
    app.secrets.remove(SecretStore.passwordName(session.id))
    val key = session.keyPath ?: return
    if (app.sessions.sessions.value.none { it.keyPath == key }) {
        app.secrets.remove(SecretStore.passphraseName(key))
        app.keys.delete(key)
    }
}

@Composable
private fun SectionHeader(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 4.dp),
    )
}

@Composable
private fun OpenConnectionItem(connection: RemoteConnection, onOpen: () -> Unit, onClose: () -> Unit) {
    val state by connection.state.collectAsState()
    val status = when (val s = state) {
        is ConnectionState.Connecting -> s.step
        ConnectionState.Connected -> "Connectée"
        is ConnectionState.Closed -> s.message
    }
    ListItem(
        modifier = Modifier.clickable(onClick = onOpen),
        headlineContent = { Text(connection.session.displayName) },
        supportingContent = {
            Text(
                status,
                maxLines = 1,
                color = if ((state as? ConnectionState.Closed)?.error == true) {
                    MaterialTheme.colorScheme.error
                } else {
                    MaterialTheme.colorScheme.onSurfaceVariant
                },
            )
        },
        trailingContent = {
            IconButton(onClick = onClose) { Icon(Icons.Default.Close, contentDescription = "Fermer") }
        },
    )
}

@Composable
private fun SessionItem(session: Session, onConnect: () -> Unit, onEdit: () -> Unit, onDelete: () -> Unit) {
    var menu by remember { mutableStateOf(false) }
    ListItem(
        modifier = Modifier.clickable(onClick = onConnect),
        headlineContent = { Text(session.displayName) },
        supportingContent = {
            val protocol = if (session.protocol == Protocol.SSH) "" else " · ${session.protocol.name}"
            Text(session.target + protocol, maxLines = 1)
        },
        trailingContent = {
            Box {
                IconButton(onClick = { menu = true }) { Icon(Icons.Default.MoreVert, contentDescription = "Actions") }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    DropdownMenuItem(text = { Text("Modifier") }, onClick = { menu = false; onEdit() })
                    DropdownMenuItem(text = { Text("Supprimer") }, onClick = { menu = false; onDelete() })
                }
            }
        },
    )
}
