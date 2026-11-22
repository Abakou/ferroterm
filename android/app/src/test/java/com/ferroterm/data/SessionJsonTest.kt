package com.ferroterm.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SessionJsonTest {
    /** Session telle que l'écrit la version bureau (`src/session.rs`). */
    @Test
    fun litUneSessionDuBureau() {
        val json = """
            {"id":7,"name":"Prod","folder":"Clients/A","host":"10.0.0.5","port":2222,
             "username":"root","key_path":"C:\\Users\\a\\.ssh\\id_ed25519",
             "profiles":[{"id":0,"name":"Principal","host":"10.0.0.5","port":2222}],"profile":0}
        """.trimIndent()
        val session = sessionJson.decodeFromString<Session>(json)
        assertEquals(7L, session.id)
        assertEquals("Clients/A", session.folder)
        assertEquals(Protocol.SSH, session.protocol) // champ absent = SSH, comme le bureau
        assertEquals(2222, session.port)
        assertEquals("C:\\Users\\a\\.ssh\\id_ed25519", session.keyPath)
        assertEquals("root@10.0.0.5:2222", session.target)
    }

    @Test
    fun litUneSessionFtpDuBureau() {
        val json = """{"id":3,"name":"","protocol":"ftp","ftp":{"tls":"implicit","active_mode":true},"host":"ftp.ex","port":990}"""
        val session = sessionJson.decodeFromString<Session>(json)
        assertEquals(Protocol.FTP, session.protocol)
        assertEquals(FtpTls.IMPLICIT, session.ftp.tls)
        assertTrue(session.ftp.activeMode)
        assertNull(session.keyPath)
        assertEquals("", session.username)
        assertEquals("ftp.ex", session.target) // 990 = port par défaut du FTPS implicite
        assertEquals("ftp.ex", session.displayName)
    }

    /** Le bureau exige `id`, `name`, `host` et `port` : ils doivent toujours être écrits. */
    @Test
    fun ecritLesChampsExigesParLeBureau() {
        val text = sessionJson.encodeToString(Session.serializer(), Session(host = "h"))
        for (field in listOf("\"id\"", "\"name\"", "\"host\"", "\"port\"")) {
            assertTrue("$field absent de $text", field in text)
        }
        assertEquals(Session(host = "h"), sessionJson.decodeFromString<Session>(text))
    }
}
