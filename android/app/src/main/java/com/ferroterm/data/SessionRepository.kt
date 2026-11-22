package com.ferroterm.data

import android.util.AtomicFile
import android.util.Log
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.io.File

/**
 * Sessions enregistrées, gardées en mémoire et écrites dans `sessions.json`
 * (écriture atomique : un arrêt brutal ne laisse jamais un fichier à moitié écrit).
 */
class SessionRepository(dir: File) {
    private val file = AtomicFile(File(dir, "sessions.json"))
    private val _sessions = MutableStateFlow(load())
    val sessions: StateFlow<List<Session>> = _sessions.asStateFlow()

    fun get(id: Long): Session? = _sessions.value.firstOrNull { it.id == id }

    /** Ajoute (id 0) ou remplace une session ; renvoie son id. */
    @Synchronized
    fun save(session: Session): Long {
        val current = _sessions.value
        val stored = if (session.id == 0L) {
            session.copy(id = (current.maxOfOrNull { it.id } ?: 0L) + 1)
        } else {
            session
        }
        val updated = if (current.any { it.id == stored.id }) {
            current.map { if (it.id == stored.id) stored else it }
        } else {
            current + stored
        }
        write(updated)
        return stored.id
    }

    /** Ajoute des sessions (leur id est ignoré) en une seule écriture ; renvoie leurs ids. */
    @Synchronized
    fun addAll(sessions: List<Session>): List<Long> {
        if (sessions.isEmpty()) return emptyList()
        val current = _sessions.value
        val first = (current.maxOfOrNull { it.id } ?: 0L) + 1
        val added = sessions.mapIndexed { index, session -> session.copy(id = first + index) }
        write(current + added)
        return added.map { it.id }
    }

    @Synchronized
    fun remove(id: Long) {
        write(_sessions.value.filterNot { it.id == id })
    }

    private fun load(): List<Session> {
        if (!file.baseFile.exists()) return emptyList()
        return try {
            sessionJson.decodeFromString<SessionFile>(file.readFully().decodeToString()).sessions
        } catch (e: Exception) {
            // Fichier illisible : on le garde de côté plutôt que de l'écraser au prochain enregistrement.
            Log.e(TAG, "sessions.json illisible, mis de côté", e)
            file.baseFile.renameTo(File(file.baseFile.parentFile, "sessions.json.illisible"))
            emptyList()
        }
    }

    private fun write(sessions: List<Session>) {
        val text = sessionJson.encodeToString(SessionFile.serializer(), SessionFile(sessions))
        val out = file.startWrite()
        try {
            out.write(text.encodeToByteArray())
            file.finishWrite(out)
        } catch (e: Exception) {
            file.failWrite(out)
            throw e
        }
        _sessions.value = sessions
    }

    private companion object {
        const val TAG = "SessionRepository"
    }
}
