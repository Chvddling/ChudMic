package com.chuddling.chudmic

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.drawable.Icon
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import java.io.IOException
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.SocketTimeoutException
import kotlin.concurrent.thread
import kotlin.math.abs
import kotlin.math.max

/**
 * Foreground service that records the mic and streams it to the PC.
 * Runs as a service so audio keeps flowing with the screen off.
 */
class MicService : Service() {

    companion object {
        const val ACTION_CONNECT = "com.chuddling.chudmic.CONNECT"
        const val ACTION_DISCONNECT = "com.chuddling.chudmic.DISCONNECT"
        const val ACTION_TOGGLE_MUTE = "com.chuddling.chudmic.TOGGLE_MUTE"
        const val EXTRA_HOST = "host"
        const val EXTRA_PORT = "port"
        const val EXTRA_NAME = "name"
        private const val CHANNEL = "chudmic"
        private const val NOTIF_ID = 1

        fun connect(ctx: Context, host: String, port: Int, name: String) {
            val i = Intent(ctx, MicService::class.java)
                .setAction(ACTION_CONNECT)
                .putExtra(EXTRA_HOST, host)
                .putExtra(EXTRA_PORT, port)
                .putExtra(EXTRA_NAME, name)
            ctx.startForegroundService(i)
        }

        fun disconnect(ctx: Context) {
            ctx.startService(Intent(ctx, MicService::class.java).setAction(ACTION_DISCONNECT))
        }
    }

    private val main = Handler(Looper.getMainLooper())
    private var worker: Worker? = null
    private var wifiLock: WifiManager.WifiLock? = null
    private var wakeLock: PowerManager.WakeLock? = null
    private val stateListener: () -> Unit = { if (worker != null) updateNotification() }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL, "Microphone streaming", NotificationManager.IMPORTANCE_LOW)
        )
        MicState.addListener(stateListener)
    }

    override fun onDestroy() {
        MicState.removeListener(stateListener)
        worker?.stopNow()
        worker = null
        releaseLocks()
        super.onDestroy()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_CONNECT -> {
                val host = intent.getStringExtra(EXTRA_HOST)
                val port = intent.getIntExtra(EXTRA_PORT, Proto.SESSION_PORT)
                val name = intent.getStringExtra(EXTRA_NAME) ?: host ?: ""
                // Must go foreground right away (Android requirement).
                if (!goForeground("Connecting to $name…") || host == null) {
                    stopSelf()
                    return START_NOT_STICKY
                }
                worker?.stopNow()
                acquireLocks()
                MicState.pcName = name
                MicState.pcMuted = false
                MicState.lastError = null
                MicState.conn = MicState.Conn.CONNECTING
                MicState.changed()
                worker = Worker(host, port).also { it.start() }
            }
            ACTION_DISCONNECT -> {
                val w = worker
                if (w != null) w.stopNow() else stopSelf()
            }
            ACTION_TOGGLE_MUTE -> {
                MicState.muted = !MicState.muted
                MicState.changed()
                if (worker == null) stopSelf()
            }
            else -> if (worker == null) stopSelf()
        }
        return START_NOT_STICKY
    }

    private fun goForeground(text: String): Boolean = try {
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTIF_ID, buildNotification(text), ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
        } else {
            startForeground(NOTIF_ID, buildNotification(text))
        }
        true
    } catch (e: Exception) {
        MicState.lastError = "Android blocked the mic: ${e.message}"
        MicState.conn = MicState.Conn.IDLE
        MicState.changed()
        false
    }

    private fun statusText(): String = when (MicState.conn) {
        MicState.Conn.CONNECTED -> "Streaming to ${MicState.pcName}" + if (MicState.muted) " (muted)" else ""
        MicState.Conn.CONNECTING -> "Connecting to ${MicState.pcName}…"
        MicState.Conn.IDLE -> "Not connected"
    }

    private fun updateNotification() {
        getSystemService(NotificationManager::class.java).notify(NOTIF_ID, buildNotification(statusText()))
    }

    private fun buildNotification(text: String): Notification {
        val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        val open = PendingIntent.getActivity(
            this, 0,
            Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP), flags
        )
        val mute = PendingIntent.getService(
            this, 1, Intent(this, MicService::class.java).setAction(ACTION_TOGGLE_MUTE), flags
        )
        val stop = PendingIntent.getService(
            this, 2, Intent(this, MicService::class.java).setAction(ACTION_DISCONNECT), flags
        )
        val icon = Icon.createWithResource(this, R.drawable.ic_mic)
        return Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_mic)
            .setContentTitle("ChudMic")
            .setContentText(text)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(icon, if (MicState.muted) "Unmute" else "Mute", mute).build())
            .addAction(Notification.Action.Builder(icon, "Disconnect", stop).build())
            .build()
    }

    @Suppress("DEPRECATION")
    private fun acquireLocks() {
        if (wifiLock == null) {
            val wm = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
            val mode = if (Build.VERSION.SDK_INT >= 29) WifiManager.WIFI_MODE_FULL_LOW_LATENCY
            else WifiManager.WIFI_MODE_FULL_HIGH_PERF
            wifiLock = wm.createWifiLock(mode, "ChudMic").apply { setReferenceCounted(false); acquire() }
        }
        if (wakeLock == null) {
            val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
            wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "ChudMic:mic")
                .apply { setReferenceCounted(false); acquire() }
        }
    }

    private fun releaseLocks() {
        try { wifiLock?.release() } catch (_: Exception) {}
        try { wakeLock?.release() } catch (_: Exception) {}
        wifiLock = null
        wakeLock = null
    }

    /** Called on the main thread when a worker thread finishes. */
    private fun onWorkerEnded(w: Worker, reason: String?) {
        if (worker !== w) return // replaced by a newer connection
        worker = null
        releaseLocks()
        MicState.conn = MicState.Conn.IDLE
        MicState.level = 0f
        MicState.pcMuted = false
        MicState.lastError = reason
        MicState.changed()
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private inner class Worker(private val host: String, private val port: Int) : Thread("ChudMicWorker") {
        @Volatile private var stopping = false
        @Volatile private var lastRx = 0L
        @Volatile private var reason: String? = null
        @Volatile private var socket: DatagramSocket? = null

        fun stopNow() {
            stopping = true
        }

        override fun run() {
            var rec: AudioRecord? = null
            var rx: Thread? = null
            var sendBye = false
            try {
                val addr = InetAddress.getByName(host)
                val sock = DatagramSocket()
                socket = sock
                sock.soTimeout = 200

                // ---- handshake ----
                val hello = Proto.packet(Proto.T_HELLO, byteArrayOf(Proto.VERSION) + Proto.deviceName(this@MicService).toByteArray())
                val buf = ByteArray(2048)
                val pkt = DatagramPacket(buf, buf.size)
                val deadline = SystemClock.elapsedRealtime() + 4000
                var lastHello = 0L
                var pcName: String? = null
                while (!stopping && pcName == null && SystemClock.elapsedRealtime() < deadline) {
                    val now = SystemClock.elapsedRealtime()
                    if (now - lastHello >= 500) {
                        sock.send(DatagramPacket(hello, hello.size, addr, port))
                        lastHello = now
                    }
                    try {
                        pkt.length = buf.size
                        sock.receive(pkt)
                    } catch (e: SocketTimeoutException) {
                        continue
                    }
                    if (pkt.address != addr || !Proto.isValid(buf, pkt.length)) continue
                    when (buf[2]) {
                        Proto.T_WELCOME -> pcName = if (pkt.length > 4) String(buf, 4, pkt.length - 4, Charsets.UTF_8) else host
                        Proto.T_BUSY -> throw IOException("PC is busy with another phone")
                    }
                }
                if (stopping) return
                if (pcName == null) throw IOException("No answer from PC (same Wi-Fi? firewall?)")

                sendBye = true
                lastRx = SystemClock.elapsedRealtime()
                MicState.pcName = pcName
                MicState.conn = MicState.Conn.CONNECTED
                MicState.changed()

                // ---- receive thread: pings and bye from the PC ----
                rx = thread(name = "ChudMicRx") {
                    val b = ByteArray(2048)
                    val p = DatagramPacket(b, b.size)
                    while (!stopping) {
                        try {
                            p.length = b.size
                            sock.receive(p)
                        } catch (e: SocketTimeoutException) {
                            continue
                        } catch (e: Exception) {
                            if (stopping) break
                            SystemClock.sleep(20)
                            continue
                        }
                        if (p.address != addr || !Proto.isValid(b, p.length)) continue
                        lastRx = SystemClock.elapsedRealtime()
                        when (b[2]) {
                            Proto.T_PING -> {
                                val m = p.length > 3 && (b[3].toInt() and 1) != 0
                                if (m != MicState.pcMuted) {
                                    MicState.pcMuted = m
                                    MicState.changed()
                                }
                            }
                            Proto.T_BYE -> {
                                reason = "PC disconnected"
                                sendBye = false
                                stopping = true
                            }
                        }
                    }
                }

                // ---- record + send ----
                val minBuf = AudioRecord.getMinBufferSize(
                    Proto.SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT
                )
                val r = AudioRecord(
                    MediaRecorder.AudioSource.MIC, Proto.SAMPLE_RATE,
                    AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT,
                    max(minBuf, Proto.FRAME_SAMPLES * 2 * 8)
                )
                rec = r
                if (r.state != AudioRecord.STATE_INITIALIZED) throw IOException("Could not open the microphone")
                r.startRecording()

                val samples = ShortArray(Proto.FRAME_SAMPLES)
                val out = ByteArray(8 + Proto.FRAME_SAMPLES * 2)
                out[0] = 0x43; out[1] = 0x4D; out[2] = Proto.T_AUDIO
                var seq = 0
                var lastPing = 0L

                while (!stopping) {
                    val n = r.read(samples, 0, samples.size)
                    if (n < 0) throw IOException("Microphone error ($n)")
                    val now = SystemClock.elapsedRealtime()
                    if (now - lastRx > 3000) throw IOException("Lost connection to PC")

                    val muted = MicState.muted
                    val flags: Byte = if (muted) 1 else 0
                    if (n > 0) {
                        val gain = MicState.volume / 100f
                        var peak = 0
                        for (i in 0 until n) {
                            var v = (samples[i] * gain).toInt()
                            if (v > 32767) v = 32767 else if (v < -32768) v = -32768
                            if (abs(v) > peak) peak = abs(v)
                            out[8 + 2 * i] = (v and 0xFF).toByte()
                            out[9 + 2 * i] = ((v shr 8) and 0xFF).toByte()
                        }
                        MicState.level = if (muted) 0f else peak / 32768f
                        if (!muted) {
                            out[3] = (seq and 0xFF).toByte()
                            out[4] = ((seq shr 8) and 0xFF).toByte()
                            out[5] = ((seq shr 16) and 0xFF).toByte()
                            out[6] = ((seq shr 24) and 0xFF).toByte()
                            out[7] = flags
                            sock.send(DatagramPacket(out, 8 + n * 2, addr, port))
                            seq++
                        }
                    }
                    if (now - lastPing >= 500) {
                        lastPing = now
                        val ping = Proto.packet(Proto.T_PING, byteArrayOf(flags))
                        sock.send(DatagramPacket(ping, ping.size, addr, port))
                    }
                }
            } catch (e: Exception) {
                if (!stopping) reason = e.message ?: e.toString()
            } finally {
                stopping = true
                try { rec?.stop() } catch (_: Exception) {}
                try { rec?.release() } catch (_: Exception) {}
                val sock = socket
                if (sock != null) {
                    if (sendBye) {
                        try {
                            val bye = Proto.packet(Proto.T_BYE)
                            sock.send(DatagramPacket(bye, bye.size, InetAddress.getByName(host), port))
                        } catch (_: Exception) {}
                    }
                    sock.close()
                }
                try { rx?.join(500) } catch (_: Exception) {}
                val r = reason
                main.post { onWorkerEnded(this, r) }
            }
        }
    }
}
