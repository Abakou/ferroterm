package com.ferroterm.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test

class SessionExportTest {
    /**
     * Vecteur fixe, vérifié aussi par le test du bureau (`src/bundle.rs`,
     * `opens_known_vector`) : les deux implémentations doivent rester compatibles.
     */
    private val knownVector = Sealed(
        kdf = "pbkdf2-sha256",
        iterations = 1000,
        salt = "AQEBAQEBAQEBAQEBAQEBAQ==",
        cipher = "aes-256-gcm",
        nonce = "AgICAgICAgICAgIC",
        data = "PD7LNdnynMd7ZxX30DSHhFeFdIp2SedZp3mwAqVm0bT7+r3AoalXPH64TTxODMmE774vIvNfdZVOovkk",
    )

    @Test
    fun dechiffreLeVecteurDuBureau() {
        val secrets = SessionExport.open(knownVector, "pw")
        assertEquals(listOf(PasswordEntry(session = 1, profile = 0, password = "x")), secrets.passwords)
        assertEquals(emptyList<KeyEntry>(), secrets.keys)
    }

    @Test
    fun refuseUnMauvaisMotDePasse() {
        assertThrows(WrongPasswordException::class.java) { SessionExport.open(knownVector, "autre") }
        assertThrows(IllegalArgumentException::class.java) {
            SessionExport.open(knownVector.copy(iterations = Int.MAX_VALUE), "pw")
        }
        assertThrows(IllegalArgumentException::class.java) {
            SessionExport.open(knownVector.copy(kdf = "argon2id"), "pw")
        }
    }

    /** Export du bureau : sessions en clair, profil actif, secrets chiffrés. */
    @Test
    fun litUnExportDuBureau() {
        val text = "\uFEFF" + """
            {"format":"ferroterm-sessions","version":1,
             "sessions":[
               {"id":7,"name":"Prod","folder":"Clients/A","host":"10.0.0.5","port":22,
                "username":"root","key_path":"C:\\Users\\a\\.ssh\\id_ed25519",
                "profiles":[{"id":0,"name":"Principal","host":"10.0.0.5","port":22},
                            {"id":2,"name":"Secours","host":"10.0.0.6","port":22}],"profile":2},
               {"id":8,"name":"Web","host":"web","port":22}],
             "folders":["Clients/A"],
             "secrets":{"kdf":"pbkdf2-sha256","iterations":1000,"salt":"AQEBAQEBAQEBAQEBAQEBAQ==",
                        "cipher":"aes-256-gcm","nonce":"AgICAgICAgICAgIC","data":"${knownVector.data}"}}
        """.trimIndent()
        val export = SessionExport.parse(text)
        assertEquals(2, export.sessions.size)
        assertEquals(2L, SessionExport.activeProfile(export.sessions[0]))
        assertEquals(0L, SessionExport.activeProfile(export.sessions[1]))
        assertEquals(knownVector, export.secrets)
        val session = sessionJson.decodeFromJsonElement(Session.serializer(), export.sessions[0])
        assertEquals("C:\\Users\\a\\.ssh\\id_ed25519", session.keyPath)
    }

    @Test
    fun exportSansSecrets() {
        val export = SessionExport.parse("""{"format":"ferroterm-sessions","version":1,"sessions":[]}""")
        assertNull(export.secrets)
        assertNotNull(export.folders)
    }

    @Test
    fun refuseUnAutreFichier() {
        assertThrows(IllegalArgumentException::class.java) { SessionExport.parse("""{"sessions":[]}""") }
        assertThrows(IllegalArgumentException::class.java) { SessionExport.parse("pas du json") }
    }
}
