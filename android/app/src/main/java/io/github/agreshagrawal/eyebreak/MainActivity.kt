package io.github.agreshagrawal.eyebreak

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.ColorStateList
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.RippleDrawable
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.provider.Settings
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.Switch
import android.widget.TextView
import android.widget.Toast
import kotlin.math.roundToInt

/** Home screen: status, quick setup, and the few controls you need. */
class MainActivity : Activity() {
    private lateinit var prefs: Prefs
    private val handler = Handler(Looper.getMainLooper())
    private lateinit var ring: RingView
    private lateinit var status: TextView
    private lateinit var chip: TextView
    private lateinit var setup: LinearLayout
    private lateinit var actions: LinearLayout
    private lateinit var runSwitch: Switch
    private lateinit var badgeSwitch: Switch
    private var setupKey = ""

    private val loop = object : Runnable {
        override fun run() {
            refresh()
            handler.postDelayed(this, 1000)
        }
    }

    private fun px(v: Float) = dp(v).roundToInt()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        prefs = Prefs(this)

        val col = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(px(20f), px(12f), px(20f), px(28f))
        }
        val scroll = ScrollView(this).apply {
            isFillViewport = true
            addView(col)
        }
        scroll.setOnApplyWindowInsetsListener { v, insets ->
            @Suppress("DEPRECATION")
            v.setPadding(insets.systemWindowInsetLeft, insets.systemWindowInsetTop, insets.systemWindowInsetRight, insets.systemWindowInsetBottom)
            insets
        }
        setContentView(scroll)

        // header
        val header = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(0, px(8f), 0, px(8f))
        }
        header.addView(ImageView(this).apply { setImageResource(R.mipmap.ic_launcher) }, LinearLayout.LayoutParams(px(34f), px(34f)))
        val title = label("EyeBreak", 20f, Col.TEXT, bold = true).apply { setPadding(px(10f), 0, 0, 0) }
        header.addView(title, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
        header.addView(label("v3.1", 12f, Col.DIM))
        // hidden: long-press the title for a fast test cycle (20 s / 15 s)
        title.setOnLongClickListener {
            prefs.testMode = !prefs.testMode
            if (prefs.enabled) EyeBreakService.start(this, EyeBreakService.ACTION_RESTART)
            Toast.makeText(this, if (prefs.testMode) "Test mode: 20 s work, 15 s break" else "Normal mode: 20 minutes", Toast.LENGTH_SHORT).show()
            true
        }
        col.addView(header)

        ring = RingView(this)
        col.addView(ring, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, px(250f)).apply { topMargin = px(12f) })
        status = label("", 15f, Col.SUB).apply { gravity = Gravity.CENTER }
        col.addView(status, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { topMargin = px(10f) })
        chip = label("", 12.5f, Col.SUB).apply {
            background = rounded(0x14FFFFFF, px(14f).toFloat())
            setPadding(px(14f), px(5f), px(14f), px(5f))
        }
        col.addView(FrameLayout(this).apply {
            addView(chip, FrameLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.CENTER))
        }, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { topMargin = px(10f) })

        setup = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        col.addView(setup, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { topMargin = px(20f) })

        actions = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        actions.addView(button("Take an eye break now", primary = true) {
            EyeBreakService.start(this, EyeBreakService.ACTION_BREAK_NOW)
        })
        actions.addView(button("Restart the 20-minute timer", primary = false) {
            EyeBreakService.start(this, EyeBreakService.ACTION_RESTART)
            Toast.makeText(this, "Timer restarted", Toast.LENGTH_SHORT).show()
        }, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, px(50f)).apply { topMargin = px(10f) })
        col.addView(actions, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { topMargin = px(8f) })

        val toggles = card()
        runSwitch = toggle(toggles, "EyeBreak is on", "Every 20 minutes, a guided 1-minute break") { on ->
            prefs.enabled = on
            if (on) EyeBreakService.start(this) else stopService(Intent(this, EyeBreakService::class.java))
            refresh()
        }
        toggles.addView(View(this).apply { setBackgroundColor(0x12FFFFFF) }, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 1))
        badgeSwitch = toggle(toggles, "Floating badge", "Minutes left, on top of every app. Drag to move.") { on ->
            prefs.badgeOn = on
            if (prefs.enabled) EyeBreakService.start(this, EyeBreakService.ACTION_REFRESH)
        }
        col.addView(toggles, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { topMargin = px(20f) })

        col.addView(
            label(
                "Breaks wait while you're on a call, the camera or mic is in use, or you're casting your screen. " +
                    "Screen off for a minute counts as a rest. To skip a break, touch and hold for 3 seconds.",
                13f, Col.DIM
            ).apply { setLineSpacing(0f, 1.25f) },
            LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { topMargin = px(18f) }
        )
    }

    override fun onResume() {
        super.onResume()
        if (prefs.enabled) EyeBreakService.start(this, EyeBreakService.ACTION_REFRESH)
        handler.post(loop)
    }

    override fun onPause() {
        handler.removeCallbacks(loop)
        super.onPause()
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        setupKey = ""
        refresh()
    }

    private fun refresh() {
        val svc = EyeBreakService.instance
        runSwitch.isChecked = prefs.enabled
        badgeSwitch.isChecked = prefs.badgeOn
        actions.visibility = if (svc != null) View.VISIBLE else View.GONE
        status.text = when {
            !prefs.enabled -> "EyeBreak is off"
            svc == null -> "Starting…"
            else -> svc.statusLine()
        }
        chip.text = when (val n = prefs.breaksToday()) {
            0 -> "No eye breaks yet today"
            1 -> "1 eye break today"
            else -> "$n eye breaks today"
        }
        ring.invalidate()
        buildSetup()
    }

    // ---------- setup checklist (only what's missing) ----------
    private fun buildSetup() {
        val overlay = Settings.canDrawOverlays(this)
        val notes = Build.VERSION.SDK_INT < 33 || checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        val battery = getSystemService(PowerManager::class.java)?.isIgnoringBatteryOptimizations(packageName) ?: true
        val key = "$overlay$notes$battery"
        if (key == setupKey) return
        setupKey = key
        setup.removeAllViews()
        if (!overlay) {
            setup.addView(step("Allow \"Display over other apps\"", "Needed for the break screen and the floating badge.", "Allow") {
                startActivity(Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION, Uri.parse("package:$packageName")))
            })
        }
        if (!notes) {
            setup.addView(step("Allow notifications", "Shows when your next break is, with a Break now button.", "Allow") {
                requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
            })
        }
        if (!battery) {
            setup.addView(step("Keep the timer running", "Stops Android from pausing EyeBreak in the background.", "Allow") {
                @SuppressLint("BatteryLife")
                val i = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName"))
                try {
                    startActivity(i)
                } catch (_: Exception) {
                    startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
                }
            })
        }
    }

    // ---------- small view helpers ----------
    private fun label(s: String, sp: Float, color: Int, bold: Boolean = false) = TextView(this).apply {
        text = s
        textSize = sp
        setTextColor(color)
        if (bold) typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
    }

    private fun rounded(color: Int, r: Float, stroke: Int = 0) = GradientDrawable().apply {
        setColor(color)
        cornerRadius = r
        if (stroke != 0) setStroke(px(1f), stroke)
    }

    private fun button(s: String, primary: Boolean, onClick: () -> Unit): TextView = TextView(this).apply {
        text = s
        textSize = 15.5f
        gravity = Gravity.CENTER
        typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
        setTextColor(if (primary) 0xFF05201C.toInt() else Col.TEXT)
        val bg = if (primary) rounded(Col.TEAL, px(14f).toFloat()) else rounded(0x0AFFFFFF, px(14f).toFloat(), 0x22FFFFFF)
        background = RippleDrawable(ColorStateList.valueOf(0x33FFFFFF), bg, null)
        isClickable = true
        layoutParams = LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, px(52f))
        setOnClickListener { onClick() }
    }

    private fun card() = LinearLayout(this).apply {
        orientation = LinearLayout.VERTICAL
        background = rounded(Col.PANEL, px(18f).toFloat(), 0x14FFFFFF)
        setPadding(px(16f), px(4f), px(16f), px(4f))
    }

    private fun toggle(parent: LinearLayout, title: String, sub: String, onChange: (Boolean) -> Unit): Switch {
        val row = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(0, px(14f), 0, px(14f))
        }
        val texts = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        texts.addView(label(title, 15.5f, Col.TEXT))
        texts.addView(label(sub, 12.5f, Col.SUB).apply { setPadding(0, px(2f), 0, 0) })
        row.addView(texts, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
        val sw = Switch(this).apply {
            thumbTintList = ColorStateList(arrayOf(intArrayOf(android.R.attr.state_checked), intArrayOf()), intArrayOf(Col.TEAL, 0xFFB8BEC8.toInt()))
            trackTintList = ColorStateList(arrayOf(intArrayOf(android.R.attr.state_checked), intArrayOf()), intArrayOf(0x882EC4B0.toInt(), 0x44FFFFFF))
        }
        row.addView(sw)
        sw.setOnClickListener { onChange(sw.isChecked) }
        row.setOnClickListener {
            sw.isChecked = !sw.isChecked
            onChange(sw.isChecked)
        }
        parent.addView(row)
        return sw
    }

    private fun step(title: String, sub: String, cta: String, onClick: () -> Unit): View {
        val box = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            background = rounded(0x14F5B946, px(16f).toFloat(), 0x40F5B946)
            setPadding(px(16f), px(14f), px(12f), px(14f))
            layoutParams = LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { bottomMargin = px(10f) }
        }
        val texts = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        texts.addView(label(title, 15f, Col.TEXT, bold = true))
        texts.addView(label(sub, 12.5f, Col.SUB).apply { setPadding(0, px(2f), 0, 0) })
        box.addView(texts, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f).apply { marginEnd = px(10f) })
        box.addView(TextView(this).apply {
            text = cta
            textSize = 14f
            typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
            setTextColor(0xFF2A1C04.toInt())
            background = RippleDrawable(ColorStateList.valueOf(0x33000000), rounded(Col.AMBER, px(10f).toFloat()), null)
            setPadding(px(16f), px(9f), px(16f), px(9f))
            setOnClickListener { onClick() }
        })
        return box
    }

    /** Big progress ring with the minutes left. */
    class RingView(ctx: Context) : View(ctx) {
        private val line = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            style = Paint.Style.STROKE
            strokeCap = Paint.Cap.ROUND
        }
        private val fill = Paint(Paint.ANTI_ALIAS_FLAG)
        private val big = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            textAlign = Paint.Align.CENTER
            typeface = Typeface.create("sans-serif-light", Typeface.NORMAL)
        }
        private val small = Paint(Paint.ANTI_ALIAS_FLAG).apply { textAlign = Paint.Align.CENTER }
        private val rect = RectF()

        override fun onDraw(c: Canvas) {
            val svc = EyeBreakService.instance
            val cx = width / 2f
            val cy = height / 2f
            val r = minOf(width, height) / 2f - context.dp(14f)
            fill.color = 0x0D2EC4B0
            c.drawCircle(cx, cy, r + context.dp(10f), fill)
            line.strokeWidth = context.dp(8f)
            line.color = 0x14FFFFFF
            c.drawCircle(cx, cy, r, line)
            if (svc == null) {
                big.textSize = context.dp(40f)
                big.color = Col.DIM
                c.drawText("Off", cx, cy + big.textSize * 0.35f, big)
                return
            }
            val st = svc.pillState()
            line.color = st.color
            rect.set(cx - r, cy - r, cx + r, cy + r)
            c.drawArc(rect, -90f, 360f * st.frac.coerceIn(0f, 1f), false, line)
            val (main, unit) = when {
                svc.hold && svc.phase == EyeBreakService.Phase.WORK -> "II" to st.small
                svc.phase == EyeBreakService.Phase.WARN -> st.big to "until your break"
                svc.phase == EyeBreakService.Phase.BREAK -> "Now" to "eye break"
                st.big.endsWith(" min") -> st.big.removeSuffix(" min") to "minutes left"
                else -> st.big to "until your break"
            }
            big.textSize = context.dp(64f)
            big.color = if (svc.hold) Col.AMBER else Col.TEXT
            c.drawText(main, cx, cy + context.dp(14f), big)
            small.textSize = context.dp(14f)
            small.color = Col.SUB
            c.drawText(unit, cx, cy + context.dp(42f), small)
            if (st.pulse) postInvalidateDelayed(100)
        }
    }
}
