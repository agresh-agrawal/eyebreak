package io.github.agreshagrawal.eyebreak

import android.content.Context
import android.hardware.camera2.CameraManager
import android.hardware.display.DisplayManager
import android.media.AudioManager
import android.os.Handler
import android.os.SystemClock

/** Something important is on: hold the break. */
data class Busy(val reason: String, val short: String, val presenting: Boolean)

/**
 * "Is the phone busy with something important right now?"
 *  - casting / presenting to another screen
 *  - a phone call, or a VoIP call/meeting (WhatsApp, Meet, Zoom, Teams... all
 *    switch the audio system into "communication" mode)
 *  - another app has the camera open
 *  - another app is recording from the microphone
 * Games and videos are deliberately not important.
 */
class Detector(ctx: Context) {
    private val audio = ctx.getSystemService(AudioManager::class.java)
    private val cameras = ctx.getSystemService(CameraManager::class.java)
    private val displays = ctx.getSystemService(DisplayManager::class.java)
    private val busyCams = mutableSetOf<String>()
    private var micSince = -1L

    private val camCallback = object : CameraManager.AvailabilityCallback() {
        override fun onCameraAvailable(cameraId: String) {
            busyCams.remove(cameraId)
        }

        override fun onCameraUnavailable(cameraId: String) {
            busyCams.add(cameraId)
        }
    }

    fun start(handler: Handler) {
        try {
            cameras?.registerAvailabilityCallback(camCallback, handler)
        } catch (_: Exception) {
        }
    }

    fun stop() {
        try {
            cameras?.unregisterAvailabilityCallback(camCallback)
        } catch (_: Exception) {
        }
    }

    fun detect(): Busy? {
        val presentation = displays?.getDisplays(DisplayManager.DISPLAY_CATEGORY_PRESENTATION)
        if (presentation != null && presentation.isNotEmpty()) {
            return Busy("You're casting or presenting to another screen", "Presenting", true)
        }
        when (audio?.mode) {
            // MODE_IN_CALL, MODE_RINGTONE, MODE_CALL_SCREENING (4), MODE_CALL_REDIRECT (5)
            AudioManager.MODE_IN_CALL, AudioManager.MODE_RINGTONE, 4, 5 ->
                return Busy("You're on a phone call", "Phone call", false)
            // MODE_IN_COMMUNICATION, MODE_COMMUNICATION_REDIRECT (6)
            AudioManager.MODE_IN_COMMUNICATION, 6 ->
                return Busy("You're in a call or meeting", "In a call", false)
        }
        if (busyCams.isNotEmpty()) {
            return Busy("Another app is using the camera", "Camera on", false)
        }
        val recording = try {
            audio?.activeRecordingConfigurations?.isNotEmpty() == true
        } catch (_: Exception) {
            false
        }
        if (recording) {
            val now = SystemClock.elapsedRealtime()
            if (micSince < 0) micSince = now
            // A recorder left running for ages shouldn't block breaks forever.
            if (now - micSince < 45 * 60_000L) {
                return Busy("Another app is using the microphone", "Mic in use", false)
            }
        } else {
            micSince = -1
        }
        return null
    }
}
