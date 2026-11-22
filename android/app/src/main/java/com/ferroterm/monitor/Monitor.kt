package com.ferroterm.monitor

import java.util.Locale

/**
 * Supervision d'un serveur Linux (CPU, mémoire, réseau, disques, processus), même
 * script et même analyse que la version bureau (`src/monitor.rs`). Le script tourne
 * sur un canal SSH dédié de la connexion du terminal et envoie un bloc toutes les
 * deux secondes.
 */
object MonitorScript {
    /** Script POSIX ; sans guillemets simples, pour être passé tel quel à `sh -c '…'`. */
    val SCRIPT: String by lazy {
        // Chemin absolu : R8 renomme et déplace la classe dans la version de production.
        val stream = MonitorScript::class.java.getResourceAsStream("/com/ferroterm/monitor/monitor.sh")
            ?: error("monitor.sh absent des ressources")
        stream.bufferedReader().use { it.readText() }.trimEnd()
    }

    fun command(): String = "exec sh -c '$SCRIPT'"
}

data class MonitorProcess(val name: String, val cpuPercent: Float, val rss: Long)

data class MonitorStats(
    val host: String = "",
    /** Charge CPU en pourcentage, connue à partir du second bloc. */
    val cpuPercent: Float? = null,
    val memUsed: Long = 0,
    val memTotal: Long = 0,
    /** Débits en octets par seconde, connus à partir du second bloc. */
    val rxRate: Double? = null,
    val txRate: Double? = null,
    val uptimeSecs: Long = 0,
    val sessions: Int = 0,
    val user: String = "",
    /** (point de montage, pourcentage utilisé). */
    val disks: List<Pair<String, Int>> = emptyList(),
    /** Dossier courant du shell du terminal, si le script l'a retrouvé. */
    val cwd: String? = null,
    /** Nom de la distribution (`PRETTY_NAME` de `/etc/os-release`). */
    val os: String = "",
    val cpus: Int = 0,
    /** Système de fichiers racine, en octets. */
    val rootUsed: Long = 0,
    val rootTotal: Long = 0,
    /** Processus les plus gourmands en CPU. */
    val processes: List<MonitorProcess> = emptyList(),
)

/** Analyseur avec mémoire du bloc précédent (nécessaire pour les deltas). */
class MonitorParser {
    private class Sample(val atNanos: Long, val cpuIdle: Long, val cpuTotal: Long, val rx: Long, val tx: Long)

    private val buffer = StringBuilder()
    private var previous: Sample? = null

    /** Ajoute du texte reçu et renvoie les mesures des blocs complets. */
    fun feed(text: String, nowNanos: Long = System.nanoTime()): List<MonitorStats> {
        buffer.append(text)
        val out = mutableListOf<MonitorStats>()
        while (true) {
            val end = buffer.indexOf(END)
            if (end < 0) break
            val block = buffer.substring(0, end)
            buffer.delete(0, end + END.length)
            val start = block.indexOf(BEGIN)
            if (start >= 0) out += parseBlock(block.substring(start + BEGIN.length), nowNanos)
        }
        if (buffer.length > 64 * 1024) buffer.setLength(0)
        return out
    }

    internal fun parseBlock(block: String, nowNanos: Long): MonitorStats {
        var stats = MonitorStats()
        var cpuIdle = 0L
        var cpuTotal = 0L
        var rx = 0L
        var tx = 0L
        var haveCpu = false
        var haveNet = false
        val processes = mutableListOf<MonitorProcess>()

        for (raw in block.lines()) {
            val line = raw.trim()
            val space = line.indexOf(' ')
            if (space < 0) continue
            val key = line.substring(0, space)
            val value = line.substring(space + 1)
            when (key) {
                "host" -> stats = stats.copy(host = value.trim())
                "cpu" -> {
                    // « cpu user nice system idle iowait irq softirq steal … »
                    val fields = value.split(WHITESPACE).drop(1).mapNotNull { it.toLongOrNull() }
                    if (fields.size >= 4) {
                        cpuIdle = fields[3] + fields.getOrElse(4) { 0 }
                        cpuTotal = fields.sum()
                        haveCpu = true
                    }
                }
                "mem" -> {
                    var total = 0L
                    var available = 0L
                    value.trim().split(WHITESPACE).chunked(3).forEach { pair ->
                        if (pair.size >= 2) {
                            val bytes = (pair[1].toLongOrNull() ?: 0) * 1024
                            when (pair[0]) {
                                "MemTotal:" -> total = bytes
                                "MemAvailable:" -> available = bytes
                            }
                        }
                    }
                    stats = stats.copy(memTotal = total, memUsed = (total - available).coerceAtLeast(0))
                }
                "net" -> {
                    val numbers = value.trim().split(WHITESPACE).mapNotNull { it.toLongOrNull() }
                    if (numbers.size >= 2) {
                        rx = numbers[0]
                        tx = numbers[1]
                        haveNet = true
                    }
                }
                "up" -> stats = stats.copy(uptimeSecs = value.trim().toDoubleOrNull()?.toLong() ?: 0)
                "users" -> {
                    val parts = value.trim().split(WHITESPACE)
                    stats = stats.copy(sessions = parts.getOrNull(0)?.toIntOrNull() ?: 0, user = parts.getOrNull(1).orEmpty())
                }
                "cwd" -> value.trim().takeIf { it.startsWith("/") }?.let { stats = stats.copy(cwd = it) }
                "os" -> stats = stats.copy(os = value.trim())
                "ncpu" -> stats = stats.copy(cpus = value.trim().toIntOrNull() ?: 0)
                "rootfs" -> {
                    val numbers = value.trim().split(WHITESPACE).mapNotNull { it.toLongOrNull() }
                    if (numbers.size >= 2) stats = stats.copy(rootUsed = numbers[0] * 1024, rootTotal = numbers[1] * 1024)
                }
                "proc" -> {
                    val parts = value.split(WHITESPACE, limit = 3)
                    val cpu = parts.getOrNull(0)?.toFloatOrNull()
                    val rss = parts.getOrNull(1)?.toLongOrNull()
                    val name = parts.getOrNull(2)?.trim().orEmpty()
                    if (cpu != null && rss != null && name.isNotEmpty()) processes += MonitorProcess(name, cpu, rss * 1024)
                }
                "disk" -> stats = stats.copy(
                    disks = value.split(';').mapNotNull { entry ->
                        val trimmed = entry.trim()
                        val cut = trimmed.lastIndexOf(' ')
                        if (cut <= 0) return@mapNotNull null
                        val pct = trimmed.substring(cut + 1).trimEnd('%').toIntOrNull() ?: return@mapNotNull null
                        trimmed.substring(0, cut) to pct
                    },
                )
            }
        }
        stats = stats.copy(processes = processes)

        previous?.let { prev ->
            val elapsed = (nowNanos - prev.atNanos) / 1e9
            if (haveCpu && cpuTotal > prev.cpuTotal) {
                val total = (cpuTotal - prev.cpuTotal).toFloat()
                val idle = (cpuIdle - prev.cpuIdle).coerceAtLeast(0).toFloat()
                stats = stats.copy(cpuPercent = ((1f - idle / total) * 100f).coerceIn(0f, 100f))
            }
            if (haveNet && elapsed > 0) {
                stats = stats.copy(
                    rxRate = (rx - prev.rx).coerceAtLeast(0) / elapsed,
                    txRate = (tx - prev.tx).coerceAtLeast(0) / elapsed,
                )
            }
        }
        if (haveCpu || haveNet) previous = Sample(nowNanos, cpuIdle, cpuTotal, rx, tx)
        return stats
    }

    private companion object {
        const val BEGIN = "@@BEGIN"
        const val END = "@@END"
        val WHITESPACE = Regex("\\s+")
    }
}

/** Formats d'affichage, en français (virgule décimale). */
object MonitorFormat {
    /** « 12 j 3 h », « 3 h 07 », « 12 min ». */
    fun uptime(secs: Long): String {
        val days = secs / 86_400
        val hours = (secs % 86_400) / 3_600
        val minutes = (secs % 3_600) / 60
        return when {
            days > 0 -> "$days j $hours h"
            hours > 0 -> "$hours h %02d".format(minutes)
            else -> "$minutes min"
        }
    }

    /** Débit en octets par seconde : « 3,0 Mo/s ». */
    fun bytesRate(bytesPerSec: Double): String = when {
        bytesPerSec >= 1e9 -> fr("%.1f Go/s", bytesPerSec / 1e9)
        bytesPerSec >= 1e6 -> fr("%.1f Mo/s", bytesPerSec / 1e6)
        bytesPerSec >= 1e3 -> fr("%.0f Ko/s", bytesPerSec / 1e3)
        else -> fr("%.0f o/s", bytesPerSec)
    }

    /** Taille mémoire courte : « 184 Mo », « 1,2 Go ». */
    fun mem(bytes: Long): String {
        val mb = bytes / 1_048_576.0
        return if (mb >= 1024) fr("%.1f Go", mb / 1024) else fr("%.0f Mo", mb)
    }

    fun gb(bytes: Long): String = fr("%.2f Go", bytes / 1_073_741_824.0)

    private fun fr(pattern: String, value: Double) = String.format(Locale.FRANCE, pattern, value)
}
