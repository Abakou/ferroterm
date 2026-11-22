package com.ferroterm.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.connectbot.terminal.ModifierManager
import org.connectbot.terminal.TerminalEmulator
import org.connectbot.terminal.VTermKey

/**
 * Ctrl et Alt « collants » de la rangée de touches : un appui les arme pour la
 * touche suivante, un second les verrouille, un troisième les relâche. Le terminal
 * les consulte aussi pour les touches du clavier virtuel.
 */
class StickyModifiers : ModifierManager {
    enum class Mode { OFF, ONCE, LOCKED }

    var ctrl by mutableStateOf(Mode.OFF)
    var alt by mutableStateOf(Mode.OFF)

    override fun isCtrlActive() = ctrl != Mode.OFF
    override fun isAltActive() = alt != Mode.OFF
    override fun isShiftActive() = false

    override fun clearTransients() {
        if (ctrl == Mode.ONCE) ctrl = Mode.OFF
        if (alt == Mode.ONCE) alt = Mode.OFF
    }

    /** Masque attendu par libvterm : 2 = Alt, 4 = Ctrl. */
    fun mask(): Int = (if (isAltActive()) 2 else 0) or (if (isCtrlActive()) 4 else 0)

    fun next(mode: Mode) = when (mode) {
        Mode.OFF -> Mode.ONCE
        Mode.ONCE -> Mode.LOCKED
        Mode.LOCKED -> Mode.OFF
    }
}

private sealed interface ExtraKey {
    val label: String

    data class Key(override val label: String, val code: Int) : ExtraKey
    data class Char(override val label: String, val codepoint: Int) : ExtraKey
    data object Ctrl : ExtraKey { override val label = "CTRL" }
    data object Alt : ExtraKey { override val label = "ALT" }
}

private val KEYS: List<ExtraKey> = listOf(
    ExtraKey.Key("ÉCHAP", VTermKey.ESCAPE),
    ExtraKey.Key("TAB", VTermKey.TAB),
    ExtraKey.Ctrl,
    ExtraKey.Alt,
    ExtraKey.Key("←", VTermKey.LEFT),
    ExtraKey.Key("↓", VTermKey.DOWN),
    ExtraKey.Key("↑", VTermKey.UP),
    ExtraKey.Key("→", VTermKey.RIGHT),
    ExtraKey.Char("-", '-'.code),
    ExtraKey.Char("/", '/'.code),
    ExtraKey.Char("|", '|'.code),
    ExtraKey.Char("~", '~'.code),
    ExtraKey.Key("ORIG", VTermKey.HOME),
    ExtraKey.Key("FIN", VTermKey.END),
    ExtraKey.Key("PG↑", VTermKey.PAGEUP),
    ExtraKey.Key("PG↓", VTermKey.PAGEDOWN),
)

/** Rangée de touches absentes du clavier virtuel, au-dessus de celui-ci. */
@Composable
fun ExtraKeysRow(emulator: TerminalEmulator, modifiers: StickyModifiers, enabled: Boolean, modifier: Modifier = Modifier) {
    Row(
        modifier = modifier
            .fillMaxWidth()
            .height(44.dp)
            .background(MaterialTheme.colorScheme.surfaceContainer)
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = 4.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        KEYS.forEach { key ->
            val mode = when (key) {
                ExtraKey.Ctrl -> modifiers.ctrl
                ExtraKey.Alt -> modifiers.alt
                else -> StickyModifiers.Mode.OFF
            }
            KeyButton(key.label, mode, enabled) {
                when (key) {
                    ExtraKey.Ctrl -> modifiers.ctrl = modifiers.next(modifiers.ctrl)
                    ExtraKey.Alt -> modifiers.alt = modifiers.next(modifiers.alt)
                    is ExtraKey.Key -> {
                        emulator.dispatchKey(modifiers.mask(), key.code)
                        modifiers.clearTransients()
                    }
                    is ExtraKey.Char -> {
                        emulator.dispatchCharacter(modifiers.mask(), key.codepoint)
                        modifiers.clearTransients()
                    }
                }
            }
        }
    }
}

@Composable
private fun KeyButton(label: String, mode: StickyModifiers.Mode, enabled: Boolean, onClick: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    val background = when (mode) {
        StickyModifiers.Mode.OFF -> colors.surfaceContainerHighest
        StickyModifiers.Mode.ONCE -> colors.primaryContainer
        StickyModifiers.Mode.LOCKED -> colors.primary
    }
    val foreground = when (mode) {
        StickyModifiers.Mode.OFF -> colors.onSurface
        StickyModifiers.Mode.ONCE -> colors.onPrimaryContainer
        StickyModifiers.Mode.LOCKED -> colors.onPrimary
    }
    Box(
        modifier = Modifier
            .padding(horizontal = 2.dp)
            .fillMaxHeight()
            .widthIn(min = 44.dp)
            .clip(RoundedCornerShape(6.dp))
            .background(background)
            // Pas de focus : le terminal doit le garder pour que le clavier virtuel reste ouvert.
            .clickable(enabled = enabled, onClick = onClick)
            .padding(horizontal = 8.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            label,
            color = if (enabled) foreground else foreground.copy(alpha = 0.4f),
            fontFamily = FontFamily.Monospace,
            fontWeight = if (mode == StickyModifiers.Mode.OFF) FontWeight.Normal else FontWeight.Bold,
            fontSize = 13.sp,
        )
    }
}
