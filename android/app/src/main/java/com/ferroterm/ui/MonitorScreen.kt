package com.ferroterm.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Card
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.ferroterm.FerrotermApp
import com.ferroterm.monitor.MonitorFormat
import com.ferroterm.monitor.MonitorStats
import com.ferroterm.ssh.ConnectionState
import com.ferroterm.ssh.SshConnection
import java.util.Locale

/** Résumé pour la barre de titre du terminal : « CPU 12 % · RAM 1,2 / 3,8 Go ». */
fun monitorSummary(stats: MonitorStats): String = buildList {
    stats.cpuPercent?.let { add("CPU ${it.toInt()} %") }
    if (stats.memTotal > 0) add("RAM ${MonitorFormat.mem(stats.memUsed)} / ${MonitorFormat.mem(stats.memTotal)}")
}.joinToString(" · ")

/** Supervision du serveur d'une session SSH, comme le panneau de la version bureau. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MonitorScreen(app: FerrotermApp, connectionId: Long, onBack: () -> Unit) {
    val connection = remember(connectionId) { app.connections.get(connectionId) as? SshConnection }
    if (connection == null) {
        LaunchedEffect(Unit) { onBack() }
        return
    }
    val state by connection.state.collectAsState()
    val stats by connection.stats.collectAsState()
    val history by connection.cpuHistory.collectAsState()

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(connection.session.displayName, maxLines = 1)
                        Text(
                            stats?.let { s -> listOf(s.host, s.os).filter { it.isNotBlank() }.joinToString(" · ") }
                                ?.takeIf { it.isNotBlank() } ?: statusText(state),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                },
                navigationIcon = {
                    IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Retour") }
                },
            )
        },
    ) { padding ->
        val s = stats
        if (s == null) {
            Column(Modifier.fillMaxSize().padding(padding).padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(
                    if (state == ConnectionState.Connected) {
                        "En attente des premières mesures…"
                    } else {
                        "La supervision reprendra à la connexion."
                    },
                    style = MaterialTheme.typography.bodyLarge,
                )
                Text(
                    "Les mesures viennent de /proc : elles ne sont disponibles que sur un serveur Linux.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            return@Scaffold
        }
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(
                start = 16.dp,
                end = 16.dp,
                top = padding.calculateTopPadding() + 8.dp,
                bottom = padding.calculateBottomPadding() + 16.dp,
            ),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            item {
                InfoCard("Système") {
                    Detail("Actif depuis", MonitorFormat.uptime(s.uptimeSecs))
                    if (s.cpus > 0) Detail("Processeurs", s.cpus.toString())
                    Detail("Sessions ouvertes", if (s.user.isNotBlank()) "${s.sessions} (vous : ${s.user})" else "${s.sessions}")
                    if (s.rootTotal > 0) Detail("Disque système", "${MonitorFormat.gb(s.rootUsed)} / ${MonitorFormat.gb(s.rootTotal)}")
                }
            }
            item {
                InfoCard("Processeur") {
                    Text(
                        s.cpuPercent?.let { "${it.toInt()} %" } ?: "…",
                        style = MaterialTheme.typography.headlineMedium,
                        fontWeight = FontWeight.SemiBold,
                    )
                    CpuChart(history, Modifier.fillMaxWidth().height(72.dp))
                    Text(
                        "Deux dernières minutes",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            item {
                InfoCard("Mémoire") {
                    Detail("Utilisée", "${MonitorFormat.mem(s.memUsed)} / ${MonitorFormat.mem(s.memTotal)}")
                    if (s.memTotal > 0) Gauge(s.memUsed.toFloat() / s.memTotal)
                }
            }
            if (s.rxRate != null || s.txRate != null) {
                item {
                    InfoCard("Réseau") {
                        Detail("↓ Reçu", s.rxRate?.let(MonitorFormat::bytesRate) ?: "…")
                        Detail("↑ Envoyé", s.txRate?.let(MonitorFormat::bytesRate) ?: "…")
                    }
                }
            }
            if (s.disks.isNotEmpty()) {
                item {
                    InfoCard("Disques") {
                        s.disks.forEach { (mount, percent) ->
                            Detail(mount, "$percent %", monospace = true)
                            Gauge(percent / 100f)
                        }
                    }
                }
            }
            if (s.processes.isNotEmpty()) {
                item {
                    InfoCard("Processus les plus actifs") {
                        s.processes.forEach { p ->
                            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                                Text(p.name, Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis, fontFamily = FontFamily.Monospace)
                                Text(String.format(Locale.FRANCE, "%.1f %%", p.cpuPercent), Modifier.width(64.dp))
                                Text(MonitorFormat.mem(p.rss), Modifier.width(72.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun InfoCard(title: String, content: @Composable () -> Unit) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(title, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary)
            content()
        }
    }
}

@Composable
private fun Detail(label: String, value: String, monospace: Boolean = false) {
    Row(Modifier.fillMaxWidth()) {
        Text(
            label,
            Modifier.weight(1f),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontFamily = if (monospace) FontFamily.Monospace else null,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(value)
    }
}

/** Jauge d'occupation ; rouge au-delà de 90 %. */
@Composable
private fun Gauge(fraction: Float) {
    val value = fraction.coerceIn(0f, 1f)
    LinearProgressIndicator(
        progress = { value },
        modifier = Modifier.fillMaxWidth(),
        color = if (value >= 0.9f) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary,
    )
}

/** Courbe de la charge CPU (0 à 100 %), les mesures les plus récentes à droite. */
@Composable
private fun CpuChart(values: List<Float>, modifier: Modifier) {
    val line = MaterialTheme.colorScheme.primary
    val grid = MaterialTheme.colorScheme.outlineVariant
    Canvas(modifier) {
        val step = size.width / (CHART_POINTS - 1)
        drawLine(grid, Offset(0f, size.height), Offset(size.width, size.height), strokeWidth = 1.dp.toPx())
        drawLine(grid, Offset(0f, size.height / 2), Offset(size.width, size.height / 2), strokeWidth = 0.5.dp.toPx())
        if (values.size < 2) return@Canvas
        val path = Path()
        val offset = CHART_POINTS - values.size
        values.forEachIndexed { index, value ->
            val x = (offset + index) * step
            val y = size.height * (1f - value.coerceIn(0f, 100f) / 100f)
            if (index == 0) path.moveTo(x, y) else path.lineTo(x, y)
        }
        drawPath(path, line, style = Stroke(width = 2.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round))
    }
}

private const val CHART_POINTS = 60
