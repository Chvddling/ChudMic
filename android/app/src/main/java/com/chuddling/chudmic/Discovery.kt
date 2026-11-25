package com.chuddling.chudmic

import android.content.Context
import android.net.wifi.WifiManager
import android.os.SystemClock
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.SocketAddress
import java.net.SocketTimeoutException
import java.util.concurrent.ConcurrentHashMap
import kotlin.concurrent.thread

/**
 * Finds ChudMic PCs on the local network (UDP broadcast on port 47800) and
 * listens for "connect" requests sent from the PC app.
 */
class Discovery(
    private val ctx: Context,
    private val onListChanged: () -> Unit,
    private val onConnectRequest: (host: String, port: Int, name: String) -> Unit,
) {
    data class Pc(val host: String, val port: Int, val name: String, val lastSeen: Long)

    private val pcs = ConcurrentHashMap<String, Pc>()
    @Volatile private var running = false
    @Volatile private var socket: DatagramSocket? = null
    private var worker: Thread? = null
    private var multicastLock: WifiManager.MulticastLock? = null

    fun list(): List<Pc> = pcs.values.sortedBy { it.name.lowercase() }

    fun start() {
        if (running) return
        running = true
        // Some phones drop incoming broadcasts unless this lock is held.
        val wm = ctx.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        multicastLock = wm.createMulticastLock("ChudMicDiscovery").apply {
            setReferenceCounted(false)
            acquire()
        }
        worker = thread(name = "ChudMicDiscovery") { loop() }
    }

    fun stop() {
        running = false
        socket?.close()
        try { worker?.join(500) } catch (_: Exception) {}
        worker = null
        try { multicastLock?.release() } catch (_: Exception) {}
        multicastLock = null
    }

    private fun openSocket(): DatagramSocket {
        return try {
            DatagramSocket(null as SocketAddress?).apply {
                reuseAddress = true
                bind(InetSocketAddress(Proto.DISCOVERY_PORT))
                broadcast = true
                soTimeout = 300
            }
        } catch (e: Exception) {
            // Port taken: we can still announce ourselves and hear the PCs' replies.
            DatagramSocket().apply {
                broadcast = true
                soTimeout = 300
            }
        }
    }

    private fun loop() {
        val sock = try { openSocket() } catch (e: Exception) { return }
        socket = sock
        val name = Proto.deviceName(ctx)
        val ann = Proto.discovery("ANN", 0, name)
        val rep = Proto.discovery("REP", 0, name)
        val broadcast = InetAddress.getByName("255.255.255.255")
        val buf = ByteArray(1500)
        val pkt = DatagramPacket(buf, buf.size)
        var lastAnn = 0L

        while (running) {
            val now = SystemClock.elapsedRealtime()
            if (now - lastAnn >= 1500) {
                lastAnn = now
                try { sock.send(DatagramPacket(ann, ann.size, broadcast, Proto.DISCOVERY_PORT)) } catch (_: Exception) {}
                // Forget PCs we haven't heard from in a while.
                if (pcs.values.removeIf { now - it.lastSeen > 6000 }) onListChanged()
            }
            try {
                pkt.length = buf.size
                sock.receive(pkt)
            } catch (e: SocketTimeoutException) {
                continue
            } catch (e: Exception) {
                if (!running) break
                SystemClock.sleep(200)
                continue
            }

            val parts = String(buf, 0, pkt.length, Charsets.UTF_8).split('|', limit = 6)
            if (parts.size < 6 || parts[0] != "CHUDMIC" || parts[3] != "pc") continue
            val host = pkt.address?.hostAddress ?: continue
            val port = parts[4].toIntOrNull() ?: Proto.SESSION_PORT
            val pcName = parts[5].ifBlank { host }

            when (parts[2]) {
                "ANN", "REP" -> {
                    val isNew = !pcs.containsKey(host)
                    pcs[host] = Pc(host, port, pcName, SystemClock.elapsedRealtime())
                    if (isNew) onListChanged()
                    if (parts[2] == "ANN") {
                        try { sock.send(DatagramPacket(rep, rep.size, pkt.address, pkt.port)) } catch (_: Exception) {}
                    }
                }
                "CONNECT" -> onConnectRequest(host, port, pcName)
            }
        }
        sock.close()
    }
}
