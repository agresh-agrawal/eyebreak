package io.github.agreshagrawal.eyebreak

import android.content.Context
import android.content.SharedPreferences
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import java.time.LocalDate

/** Same palette as the Windows app. */
object Col {
    const val TEAL = 0xFF2EC4B0.toInt()
    const val TEAL_SOFT = 0xFF8CE2D4.toInt()
    const val AMBER = 0xFFF5B946.toInt()
    const val TEXT = 0xFFECEFF3.toInt()
    const val SUB = 0xFF9AA1AD.toInt()
    const val DIM = 0xFF6B717C.toInt()
    const val BG = 0xFF0B0C0F.toInt()
    const val PANEL = 0xFF15171C.toInt()
    const val BODY = 0xF2151719.toInt()
    const val EDGE = 0x22FFFFFF
    const val TRACK = 0x1FFFFFFF
}

fun Context.dp(v: Float): Float = v * resources.displayMetrics.density

fun ease(t: Float): Float {
    val x = t.coerceIn(0f, 1f)
    return x * x * (3f - 2f * x)
}

fun mmss(secs: Long): String {
    val s = secs.coerceAtLeast(0)
    return "%d:%02d".format(s / 60, s % 60)
}

/** Multiply a colour's alpha by k (0..1). */
fun fade(c: Int, k: Float): Int {
    val a = ((c ushr 24) * k.coerceIn(0f, 1f)).toInt()
    return (a shl 24) or (c and 0x00FFFFFF)
}

/** Almond eye outline + pupil, `w` wide. */
fun drawEye(c: Canvas, cx: Float, cy: Float, w: Float, stroke: Float, color: Int, p: Paint) {
    val hw = w / 2f
    val k = w * 0.34f / 0.75f
    val path = Path().apply {
        moveTo(cx - hw, cy)
        cubicTo(cx - hw * 0.45f, cy - k, cx + hw * 0.45f, cy - k, cx + hw, cy)
        cubicTo(cx + hw * 0.45f, cy + k, cx - hw * 0.45f, cy + k, cx - hw, cy)
        close()
    }
    p.style = Paint.Style.STROKE
    p.strokeWidth = stroke
    p.strokeJoin = Paint.Join.ROUND
    p.color = color
    c.drawPath(path, p)
    p.style = Paint.Style.FILL
    c.drawCircle(cx, cy, w * 0.19f, p)
}

class Prefs(ctx: Context) {
    private val p: SharedPreferences = ctx.getSharedPreferences("eyebreak", Context.MODE_PRIVATE)

    var enabled: Boolean
        get() = p.getBoolean("enabled", true)
        set(v) = p.edit().putBoolean("enabled", v).apply()
    var badgeOn: Boolean
        get() = p.getBoolean("badge", true)
        set(v) = p.edit().putBoolean("badge", v).apply()
    var testMode: Boolean
        get() = p.getBoolean("test", false)
        set(v) = p.edit().putBoolean("test", v).apply()
    var badgeX: Int
        get() = p.getInt("bx", -1)
        set(v) = p.edit().putInt("bx", v).apply()
    var badgeY: Int
        get() = p.getInt("by", -1)
        set(v) = p.edit().putInt("by", v).apply()

    fun breaksToday(): Int = if (p.getString("day", "") == LocalDate.now().toString()) p.getInt("count", 0) else 0

    fun addBreak() {
        val n = breaksToday() + 1
        p.edit().putString("day", LocalDate.now().toString()).putInt("count", n).apply()
    }
}
