package com.chuddling.chudmic

import android.content.Context
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import java.util.concurrent.CopyOnWriteArrayList

/** Wire protocol shared with the PC app. Keep in sync with pc/src/proto.rs. */
object Proto {
    const val DISCOVERY_PORT = 47800
    const val SESSION_PORT = 47801
    const val VERSION: Byte = 1
    const val SAMPLE_RATE = 48000
    const val FRAME_SAMPLES = 480 // 10 ms

    const val T_HELLO: Byte = 0x48   // 'H'
    const val T_WELCOME: Byte = 0x57 // 'W'
    const val T_BUSY: Byte = 0x42    // 'B'
    const val T_AUDIO: Byte = 0x41   // 'A'
    const val T_PING: Byte = 0x50    // 'P'
    const val T_BYE: Byte = 0x58     // 'X'

    fun packet(type: Byte, payload: ByteArray = ByteArray(0)): ByteArray {
        val out = ByteArray(3 + payload.size)
        out[0] = 0x43 // 'C'
        out[1] = 0x4D // 'M'
        out[2] = type
        payload.copyInto(out, 3)
        return out
    }

    fun isValid(buf: ByteArray, len: Int) = len >= 3 && buf[0] == 0x43.toByte() && buf[1] == 0x4D.toByte()

    fun discovery(kind: String, port: Int, name: String) =
        "CHUDMIC|1|$kind|phone|$port|$name".toByteArray(Charsets.UTF_8)

    fun deviceName(ctx: Context): String {
        val n = try {
            Settings.Global.getString(ctx.contentResolver, "device_name")
        } catch (e: Exception) {
            null
        }
        return if (n.isNullOrBlank()) Build.MODEL else n
    }
}

/** State shared between the screen and the background mic service. */
object MicState {
    enum class Conn { IDLE, CONNECTING, CONNECTED }

    @Volatile var conn = Conn.IDLE
    @Volatile var pcName = ""
    @Volatile var muted = false
    @Volatile var volume = 100 // percent, 0..200
    @Volatile var level = 0f   // 0..1, last mic peak
    @Volatile var pcMuted = false
    @Volatile var lastError: String? = null

    private val listeners = CopyOnWriteArrayList<() -> Unit>()
    private val main = Handler(Looper.getMainLooper())

    fun addListener(l: () -> Unit) { listeners.add(l) }
    fun removeListener(l: () -> Unit) { listeners.remove(l) }

    /** Notify listeners on the main thread. */
    fun changed() {
        main.post { for (l in listeners) l() }
    }
}
