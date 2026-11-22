package com.ferroterm.monitor

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Mêmes cas que les tests de `src/monitor.rs` (version bureau). */
class MonitorParserTest {
    private val block1 = "@@BEGIN\nhost pi\ncpu cpu 100 0 100 800 0 0 0 0 0 0\nmem MemTotal: 4000000 kB MemAvailable: 3000000 kB \nnet 1000 2000\nup 86400.50\nusers 5 deployer\ndisk / 68%;/tmp 0%;/boot/firmware 14%;\ncwd /home/deployer/app\nos Debian GNU/Linux 12 (bookworm)\nncpu 4\nrootfs 51200000 125000000\nproc 12.4 188416 nginx: worker process\nproc 0.3 8192 sshd\n@@END\n"
    private val block2 = "@@BEGIN\nhost pi\ncpu cpu 150 0 150 900 0 0 0 0 0 0\nmem MemTotal: 4000000 kB MemAvailable: 3000000 kB \nnet 1000 2000\nup 86402.50\nusers 5 deployer\ndisk / 68%;\n@@END\n"

    @Test
    fun premierBlocSansDebits() {
        val stats = MonitorParser().feed(block1)
        assertEquals(1, stats.size)
        val s = stats[0]
        assertEquals("pi", s.host)
        assertNull(s.cpuPercent)
        assertNull(s.rxRate)
        assertEquals(4_096_000_000L, s.memTotal)
        assertEquals(1_024_000_000L, s.memUsed)
        assertEquals(86_400L, s.uptimeSecs)
        assertEquals(5, s.sessions)
        assertEquals("deployer", s.user)
        assertEquals(listOf("/" to 68, "/tmp" to 0, "/boot/firmware" to 14), s.disks)
        assertEquals("/home/deployer/app", s.cwd)
        assertEquals("Debian GNU/Linux 12 (bookworm)", s.os)
        assertEquals(4, s.cpus)
        assertEquals(51_200_000L * 1024, s.rootUsed)
        assertEquals(125_000_000L * 1024, s.rootTotal)
        assertEquals(2, s.processes.size)
        assertEquals(MonitorProcess("nginx: worker process", 12.4f, 188_416L * 1024), s.processes[0])
    }

    @Test
    fun secondBlocCalculeLeCpu() {
        val parser = MonitorParser()
        val t0 = 1_000_000_000L
        parser.feed(block1, t0)
        val s = parser.feed(block2, t0 + 2_000_000_000L).single()
        // Delta : 200 au total dont 100 inactifs → 50 %.
        assertEquals(50f, s.cpuPercent!!, 0.1f)
        assertEquals(0.0, s.rxRate!!, 0.0)
        assertNull("bloc sans ligne cwd", s.cwd)
    }

    @Test
    fun blocsReassembles() {
        val parser = MonitorParser()
        assertTrue(parser.feed(block1.substring(0, 40)).isEmpty())
        assertEquals(1, parser.feed(block1.substring(40)).size)
    }

    @Test
    fun formats() {
        assertEquals("12 j 3 h", MonitorFormat.uptime(12 * 86_400L + 3 * 3_600))
        assertEquals("3 h 07", MonitorFormat.uptime(3 * 3_600L + 7 * 60))
        assertEquals("12 min", MonitorFormat.uptime(720))
        assertEquals("1,00 Go", MonitorFormat.gb(1_073_741_824))
        assertEquals("3,0 Mo/s", MonitorFormat.bytesRate(3_000_000.0))
        assertEquals("3 Ko/s", MonitorFormat.bytesRate(2_600.0))
        assertEquals("184 Mo", MonitorFormat.mem(188_416L * 1024))
        assertEquals("1,2 Go", MonitorFormat.mem(1_288_490_189))
    }

    @Test
    fun scriptSansGuillemetSimple() {
        assertFalse("le script est passé dans sh -c '…'", MonitorScript.SCRIPT.contains('\''))
        assertTrue(MonitorScript.SCRIPT.startsWith("while :; do"))
    }
}
