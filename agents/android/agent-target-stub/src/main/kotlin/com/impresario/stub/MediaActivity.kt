package com.impresario.stub

import android.app.Activity
import android.media.MediaMetadata
import android.media.MediaPlayer
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.ViewGroup
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.VideoView
import com.impresario.hook.ImpresarioHook

/**
 * Playback fixture: a looping local MP4, a decoded image, an empty image slot
 * and a real MediaSession. The probe observes the same platform state a media
 * notification would render; it does not parse this app's private state.
 */
class MediaActivity : Activity() {
    private lateinit var video: VideoView
    private lateinit var status: TextView
    private lateinit var session: MediaSession
    private lateinit var player: MediaPlayer
    private val handler = Handler(Looper.getMainLooper())
    private val tick = object : Runnable {
        override fun run() {
            publishState()
            handler.postDelayed(this, 250)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        ImpresarioHook.start(this)
        val density = resources.displayMetrics.density
        fun dp(value: Int): Int = (value * density).toInt()

        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(24), dp(20), dp(24), dp(20))
        }
        column.addView(
            TextView(this).apply {
                id = R.id.stub_media_title
                text = "Playback fixture"
                textSize = 22f
            },
        )
        video = VideoView(this).apply {
            id = R.id.stub_media_video
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                dp(220),
            ).apply { topMargin = dp(12) }
        }
        column.addView(video)
        column.addView(
            ImageView(this).apply {
                id = R.id.stub_media_image
                contentDescription = "poster image"
                setImageResource(R.drawable.sample_image)
                layoutParams = LinearLayout.LayoutParams(dp(120), dp(120)).apply {
                    topMargin = dp(12)
                }
            },
        )
        // Deliberately empty: a painted background is not a decoded image.
        column.addView(
            ImageView(this).apply {
                id = R.id.stub_media_broken
                contentDescription = "missing image"
                setBackgroundColor(0xFF202020.toInt())
                layoutParams = LinearLayout.LayoutParams(dp(120), dp(60)).apply {
                    topMargin = dp(12)
                }
            },
        )
        status = TextView(this).apply {
            id = R.id.stub_media_status
            text = "loading"
            textSize = 16f
            setPadding(0, dp(12), 0, 0)
        }
        column.addView(status)
        setContentView(column)

        session = MediaSession(this, "impresario-stub-media").apply {
            setCallback(
                object : MediaSession.Callback() {
                    override fun onPlay() = startPlayback()
                    override fun onPause() {
                        player.pause()
                        publishState()
                    }
                    override fun onSeekTo(pos: Long) {
                        player.seekTo(pos.toInt())
                        publishState()
                    }
                },
            )
            isActive = true
        }

        video.setVideoURI(Uri.parse("android.resource://$packageName/${R.raw.media_sample}"))
        video.setOnPreparedListener { prepared ->
            player = prepared
            prepared.isLooping = true
            prepared.setVolume(0f, 0f)
            publishMetadata(prepared.duration)
            prepared.start()
            session.isActive = true
            publishState()
            handler.post(tick)
        }
        video.setOnErrorListener { _, _, _ ->
            status.text = "error"
            publishState(error = true)
            true
        }
        video.start()
    }

    private fun startPlayback() {
        if (::player.isInitialized) {
            player.start()
            publishState()
        }
    }

    private fun publishMetadata(durationMs: Int) {
        // MediaMetadata's platform keys cover duration/title; video pixel
        // dimensions are not a public platform metadata key, so the probe
        // leaves width/height UNVERIFIABLE rather than guessing.
        session.setMetadata(
            MediaMetadata.Builder()
                .putString(MediaMetadata.METADATA_KEY_TITLE, "Impresario sample")
                .putLong(MediaMetadata.METADATA_KEY_DURATION, durationMs.toLong())
                .build(),
        )
    }

    private fun publishState(error: Boolean = false) {
        if (!::player.isInitialized) return
        val state = when {
            error -> PlaybackState.STATE_ERROR
            !player.isPlaying && player.currentPosition == 0 -> PlaybackState.STATE_PAUSED
            player.isPlaying -> PlaybackState.STATE_PLAYING
            else -> PlaybackState.STATE_PAUSED
        }
        status.text = when (state) {
            PlaybackState.STATE_PLAYING -> "playing"
            PlaybackState.STATE_PAUSED -> "paused"
            else -> "error"
        }
        session.setPlaybackState(
            PlaybackState.Builder()
                .setActions(
                    PlaybackState.ACTION_PLAY or
                        PlaybackState.ACTION_PAUSE or
                        PlaybackState.ACTION_SEEK_TO,
                )
                .setState(state, player.currentPosition.toLong(), if (player.isPlaying) 1f else 0f)
                .build(),
        )
        val reported = when (state) {
            PlaybackState.STATE_PLAYING -> "Playing"
            PlaybackState.STATE_PAUSED -> "Paused"
            PlaybackState.STATE_ERROR -> "Error"
            else -> "Idle"
        }
        val duration = runCatching { player.duration / 1000.0 }.getOrDefault(0.0)
        // Debug-only hook (D-41): state comes from the app's own player, so it
        // upgrades the achievable confidence without changing playback.
        ImpresarioHook.player(
            ImpresarioHook.Media(
                state = reported,
                positionSeconds = player.currentPosition / 1000.0,
                durationSeconds = duration,
                rate = if (player.isPlaying) 1.0 else 0.0,
                volume = 0.0,
                muted = true,
                error = if (state == PlaybackState.STATE_ERROR) "playback error" else "",
                decoded = state != PlaybackState.STATE_ERROR,
                source = "android.resource://$packageName/${R.raw.media_sample}",
            ),
        )
        ImpresarioHook.showing(
            ImpresarioHook.Showing(
                decoded = state != PlaybackState.STATE_ERROR,
                source = "android.resource://$packageName/${R.raw.media_sample}",
                piece = "stub_media_video",
            ),
        )
    }

    override fun onDestroy() {
        handler.removeCallbacks(tick)
        ImpresarioHook.clear()
        if (::player.isInitialized) {
            player.release()
        }
        if (::session.isInitialized) {
            session.isActive = false
            session.release()
        }
        super.onDestroy()
    }
}
