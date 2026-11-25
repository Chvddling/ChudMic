package com.chuddling.chudmic

import android.Manifest
import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.ColorStateList
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.text.InputType
import android.view.View
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.SeekBar
import android.widget.TextView
import android.widget.Toast
import com.google.zxing.integration.android.IntentIntegrator
import kotlin.math.sqrt

class MainActivity : Activity() {

    private lateinit var statusText: TextView
    private lateinit var pcMutedText: TextView
    private lateinit var connectBtn: Button
    private lateinit var muteBtn: Button
    private lateinit var volumeLabel: TextView
    private lateinit var volumeBar: SeekBar
    private lateinit var levelBar: ProgressBar
    private lateinit var emptyText: TextView
    private lateinit var pcList: LinearLayout
    private lateinit var discovery: Discovery

    private val prefs by lazy { getSharedPreferences("chudmic", MODE_PRIVATE) }
    private val handler = Handler(Looper.getMainLooper())
    private val stateListener: () -> Unit = { refreshUi() }
    private var pendingConnect: Triple<String, Int, String>? = null

    private val levelTick = object : Runnable {
        override fun run() {
            levelBar.progress = (sqrt(MicState.level) * 100).toInt().coerceIn(0, 100)
            handler.postDelayed(this, 60)
        }
    }

    companion object {
        private const val REQ_PERMS = 1
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)
        statusText = findViewById(R.id.statusText)
        pcMutedText = findViewById(R.id.pcMutedText)
        connectBtn = findViewById(R.id.connectBtn)
        muteBtn = findViewById(R.id.muteBtn)
        volumeLabel = findViewById(R.id.volumeLabel)
        volumeBar = findViewById(R.id.volumeBar)
        levelBar = findViewById(R.id.levelBar)
        emptyText = findViewById(R.id.emptyText)
        pcList = findViewById(R.id.pcList)

        MicState.volume = prefs.getInt("volume", 100)
        volumeBar.progress = MicState.volume
        volumeLabel.text = "Mic volume: ${MicState.volume}%"

        connectBtn.setOnClickListener { onConnectButton() }
        muteBtn.setOnClickListener {
            MicState.muted = !MicState.muted
            MicState.changed()
        }
        volumeBar.setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
            override fun onProgressChanged(sb: SeekBar, value: Int, fromUser: Boolean) {
                MicState.volume = value
                volumeLabel.text = "Mic volume: $value%"
            }
            override fun onStartTrackingTouch(sb: SeekBar) {}
            override fun onStopTrackingTouch(sb: SeekBar) {
                prefs.edit().putInt("volume", MicState.volume).apply()
            }
        })
        findViewById<Button>(R.id.scanBtn).setOnClickListener { scanQr() }
        findViewById<Button>(R.id.manualBtn).setOnClickListener { askForIp() }

        discovery = Discovery(
            this,
            onListChanged = { runOnUiThread { refreshPcList() } },
            onConnectRequest = { host, port, name ->
                runOnUiThread {
                    if (MicState.conn == MicState.Conn.IDLE) connectTo(host, port, name)
                }
            },
        )

        // Ask for the mic (and notifications on Android 13+) up front.
        val wanted = mutableListOf(Manifest.permission.RECORD_AUDIO)
        if (Build.VERSION.SDK_INT >= 33) wanted.add(Manifest.permission.POST_NOTIFICATIONS)
        val missing = wanted.filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (missing.isNotEmpty()) requestPermissions(missing.toTypedArray(), REQ_PERMS)
    }

    override fun onStart() {
        super.onStart()
        MicState.addListener(stateListener)
        discovery.start()
        handler.post(levelTick)
        refreshUi()
    }

    override fun onStop() {
        MicState.removeListener(stateListener)
        discovery.stop()
        handler.removeCallbacks(levelTick)
        super.onStop()
    }

    // ---------- connecting ----------

    private fun onConnectButton() {
        if (MicState.conn != MicState.Conn.IDLE) {
            MicService.disconnect(this)
            return
        }
        val found = discovery.list()
        val lastHost = prefs.getString("lastHost", null)
        val last = found.firstOrNull { it.host == lastHost }
        when {
            last != null -> connectTo(last.host, last.port, last.name)
            found.isNotEmpty() -> found[0].let { connectTo(it.host, it.port, it.name) }
            lastHost != null -> connectTo(
                lastHost,
                prefs.getInt("lastPort", Proto.SESSION_PORT),
                prefs.getString("lastName", lastHost) ?: lastHost,
            )
            else -> toast("No PC found. Open ChudMic on your PC, or tap Scan QR.")
        }
    }

    private fun connectTo(host: String, port: Int, name: String) {
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            pendingConnect = Triple(host, port, name)
            requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), REQ_PERMS)
            return
        }
        prefs.edit()
            .putString("lastHost", host)
            .putInt("lastPort", port)
            .putString("lastName", name)
            .apply()
        MicService.connect(this, host, port, name)
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != REQ_PERMS) return
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
            pendingConnect?.let { (h, p, n) -> connectTo(h, p, n) }
        } else {
            toast("ChudMic needs microphone permission to work.")
        }
        pendingConnect = null
    }

    // ---------- QR / manual ----------

    private fun scanQr() {
        IntentIntegrator(this)
            .setDesiredBarcodeFormats(IntentIntegrator.QR_CODE)
            .setPrompt("Scan the QR code shown in ChudMic on your PC")
            .setBeepEnabled(false)
            .initiateScan()
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        val result = IntentIntegrator.parseActivityResult(requestCode, resultCode, data)
        if (result == null) {
            super.onActivityResult(requestCode, resultCode, data)
            return
        }
        val text = result.contents ?: return
        // Format: CHUDMIC|1|<ip>|<port>|<pc name>
        val parts = text.split('|', limit = 5)
        if (parts.size >= 4 && parts[0] == "CHUDMIC") {
            val port = parts[3].toIntOrNull() ?: Proto.SESSION_PORT
            connectTo(parts[2], port, parts.getOrNull(4)?.ifBlank { null } ?: parts[2])
        } else {
            toast("That's not a ChudMic QR code.")
        }
    }

    private fun askForIp() {
        val input = EditText(this).apply {
            hint = "192.168.1.10"
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI
            setText(prefs.getString("lastHost", "") ?: "")
        }
        AlertDialog.Builder(this)
            .setTitle("PC address")
            .setMessage("The IP is shown under the QR code in the PC app.")
            .setView(input)
            .setPositiveButton("Connect") { _, _ ->
                val raw = input.text.toString().trim()
                if (raw.isEmpty()) return@setPositiveButton
                val host = raw.substringBefore(':')
                val port = raw.substringAfter(':', "").toIntOrNull() ?: Proto.SESSION_PORT
                connectTo(host, port, host)
            }
            .setNegativeButton("Cancel", null)
            .show()
    }

    // ---------- UI ----------

    private fun refreshUi() {
        when (MicState.conn) {
            MicState.Conn.IDLE -> {
                statusText.text = MicState.lastError?.let { "Not connected: $it" } ?: "Not connected"
                connectBtn.text = "Connect"
            }
            MicState.Conn.CONNECTING -> {
                statusText.text = "Connecting to ${MicState.pcName}…"
                connectBtn.text = "Cancel"
            }
            MicState.Conn.CONNECTED -> {
                statusText.text = "Connected to ${MicState.pcName}"
                connectBtn.text = "Disconnect"
            }
        }
        val connected = MicState.conn == MicState.Conn.CONNECTED
        statusText.setTextColor(getColor(if (connected) R.color.accent else R.color.text))
        pcMutedText.visibility = if (connected && MicState.pcMuted) View.VISIBLE else View.GONE

        muteBtn.text = if (MicState.muted) "Unmute" else "Mute"
        muteBtn.backgroundTintList =
            ColorStateList.valueOf(getColor(if (MicState.muted) R.color.red else R.color.button))
        refreshPcList()
    }

    private fun refreshPcList() {
        pcList.removeAllViews()
        val found = discovery.list()
        emptyText.visibility = if (found.isEmpty()) View.VISIBLE else View.GONE
        for (pc in found) {
            val b = Button(this).apply {
                text = "${pc.name}\n${pc.host}"
                setAllCaps(false)
                setOnClickListener { connectTo(pc.host, pc.port, pc.name) }
            }
            pcList.addView(b, LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT
            ))
        }
    }

    private fun toast(msg: String) = Toast.makeText(this, msg, Toast.LENGTH_LONG).show()
}
