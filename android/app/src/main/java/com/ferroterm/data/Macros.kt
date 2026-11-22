package com.ferroterm.data

import android.util.AtomicFile
import android.util.Log
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import java.io.File

/**
 * Macro : suite de commandes envoyée au terminal, une ligne à la fois. Mêmes champs
 * que la version bureau (`src/session.rs`).
 */
@Serializable
data class Macro(
    val id: Long = 0,
    val name: String = "",
    /** Une commande par ligne ; chaque ligne est suivie d'Entrée à l'exécution. */
    val commands: String = "",
    /** Touchée, la macro est exécutée ; sinon elle est seulement posée sur la ligne de commande. */
    @SerialName("auto_run") val autoRun: Boolean = false,
) {
    /** Lignes à envoyer : les lignes vides sont ignorées. */
    val lines: List<String>
        get() = commands.lines().map { it.trimEnd() }.filter { it.isNotBlank() }

    /** Octets à écrire dans le terminal : chaque ligne suivie d'un retour chariot. */
    fun input(): ByteArray = lines.joinToString("") { "$it\r" }.encodeToByteArray()

    /**
     * Texte à poser sur la ligne de commande sans l'exécuter : les lignes sont jointes
     * par « ; » pour qu'aucun retour chariot ne les lance.
     */
    fun commandLine(): String = lines.joinToString("; ") { it.trim() }
}

@Serializable
private data class MacroFile(val macros: List<Macro> = emptyList())

/** Macros enregistrées, dans `macros.json` (écriture atomique). */
class MacroRepository(dir: File) {
    private val file = AtomicFile(File(dir, "macros.json"))
    private val _macros = MutableStateFlow(load())
    val macros: StateFlow<List<Macro>> = _macros.asStateFlow()

    /** Ajoute (id 0) ou remplace une macro ; renvoie son id. */
    @Synchronized
    fun save(macro: Macro): Long {
        val current = _macros.value
        val stored = if (macro.id == 0L) macro.copy(id = (current.maxOfOrNull { it.id } ?: 0L) + 1) else macro
        val updated = if (current.any { it.id == stored.id }) {
            current.map { if (it.id == stored.id) stored else it }
        } else {
            current + stored
        }
        write(updated)
        return stored.id
    }

    @Synchronized
    fun remove(id: Long) = write(_macros.value.filterNot { it.id == id })

    private fun load(): List<Macro> {
        if (!file.baseFile.exists()) return emptyList()
        return try {
            sessionJson.decodeFromString<MacroFile>(file.readFully().decodeToString()).macros
        } catch (e: Exception) {
            Log.e("MacroRepository", "macros.json illisible, mis de côté", e)
            file.baseFile.renameTo(File(file.baseFile.parentFile, "macros.json.illisible"))
            emptyList()
        }
    }

    private fun write(macros: List<Macro>) {
        val out = file.startWrite()
        try {
            out.write(sessionJson.encodeToString(MacroFile.serializer(), MacroFile(macros)).encodeToByteArray())
            file.finishWrite(out)
        } catch (e: Exception) {
            file.failWrite(out)
            throw e
        }
        _macros.value = macros
    }
}
