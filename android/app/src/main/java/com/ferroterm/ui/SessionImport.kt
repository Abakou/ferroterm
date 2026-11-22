package com.ferroterm.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import com.ferroterm.FerrotermApp
import com.ferroterm.data.ExportSecrets
import com.ferroterm.data.ImportReport
import com.ferroterm.data.SessionExport
import com.ferroterm.data.SessionExportFile
import com.ferroterm.data.WrongPasswordException
import com.ferroterm.data.importSessions
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Taille maximale d'un fichier d'export (largement au-dessus d'un export réel). */
private const val MAX_EXPORT_SIZE = 16 * 1024 * 1024

/**
 * « Importer des sessions » : fichier exporté par Ferroterm sur PC, avec ou sans
 * secrets. Renvoie l'action qui ouvre le sélecteur de fichiers ; le dialogue du
 * mot de passe est affiché par ce composable. [onDone] reçoit le bilan à afficher.
 */
@Composable
fun rememberSessionImport(app: FerrotermApp, onDone: (String) -> Unit): () -> Unit {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var locked by remember { mutableStateOf<SessionExportFile?>(null) }
    var password by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }

    suspend fun finish(export: SessionExportFile, secrets: ExportSecrets?) {
        val report = withContext(Dispatchers.IO) {
            importSessions(export, secrets, app.sessions, app.keys, app.secrets)
        }
        onDone(describe(report, secrets != null, export.secrets != null))
    }

    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        scope.launch {
            try {
                val export = withContext(Dispatchers.IO) {
                    val bytes = context.contentResolver.openInputStream(uri)?.use { it.readNBytes(MAX_EXPORT_SIZE + 1) }
                        ?: error("Fichier illisible.")
                    require(bytes.size <= MAX_EXPORT_SIZE) { "Fichier trop gros pour un export de sessions." }
                    SessionExport.parse(bytes.decodeToString())
                }
                if (export.secrets != null) {
                    password = ""
                    error = null
                    locked = export
                } else {
                    finish(export, null)
                }
            } catch (e: Exception) {
                onDone("Import impossible : ${e.message ?: e.javaClass.simpleName}")
            }
        }
    }

    locked?.let { export ->
        val unlock = {
            busy = true
            scope.launch {
                try {
                    val sealed = export.secrets ?: return@launch
                    val secrets = withContext(Dispatchers.Default) { SessionExport.open(sealed, password) }
                    locked = null
                    finish(export, secrets)
                } catch (e: WrongPasswordException) {
                    error = "Mot de passe incorrect."
                } catch (e: Exception) {
                    error = e.message ?: "Déchiffrement impossible."
                } finally {
                    busy = false
                }
            }
        }
        AlertDialog(
            onDismissRequest = { if (!busy) locked = null },
            title = { Text("Importer des sessions") },
            text = {
                Column {
                    Text("Ce fichier contient des clés SSH et des mots de passe chiffrés. Saisissez le mot de passe choisi à l'export.")
                    OutlinedTextField(
                        value = password,
                        onValueChange = {
                            password = it
                            error = null
                        },
                        modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
                        label = { Text("Mot de passe") },
                        singleLine = true,
                        enabled = !busy,
                        isError = error != null,
                        supportingText = error?.let { { Text(it) } },
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                    )
                    if (busy) LinearProgressIndicator(Modifier.fillMaxWidth().padding(top = 8.dp))
                }
            },
            confirmButton = {
                TextButton(onClick = { unlock() }, enabled = !busy && password.isNotEmpty()) { Text("Importer") }
            },
            dismissButton = {
                Row {
                    TextButton(
                        onClick = {
                            locked = null
                            scope.launch { finish(export, null) }
                        },
                        enabled = !busy,
                    ) { Text("Sans les secrets") }
                    TextButton(onClick = { locked = null }, enabled = !busy) { Text("Annuler") }
                }
            },
        )
    }

    // Tous types : selon l'emplacement (Drive, téléchargements…), un .json n'a pas toujours le bon type.
    return { pick.launch(arrayOf("*/*")) }
}

private fun describe(report: ImportReport, withSecrets: Boolean, fileHadSecrets: Boolean): String = buildString {
    append("${report.found} session(s) trouvée(s)\n\n")
    append("• ${report.added} importée(s)\n")
    append("• ${report.duplicates} déjà présente(s) (même groupe et même nom)")
    when {
        withSecrets -> {
            append("\n• ${report.keys} clé(s) privée(s) importée(s)")
            append("\n• ${report.passwords} mot(s) de passe mémorisé(s)")
            for ((name, reason) in report.badKeys) append("\n\nClé « $name » ignorée : $reason")
            if (report.duplicates > 0) {
                append("\n\nLes sessions déjà présentes gardent leurs réglages, leur clé et leur mot de passe.")
            }
        }
        fileHadSecrets -> append(
            "\n\nSecrets non importés : les mots de passe seront demandés à la connexion, " +
                "et les clés privées sont à importer dans chaque session.",
        )
        else -> append(
            "\n\nCe fichier ne contient ni clés ni mots de passe. Pour les joindre, exportez depuis " +
                "Ferroterm sur PC en cochant « Inclure les clés SSH et les mots de passe ».",
        )
    }
}
