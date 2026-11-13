package io.github.agreshagrawal.eyebreak

import android.annotation.SuppressLint
import android.content.Intent
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.PixelFormat
import android.graphics.RectF
import android.graphics.Typeface
import android.os.Build
import android.os.SystemClock
import android.provider.Settings
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import android.view.WindowManager
import kotlin.math.hypot
import kotlin.math.roundToInt
import kotlin.math.sin

/**
 * The floating badge: a small ring with the minutes left, on top of every
 * app (games too). Tap to open it into a pill with details, tap again to
 * open EyeBreak. Drag it anywhere; it snaps to the nearest side.
 */
class Badge(private val svc: EyeBreakService) {
    private val wm = svc.getSystemService(WindowManager::class.java)
    val size = svc.dp(40f).roundToInt()
    val pad = svc.dp(8f).roundToInt()
    private val openW = svc.dp(158f).roundToInt()
    private val margin = svc.dp(6f).roundToInt()
    private val view = BadgeView()
    private val lp = WindowManager.LayoutParams(
        1, 1,
        WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
        WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN,
        PixelFormat.TRANSLUCENT
    ).apply {
        gravity = Gravity.TOP or Gravity.START
        if (Build.VERSION.SDK_INT >= 28) layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
    }
    private var attached = false
    private var x = svc.prefs.badgeX
    private var y = svc.prefs.badgeY
    private var expand = 0f
    private var openUntil = 0L
    private var animating = false
    private var lastFrame = 0L

    init {
        if (x < 0 || y < 0) resetPosition() else clamp(false)
    }

    private fun screenW(): Int =
        if (Build.VERSION.SDK_INT >= 30) wm.currentWindowMetrics.bounds.width() else svc.resources.displayMetrics.widthPixels

    private fun screenH(): Int =
        if (Build.VERSION.SDK_INT >= 30) wm.currentWindowMetrics.bounds.height() else svc.resources.displayMetrics.heightPixels

    private fun onRight() = x + size / 2 > screenW() / 2

    private fun resetPosition() {
        x = screenW() - size - margin
        y = (screenH() * 0.38f).toInt()
    }

    private fun clamp(snap: Boolean) {
        val w = screenW()
        val h = screenH()
        if (snap) x = if (x + size / 2 > w / 2) w - size - margin else margin
        x = x.coerceIn(margin, (w - size - margin).coerceAtLeast(margin))
        y = y.coerceIn(svc.dp(24f).toInt(), (h - size - svc.dp(48f).toInt()).coerceAtLeast(0))
    }

    private fun wanted(): Boolean =
        svc.prefs.badgeOn && Settings.canDrawOverlays(svc) &&
            svc.phase != EyeBreakService.Phase.BREAK && !(svc.hold && svc.holdPresenting)

    /** Called every tick and whenever state changes. */
    fun sync() {
        val want = wanted()
        if (want && !attached) {
            layout()
            try {
                wm.addView(view, lp)
                attached = true
            } catch (_: Exception) {
            }
        } else if (!want && attached) {
            remove()
        }
        if (attached) {
            kick()
            view.invalidate()
        }
    }

    fun remove() {
        if (!attached) return
        try {
            wm.removeViewImmediate(view)
        } catch (_: Exception) {
        }
        attached = false
    }

    fun onScreenChanged() {
        clamp(true)
        layout()
    }

    private fun target(): Float {
        val open = SystemClock.elapsedRealtime() < openUntil || svc.phase == EyeBreakService.Phase.WARN
        return if (open && !view.dragging) 1f else 0f
    }

    private fun kick() {
        if (animating || !attached) return
        animating = true
        lastFrame = SystemClock.uptimeMillis()
        view.postOnAnimation(frame)
    }

    private val frame = object : Runnable {
        override fun run() {
            if (!attached) {
                animating = false
                return
            }
            val t = SystemClock.uptimeMillis()
            val dt = (t - lastFrame).coerceIn(0, 50)
            lastFrame = t
            val tg = target()
            expand = if (expand < tg) minOf(tg, expand + dt / 180f) else maxOf(tg, expand - dt / 180f)
            layout()
            view.invalidate()
            when {
                expand != tg -> view.postOnAnimation(this)
                svc.pillState().pulse -> view.postDelayed(this, 50)
                else -> animating = false
            }
        }
    }

    private fun bodyW(): Int = size + ((openW - size) * ease(expand)).roundToInt()

    private fun layout() {
        val bw = bodyW()
        lp.x = (if (onRight()) x + size - bw else x) - pad
        lp.y = y - pad
        lp.width = bw + 2 * pad
        lp.height = size + 2 * pad
        if (attached) {
            try {
                wm.updateViewLayout(view, lp)
            } catch (_: Exception) {
            }
        }
    }

    private fun openApp() {
        val i = Intent(svc, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        try {
            svc.startActivity(i)
        } catch (_: Exception) {
        }
    }

    @SuppressLint("ViewConstructor")
    inner class BadgeView : View(svc) {
        var dragging = false
        private val slop = ViewConfiguration.get(svc).scaledTouchSlop
        private var downX = 0f
        private var downY = 0f
        private var startX = 0
        private var startY = 0
        private val fill = Paint(Paint.ANTI_ALIAS_FLAG)
        private val line = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            style = Paint.Style.STROKE
            strokeCap = Paint.Cap.ROUND
        }
        private val text = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
        }
        private val small = Paint(Paint.ANTI_ALIAS_FLAG)
        private val rect = RectF()

        init {
            setLayerType(LAYER_TYPE_SOFTWARE, null) // for the soft shadow
        }

        override fun onDraw(c: Canvas) {
            val st = svc.pillState()
            val p = pad.toFloat()
            val b = size.toFloat()
            val bw = bodyW().toFloat()
            val r = b / 2f
            val right = onRight()
            val k = if (st.pulse) 0.5f + 0.5f * (sin(SystemClock.uptimeMillis() / 1000f * Math.PI.toFloat() * 1.4f) * 0.5f + 0.5f) else 1f

            rect.set(p, p, p + bw, p + b)
            fill.style = Paint.Style.FILL
            fill.color = Col.BODY
            fill.setShadowLayer(svc.dp(5f), 0f, svc.dp(1.5f), 0x66000000)
            c.drawRoundRect(rect, r, r, fill)
            fill.clearShadowLayer()
            line.strokeWidth = 1f
            line.color = if (expand > 0.05f) fade(st.color, 0.45f) else Col.EDGE
            rect.inset(0.5f, 0.5f)
            c.drawRoundRect(rect, r, r, line)

            // ring + what's inside it
            val cx = if (right) p + bw - r else p + r
            val cy = p + r
            val rr = r - svc.dp(5.5f)
            line.strokeWidth = svc.dp(2.6f)
            line.color = Col.TRACK
            c.drawCircle(cx, cy, rr, line)
            line.color = fade(st.color, k)
            rect.set(cx - rr, cy - rr, cx + rr, cy + rr)
            c.drawArc(rect, -90f, 360f * st.frac.coerceIn(0f, 1f), false, line)
            if (st.num.isEmpty()) {
                // on hold: pause bars
                val bw2 = svc.dp(2.6f)
                val bh2 = svc.dp(10f)
                fill.color = fade(st.color, k)
                for (dx in floatArrayOf(-svc.dp(3f), svc.dp(3f))) {
                    rect.set(cx + dx - bw2 / 2, cy - bh2 / 2, cx + dx + bw2 / 2, cy + bh2 / 2)
                    c.drawRoundRect(rect, bw2 / 2, bw2 / 2, fill)
                }
            } else {
                text.textSize = svc.dp(13f)
                text.textAlign = Paint.Align.CENTER
                text.color = if (svc.phase == EyeBreakService.Phase.WARN) Col.TEAL_SOFT else Col.TEXT
                c.drawText(st.num, cx, cy - (text.descent() + text.ascent()) / 2f, text)
            }

            // details fade in as it opens
            val t = ((expand - 0.45f) / 0.55f).coerceIn(0f, 1f)
            if (t > 0f) {
                val tx = if (right) p + svc.dp(14f) else p + b + svc.dp(4f)
                val tw = bw - b - svc.dp(18f)
                text.textAlign = Paint.Align.LEFT
                text.textSize = svc.dp(14f)
                text.color = fade(Col.TEXT, t)
                c.drawText(ellipsize(st.big, text, tw), tx, p + svc.dp(18f), text)
                small.textSize = svc.dp(11f)
                small.color = fade(if (svc.hold) Col.AMBER else Col.SUB, t)
                c.drawText(ellipsize(st.small, small, tw), tx, p + svc.dp(32f), small)
            }
        }

        private fun ellipsize(s: String, p: Paint, w: Float): String {
            if (p.measureText(s) <= w) return s
            var n = s.length
            while (n > 1 && p.measureText(s, 0, n) + p.measureText("…") > w) n--
            return s.substring(0, n) + "…"
        }

        @SuppressLint("ClickableViewAccessibility")
        override fun onTouchEvent(e: MotionEvent): Boolean {
            when (e.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    downX = e.rawX
                    downY = e.rawY
                    startX = x
                    startY = y
                    dragging = false
                }
                MotionEvent.ACTION_MOVE -> {
                    val dx = e.rawX - downX
                    val dy = e.rawY - downY
                    if (!dragging && hypot(dx, dy) > slop) {
                        dragging = true
                        expand = 0f
                        openUntil = 0L
                    }
                    if (dragging) {
                        x = startX + dx.toInt()
                        y = startY + dy.toInt()
                        layout()
                        invalidate()
                    }
                }
                MotionEvent.ACTION_UP -> {
                    if (dragging) {
                        dragging = false
                        clamp(true)
                        svc.prefs.badgeX = x
                        svc.prefs.badgeY = y
                        layout()
                        kick()
                    } else if (expand > 0.6f) {
                        openApp()
                    } else {
                        openUntil = SystemClock.elapsedRealtime() + 4000
                        kick()
                    }
                }
                MotionEvent.ACTION_CANCEL -> {
                    if (dragging) {
                        dragging = false
                        clamp(true)
                        layout()
                    }
                }
            }
            return true
        }
    }
}
