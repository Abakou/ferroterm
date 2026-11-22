package com.ferroterm.ui

import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.ferroterm.FerrotermApp
import com.ferroterm.data.FtpOptions
import com.ferroterm.data.FtpTls
import com.ferroterm.data.Protocol
import com.ferroterm.data.defaultPort
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material3.Checkbox
import androidx.compose.material3.RadioButton
import androidx.compose.ui.semantics.Role
import com.ferroterm.data.SecretStore
import com.ferroterm.data.Session

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SessionEditScreen(app: FerrotermApp, sessionId: Long, onDone: () -> Unit) {
    val original = remember(sessionId) { app.sessions.get(sessionId) ?: Session() }
    var name by rememberSaveable { mutableStateOf(original.name) }
    var folder by rememberSaveable { mutableStateOf(original.folder) }
    var host by rememberSaveable { mutableStateOf(original.host) }
    var port by rememberSaveable { mutableStateOf(original.port.toString()) }
    var username by rememberSaveable { mutableStateOf(original.username) }
    var useKey by rememberSaveable { mutableStateOf(original.keyPath != null) }
    var keyPath by rememberSaveable { mutableStateOf(original.keyPath) }
    var keyError by rememberSaveable { mutableStateOf<String?>(null) }
    var protocol by rememberSaveable { mutableStateOf(original.protocol) }
    var ftpTls by rememberSaveable { mutableStateOf(original.ftp.tls) }
    var ftpActive by rememberSaveable { mutableStateOf(original.ftp.activeMode) }
    var hasStoredPassword by remember {
        mutableStateOf(sessionId != 0L && app.secrets.contains(SecretStore.passwordName(sessionId)))
    }
    var showErrors by rememberSaveable { mutableStateOf(false) }

    val context = LocalContext.current
    val importKey = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        try {
            val imported = app.keys.import(context.contentResolver, uri)
            // Clé importée juste avant dans cet écran et pas encore enregistrée : inutile désormais.
            keyPath?.takeIf { it != original.keyPath }?.let(app.keys::delete)
            keyPath = imported
            keyError = null
        } catch (e: Exception) {
            keyError = e.message ?: "Import impossible"
        }
    }

    val portNumber = port.toIntOrNull()?.takeIf { it in 1..65535 }
    val hostError = host.isBlank()
    val keyMissing = useKey && keyPath == null

    // Relit l'état au moment du clic : Compose peut garder le premier lambda `onClick`,
    // dont les valeurs calculées ci-dessus dateraient de la première composition.
    fun save() {
        showErrors = true
        val portNumber = port.toIntOrNull()?.takeIf { it in 1..65535 }
        // Pas de clé privée en FTP.
        val withKey = useKey && protocol != Protocol.FTP
        if (host.isBlank() || portNumber == null || (withKey && keyPath == null)) return
        val session = original.copy(
            name = name.trim(),
            folder = folder.trim().trim('/'),
            host = host.trim(),
            port = portNumber,
            username = username.trim(),
            keyPath = if (withKey) keyPath else null,
            protocol = protocol,
            ftp = FtpOptions(tls = ftpTls, activeMode = ftpActive),
        )
        app.sessions.save(session)
        // Clé importée puis abandonnée (retour au mot de passe) : jamais enregistrée.
        if (!withKey) keyPath?.takeIf { it != original.keyPath }?.let(app.keys::delete)
        // Clé remplacée ou abandonnée : l'ancien fichier ne sert plus à rien.
        val oldKey = original.keyPath
        if (oldKey != null && oldKey != session.keyPath &&
            app.sessions.sessions.value.none { it.keyPath == oldKey }
        ) {
            app.secrets.remove(SecretStore.passphraseName(oldKey))
            app.keys.delete(oldKey)
        }
        onDone()
    }

    fun cancel() {
        keyPath?.takeIf { it != original.keyPath }?.let(app.keys::delete)
        onDone()
    }
    BackHandler { cancel() }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(if (sessionId == 0L) "Nouvelle session" else "Modifier la session") },
                navigationIcon = {
                    IconButton(onClick = { cancel() }) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Retour")
                    }
                },
                actions = { TextButton(onClick = { save() }) { Text("Enregistrer") } },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .imePadding()
                .verticalScroll(rememberScrollState())
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            // Comme sur le bureau : SSH = terminal + fichiers, SFTP = fichiers seuls par SSH,
            // FTP = fichiers seuls, en clair ou en TLS.
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                val choices = listOf(Protocol.SSH to "SSH", Protocol.SFTP to "SFTP", Protocol.FTP to "FTP")
                choices.forEachIndexed { index, (value, label) ->
                    SegmentedButton(
                        selected = protocol == value,
                        onClick = {
                            // Port encore à la valeur par défaut de l'ancien protocole : on le suit.
                            if (port == defaultPort(protocol, ftpTls).toString()) {
                                port = defaultPort(value, ftpTls).toString()
                            }
                            protocol = value
                        },
                        shape = SegmentedButtonDefaults.itemShape(index, choices.size),
                    ) { Text(label) }
                }
            }
            Text(
                when (protocol) {
                    Protocol.SSH -> "Terminal et fichiers (SFTP) sur la même connexion."
                    Protocol.SFTP -> "Fichiers seuls, par SSH."
                    Protocol.FTP -> "Fichiers seuls. Préférez TLS : en FTP clair, mot de passe et fichiers circulent sans chiffrement."
                },
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (protocol == Protocol.FTP) {
                FtpTlsChoice(ftpTls) { value ->
                    if (port == defaultPort(Protocol.FTP, ftpTls).toString()) {
                        port = defaultPort(Protocol.FTP, value).toString()
                    }
                    ftpTls = value
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(checked = ftpActive, onCheckedChange = { ftpActive = it })
                    Text("Mode actif (le serveur se connecte au téléphone ; rarement utile)")
                }
            }
            OutlinedTextField(
                value = host,
                onValueChange = { host = it },
                label = { Text("Hôte") },
                placeholder = { Text("serveur.exemple.com ou 192.168.1.10") },
                singleLine = true,
                isError = showErrors && hostError,
                supportingText = if (showErrors && hostError) ({ Text("Obligatoire") }) else null,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, autoCorrectEnabled = false),
                modifier = Modifier.fillMaxWidth(),
            )
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(
                    value = username,
                    onValueChange = { username = it },
                    label = { Text("Utilisateur") },
                    placeholder = { Text("demandé à la connexion") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(autoCorrectEnabled = false),
                    modifier = Modifier.weight(1f),
                )
                OutlinedTextField(
                    value = port,
                    onValueChange = { value -> port = value.filter(Char::isDigit).take(5) },
                    label = { Text("Port") },
                    singleLine = true,
                    isError = showErrors && portNumber == null,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                    modifier = Modifier.width(96.dp),
                )
            }
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                label = { Text("Nom (facultatif)") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = folder,
                onValueChange = { folder = it },
                label = { Text("Dossier (facultatif)") },
                placeholder = { Text("ex. Clients/Prod") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )

            Text("Authentification", style = MaterialTheme.typography.titleSmall)
            if (protocol != Protocol.FTP) SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                SegmentedButton(
                    selected = !useKey,
                    onClick = { useKey = false },
                    shape = SegmentedButtonDefaults.itemShape(0, 2),
                ) { Text("Mot de passe") }
                SegmentedButton(
                    selected = useKey,
                    onClick = { useKey = true },
                    shape = SegmentedButtonDefaults.itemShape(1, 2),
                ) { Text("Clé privée") }
            }

            if (useKey && protocol != Protocol.FTP) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text(
                        keyPath?.let { app.keys.label(it) } ?: "Aucune clé",
                        modifier = Modifier.weight(1f),
                        color = if (showErrors && keyMissing) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface,
                    )
                    OutlinedButton(onClick = { importKey.launch(arrayOf("*/*")) }) {
                        Text(if (keyPath == null) "Importer…" else "Remplacer…")
                    }
                }
                Text(
                    "Formats OpenSSH, PEM ou PuTTY (.ppk). La clé est copiée dans l'application ; " +
                        "la phrase de passe éventuelle sera demandée à la connexion.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                keyError?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            } else if (hasStoredPassword) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Mot de passe mémorisé", modifier = Modifier.weight(1f))
                    TextButton(onClick = {
                        app.secrets.remove(SecretStore.passwordName(sessionId))
                        hasStoredPassword = false
                    }) { Text("Oublier") }
                }
            } else {
                Text(
                    "Le mot de passe sera demandé à la connexion, avec la possibilité de le mémoriser.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

/** Chiffrement FTP, mêmes choix et libellés que la version bureau. */
@Composable
private fun FtpTlsChoice(selected: FtpTls, onSelect: (FtpTls) -> Unit) {
    val labels = listOf(
        FtpTls.AUTO to "TLS si disponible (recommandé)",
        FtpTls.REQUIRED to "TLS explicite obligatoire (FTPS)",
        FtpTls.IMPLICIT to "TLS implicite (FTPS, port 990)",
        FtpTls.OFF to "Aucun (FTP en clair)",
    )
    Column(Modifier.selectableGroup()) {
        Text("Chiffrement", style = MaterialTheme.typography.titleSmall)
        labels.forEach { (value, label) ->
            Row(
                Modifier
                    .fillMaxWidth()
                    .selectable(selected = selected == value, onClick = { onSelect(value) }, role = Role.RadioButton)
                    .padding(vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                RadioButton(selected = selected == value, onClick = null)
                Text(
                    label,
                    modifier = Modifier.padding(start = 12.dp),
                    color = if (value == FtpTls.OFF) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface,
                )
            }
        }
    }
}
