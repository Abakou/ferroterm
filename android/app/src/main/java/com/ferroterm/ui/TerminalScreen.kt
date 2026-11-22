package com.ferroterm.ui

import android.app.Activity
import android.content.ClipboardManager
import android.widget.Toast
import androidx.compose.foundation.clickable
import androidx.compose.material3.HorizontalDivider
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.core.view.WindowCompat
import com.ferroterm.FerrotermApp
import com.ferroterm.R
import com.ferroterm.ssh.ConnectionState
import com.ferroterm.ssh.SshConnection
import org.connectbot.terminal.Terminal

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TerminalScreen(
    app: FerrotermApp,
    connectionId: Long,
    onBack: () -> Unit,
    onOpenFiles: () -> Unit,
    onOpenMonitor: () -> Unit,
) {
    val connection = remember(connectionId) { app.connections.get(connectionId) as? SshConnection }
    if (connection == null) {
        // Connexion fermée entre-temps (ou processus relancé par Android).
        LaunchedEffect(Unit) { onBack() }
        return
    }
    val state by connection.state.collectAsState()
    val stats by connection.stats.collectAsState()
    var macros by remember { mutableStateOf(false) }
    val modifiers = remember(connection) { StickyModifiers() }
    val focus = remember { FocusRequester() }
    val context = LocalContext.current
    var menu by remember { mutableStateOf(false) }

    // Hors connexion, la saisie est désactivée : le clavier ne ferait que cacher le message.
    val keyboard = LocalSoftwareKeyboardController.current
    val connected = state == ConnectionState.Connected
    LaunchedEffect(connected) { if (!connected) keyboard?.hide() }

    // Terminal toujours sombre : icônes claires dans la barre d'état, même en thème clair.
    val view = LocalView.current
    DisposableEffect(view) {
        val window = (view.context as? Activity)?.window
        val controller = window?.let { WindowCompat.getInsetsController(it, view) }
        val previous = controller?.isAppearanceLightStatusBars
        controller?.isAppearanceLightStatusBars = false
        onDispose { if (previous != null) controller.isAppearanceLightStatusBars = previous }
    }

    TerminalTheme {
        Scaffold(
            topBar = {
                TopAppBar(
                    title = {
                        // Toucher le titre ouvre la supervision (résumé CPU / RAM en sous-titre).
                        Column(Modifier.clickable(enabled = stats != null, onClick = onOpenMonitor)) {
                            Text(connection.session.displayName, maxLines = 1)
                            Text(
                                stats?.takeIf { connected }?.let(::monitorSummary)?.takeIf { it.isNotEmpty() }
                                    ?: statusText(state),
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    },
                    navigationIcon = {
                        IconButton(onClick = onBack) {
                            Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Sessions")
                        }
                    },
                    actions = {
                        IconButton(onClick = { macros = true }) {
                            Icon(painterResource(R.drawable.ic_macro), contentDescription = "Macros")
                        }
                        IconButton(onClick = onOpenFiles) {
                            Icon(painterResource(R.drawable.ic_folder), contentDescription = "Fichiers (SFTP)")
                        }
                        Box {
                            IconButton(onClick = { menu = true }) {
                                Icon(Icons.Default.MoreVert, contentDescription = "Actions")
                            }
                            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                                DropdownMenuItem(
                                    text = { Text("Coller") },
                                    enabled = connected,
                                    onClick = {
                                        menu = false
                                        val clipboard = context.getSystemService(ClipboardManager::class.java)
                                        val text = clipboard.primaryClip?.takeIf { it.itemCount > 0 }
                                            ?.getItemAt(0)?.coerceToText(context)?.toString()
                                        if (!text.isNullOrEmpty()) connection.emulator.pasteText(text)
                                    },
                                )
                                DropdownMenuItem(
                                    text = { Text("Supervision du serveur") },
                                    leadingIcon = { Icon(painterResource(R.drawable.ic_monitor), contentDescription = null) },
                                    onClick = { menu = false; onOpenMonitor() },
                                )
                                HorizontalDivider()
                                if (state is ConnectionState.Closed) {
                                    DropdownMenuItem(
                                        text = { Text("Reconnecter") },
                                        onClick = { menu = false; app.connections.reconnect(connection) },
                                    )
                                } else {
                                    DropdownMenuItem(
                                        text = { Text("Déconnecter") },
                                        onClick = { menu = false; connection.disconnect() },
                                    )
                                }
                                DropdownMenuItem(
                                    text = { Text("Fermer la session") },
                                    onClick = {
                                        menu = false
                                        app.connections.close(connection.id)
                                        onBack()
                                    },
                                )
                            }
                        }
                    },
                )
            },
        ) { padding ->
            Column(
                Modifier
                    .fillMaxSize()
                    .padding(padding)
                    // Clavier ouvert : la rangée de touches remonte juste au-dessus de lui.
                    .consumeWindowInsets(padding)
                    .imePadding(),
            ) {
                Box(Modifier.weight(1f).fillMaxWidth()) {
                    Terminal(
                        terminalEmulator = connection.emulator,
                        modifier = Modifier.fillMaxSize(),
                        backgroundColor = SshConnection.TERMINAL_BACKGROUND,
                        foregroundColor = SshConnection.TERMINAL_FOREGROUND,
                        keyboardEnabled = connected,
                        focusRequester = focus,
                        modifierManager = modifiers,
                    )
                    StatusOverlay(app, connection, state, Modifier.align(Alignment.TopCenter))
                }
                ExtraKeysRow(emulator = connection.emulator, modifiers = modifiers, enabled = connected)
            }
        }
        ConnectionPrompts(connection)
        if (macros) {
            MacroSheet(
                app = app,
                onDismiss = { macros = false },
                onSend = { macro, execute ->
                    macros = false
                    val bytes = if (execute) macro.input() else macro.commandLine().encodeToByteArray()
                    if (!connection.sendInput(bytes)) {
                        Toast.makeText(context, "Terminal non connecté : « ${macro.name} » n'a pas été envoyée.", Toast.LENGTH_LONG).show()
                    }
                    runCatching { focus.requestFocus() }
                },
            )
        }
    }
}
