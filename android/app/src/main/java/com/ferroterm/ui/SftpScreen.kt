package com.ferroterm.ui

import android.content.ActivityNotFoundException
import android.content.ContentResolver
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.painter.Painter
import android.app.Activity
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import com.ferroterm.FerrotermApp
import com.ferroterm.R
import com.ferroterm.files.RemoteEntry
import com.ferroterm.files.RemotePaths
import com.ferroterm.files.Transfer
import com.ferroterm.ssh.ConnectionState
import com.ferroterm.ssh.SshConnection
import java.io.File
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

/** Ce que l'utilisateur est en train de faire sur un élément (dialogue ouvert). */
private sealed interface EntryDialog {
    data class Rename(val entry: RemoteEntry) : EntryDialog
    data class Delete(val entry: RemoteEntry) : EntryDialog
    data object NewFolder : EntryDialog
    data class Overwrite(val uris: List<Uri>, val existing: List<String>) : EntryDialog
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SftpScreen(app: FerrotermApp, connectionId: Long, onBack: () -> Unit, onOpenTerminal: () -> Unit) {
    val connection = remember(connectionId) { app.connections.get(connectionId) }
    if (connection == null) {
        LaunchedEffect(Unit) { onBack() }
        return
    }
    val browser = connection.files
    val state by connection.state.collectAsState()
    val path by browser.path.collectAsState()
    val entries by browser.entries.collectAsState()
    val loading by browser.loading.collectAsState()
    val error by browser.error.collectAsState()
    val transfers by browser.transfers.collectAsState()
    val connected = state == ConnectionState.Connected
    // Dossier courant du shell (sessions SSH, serveur Linux), connu par la supervision.
    val monitorStats = (connection as? SshConnection)?.stats?.collectAsState()
    val terminalDir = monitorStats?.value?.cwd

    var showHidden by rememberSaveable { mutableStateOf(false) }
    var selected by remember { mutableStateOf<RemoteEntry?>(null) }
    var dialog by remember { mutableStateOf<EntryDialog?>(null) }
    var pendingDownload by remember { mutableStateOf<RemoteEntry?>(null) }
    var menu by remember { mutableStateOf(false) }
    val snackbar = remember { SnackbarHostState() }
    val context = LocalContext.current

    // Arrivée depuis le terminal : son clavier cacherait la liste et le bouton d'envoi.
    // Il a été ouvert par la vue du terminal : on le masque au niveau de la fenêtre.
    val view = LocalView.current
    LaunchedEffect(Unit) {
        (view.context as? Activity)?.window?.let { window ->
            WindowCompat.getInsetsController(window, view).hide(WindowInsetsCompat.Type.ime())
        }
    }
    LaunchedEffect(browser) { browser.messages.collect { snackbar.showSnackbar(it) } }
    // Première ouverture, ou retour de la connexion : (re)lit le dossier courant.
    LaunchedEffect(connected) { if (connected) browser.refreshOrStart() }

    val downloadLauncher = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("*/*")) { uri ->
        val entry = pendingDownload
        pendingDownload = null
        if (uri != null && entry != null) browser.download(app.contentResolver, entry, uri)
    }
    val uploadLauncher = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isEmpty()) return@rememberLauncherForActivityResult
        val existing = browser.existingNames()
        val clashes = uris.map { displayName(app.contentResolver, it) }.filter { it in existing }
        if (clashes.isEmpty()) browser.upload(app.contentResolver, uris) else dialog = EntryDialog.Overwrite(uris, clashes)
    }

    val openFile: (RemoteEntry) -> Unit = { entry ->
        browser.downloadToCache(app.cacheDir, entry) { file -> openWithApp(context, file) { snackbarMessage ->
            browser.notify(snackbarMessage)
        } }
    }

    // Retour : remonte jusqu'au dossier de départ, puis quitte l'écran.
    val current = path
    BackHandler(enabled = current != null && current != browser.home && RemotePaths.parentOf(current) != null) {
        browser.up()
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(connection.session.displayName, maxLines = 1)
                        Text(
                            if (connected) path ?: "…" else statusText(state),
                            style = MaterialTheme.typography.bodySmall,
                            fontFamily = if (connected) FontFamily.Monospace else null,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            maxLines = 1,
                            overflow = TextOverflow.StartEllipsis,
                        )
                    }
                },
                navigationIcon = {
                    IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Retour") }
                },
                actions = {
                    if (connection.hasShell) {
                        IconButton(onClick = onOpenTerminal) {
                            Icon(painterResource(R.drawable.ic_terminal), contentDescription = "Terminal")
                        }
                    }
                    IconButton(enabled = connected, onClick = { browser.refresh() }) {
                        Icon(Icons.Default.Refresh, contentDescription = "Actualiser")
                    }
                    Box {
                        IconButton(onClick = { menu = true }) { Icon(Icons.Default.MoreVert, contentDescription = "Actions") }
                        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                            DropdownMenuItem(
                                text = { Text("Nouveau dossier…") },
                                enabled = connected && path != null,
                                leadingIcon = { Icon(painterResource(R.drawable.ic_new_folder), null) },
                                onClick = { menu = false; dialog = EntryDialog.NewFolder },
                            )
                            DropdownMenuItem(
                                text = { Text("Dossier personnel") },
                                enabled = connected,
                                onClick = { menu = false; browser.goHome() },
                            )
                            // Comme le bureau : l'explorateur peut suivre le dossier courant du shell,
                            // retrouvé par la supervision (serveurs Linux).
                            terminalDir?.let { dir ->
                                DropdownMenuItem(
                                    text = { Text("Dossier du terminal") },
                                    enabled = connected && dir != path,
                                    leadingIcon = { Icon(painterResource(R.drawable.ic_terminal), null) },
                                    onClick = { menu = false; browser.open(dir) },
                                )
                            }
                            DropdownMenuItem(
                                text = { Text(if (showHidden) "Masquer les fichiers cachés" else "Afficher les fichiers cachés") },
                                onClick = { menu = false; showHidden = !showHidden },
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
                                onClick = { menu = false; app.connections.close(connection.id); onBack() },
                            )
                        }
                    }
                },
            )
        },
        floatingActionButton = {
            if (connected && path != null) {
                FloatingActionButton(onClick = { uploadLauncher.launch(arrayOf("*/*")) }) {
                    Icon(painterResource(R.drawable.ic_upload), contentDescription = "Envoyer des fichiers ici")
                }
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
        bottomBar = { TransfersPanel(transfers, onCancel = browser::cancelTransfer) },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding)) {
            if (connected || entries.isNotEmpty()) {
                FileList(
                    path = path,
                    entries = if (showHidden) entries else entries.filterNot { it.name.startsWith(".") },
                    hiddenCount = if (showHidden) 0 else entries.count { it.name.startsWith(".") },
                    error = error,
                    enabled = connected,
                    onUp = browser::up,
                    onRetry = browser::refresh,
                    onClick = { entry -> if (entry.isDirectory) browser.open(entry.path) else selected = entry },
                    onMore = { selected = it },
                )
            }
            if (loading && connected) LinearProgressIndicator(Modifier.fillMaxWidth().align(Alignment.TopCenter))
            StatusOverlay(app, connection, state, Modifier.align(Alignment.TopCenter))
        }
    }

    ConnectionPrompts(connection)

    selected?.let { entry ->
        EntrySheet(
            entry = entry,
            onDismiss = { selected = null },
            onOpenDir = { browser.open(entry.path) },
            onOpenFile = { openFile(entry) },
            onDownload = { pendingDownload = entry; downloadLauncher.launch(entry.name) },
            onRename = { dialog = EntryDialog.Rename(entry) },
            onDelete = { dialog = EntryDialog.Delete(entry) },
        )
    }

    when (val d = dialog) {
        EntryDialog.NewFolder -> NameDialog(
            title = "Nouveau dossier",
            initial = "",
            confirm = "Créer",
            onConfirm = { name -> dialog = null; browser.mkdir(name) },
            onDismiss = { dialog = null },
        )
        is EntryDialog.Rename -> NameDialog(
            title = "Renommer « ${d.entry.name} »",
            initial = d.entry.name,
            confirm = "Renommer",
            onConfirm = { name -> dialog = null; if (name != d.entry.name) browser.rename(d.entry, name) },
            onDismiss = { dialog = null },
        )
        is EntryDialog.Delete -> AlertDialog(
            onDismissRequest = { dialog = null },
            title = { Text("Supprimer « ${d.entry.name} » ?") },
            text = {
                Text(
                    if (d.entry.isDirectory && !d.entry.isLink) {
                        "Le dossier doit être vide. La suppression est définitive."
                    } else {
                        "La suppression sur le serveur est définitive."
                    },
                )
            },
            confirmButton = { TextButton(onClick = { dialog = null; browser.delete(d.entry) }) { Text("Supprimer") } },
            dismissButton = { TextButton(onClick = { dialog = null }) { Text("Annuler") } },
        )
        is EntryDialog.Overwrite -> AlertDialog(
            onDismissRequest = { dialog = null },
            title = { Text(if (d.existing.size == 1) "Remplacer « ${d.existing[0]} » ?" else "Remplacer ${d.existing.size} fichiers ?") },
            text = {
                Text(
                    if (d.existing.size == 1) "Un fichier de ce nom existe déjà dans ce dossier."
                    else "Ces fichiers existent déjà : " + d.existing.joinToString(", "),
                )
            },
            confirmButton = {
                TextButton(onClick = { dialog = null; browser.upload(app.contentResolver, d.uris) }) { Text("Remplacer") }
            },
            dismissButton = {
                Row {
                    val others = d.uris.filter { displayName(app.contentResolver, it) !in d.existing }
                    if (others.isNotEmpty()) {
                        TextButton(onClick = { dialog = null; browser.upload(app.contentResolver, others) }) {
                            Text("Ignorer ceux-là")
                        }
                    }
                    TextButton(onClick = { dialog = null }) { Text("Annuler") }
                }
            },
        )
        null -> Unit
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun FileList(
    path: String?,
    entries: List<RemoteEntry>,
    hiddenCount: Int,
    error: String?,
    enabled: Boolean,
    onUp: () -> Unit,
    onRetry: () -> Unit,
    onClick: (RemoteEntry) -> Unit,
    onMore: (RemoteEntry) -> Unit,
) {
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 96.dp)) {
        if (error != null) {
            item(key = "error") {
                Card(
                    modifier = Modifier.padding(12.dp).fillMaxWidth(),
                    colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer),
                ) {
                    Column(Modifier.padding(16.dp)) {
                        Text(error)
                        TextButton(onClick = onRetry, enabled = enabled) { Text("Réessayer") }
                    }
                }
            }
        }
        if (path != null && RemotePaths.parentOf(path) != null) {
            item(key = "..") {
                ListItem(
                    modifier = Modifier.combinedClickable(enabled = enabled, onClick = onUp),
                    leadingContent = { Icon(painterResource(R.drawable.ic_arrow_upward), contentDescription = null) },
                    headlineContent = { Text("Dossier parent") },
                    supportingContent = { Text(RemotePaths.parentOf(path)!!, fontFamily = FontFamily.Monospace, maxLines = 1) },
                )
            }
        }
        items(entries, key = { it.path }) { entry ->
            ListItem(
                modifier = Modifier.combinedClickable(
                    enabled = enabled,
                    onClick = { onClick(entry) },
                    onLongClick = { onMore(entry) },
                ),
                leadingContent = { EntryIcon(entry) },
                headlineContent = { Text(entry.name, maxLines = 1, overflow = TextOverflow.MiddleEllipsis) },
                supportingContent = { Text(entryDetails(entry), maxLines = 1, style = MaterialTheme.typography.bodySmall) },
                trailingContent = {
                    IconButton(enabled = enabled, onClick = { onMore(entry) }) {
                        Icon(Icons.Default.MoreVert, contentDescription = "Actions sur ${entry.name}")
                    }
                },
            )
        }
        if (path != null && entries.isEmpty() && error == null) {
            item(key = "empty") {
                Text(
                    if (hiddenCount > 0) "Aucun fichier visible ($hiddenCount caché(s))." else "Dossier vide.",
                    modifier = Modifier.fillMaxWidth().padding(32.dp),
                    textAlign = TextAlign.Center,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun EntryIcon(entry: RemoteEntry) {
    val icon: Painter = painterResource(if (entry.isDirectory) R.drawable.ic_folder else R.drawable.ic_file)
    val tint = if (entry.isDirectory) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant
    Box {
        Icon(icon, contentDescription = if (entry.isDirectory) "Dossier" else "Fichier", tint = tint)
        if (entry.isLink) {
            Icon(
                painterResource(R.drawable.ic_link),
                contentDescription = "Lien symbolique",
                modifier = Modifier.size(14.dp).align(Alignment.BottomEnd),
                tint = MaterialTheme.colorScheme.tertiary,
            )
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun EntrySheet(
    entry: RemoteEntry,
    onDismiss: () -> Unit,
    onOpenDir: () -> Unit,
    onOpenFile: () -> Unit,
    onDownload: () -> Unit,
    onRename: () -> Unit,
    onDelete: () -> Unit,
) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.navigationBarsPadding().padding(bottom = 8.dp)) {
            ListItem(
                leadingContent = { EntryIcon(entry) },
                headlineContent = { Text(entry.name) },
                supportingContent = {
                    Text(entryDetails(entry) + "\n" + entry.path, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
                },
            )
            HorizontalDivider()
            val act: (() -> Unit) -> () -> Unit = { action -> { onDismiss(); action() } }
            if (entry.isDirectory) {
                SheetAction(painterResource(R.drawable.ic_folder), "Ouvrir", act(onOpenDir))
            } else {
                SheetAction(painterResource(R.drawable.ic_file), "Ouvrir avec une application", act(onOpenFile))
                SheetAction(painterResource(R.drawable.ic_download), "Télécharger sur le téléphone…", act(onDownload))
            }
            SheetAction(null, "Renommer…", act(onRename), Icons.Default.Edit)
            SheetAction(null, "Supprimer…", act(onDelete), Icons.Default.Delete)
        }
    }
}

@Composable
private fun SheetAction(
    painter: Painter?,
    label: String,
    onClick: () -> Unit,
    vector: androidx.compose.ui.graphics.vector.ImageVector? = null,
) {
    ListItem(
        modifier = Modifier.clickable(onClick = onClick),
        leadingContent = {
            when {
                painter != null -> Icon(painter, contentDescription = null)
                vector != null -> Icon(vector, contentDescription = null)
            }
        },
        headlineContent = { Text(label) },
    )
}

@Composable
private fun TransfersPanel(transfers: List<Transfer>, onCancel: (Long) -> Unit) {
    if (transfers.isEmpty()) return
    Surface(tonalElevation = 3.dp) {
        Column(Modifier.navigationBarsPadding().padding(horizontal = 16.dp, vertical = 8.dp)) {
            transfers.forEach { t ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Icon(
                        painterResource(if (t.upload) R.drawable.ic_upload else R.drawable.ic_download),
                        contentDescription = if (t.upload) "Envoi" else "Téléchargement",
                        modifier = Modifier.size(18.dp),
                    )
                    Spacer(Modifier.width(8.dp))
                    Text(t.name, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.MiddleEllipsis)
                    Text(
                        if (t.total > 0) "${formatSize(t.done)} / ${formatSize(t.total)}" else formatSize(t.done),
                        style = MaterialTheme.typography.bodySmall,
                    )
                    IconButton(onClick = { onCancel(t.id) }) { Icon(Icons.Default.Close, contentDescription = "Annuler") }
                }
                if (t.total > 0) {
                    LinearProgressIndicator(progress = { (t.done.toFloat() / t.total).coerceIn(0f, 1f) }, modifier = Modifier.fillMaxWidth())
                } else {
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                }
            }
        }
    }
}

@Composable
private fun NameDialog(title: String, initial: String, confirm: String, onConfirm: (String) -> Unit, onDismiss: () -> Unit) {
    // Nom présélectionné sans l'extension, comme dans les gestionnaires de fichiers.
    val stem = initial.lastIndexOf('.').takeIf { it > 0 } ?: initial.length
    var value by remember { mutableStateOf(TextFieldValue(initial, TextRange(0, stem))) }
    val name = value.text.trim()
    val problem = when {
        name.isEmpty() -> null
        '/' in name -> "Le nom ne peut pas contenir « / »"
        name == "." || name == ".." -> "Nom réservé"
        else -> null
    }
    val valid = name.isNotEmpty() && problem == null
    val focus = remember { FocusRequester() }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            OutlinedTextField(
                value = value,
                onValueChange = { value = it },
                singleLine = true,
                isError = problem != null,
                supportingText = problem?.let { { Text(it) } },
                keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { if (valid) onConfirm(name) }),
                modifier = Modifier.focusRequester(focus),
            )
        },
        confirmButton = { TextButton(enabled = valid, onClick = { onConfirm(name) }) { Text(confirm) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Annuler") } },
    )
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
}

/** Ouvre [file] (copie locale d'un fichier distant) avec l'application choisie par l'utilisateur. */
private fun openWithApp(context: android.content.Context, file: File, onError: (String) -> Unit) {
    val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", file)
    val extension = file.extension.lowercase(Locale.ROOT)
    // Sans type connu, on suppose du texte : journaux et fichiers de configuration, l'essentiel sur un serveur.
    val mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension) ?: "text/plain"
    val intent = Intent(Intent.ACTION_VIEW).setDataAndType(uri, mime).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    try {
        context.startActivity(Intent.createChooser(intent, "Ouvrir ${file.name}"))
    } catch (e: ActivityNotFoundException) {
        onError("Aucune application ne sait ouvrir ce type de fichier")
    }
}

private fun displayName(resolver: ContentResolver, uri: Uri): String =
    resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
        if (c.moveToFirst()) c.getString(0)?.replace('/', '_') else null
    } ?: (uri.lastPathSegment?.substringAfterLast('/') ?: "fichier")

private val DATE_FORMAT = DateTimeFormatter.ofPattern("dd/MM/yyyy HH:mm", Locale.FRANCE)

private fun entryDetails(entry: RemoteEntry): String {
    val date = if (entry.modified > 0) {
        DATE_FORMAT.format(Instant.ofEpochSecond(entry.modified).atZone(ZoneId.systemDefault()))
    } else {
        ""
    }
    val parts = buildList {
        if (!entry.isDirectory) add(formatSize(entry.size))
        if (date.isNotEmpty()) add(date)
        add(entry.permissions)
    }
    return parts.joinToString(" · ")
}

/** Taille lisible en unités françaises (octets, Ko, Mo, Go). */
fun formatSize(bytes: Long): String {
    if (bytes < 1024) return "$bytes o"
    val units = listOf("Ko", "Mo", "Go", "To")
    var value = bytes / 1024.0
    var unit = 0
    while (value >= 1024 && unit < units.lastIndex) {
        value /= 1024
        unit++
    }
    return String.format(Locale.FRANCE, if (value < 10) "%.1f %s" else "%.0f %s", value, units[unit])
}
