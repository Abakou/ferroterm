package com.ferroterm.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class MacroTest {
    private val macro = Macro(name = "Mise à jour", commands = "sudo apt update\n\n  \nsudo apt upgrade -y  \n")

    @Test
    fun lignesVidesIgnorees() {
        assertEquals(listOf("sudo apt update", "sudo apt upgrade -y"), macro.lines)
    }

    @Test
    fun executionAvecRetourChariot() {
        assertEquals("sudo apt update\rsudo apt upgrade -y\r", macro.input().decodeToString())
    }

    @Test
    fun ligneDeCommandeSansRetourChariot() {
        assertEquals("sudo apt update; sudo apt upgrade -y", macro.commandLine())
    }

    /** Fichier du bureau : `auto_run` (et l'ancien nom `run_on_click` ignoré sans erreur). */
    @Test
    fun formatDuBureau() {
        val read = sessionJson.decodeFromString<Macro>("""{"id":4,"name":"Logs","commands":"tail -f /var/log/syslog","auto_run":true}""")
        assertEquals(Macro(4, "Logs", "tail -f /var/log/syslog", autoRun = true), read)
        assertTrue("auto_run" in sessionJson.encodeToString(Macro.serializer(), read))
    }
}
