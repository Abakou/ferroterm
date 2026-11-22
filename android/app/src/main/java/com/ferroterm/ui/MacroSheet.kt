package com.ferroterm.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.ferroterm.FerrotermApp
import com.ferroterm.R
import com.ferroterm.data.Macro

/**
 * Macros, depuis le terminal : un toucher exécute la macro (ou la pose sur la ligne de
 * commande, selon son réglage) ; le menu propose l'autre action, la modification et
 * la suppression.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MacroSheet(app: FerrotermApp, onDismiss: () -> Unit, onSend: (Macro, execute: Boolean) -> Unit) {
    val macros by app.macros.macros.collectAsStateWithLifecycle()
    var editing by remember { mutableStateOf<Macro?>(null) }
    var toDelete by remember { mutableStateOf<Macro?>(null) }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.navigationBarsPadding().padding(bottom = 8.dp)) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("Macros", style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
                TextButton(onClick = { editing = Macro() }) {
                    Icon(Icons.Default.Add, contentDescription = null)
                    Text("Nouvelle")
                }
            }
            HorizontalDivider()
            if (macros.isEmpty()) {
                Text(
                    "Aucune macro. Une macro envoie une suite de commandes au terminal, une ligne à la fois.",
                    modifier = Modifier.padding(16.dp),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            LazyColumn(Modifier.heightIn(max = 420.dp)) {
                items(macros.sortedBy { it.name.lowercase() }, key = { it.id }) { macro ->
                    MacroItem(
                        macro = macro,
                        onTap = { onSend(macro, macro.autoRun) },
                        onOther = { onSend(macro, !macro.autoRun) },
                        onEdit = { editing = macro },
                        onDelete = { toDelete = macro },
                    )
                }
            }
        }
    }

    editing?.let { macro ->
        MacroEditor(
            initial = macro,
            onSave = { saved -> app.macros.save(saved); editing = null },
            onDismiss = { editing = null },
        )
    }
    toDelete?.let { macro ->
        AlertDialog(
            onDismissRequest = { toDelete = null },
            title = { Text("Supprimer « ${macro.name} » ?") },
            confirmButton = { TextButton(onClick = { app.macros.remove(macro.id); toDelete = null }) { Text("Supprimer") } },
            dismissButton = { TextButton(onClick = { toDelete = null }) { Text("Annuler") } },
        )
    }
}

@Composable
private fun MacroItem(macro: Macro, onTap: () -> Unit, onOther: () -> Unit, onEdit: () -> Unit, onDelete: () -> Unit) {
    var menu by remember { mutableStateOf(false) }
    ListItem(
        modifier = Modifier.clickable(onClick = onTap),
        leadingContent = {
            Icon(
                painterResource(if (macro.autoRun) R.drawable.ic_play else R.drawable.ic_macro),
                contentDescription = if (macro.autoRun) "Exécutée au toucher" else "Posée sur la ligne au toucher",
            )
        },
        headlineContent = { Text(macro.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        supportingContent = {
            val lines = macro.lines
            Text(
                lines.firstOrNull().orEmpty() + if (lines.size > 1) "  (+${lines.size - 1})" else "",
                fontFamily = FontFamily.Monospace,
                style = MaterialTheme.typography.bodySmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        },
        trailingContent = {
            Box {
                IconButton(onClick = { menu = true }) { Icon(Icons.Default.MoreVert, contentDescription = "Actions sur ${macro.name}") }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    DropdownMenuItem(
                        text = { Text(if (macro.autoRun) "Poser sans exécuter" else "Exécuter") },
                        onClick = { menu = false; onOther() },
                    )
                    DropdownMenuItem(text = { Text("Modifier…") }, onClick = { menu = false; onEdit() })
                    DropdownMenuItem(text = { Text("Supprimer…") }, onClick = { menu = false; onDelete() })
                }
            }
        },
    )
}

@Composable
private fun MacroEditor(initial: Macro, onSave: (Macro) -> Unit, onDismiss: () -> Unit) {
    var name by remember { mutableStateOf(initial.name) }
    var commands by remember { mutableStateOf(initial.commands) }
    var autoRun by remember { mutableStateOf(if (initial.id == 0L) true else initial.autoRun) }
    val candidate = initial.copy(name = name.trim(), commands = commands.trimEnd(), autoRun = autoRun)
    val valid = candidate.name.isNotEmpty() && candidate.lines.isNotEmpty()

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (initial.id == 0L) "Nouvelle macro" else "Modifier la macro") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    label = { Text("Nom") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = commands,
                    onValueChange = { commands = it },
                    label = { Text("Commandes (une par ligne)") },
                    minLines = 4,
                    maxLines = 10,
                    textStyle = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
                    keyboardOptions = KeyboardOptions(autoCorrectEnabled = false),
                    modifier = Modifier.fillMaxWidth(),
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(checked = autoRun, onCheckedChange = { autoRun = it })
                    Text(
                        if (autoRun) "Exécuter au toucher" else "Au toucher, poser sur la ligne de commande sans exécuter",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            }
        },
        confirmButton = { TextButton(enabled = valid, onClick = { onSave(candidate) }) { Text("Enregistrer") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Annuler") } },
    )
}
