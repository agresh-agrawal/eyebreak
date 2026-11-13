package io.github.agreshagrawal.eyebreak

import android.annotation.SuppressLint
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.PixelFormat
import android.graphics.RectF
import android.graphics.Typeface
import android.os.Build
import android.os.SystemClock
import android.text.Layout
import android.text.StaticLayout
import android.text.TextPaint
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import kotlin.math.min

private val STAGES = arrayOf(
    "Look far away" to "Find something at least 6 m (20 ft) away and let your focus go soft.",
    "Close your eyes" to "Breathe slowly with the circle. Relax your forehead, jaw and shoulders.",
    "Blink and roll" to "Blink slowly 10 times, then roll your eyes gently: up, right, down, left.",
)

/**
 * The 1-minute break: a black window on top of everything. Nothing
 * underneath is paused. Touch and hold anywhere for 3 s to skip.
 */
class BreakWindow(private val svc: EyeBreakService) {
    private val wm = svc.getSystemService(WindowManager::class.java)
    private val view = BreakView()
    private var shown = false

    @Suppress("DEPRECATION")
    fun show() {
        val lp = WindowManager.LayoutParams(
            WindowManager.LayoutParams.MATCH_PARENT,
            WindowManager.LayoutParams.MATCH_PARENT,
            WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
            WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN or
                WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS or
                WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON or
                WindowManager.LayoutParams.FLAG_FULLSCREEN,
            PixelFormat.OPAQUE
        )
        if (Build.VERSION.SDK_INT >= 28) lp.layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
        view.alpha = 0f
        try {
            wm.addView(view, lp)
            shown = true
            view.animate().alpha(1f).setDuration(700).start()
        } catch (_: Exception) {
        }
    }

    fun dismiss() {
        if (!shown) return
        view.animate().alpha(0f).setDuration(450).withEndAction { remove() }.start()
    }

    fun remove() {
        if (!shown) return
        shown = false
        try {
            wm.removeViewImmediate(view)
        } catch (_: Exception) {
        }
    }

    @SuppressLint("ViewConstructor")
    inner class BreakView : View(svc) {
        private var holdStart = -1L
        private val fill = Paint(Paint.ANTI_ALIAS_FLAG)
        private val line = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            style = Paint.Style.STROKE
            strokeCap = Paint.Cap.ROUND
        }
        private val light = Typeface.create("sans-serif-light", Typeface.NORMAL)
        private val medium = Typeface.create("sans-serif-medium", Typeface.NORMAL)
        private val text = TextPaint(Paint.ANTI_ALIAS_FLAG).apply { textAlign = Paint.Align.CENTER }
        private val body = TextPaint(Paint.ANTI_ALIAS_FLAG)
        private val rect = RectF()
        private var subLayout: StaticLayout? = null
        private var subKey = ""

        init {
            isFocusable = true
            isFocusableInTouchMode = true
            keepScreenOn = true
        }

        @Suppress("DEPRECATION")
        override fun onAttachedToWindow() {
            super.onAttachedToWindow()
            requestFocus()
            if (Build.VERSION.SDK_INT >= 30) {
                windowInsetsController?.let {
                    it.hide(WindowInsets.Type.systemBars())
                    it.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
                }
            } else {
                systemUiVisibility = SYSTEM_UI_FLAG_FULLSCREEN or SYSTEM_UI_FLAG_HIDE_NAVIGATION or SYSTEM_UI_FLAG_IMMERSIVE_STICKY
            }
        }

        override fun dispatchKeyEvent(event: KeyEvent): Boolean {
            // Back does nothing during the break (Home still works: Android never lets apps block it).
            if (event.keyCode == KeyEvent.KEYCODE_BACK) return true
            return super.dispatchKeyEvent(event)
        }

        @SuppressLint("ClickableViewAccessibility")
        override fun onTouchEvent(e: MotionEvent): Boolean {
            when (e.actionMasked) {
                MotionEvent.ACTION_DOWN -> holdStart = SystemClock.elapsedRealtime()
                MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> holdStart = -1
            }
            return true
        }

        override fun onDraw(c: Canvas) {
            val w = width.toFloat()
            val h = height.toFloat()
            val now = svc.now()
            c.drawColor(0xFF000000.toInt())

            if (holdStart > 0 && now - holdStart >= 3000) {
                holdStart = -1
                svc.skipBreak()
            }

            val u = min(w, h * 0.62f)
            val total = svc.breakMs.coerceAtLeast(1).toFloat()
            val leftMs = (svc.phaseEnd - now).coerceAtLeast(0).toFloat()
            val frac = ((total - leftMs) / total).coerceIn(0f, 1f)
            val cx = w / 2f
            val cy = h * 0.36f

            // breathing circle: 4 s in, 6 s out
            val t = ((now - svc.breakStarted).coerceAtLeast(0) % 10_000) / 1000f
            val inhale = t < 4f
            val b = if (inhale) ease(t / 4f) else 1f - ease((t - 4f) / 6f)
            val br = u * (0.13f + 0.065f * b)
            fill.style = Paint.Style.FILL
            for (i in 1..8) {
                fill.color = 0x052EC4B0
                c.drawCircle(cx, cy, br + i * u * 0.012f, fill)
            }
            fill.color = 0x282EC4B0
            c.drawCircle(cx, cy, br, fill)
            line.strokeWidth = svc.dp(1f)
            line.color = 0x552EC4B0
            c.drawCircle(cx, cy, br, line)

            // time-left ring
            val rr = u * 0.29f
            line.strokeWidth = svc.dp(3f)
            line.color = 0xFF1A1C21.toInt()
            c.drawCircle(cx, cy, rr, line)
            line.color = 0xD22EC4B0.toInt()
            rect.set(cx - rr, cy - rr, cx + rr, cy + rr)
            c.drawArc(rect, -90f, 360f * (1f - frac), false, line)

            text.typeface = medium
            text.textSize = svc.dp(13f)
            text.letterSpacing = 0.12f
            text.color = fade(Col.TEAL, 0.85f)
            c.drawText("EYE BREAK", cx, cy - rr - svc.dp(28f), text)
            text.letterSpacing = 0f

            text.typeface = light
            text.textSize = u * 0.15f
            text.color = 0xFFDCE0E4.toInt()
            c.drawText(mmss(svc.secsLeft(svc.phaseEnd)), cx, cy + text.textSize * 0.32f, text)
            text.typeface = Typeface.DEFAULT
            text.textSize = svc.dp(13f)
            text.color = fade(Col.TEAL_SOFT, 0.7f)
            c.drawText(if (inhale) "breathe in" else "breathe out", cx, cy + u * 0.12f, text)

            val stage = (frac * 3f).toInt().coerceAtMost(2)
            val (title, sub) = STAGES[stage]
            val ty = cy + rr + svc.dp(56f)
            text.typeface = medium
            text.textSize = svc.dp(24f)
            text.color = 0xFFE2E5E9.toInt()
            c.drawText(title, cx, ty, text)

            val sw = (w * 0.82f).toInt()
            if (subKey != sub + sw) {
                subKey = sub + sw
                body.textSize = svc.dp(15.5f)
                body.color = 0xFF9298A2.toInt()
                subLayout = StaticLayout.Builder.obtain(sub, 0, sub.length, body, sw)
                    .setAlignment(Layout.Alignment.ALIGN_CENTER)
                    .setLineSpacing(0f, 1.2f)
                    .build()
            }
            subLayout?.let {
                c.save()
                c.translate(cx - sw / 2f, ty + svc.dp(16f))
                it.draw(c)
                c.restore()
                val dy = ty + svc.dp(16f) + it.height + svc.dp(26f)
                for (i in 0..2) {
                    fill.color = if (i <= stage) Col.TEAL else 0xFF32363C.toInt()
                    c.drawCircle(cx + (i - 1) * svc.dp(16f), dy, svc.dp(if (i == stage) 3.5f else 2.5f), fill)
                }
            }

            text.typeface = Typeface.DEFAULT
            text.textSize = svc.dp(12.5f)
            if (holdStart > 0) {
                val left = (3000 - (now - holdStart)).coerceAtLeast(0)
                text.color = Col.AMBER
                c.drawText("Keep holding to skip… ${(left + 999) / 1000}", cx, h - svc.dp(64f), text)
            } else {
                text.color = 0xFF5A5F69.toInt()
                c.drawText("Touch and hold for 3 s to skip", cx, h - svc.dp(72f), text)
                c.drawText("Your apps and downloads keep running", cx, h - svc.dp(52f), text)
            }
            postInvalidateOnAnimation()
        }
    }
}
