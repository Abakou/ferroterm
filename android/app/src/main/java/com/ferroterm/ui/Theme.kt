package com.ferroterm.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext

/** Couleurs du système (Material You) : toujours disponibles à partir d'Android 12. */
@Composable
fun FerrotermTheme(content: @Composable () -> Unit) {
    val context = LocalContext.current
    val colors = if (isSystemInDarkTheme()) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
    MaterialTheme(colorScheme = colors, content = content)
}

/** Écran du terminal : toujours sombre, quel que soit le thème du téléphone. */
@Composable
fun TerminalTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = dynamicDarkColorScheme(LocalContext.current), content = content)
}
