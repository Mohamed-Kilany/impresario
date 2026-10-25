import AVFoundation
import ImpresarioHook
import UIKit

/// Playback fixture for the Apple stub (TASK-3 section 5, D-40).
///
/// It plays only public/local material: a bundled H.264/AAC clip, the bundled
/// still image and Apple's public BipBop HLS stream. No DRM asset is shipped.
/// The accessibility identifiers mirror the Android `MediaActivity` so a play
/// differs only in selector strategy.
final class MediaViewController: UIViewController {
    private let videoContainer = UIView()
    private let videoLayer = AVPlayerLayer()
    private var videoPlayer: AVPlayer?
    private var audioPlayer: AVAudioPlayer?
    private let statusLabel = UILabel()
    private let durationLabel = UILabel()
    private var timeObserver: Any?
    private var statusObservation: NSKeyValueObservation?
    private var hookTimer: Timer?

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        view.accessibilityIdentifier = "stub_media_root"

        let content = UIStackView()
        content.axis = .vertical
        content.spacing = 12
        content.alignment = .fill
        content.translatesAutoresizingMaskIntoConstraints = false

        let title = UILabel()
        title.text = "Playback"
        title.font = .preferredFont(forTextStyle: .largeTitle)
        title.accessibilityIdentifier = "stub_media_title"
        content.addArrangedSubview(title)

        videoContainer.backgroundColor = .black
        videoContainer.accessibilityIdentifier = "stub_media_video"
        videoContainer.translatesAutoresizingMaskIntoConstraints = false
        videoContainer.heightAnchor.constraint(equalToConstant: 220).isActive = true
        videoLayer.videoGravity = .resizeAspect
        videoContainer.layer.addSublayer(videoLayer)
        content.addArrangedSubview(videoContainer)

        statusLabel.text = "state: Idle"
        statusLabel.accessibilityIdentifier = "stub_media_status"
        statusLabel.font = .monospacedSystemFont(ofSize: UIFont.systemFontSize, weight: .regular)
        content.addArrangedSubview(statusLabel)
        durationLabel.text = "time: 0 / 0"
        durationLabel.accessibilityIdentifier = "stub_media_time"
        durationLabel.font = .monospacedSystemFont(ofSize: UIFont.systemFontSize, weight: .regular)
        content.addArrangedSubview(durationLabel)

        content.addArrangedSubview(row([
            button("Play local", "stub_media_play", #selector(playLocal)),
            button("Pause", "stub_media_pause", #selector(pause)),
            button("Load HLS", "stub_media_stream", #selector(playStream)),
        ]))

        let image = UIImageView(image: UIImage(named: "sample_image"))
        image.contentMode = .scaleAspectFit
        image.accessibilityIdentifier = "stub_media_image"
        image.accessibilityLabel = "sample image"
        image.translatesAutoresizingMaskIntoConstraints = false
        image.heightAnchor.constraint(equalToConstant: 120).isActive = true
        content.addArrangedSubview(image)

        // A deliberately empty image slot: probes must not report a painted
        // image where there is none.
        let broken = UIImageView()
        broken.backgroundColor = .secondarySystemBackground
        broken.accessibilityIdentifier = "stub_media_broken"
        broken.accessibilityLabel = "missing image"
        broken.translatesAutoresizingMaskIntoConstraints = false
        broken.heightAnchor.constraint(equalToConstant: 60).isActive = true
        content.addArrangedSubview(broken)

        content.addArrangedSubview(row([
            button("Play audio", "stub_media_audio_play", #selector(playAudio)),
            button("Pause audio", "stub_media_audio_pause", #selector(pauseAudio)),
        ]))

        let close = button("Close", "stub_media_close", #selector(close))
        content.addArrangedSubview(close)

        let scroll = UIScrollView()
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.addSubview(content)
        view.addSubview(scroll)
        NSLayoutConstraint.activate([
            scroll.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            content.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor, constant: 16),
            content.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor, constant: -16),
            content.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor, constant: 16),
            content.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor, constant: -16),
            content.widthAnchor.constraint(equalTo: scroll.frameLayoutGuide.widthAnchor, constant: -32),
        ])

        loadLocal()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        videoLayer.frame = videoContainer.bounds
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        hookTimer?.invalidate()
        hookTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            self?.reportHook()
        }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        hookTimer?.invalidate()
        hookTimer = nil
        videoPlayer?.pause()
        audioPlayer?.pause()
    }

    deinit {
        if let timeObserver, let videoPlayer {
            videoPlayer.removeTimeObserver(timeObserver)
        }
    }

    // ------------------------------------------------------------------
    // playback
    // ------------------------------------------------------------------

    private func loadLocal() {
        guard let url = Bundle.main.url(forResource: "media_sample", withExtension: "mp4") else {
            statusLabel.text = "state: Error (media_sample.mp4 missing)"
            return
        }
        install(AVPlayer(url: url))
    }

    private func install(_ player: AVPlayer) {
        if let timeObserver, let videoPlayer {
            videoPlayer.removeTimeObserver(timeObserver)
        }
        videoPlayer = player
        videoLayer.player = player
        statusObservation = player.observe(\.timeControlStatus) { [weak self] player, _ in
            DispatchQueue.main.async {
                self?.statusLabel.text = "state: \(Self.stateName(player))"
            }
        }
        timeObserver = player.addPeriodicTimeObserver(
            forInterval: CMTime(seconds: 0.25, preferredTimescale: 600),
            queue: .main
        ) { [weak self] time in
            guard let self else { return }
            let current = time.seconds.isFinite ? time.seconds : 0
            let total = self.videoPlayer?.currentItem?.duration.seconds ?? 0
            self.durationLabel.text = String(
                format: "time: %.1f / %.1f",
                current,
                total.isFinite ? total : 0
            )
        }
        statusLabel.text = "state: \(Self.stateName(player))"
    }

    @objc private func playLocal() {
        stopAudio()
        loadLocal()
        videoPlayer?.play()
    }

    @objc private func playStream() {
        // Apple's public HLS sample; no authentication and no DRM.
        guard let url = URL(string:
            "https://devstreaming-cdn.apple.com/videos/streaming/examples/bipbop_4x3/bipbop_4x3_variant.m3u8")
        else { return }
        stopAudio()
        install(AVPlayer(url: url))
        videoPlayer?.play()
    }

    private func stopAudio() {
        audioPlayer?.stop()
        audioPlayer = nil
    }

    @objc private func pause() {
        videoPlayer?.pause()
    }

    @objc private func playAudio() {
        guard let url = Bundle.main.url(forResource: "sample_audio", withExtension: "m4a") else {
            statusLabel.text = "state: Error (sample_audio.m4a missing)"
            return
        }
        do {
            videoPlayer?.pause()
            let audio = try AVAudioPlayer(contentsOf: url)
            audioPlayer = audio
            if audio.play() {
                statusLabel.text = "state: Playing (audio)"
            } else {
                audioPlayer = nil
                statusLabel.text = "state: Error (audio did not start)"
            }
        } catch {
            statusLabel.text = "state: Error (\(error.localizedDescription))"
        }
    }

    @objc private func pauseAudio() {
        audioPlayer?.pause()
        statusLabel.text = "state: Paused (audio)"
    }

    @objc private func close() {
        dismiss(animated: true)
    }

    private func reportHook() {
        // Audio-only playback is a fixture too: report the audio player while
        // it owns playback, so the same probe path covers audio and video.
        if let audio = audioPlayer {
            let source = audio.url.absoluteString
            ImpresarioHook.shared.report(
                media: ImpresarioHook.Media(
                    state: audio.isPlaying ? "Playing" : "Paused",
                    positionSeconds: audio.currentTime,
                    durationSeconds: audio.duration,
                    rate: audio.isPlaying ? Double(audio.rate) : 0,
                    volume: Double(audio.volume),
                    muted: false,
                    decoded: true,
                    source: source
                ),
                showing: ImpresarioHook.Showing(
                    decoded: true,
                    source: source,
                    piece: "stub_media_audio"
                )
            )
            return
        }
        guard let player = videoPlayer, let item = player.currentItem else { return }
        let duration = item.duration.seconds
        let position = player.currentTime().seconds
        let natural = item.presentationSize
        let decoded = item.status == .readyToPlay && (item.isPlaybackLikelyToKeepUp || player.timeControlStatus == .playing)
        ImpresarioHook.shared.report(
            media: ImpresarioHook.Media(
                state: Self.stateName(player),
                positionSeconds: position.isFinite ? position : nil,
                durationSeconds: duration.isFinite ? duration : nil,
                rate: Double(player.rate),
                volume: Double(player.volume),
                muted: player.isMuted,
                widthPx: natural.width > 0 ? Int(natural.width) : nil,
                heightPx: natural.height > 0 ? Int(natural.height) : nil,
                error: item.error?.localizedDescription,
                decoded: decoded,
                source: (item.asset as? AVURLAsset)?.url.absoluteString
            ),
            showing: ImpresarioHook.Showing(
                decoded: decoded,
                source: (item.asset as? AVURLAsset)?.url.absoluteString,
                piece: "stub_media_video"
            )
        )
    }

    // ------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------

    private static func stateName(_ player: AVPlayer) -> String {
        switch player.timeControlStatus {
        case .playing:
            return "Playing"
        case .waitingToPlayAtSpecifiedRate:
            return "Buffering"
        case .paused:
            return player.currentItem?.status == .readyToPlay ? "Paused" : "Loading"
        @unknown default:
            return "Idle"
        }
    }

    private func button(_ title: String, _ id: String, _ action: Selector) -> UIButton {
        let button = UIButton(type: .system)
        button.setTitle(title, for: .normal)
        button.accessibilityIdentifier = id
        button.addTarget(self, action: action, for: .primaryActionTriggered)
        return button
    }

    private func row(_ views: [UIView]) -> UIStackView {
        let stack = UIStackView(arrangedSubviews: views)
        stack.axis = .horizontal
        stack.spacing = 8
        stack.distribution = .fillEqually
        return stack
    }
}
