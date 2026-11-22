package com.ferroterm.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.DialogProperties
import com.ferroterm.FerrotermApp
import com.ferroterm.ssh.ConnectionState
import com.ferroterm.ssh.CredentialsAnswer
import com.ferroterm.ssh.HostKeyInfo
import com.ferroterm.ssh.Prompt
import com.ferroterm.ssh.RemoteConnection

/** Ligne d'état sous le titre : courte, le détail est dans le cadre [StatusOverlay]. */
fun statusText(state: ConnectionState) = when (state) {
    is ConnectionState.Connecting -> state.step
    ConnectionState.Connected -> "Connectée"
    is ConnectionState.Closed ->
        if (state.changedHostKey != null) "Refusée : clé du serveur modifiée" else state.message
}

/** Questions posées pendant la connexion (mot de passe, clé serveur), sur n'importe quel écran. */
@Composable
fun ConnectionPrompts(connection: RemoteConnection) {
    val prompt by connection.prompt.collectAsState()
    when (val p = prompt) {
        is Prompt.Credentials -> CredentialsDialog(p)
        is Prompt.UnknownHostKey -> HostKeyDialog(p)
        is Prompt.Certificate -> CertificateDialog(p)
        null -> Unit
    }
}

/** Connexion en cours (barre de progression) ou fermée (message et reconnexion). */
@Composable
fun StatusOverlay(
    app: FerrotermApp,
    connection: RemoteConnection,
    state: ConnectionState,
    modifier: Modifier = Modifier,
) {
    when (state) {
        is ConnectionState.Connecting -> LinearProgressIndicator(modifier.fillMaxWidth())
        ConnectionState.Connected -> Unit
        is ConnectionState.Closed -> {
            var confirmForget by remember(state) { mutableStateOf(false) }
            Card(
                modifier = modifier.padding(12.dp).fillMaxWidth(),
                colors = CardDefaults.cardColors(
                    containerColor = if (state.error) MaterialTheme.colorScheme.errorContainer else MaterialTheme.colorScheme.surfaceContainerHigh,
                ),
            ) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(state.message)
                    val changed = state.changedHostKey
                    if (changed != null) {
                        Text("Nouvelle clé ${changed.type} :", style = MaterialTheme.typography.bodySmall)
                        Text(changed.fingerprint, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        if (changed == null) {
                            Button(onClick = { app.connections.reconnect(connection) }) { Text("Reconnecter") }
                        } else {
                            OutlinedButton(onClick = { confirmForget = true }) { Text("Accepter la nouvelle clé…") }
                        }
                    }
                }
            }
            val changed = state.changedHostKey
            if (confirmForget && changed != null) {
                ReplaceHostKeyDialog(
                    key = changed,
                    onConfirm = {
                        confirmForget = false
                        connection.forgetChangedHostKey(changed)
                        app.connections.reconnect(connection)
                    },
                    onDismiss = { confirmForget = false },
                )
            }
        }
    }
}

@Composable
private fun ReplaceHostKeyDialog(key: HostKeyInfo, onConfirm: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Remplacer la clé de ${key.host} ?") },
        text = {
            Text(
                "Ne le faites que si vous savez pourquoi la clé a changé (serveur réinstallé, " +
                    "clé régénérée). L'ancienne clé sera oubliée et la nouvelle vous sera présentée " +
                    "pour vérification.",
            )
        },
        confirmButton = { TextButton(onClick = onConfirm) { Text("Remplacer") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Annuler") } },
    )
}

@Composable
private fun CredentialsDialog(prompt: Prompt.Credentials) {
    var username by remember(prompt) { mutableStateOf(prompt.username.orEmpty()) }
    var secret by remember(prompt) { mutableStateOf("") }
    var remember by remember(prompt) { mutableStateOf(false) }
    val firstField = remember { FocusRequester() }
    val canSubmit = username.isNotBlank() || !prompt.askUsername
    val submit = {
        if (canSubmit) prompt.answer.complete(CredentialsAnswer(username.trim(), secret, remember))
    }

    AlertDialog(
        onDismissRequest = { prompt.answer.complete(null) },
        properties = DialogProperties(dismissOnClickOutside = false),
        title = { Text(prompt.title) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                prompt.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                if (prompt.askUsername) {
                    OutlinedTextField(
                        value = username,
                        onValueChange = { username = it },
                        label = { Text("Utilisateur") },
                        placeholder = prompt.usernameHint?.let { hint -> { Text(hint) } },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(
                            autoCorrectEnabled = false,
                            imeAction = if (prompt.secretLabel != null) ImeAction.Next else ImeAction.Done,
                        ),
                        keyboardActions = KeyboardActions(onDone = { submit() }),
                        modifier = Modifier.focusRequester(firstField),
                    )
                } else if (prompt.username != null) {
                    Text("Utilisateur : ${prompt.username}")
                }
                if (prompt.secretLabel != null) {
                    OutlinedTextField(
                        value = secret,
                        onValueChange = { secret = it },
                        label = { Text(prompt.secretLabel) },
                        singleLine = true,
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
                        keyboardActions = KeyboardActions(onDone = { submit() }),
                        modifier = if (prompt.askUsername) Modifier else Modifier.focusRequester(firstField),
                    )
                }
                if (prompt.canRemember && prompt.secretLabel != null) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(checked = remember, onCheckedChange = { remember = it })
                        Text("Mémoriser (chiffré sur cet appareil)")
                    }
                }
            }
        },
        confirmButton = { TextButton(enabled = canSubmit, onClick = { submit() }) { Text("Se connecter") } },
        dismissButton = { TextButton(onClick = { prompt.answer.complete(null) }) { Text("Annuler") } },
    )
    LaunchedEffect(prompt) { runCatching { firstField.requestFocus() } }
}

@Composable
private fun HostKeyDialog(prompt: Prompt.UnknownHostKey) {
    AlertDialog(
        onDismissRequest = { prompt.answer.complete(false) },
        properties = DialogProperties(dismissOnClickOutside = false),
        title = { Text("Serveur inconnu") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Première connexion à ${prompt.key.host}. Vérifiez que cette empreinte est bien celle du serveur :")
                Text("Clé ${prompt.key.type}", style = MaterialTheme.typography.bodySmall)
                Text(prompt.key.fingerprint, fontFamily = FontFamily.Monospace)
            }
        },
        confirmButton = { TextButton(onClick = { prompt.answer.complete(true) }) { Text("Faire confiance") } },
        dismissButton = { TextButton(onClick = { prompt.answer.complete(false) }) { Text("Annuler") } },
    )
}

@Composable
private fun CertificateDialog(prompt: Prompt.Certificate) {
    AlertDialog(
        onDismissRequest = { prompt.answer.complete(false) },
        properties = DialogProperties(dismissOnClickOutside = false),
        title = { Text(if (prompt.changed) "Certificat modifié" else "Certificat non reconnu") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (prompt.changed) {
                    Text(
                        "Le certificat de ${prompt.host} a changé depuis la dernière connexion. Il peut " +
                            "s'agir d'un renouvellement… ou d'une interception.",
                        color = MaterialTheme.colorScheme.error,
                    )
                }
                Text("Le certificat de ce serveur n'est pas reconnu : ${prompt.reason}.")
                Text("Vérifiez que cette empreinte SHA-256 est bien celle du serveur :")
                Text(prompt.fingerprint, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
            }
        },
        confirmButton = { TextButton(onClick = { prompt.answer.complete(true) }) { Text("Faire confiance") } },
        dismissButton = { TextButton(onClick = { prompt.answer.complete(false) }) { Text("Annuler") } },
    )
}
