package com.ferroterm

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.Composable
import androidx.navigation.NavHostController
import androidx.navigation.NavType
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import androidx.navigation.navArgument
import com.ferroterm.ui.FerrotermTheme
import com.ferroterm.ui.MonitorScreen
import com.ferroterm.ui.SessionEditScreen
import com.ferroterm.ui.SessionListScreen
import com.ferroterm.ui.SftpScreen
import com.ferroterm.ui.TerminalScreen

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val app = application as FerrotermApp
        setContent {
            FerrotermTheme { Navigation(app) }
        }
    }
}

/**
 * Bascule terminal ↔ fichiers d'une même connexion en remplaçant l'écran courant :
 * « Retour » ramène toujours à la liste des sessions, sans empiler les allers-retours.
 */
private fun NavHostController.switchTo(route: String) {
    val current = currentDestination?.route ?: return navigate(route)
    navigate(route) { popUpTo(current) { inclusive = true } }
}

@Composable
private fun Navigation(app: FerrotermApp) {
    val nav = rememberNavController()
    NavHost(navController = nav, startDestination = "sessions") {
        composable("sessions") {
            SessionListScreen(
                app = app,
                onEdit = { id -> nav.navigate("edit/$id") },
                // Sessions « SFTP » : pas de terminal, on ouvre directement les fichiers.
                onOpenConnection = { connectionId ->
                    val hasShell = app.connections.get(connectionId)?.hasShell != false
                    nav.navigate(if (hasShell) "terminal/$connectionId" else "files/$connectionId")
                },
            )
        }
        composable("edit/{id}", arguments = listOf(navArgument("id") { type = NavType.LongType })) { entry ->
            SessionEditScreen(
                app = app,
                sessionId = entry.arguments?.getLong("id") ?: 0L,
                onDone = { nav.popBackStack() },
            )
        }
        composable("terminal/{id}", arguments = listOf(navArgument("id") { type = NavType.LongType })) { entry ->
            val id = entry.arguments?.getLong("id") ?: 0L
            TerminalScreen(
                app = app,
                connectionId = id,
                onBack = { nav.popBackStack() },
                onOpenFiles = { nav.switchTo("files/$id") },
                onOpenMonitor = { nav.navigate("monitor/$id") },
            )
        }
        composable("monitor/{id}", arguments = listOf(navArgument("id") { type = NavType.LongType })) { entry ->
            MonitorScreen(app = app, connectionId = entry.arguments?.getLong("id") ?: 0L, onBack = { nav.popBackStack() })
        }
        composable("files/{id}", arguments = listOf(navArgument("id") { type = NavType.LongType })) { entry ->
            val id = entry.arguments?.getLong("id") ?: 0L
            SftpScreen(
                app = app,
                connectionId = id,
                onBack = { nav.popBackStack() },
                onOpenTerminal = { nav.switchTo("terminal/$id") },
            )
        }
    }
}
