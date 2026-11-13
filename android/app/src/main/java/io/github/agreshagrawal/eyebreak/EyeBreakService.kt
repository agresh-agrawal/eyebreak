package io.github.agreshagrawal.eyebreak

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.content.res.Configuration
import android.graphics.drawable.Icon
import android.media.AudioManager
import android.media.ToneGenerator
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import android.os.VibrationEffect
import android.os.Vibrator
import android.provider.Settings

/**
 * The 20-minute timer. Every 20 minutes (19 min + a 1-minute heads-up) the
 * screen fades to a guided 1-minute eye break. The break waits while
 * something important is on (call, camera, mic, casting) and starts shortly
 * after it ends. Everything else on the phone keeps running: the break is
 * only a window on top.
 */
class EyeBreakService : Service() {
    companion object {
        const val ACTION_BREAK_NOW = "io.github.agreshagrawal.eyebreak.BREAK_NOW"
        const val ACTION_RESTART = "io.github.agreshagrawal.eyebreak.RESTART"
        const val ACTION_REFRESH = "io.github.agreshagrawal.eyebreak.REFRESH"
        private const val CH_STATUS = "status"
        private const val CH_ALERT = "alerts"
        private const val NOTE_ID = 1
        private const val ALERT_ID = 2

        var instance: EyeBreakService? = null
            private set

        fun start(ctx: Context, action: String? = null) {
            val i = Intent(ctx, EyeBreakService::class.java)
            if (action != null) i.action = action
            try {
                ctx.startForegroundService(i)
            } catch (_: Exception) {
                // background-start restriction: the boot receiver / app launch will start it
            }
        }
    }

    enum class Phase { WORK, WARN, BREAK }

    data class PillState(val big: String, val small: String, val color: Int, val frac: Float, val pulse: Boolean, val num: String)

    lateinit var prefs: Prefs
        private set
    private val handler = Handler(Looper.getMainLooper())
    private lateinit var detector: Detector
    private lateinit var badge: Badge
    private var breakWin: BreakWindow? = null

    var phase = Phase.WORK
        private set
    var phaseEnd = 0L
        private set
    var cycleStart = 0L
        private set
    var breakStarted = 0L
        private set
    var hold = false
        private set
    var holdReason = ""
        private set
    var holdShort = ""
        private set
    var holdPresenting = false
        private set
    var clearSince = -1L
        private set
    private var afterHold = false
    private var lastCheck = 0L
    private var screenOffAt = -1L
    private var stage = 0
    private var skipped = false
    private var lastNote = ""

    val workMs: Long get() = if (prefs.testMode) 20_000L else 19 * 60_000L
    val warnMs: Long get() = if (prefs.testMode) 15_000L else 60_000L
    val breakMs: Long get() = if (prefs.testMode) 15_000L else 60_000L
    private val graceMs: Long get() = if (prefs.testMode) 4_000L else 10_000L

    fun now(): Long = SystemClock.elapsedRealtime()
    fun secsLeft(end: Long): Long = ((end - now()).coerceAtLeast(0) + 999) / 1000

    private val tickR = object : Runnable {
        override fun run() {
            tick()
            handler.postDelayed(this, if (phase == Phase.BREAK) 250L else 1000L)
        }
    }

    private val screenRx = object : BroadcastReceiver() {
        override fun onReceive(c: Context, i: Intent) {
            when (i.action) {
                Intent.ACTION_SCREEN_OFF -> {
                    screenOffAt = now()
                    handler.removeCallbacks(tickR)
                    if (phase == Phase.BREAK) finishBreak(immediate = true)
                }
                Intent.ACTION_SCREEN_ON -> {
                    val off = screenOffAt
                    screenOffAt = -1
                    // Screen off for a minute or more = your eyes rested.
                    if (off >= 0 && now() - off >= 60_000L && phase != Phase.BREAK) startWork()
                    handler.removeCallbacks(tickR)
                    handler.post(tickR)
                }
            }
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        instance = this
        prefs = Prefs(this)
        createChannels()
        goForeground()
        detector = Detector(this)
        detector.start(handler)
        badge = Badge(this)
        val f = IntentFilter().apply {
            addAction(Intent.ACTION_SCREEN_OFF)
            addAction(Intent.ACTION_SCREEN_ON)
        }
        if (Build.VERSION.SDK_INT >= 33) registerReceiver(screenRx, f, Context.RECEIVER_NOT_EXPORTED) else registerReceiver(screenRx, f)
        startWork()
        handler.post(tickR)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        goForeground()
        when (intent?.action) {
            ACTION_BREAK_NOW -> breakNow()
            ACTION_RESTART -> startWork()
            else -> badge.sync()
        }
        return START_STICKY
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        badge.onScreenChanged()
    }

    override fun onDestroy() {
        handler.removeCallbacksAndMessages(null)
        try {
            unregisterReceiver(screenRx)
        } catch (_: Exception) {
        }
        detector.stop()
        badge.remove()
        breakWin?.remove()
        breakWin = null
        instance = null
        super.onDestroy()
    }

    // ---------- phase machine ----------
    fun startWork() {
        phase = Phase.WORK
        phaseEnd = now() + workMs
        cycleStart = now()
        hold = false
        afterHold = false
        clearSince = -1
        refresh()
    }

    private fun startWarn() {
        phase = Phase.WARN
        phaseEnd = now() + warnMs
        afterHold = hold
        hold = false
        clearSince = -1
        buzz(40)
        refresh()
    }

    private fun startHold(b: Busy) {
        phase = Phase.WORK
        phaseEnd = now()
        hold = true
        holdReason = b.reason
        holdShort = b.short
        holdPresenting = b.presenting
        clearSince = -1
        lastCheck = now()
        refresh()
    }

    fun breakNow() {
        if (phase != Phase.BREAK) startBreak()
    }

    private fun startBreak() {
        val pm = getSystemService(PowerManager::class.java)
        if (pm != null && !pm.isInteractive) {
            startWork() // screen is off: eyes are already resting
            return
        }
        if (!Settings.canDrawOverlays(this)) {
            // Can't draw the break screen: fall back to a reminder.
            alert("Time for an eye break", "Look at something far away for a minute.")
            startWork()
            return
        }
        phase = Phase.BREAK
        phaseEnd = now() + breakMs
        breakStarted = now()
        stage = 0
        skipped = false
        hold = false
        badge.sync()
        breakWin = BreakWindow(this).also { it.show() }
        refresh()
    }

    /** Touch-and-hold for 3 s on the break screen. */
    fun skipBreak() {
        if (phase != Phase.BREAK) return
        skipped = true
        finishBreak()
    }

    private fun finishBreak(immediate: Boolean = false) {
        if (phase != Phase.BREAK) return
        if (!skipped) prefs.addBreak()
        if (immediate) breakWin?.remove() else breakWin?.dismiss()
        breakWin = null
        startWork()
    }

    private fun tick() {
        val now = now()
        when (phase) {
            Phase.WORK -> if (now >= phaseEnd) {
                if (hold) {
                    if (now - lastCheck >= 3000) {
                        lastCheck = now
                        val b = detector.detect()
                        if (b != null) {
                            holdReason = b.reason
                            holdShort = b.short
                            holdPresenting = b.presenting
                            clearSince = -1
                        } else {
                            if (clearSince < 0) clearSince = now
                            if (now - clearSince >= graceMs) startWarn()
                        }
                    }
                } else {
                    lastCheck = now
                    val b = detector.detect()
                    if (b != null) startHold(b) else startWarn()
                }
            }
            Phase.WARN -> if (now >= phaseEnd) {
                startBreak()
            } else if (now - lastCheck >= 3000) {
                lastCheck = now
                detector.detect()?.let { startHold(it) }
            }
            Phase.BREAK -> {
                if (now >= phaseEnd) {
                    finishBreak()
                } else {
                    val s = (((now - breakStarted) * 3) / breakMs.coerceAtLeast(1)).toInt().coerceAtMost(2)
                    if (s != stage) {
                        stage = s
                        if (s == 2) chime() // "open your eyes"
                    }
                }
            }
        }
        refresh()
    }

    private fun refresh() {
        badge.sync()
        val text = statusLine()
        if (text != lastNote) {
            lastNote = text
            getSystemService(NotificationManager::class.java)?.notify(NOTE_ID, note(text))
        }
    }

    // ---------- state for the UI ----------
    fun statusLine(): String = when {
        phase == Phase.BREAK -> "Eye break in progress"
        phase == Phase.WARN -> "Eye break in under a minute"
        hold && clearSince >= 0 -> "Looks finished: eye break in a moment"
        hold -> "On hold: $holdReason"
        else -> "Next eye break in ${(secsLeft(phaseEnd + warnMs) + 59) / 60} min"
    }

    fun pillState(): PillState {
        val now = now()
        return when {
            phase == Phase.WARN -> PillState(
                mmss(secsLeft(phaseEnd)), if (afterHold) "call ended" else "eye break soon", Col.TEAL,
                1f - (phaseEnd - now).toFloat() / warnMs, true, secsLeft(phaseEnd).toString()
            )
            phase == Phase.BREAK -> PillState("Break", "", Col.TEAL, 1f, false, "")
            hold && clearSince >= 0 -> PillState("Wrapping up", "break in a moment", Col.AMBER, 1f, true, "")
            hold -> PillState("On hold", holdShort, Col.AMBER, 1f, false, "")
            else -> {
                val left = secsLeft(phaseEnd + warnMs)
                val big = if (left > 60) "${(left + 59) / 60} min" else mmss(left)
                val num = if (left > 60) ((left + 59) / 60).toString() else left.toString()
                PillState(big, "until eye break", Col.TEAL, (now - cycleStart).toFloat() / (workMs + warnMs), false, num)
            }
        }
    }

    // ---------- notifications ----------
    private fun createChannels() {
        val nm = getSystemService(NotificationManager::class.java) ?: return
        nm.createNotificationChannel(
            NotificationChannel(CH_STATUS, "Timer", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Shows when your next eye break is"
                setShowBadge(false)
            }
        )
        nm.createNotificationChannel(
            NotificationChannel(CH_ALERT, "Break reminders", NotificationManager.IMPORTANCE_DEFAULT).apply {
                description = "Used when EyeBreak isn't allowed to show the break screen"
            }
        )
    }

    private fun pending(action: String, code: Int): PendingIntent =
        PendingIntent.getBroadcast(
            this, code, Intent(this, ActionReceiver::class.java).setAction(action),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )

    private fun note(text: String): Notification {
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )
        val icon = Icon.createWithResource(this, R.drawable.ic_stat)
        return Notification.Builder(this, CH_STATUS)
            .setSmallIcon(R.drawable.ic_stat)
            .setContentTitle("EyeBreak")
            .setContentText(text)
            .setColor(Col.TEAL)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(icon, "Break now", pending(ACTION_BREAK_NOW, 1)).build())
            .addAction(Notification.Action.Builder(icon, "Restart timer", pending(ACTION_RESTART, 2)).build())
            .build()
    }

    private fun goForeground() {
        val n = note(if (::prefs.isInitialized && cycleStart > 0) statusLine() else "Next eye break in 20 min")
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(NOTE_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else {
            startForeground(NOTE_ID, n)
        }
    }

    private fun alert(title: String, text: String) {
        val nm = getSystemService(NotificationManager::class.java) ?: return
        nm.notify(
            ALERT_ID,
            Notification.Builder(this, CH_ALERT)
                .setSmallIcon(R.drawable.ic_stat)
                .setContentTitle(title)
                .setContentText(text)
                .setColor(Col.TEAL)
                .setAutoCancel(true)
                .setTimeoutAfter(90_000)
                .build()
        )
    }

    // ---------- gentle feedback ----------
    private fun buzz(ms: Long) {
        try {
            @Suppress("DEPRECATION")
            val v = getSystemService(Vibrator::class.java)
            v?.vibrate(VibrationEffect.createOneShot(ms, VibrationEffect.DEFAULT_AMPLITUDE))
        } catch (_: Exception) {
        }
    }

    private fun chime() {
        buzz(60)
        try {
            val t = ToneGenerator(AudioManager.STREAM_NOTIFICATION, 60)
            t.startTone(ToneGenerator.TONE_PROP_ACK, 180)
            handler.postDelayed({ t.release() }, 600)
        } catch (_: Exception) {
        }
    }
}

/** Notification buttons. */
class ActionReceiver : BroadcastReceiver() {
    override fun onReceive(c: Context, i: Intent) {
        val s = EyeBreakService.instance
        if (s == null) {
            EyeBreakService.start(c, i.action)
            return
        }
        when (i.action) {
            EyeBreakService.ACTION_BREAK_NOW -> s.breakNow()
            EyeBreakService.ACTION_RESTART -> s.startWork()
        }
    }
}

/** Start again after a reboot or an app update. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(c: Context, i: Intent) {
        if (Prefs(c).enabled) EyeBreakService.start(c)
    }
}
