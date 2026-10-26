import AVFoundation
import ImpresarioHook
import UIKit

/// Playback fixture for the Apple stub, mirroring Android `MediaActivity`: a
/// looping, muted local MP4, a decoded poster image, an intentionally empty
/// image slot and the same status strings. The accessibility ids are the
/// Android ids, so a play differs only in selector strategy.
final class MediaViewController: UIViewController {
    private let titleLabel = StubTheme.titleLabel("Playback fixture")
    private let videoContainer = UIView()
    private let videoLayer = AVPlayerLayer()
    private let imageView = UIImageView()
    private let brokenView = UIImageView()
    private let statusLabel = StubTheme.label("loading", size: 16)

    private var player: AVPlayer?
    private var statusObservation: NSKeyValueObservation?
    private var itemStatusObservation: NSKeyValueObservation?
    private var endObserver: NSObjectProtocol?
    private var hookTimer: Timer?

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = StubTheme.background

        titleLabel.font = .systemFont(ofSize: StubTheme.dp(22))
        titleLabel.accessibilityIdentifier = "stub_media_title"

        videoContainer.backgroundColor = .black
        videoContainer.accessibilityIdentifier = "stub_media_video"
        videoContainer.isAccessibilityElement = true
        videoContainer.translatesAutoresizingMaskIntoConstraints = false
        videoLayer.videoGravity = .resizeAspect
        videoContainer.layer.addSublayer(videoLayer)

        imageView.image = UIImage(named: "sample_image")
        imageView.contentMode = .scaleAspectFit
        imageView.accessibilityIdentifier = "stub_media_image"
        imageView.accessibilityLabel = "poster image"
        imageView.isAccessibilityElement = true
        imageView.translatesAutoresizingMaskIntoConstraints = false

        // Deliberately empty: a painted background is not a decoded image.
        brokenView.backgroundColor = StubTheme.mediaBrokenBackground
        brokenView.accessibilityIdentifier = "stub_media_broken"
        brokenView.accessibilityLabel = "missing image"
        brokenView.isAccessibilityElement = true
        brokenView.translatesAutoresizingMaskIntoConstraints = false

        statusLabel.accessibilityIdentifier = "stub_media_status"

        for subview in [titleLabel, videoContainer, imageView, brokenView, statusLabel] {
            subview.translatesAutoresizingMaskIntoConstraints = false
            view.addSubview(subview)
        }

        let contentTop: NSLayoutYAxisAnchor
        #if os(tvOS)
        let appBar = StubAppBar()
        view.addSubview(appBar)
        NSLayoutConstraint.activate([
            appBar.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            appBar.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            appBar.topAnchor.constraint(equalTo: view.topAnchor),
        ])
        contentTop = appBar.bottomAnchor
        #else
        contentTop = view.safeAreaLayoutGuide.topAnchor
        #endif

        let inset = StubTheme.mediaHorizontalInset
        NSLayoutConstraint.activate([
            titleLabel.leadingAnchor.constraint(
                equalTo: view.safeAreaLayoutGuide.leadingAnchor,
                constant: inset
            ),
            titleLabel.trailingAnchor.constraint(
                equalTo: view.safeAreaLayoutGuide.trailingAnchor,
                constant: -inset
            ),
            titleLabel.topAnchor.constraint(
                equalTo: contentTop,
                constant: StubTheme.mediaVerticalInset
            ),

            videoContainer.leadingAnchor.constraint(
                equalTo: view.safeAreaLayoutGuide.leadingAnchor,
                constant: inset
            ),
            videoContainer.trailingAnchor.constraint(
                equalTo: view.safeAreaLayoutGuide.trailingAnchor,
                constant: -inset
            ),
            videoContainer.topAnchor.constraint(
                equalTo: titleLabel.bottomAnchor,
                constant: StubTheme.dp(12)
            ),
            videoContainer.heightAnchor.constraint(equalToConstant: StubTheme.dp(220)),

            imageView.leadingAnchor.constraint(
                equalTo: view.safeAreaLayoutGuide.leadingAnchor,
                constant: inset
            ),
            imageView.topAnchor.constraint(
                equalTo: videoContainer.bottomAnchor,
                constant: StubTheme.dp(12)
            ),
            imageView.widthAnchor.constraint(equalToConstant: StubTheme.dp(120)),
            imageView.heightAnchor.constraint(equalToConstant: StubTheme.dp(120)),

            brokenView.leadingAnchor.constraint(
                equalTo: view.safeAreaLayoutGuide.leadingAnchor,
                constant: inset
            ),
            brokenView.topAnchor.constraint(
                equalTo: imageView.bottomAnchor,
                constant: StubTheme.dp(12)
            ),
            brokenView.widthAnchor.constraint(equalToConstant: StubTheme.dp(120)),
            brokenView.heightAnchor.constraint(equalToConstant: StubTheme.dp(60)),

            statusLabel.leadingAnchor.constraint(
                equalTo: view.safeAreaLayoutGuide.leadingAnchor,
                constant: inset
            ),
            statusLabel.trailingAnchor.constraint(
                equalTo: view.safeAreaLayoutGuide.trailingAnchor,
                constant: -inset
            ),
            // Android adds 12 dp of top padding to the status TextView and has
            // no bottom constraint; on a 1080p TV the column is taller than
            // the content area, so the status line is clipped in the same
            // place on both platforms.
            statusLabel.topAnchor.constraint(
                equalTo: brokenView.bottomAnchor,
                constant: StubTheme.dp(12)
            ),
        ])

        loadLocal()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        videoLayer.frame = videoContainer.bounds
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        player?.play()
        hookTimer?.invalidate()
        hookTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            self?.reportHook()
        }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        hookTimer?.invalidate()
        hookTimer = nil
        player?.pause()
    }

    deinit {
        hookTimer?.invalidate()
        statusObservation?.invalidate()
        itemStatusObservation?.invalidate()
        if let endObserver {
            NotificationCenter.default.removeObserver(endObserver)
        }
    }

    // ------------------------------------------------------------------
    // playback
    // ------------------------------------------------------------------

    private func loadLocal() {
        guard let url = Bundle.main.url(forResource: "media_sample", withExtension: "mp4") else {
            statusLabel.text = "error"
            return
        }
        install(AVPlayer(url: url))
    }

    private func install(_ newPlayer: AVPlayer) {
        statusObservation?.invalidate()
        itemStatusObservation?.invalidate()
        if let endObserver {
            NotificationCenter.default.removeObserver(endObserver)
            self.endObserver = nil
        }

        player = newPlayer
        newPlayer.isMuted = true
        newPlayer.volume = 0
        newPlayer.actionAtItemEnd = .none
        videoLayer.player = newPlayer

        statusObservation = newPlayer.observe(\.timeControlStatus, options: [.initial, .new]) {
            [weak self] player, _ in
            DispatchQueue.main.async {
                self?.updateStatus(player)
            }
        }
        if let item = newPlayer.currentItem {
            itemStatusObservation = item.observe(\.status, options: [.initial, .new]) {
                [weak self] item, _ in
                DispatchQueue.main.async {
                    if item.status == .failed {
                        self?.statusLabel.text = "error"
                    } else if let player = self?.player {
                        self?.updateStatus(player)
                    }
                }
            }
            endObserver = NotificationCenter.default.addObserver(
                forName: .AVPlayerItemDidPlayToEndTime,
                object: item,
                queue: .main
            ) { [weak self] _ in
                self?.player?.seek(to: .zero)
                self?.player?.play()
            }
        }

        updateStatus(newPlayer)
        newPlayer.play()
    }

    private func updateStatus(_ player: AVPlayer) {
        if player.currentItem?.status == .failed {
            statusLabel.text = "error"
            return
        }
        switch player.timeControlStatus {
        case .playing:
            statusLabel.text = "playing"
        case .waitingToPlayAtSpecifiedRate:
            statusLabel.text = "loading"
        case .paused:
            statusLabel.text = player.currentItem?.status == .readyToPlay ? "paused" : "loading"
        @unknown default:
            statusLabel.text = "loading"
        }
    }

    private func reportHook() {
        guard let player, let item = player.currentItem else { return }
        let duration = item.duration.seconds
        let position = player.currentTime().seconds
        let natural = item.presentationSize
        let state: String
        if item.status == .failed {
            state = "Error"
        } else if player.timeControlStatus == .playing {
            state = "Playing"
        } else if item.status == .readyToPlay {
            state = "Paused"
        } else {
            state = "Idle"
        }
        let source = (item.asset as? AVURLAsset)?.url.absoluteString
        ImpresarioHook.shared.report(
            media: ImpresarioHook.Media(
                state: state,
                positionSeconds: position.isFinite ? position : nil,
                durationSeconds: duration.isFinite ? duration : nil,
                rate: state == "Playing" ? 1.0 : 0.0,
                volume: 0.0,
                muted: true,
                widthPx: natural.width > 0 ? Int(natural.width) : nil,
                heightPx: natural.height > 0 ? Int(natural.height) : nil,
                error: state == "Error" ? "playback error" : "",
                decoded: state != "Error",
                source: source
            ),
            showing: ImpresarioHook.Showing(
                decoded: state != "Error",
                source: source,
                piece: "stub_media_video"
            )
        )
    }
}
