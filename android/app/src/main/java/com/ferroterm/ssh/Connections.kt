package com.ferroterm.ssh

import com.ferroterm.data.Session
import com.ferroterm.files.FileBrowser
import com.ferroterm.files.FileSource
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.flow.StateFlow

/**
 * Connexion ouverte (l'équivalent d'un onglet du bureau), SSH/SFTP ou FTP. Survit aux
 * changements d'écran : c'est [ConnectionManager] qui la crée et la ferme.
 */
interface RemoteConnection : FileSource {
    val id: Long
    val session: Session
    val state: StateFlow<ConnectionState>
    val prompt: StateFlow<Prompt?>

    /** Explorateur de fichiers (dossier courant et transferts conservés). */
    val files: FileBrowser

    /** Terminal disponible (sessions SSH seulement). */
    val hasShell: Boolean

    val isActive: Boolean
        get() = state.value !is ConnectionState.Closed

    fun connect()

    /** Coupe la connexion ; elle reste affichée, et [connect] peut la rouvrir. */
    fun disconnect()

    /** Libère tout. L'objet n'est plus utilisable ensuite. */
    fun dispose()

    /** Oublie la clé (SSH) ou le certificat (FTPS) qui a changé, avant de reconnecter. */
    fun forgetChangedHostKey(key: HostKeyInfo)
}

/** Question posée à l'utilisateur pendant la connexion. Le dialogue répond via `answer`. */
sealed interface Prompt {
    /** Identifiant et/ou secret (mot de passe ou phrase de passe de la clé). */
    data class Credentials(
        val title: String,
        val username: String?,
        val askUsername: Boolean,
        val secretLabel: String?,
        val error: String?,
        val canRemember: Boolean,
        val usernameHint: String? = null,
        val answer: CompletableDeferred<CredentialsAnswer?> = CompletableDeferred(),
    ) : Prompt

    /** Serveur SSH jamais vu : faut-il faire confiance à cette clé ? */
    data class UnknownHostKey(
        val key: HostKeyInfo,
        val answer: CompletableDeferred<Boolean> = CompletableDeferred(),
    ) : Prompt

    /**
     * Certificat FTPS non reconnu par les autorités du téléphone (auto-signé, expiré…),
     * ou différent de celui accepté la dernière fois ([changed]).
     */
    data class Certificate(
        val host: String,
        val reason: String,
        val fingerprint: String,
        val changed: Boolean,
        val answer: CompletableDeferred<Boolean> = CompletableDeferred(),
    ) : Prompt
}

data class CredentialsAnswer(val username: String, val secret: String, val remember: Boolean)

sealed interface ConnectionState {
    data class Connecting(val step: String) : ConnectionState
    data object Connected : ConnectionState

    /**
     * Connexion terminée. [changedHostKey] est renseigné quand la clé du serveur
     * ne correspond plus à celle enregistrée : l'écran propose alors de l'oublier.
     */
    data class Closed(
        val message: String,
        val error: Boolean,
        val changedHostKey: HostKeyInfo? = null,
    ) : ConnectionState
}

/** Répond « non » à une question restée ouverte (connexion coupée). */
fun Prompt.dismiss() {
    when (this) {
        is Prompt.Credentials -> answer.complete(null)
        is Prompt.UnknownHostKey -> answer.complete(false)
        is Prompt.Certificate -> answer.complete(false)
    }
}

/** Attend la réponse de l'utilisateur, quel que soit le type de question. */
suspend fun Prompt.awaitAnswer() {
    when (this) {
        is Prompt.Credentials -> answer.await()
        is Prompt.UnknownHostKey -> answer.await()
        is Prompt.Certificate -> answer.await()
    }
}
