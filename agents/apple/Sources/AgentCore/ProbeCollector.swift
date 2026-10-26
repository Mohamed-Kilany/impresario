import Foundation
#if canImport(CoreGraphics)
import CoreGraphics
#endif

/// Collects TASK-3 probe evidence on iOS/tvOS.
///
/// The agent can prove what XCUITest exposes (tree visibility, element frames,
/// screenshots) and what an optional in-app hook supplies (player/DRM/decoder
/// state). It cannot inspect another app's AVPlayer or FairPlay session through
/// a public API; those fields stay `nil` and the claims become UNVERIFIABLE.
/// The verdict math here is provisional because the master canonicalizes every
/// result with the Rust core (D-27).
public final class ProbeCollector: @unchecked Sendable {
    private let driver: UIDriver
    private let extras: DriverExtras?
    private let now: () -> Int
    private let sleep: (Int) async throws -> Void

    public init(
        driver: UIDriver,
        extras: DriverExtras?,
        now: @escaping () -> Int = { Int(Date().timeIntervalSince1970 * 1000) },
        sleep: @escaping (Int) async throws -> Void = { ms in
            try await Task.sleep(nanoseconds: UInt64(max(ms, 1)) * 1_000_000)
        }
    ) {
        self.driver = driver
        self.extras = extras
        self.now = now
        self.sleep = sleep
    }

    public func collect(_ command: ProbeCommandSpec, timeoutMs: Int) async throws -> ProbeResultSpec {
        guard let selector = command.selector else {
            throw AgentError.protocolError("probe requires a selector")
        }
        let window = command.windowMs == 0 ? 3_000 : command.windowMs
        let budget = min(max(timeoutMs, 250), 2_000)
        let element = await find(selector, budgetMs: budget)
        let rect = element.map { driver.frame($0) }
        let visible = element.map { driver.isVisible($0) } ?? false

        var evidence: [ProbeEvidenceSpec] = []
        switch command.kind {
        case .showing:
            collectShowing(
                command,
                element: element,
                visible: visible,
                rect: rect,
                window: window,
                evidence: &evidence
            )
        case .player, .drm:
            await collectPlayer(
                command,
                element: element,
                rect: rect,
                window: window,
                timeoutMs: timeoutMs,
                evidence: &evidence
            )
        }

        return finish(command, window: window, evidence: evidence)
    }

    // ------------------------------------------------------------------
    // Discovery
    // ------------------------------------------------------------------

    private func find(_ selector: SelectorSpec, budgetMs: Int) async -> ElementRef? {
        let deadline = now() + budgetMs
        while true {
            let matches = (try? driver.find(selector.lookup(), timeout: 0)) ?? []
            if matches.indices.contains(selector.index) {
                return matches[selector.index]
            }
            if now() >= deadline {
                return nil
            }
            try? await sleep(50)
        }
    }

    // ------------------------------------------------------------------
    // Showing
    // ------------------------------------------------------------------

    private func collectShowing(
        _ command: ProbeCommandSpec,
        element: ElementRef?,
        visible: Bool,
        rect: CGRect?,
        window: Int,
        evidence: inout [ProbeEvidenceSpec]
    ) {
        let found = element != nil
        evidence.append(claimEvidence(
            source: .accessibilityTree,
            kind: found ? "visibility" : "element_presence",
            observationType: "bool",
            observation: String(visible),
            evidenceClass: found ? .system : .authoritative,
            claim: "showing=present",
            truth: visible,
            window: window
        ))

        var coverage: Double?
        if let element, let rect, !rect.isEmpty {
            if let png = try? driver.screenshotPNG(), !png.isEmpty {
                coverage = extras?.regionCoverage(png: png, rect: rect)
            }
            if let decoded = extras?.showingDecoded(element) {
                for claim in command.claims where claim.claim.hasPrefix("showing.decoded=") {
                    let expected = claim.claim.hasPrefix("showing.decoded=true")
                    evidence.append(claimEvidence(
                        source: .inAppHook,
                        kind: "decoded",
                        observationType: "bool",
                        observation: String(decoded),
                        evidenceClass: .authoritative,
                        claim: claim.claim,
                        truth: decoded == expected,
                        window: window
                    ))
                }
            }
            if let source = extras?.showingSource(element) {
                for claim in command.claims where claim.claim.hasPrefix("showing.source~") {
                    let expected = String(claim.claim.dropFirst("showing.source~".count))
                    evidence.append(claimEvidence(
                        source: .inAppHook,
                        kind: "media_source",
                        observationType: "text",
                        observation: source,
                        evidenceClass: .authoritative,
                        claim: claim.claim,
                        truth: source.contains(expected),
                        window: window
                    ))
                }
            }
        }

        if let coverage {
            evidence.append(claimEvidence(
                source: .screenshotRegion,
                kind: "painted_region",
                observationType: "number",
                observation: String(coverage),
                evidenceClass: .authoritative,
                claim: "showing=present",
                truth: coverage > 0.5,
                window: window
            ))
            for claim in command.claims where claim.claim.hasPrefix("showing.coverage>=") {
                let suffix = claim.claim.dropFirst("showing.coverage>=".count)
                guard suffix.hasSuffix("%"), let percent = Double(suffix.dropLast()) else {
                    continue
                }
                evidence.append(claimEvidence(
                    source: .screenshotRegion,
                    kind: "coverage",
                    observationType: "number",
                    observation: String(coverage),
                    evidenceClass: .authoritative,
                    claim: claim.claim,
                    truth: coverage >= percent / 100.0,
                    window: window
                ))
            }
        }
    }

    // ------------------------------------------------------------------
    // Player / DRM
    // ------------------------------------------------------------------

    private func collectPlayer(
        _ command: ProbeCommandSpec,
        element: ElementRef?,
        rect: CGRect?,
        window: Int,
        timeoutMs: Int,
        evidence: inout [ProbeEvidenceSpec]
    ) async {
        let first = extras?.mediaSnapshot()
        let drm = extras?.drmSnapshot()
        if command.kind == .player {
            let pause = min(max(window, 250), 1_000) / 2
            let bounded = min(pause, max(timeoutMs, 250) / 2)
            if bounded > 0 {
                try? await sleep(bounded)
            }
        }
        let second = extras?.mediaSnapshot()
        let media = second ?? first
        let found = element != nil

        let stateClaims = command.claims.filter { $0.claim.hasPrefix("state=") }
        if !found && !stateClaims.isEmpty {
            for claim in stateClaims {
                evidence.append(claimEvidence(
                    source: .accessibilityTree,
                    kind: "element_presence",
                    observationType: "bool",
                    observation: "false",
                    evidenceClass: .authoritative,
                    claim: claim.claim,
                    truth: false,
                    window: window
                ))
            }
        } else if let state = media?.state {
            for claim in stateClaims {
                evidence.append(claimEvidence(
                    source: .inAppHook,
                    kind: "media_state",
                    observationType: "enum",
                    observation: state,
                    evidenceClass: .authoritative,
                    claim: claim.claim,
                    truth: claim.claim == "state=\(state)",
                    window: window
                ))
            }
        }

        if let drm {
            for claim in command.claims {
                if claim.claim.hasPrefix("drm.enabled=") {
                    guard let enabled = drm.enabled,
                          let expected = parseBool(String(claim.claim.dropFirst("drm.enabled=".count)))
                    else { continue }
                    evidence.append(claimEvidence(
                        source: .inAppHook,
                        kind: "drm_enabled",
                        observationType: "bool",
                        observation: String(enabled),
                        evidenceClass: .authoritative,
                        claim: claim.claim,
                        truth: enabled == expected,
                        window: window
                    ))
                } else if claim.claim.hasPrefix("drm.provider=") {
                    guard let provider = drm.provider else { continue }
                    let expected = String(claim.claim.dropFirst("drm.provider=".count))
                    evidence.append(claimEvidence(
                        source: .inAppHook,
                        kind: "drm_provider",
                        observationType: "enum",
                        observation: provider,
                        evidenceClass: .authoritative,
                        claim: claim.claim,
                        truth: provider.caseInsensitiveCompare(expected) == .orderedSame,
                        window: window
                    ))
                }
            }
        }

        let fullscreen = rect.map { isFullscreen($0) }
        for assertion in command.assertions {
            guard let observation = observe(
                assertion.property,
                media: media,
                drm: drm,
                fullscreen: fullscreen
            ) else { continue }
            evidence.append(ProbeEvidenceSpec(
                source: observation.source,
                kind: "property:\(assertion.property)",
                observationType: observation.type,
                observation: observation.value,
                atUnixMs: now(),
                windowMs: window,
                evidenceClass: observation.evidenceClass,
                supports: [assertion.claimID]
            ))
        }
    }

    private func isFullscreen(_ rect: CGRect) -> Bool {
        let info = driver.deviceInfo()
        guard info.screenWidthPx > 0, info.screenHeightPx > 0 else { return false }
        return rect.width >= CGFloat(info.screenWidthPx) * 0.98
            && rect.height >= CGFloat(info.screenHeightPx) * 0.98
    }

    private struct Observation {
        let value: String
        let source: ProbeEvidenceSourceSpec
        let type: String
        let evidenceClass: ProbeEvidenceClassSpec
    }

    private func observe(
        _ property: String,
        media: MediaSnapshot?,
        drm: DrmSnapshot?,
        fullscreen: Bool?
    ) -> Observation? {
        func number(_ value: Double?, _ source: ProbeEvidenceSourceSpec = .inAppHook) -> Observation? {
            value.map { Observation(value: String($0), source: source, type: "number", evidenceClass: .authoritative) }
        }
        func text(_ value: String?, _ source: ProbeEvidenceSourceSpec) -> Observation? {
            value.map { Observation(value: $0, source: source, type: "text", evidenceClass: .authoritative) }
        }
        switch property {
        case "position": return number(media?.positionSeconds)
        case "duration": return number(media?.durationSeconds)
        case "buffered", "live", "looping", "bitrate", "framerate", "droppedFrames",
             "videoCodec", "audioCodec", "decoder":
            return nil
        case "rate": return number(media?.rate)
        case "volume": return number(media?.volume)
        case "muted":
            return media?.muted.map {
                Observation(value: String($0), source: .inAppHook, type: "bool", evidenceClass: .authoritative)
            }
        case "fullscreen":
            return fullscreen.map {
                Observation(value: String($0), source: .accessibilityTree, type: "bool", evidenceClass: .heuristic)
            }
        case "width": return number(media?.widthPx.map(Double.init))
        case "height": return number(media?.heightPx.map(Double.init))
        case "resolution": return number(media?.heightPx.map(Double.init))
        case "secure":
            return drm?.enabled.map {
                Observation(value: String($0), source: .inAppHook, type: "bool", evidenceClass: .authoritative)
            }
        case "audioTrack": return text(media?.audioTrack, .inAppHook)
        case "subtitleTrack": return text(media?.subtitleTrack, .inAppHook)
        case "error": return text(media?.error, .inAppHook)
        case "license": return text(drm?.license, .inAppHook)
        case "keyStatus": return text(drm?.keyStatus, .inAppHook)
        case "securityLevel": return text(drm?.securityLevel, .inAppHook)
        case "keySystem": return text(drm?.provider, .inAppHook)
        case "hdcp": return text(drm?.hdcp, .inAppHook)
        default: return nil
        }
    }

    // ------------------------------------------------------------------
    // Evidence and provisional verdicts
    // ------------------------------------------------------------------

    private func claimEvidence(
        source: ProbeEvidenceSourceSpec,
        kind: String,
        observationType: String,
        observation: String,
        evidenceClass: ProbeEvidenceClassSpec,
        claim: String,
        truth: Bool,
        window: Int
    ) -> ProbeEvidenceSpec {
        ProbeEvidenceSpec(
            source: source,
            kind: kind,
            observationType: observationType,
            observation: observation,
            atUnixMs: now(),
            windowMs: window,
            evidenceClass: evidenceClass,
            supports: truth ? [claim] : [],
            contradicts: truth ? [] : [claim]
        )
    }

    private func finish(
        _ command: ProbeCommandSpec,
        window: Int,
        evidence: [ProbeEvidenceSpec]
    ) -> ProbeResultSpec {
        let required = confidenceRank(command.minConfidence)
        var verdicts: [ProbeVerdictSpec] = []

        for claim in command.claims {
            let support = confidenceOf(claim.claim, positive: true, evidence: evidence)
            let contradict = confidenceOf(claim.claim, positive: false, evidence: evidence)
            let (expected, opposite) = claim.negated
                ? (contradict, support)
                : (support, contradict)
            let expectedMeets = expected >= required
            let oppositeMeets = opposite >= required
            let verdict: String
            let reason: String?
            if expectedMeets && !oppositeMeets {
                verdict = "pass"
                reason = nil
            } else if !expectedMeets && oppositeMeets {
                verdict = "fail"
                reason = "evidence contradicts the claim"
            } else {
                verdict = "unverifiable"
                reason = oppositeMeets
                    ? "conflicting evidence from different sources"
                    : "evidence is below the required confidence"
            }
            verdicts.append(ProbeVerdictSpec(
                claim: claim.claim,
                verdict: verdict,
                confidence: confidenceName(max(expected, opposite)),
                reason: reason
            ))
        }

        for assertion in command.assertions {
            verdicts.append(assertionVerdict(assertion, required: required, evidence: evidence))
        }

        var observed: [String: String] = [:]
        for verdict in verdicts {
            let item = evidence.first {
                $0.supports.contains(verdict.claim) || $0.contradicts.contains(verdict.claim)
            }
            observed[verdict.claim] = item?.observation ?? verdict.confidence
        }
        let overall = verdicts.map { confidenceRank($0.confidence) }.max() ?? 0
        let reason = verdicts.first { $0.verdict == "unverifiable" }?.reason
        return ProbeResultSpec(
            verdicts: verdicts,
            evidence: evidence,
            confidence: confidenceName(overall),
            unverifiableReason: reason,
            observed: observed,
            windowMs: window
        )
    }

    private func assertionVerdict(
        _ assertion: ProbeAssertionSpec,
        required: Int,
        evidence: [ProbeEvidenceSpec]
    ) -> ProbeVerdictSpec {
        let matching = evidence.filter { $0.supports.contains(assertion.claimID) }
        let confidence = confidenceFromItems(matching)
        guard confidence >= required else {
            return ProbeVerdictSpec(
                claim: assertion.claimID,
                verdict: "unverifiable",
                confidence: confidenceName(confidence),
                reason: "the property was not observed at the required confidence"
            )
        }
        var matched = false
        var rejected = false
        for item in matching {
            guard let value = matches(item, assertion) else { continue }
            if value { matched = true } else { rejected = true }
        }
        let verdict: String
        let reason: String?
        if matched && !rejected {
            verdict = "pass"
            reason = nil
        } else if rejected && !matched {
            verdict = "fail"
            reason = matching.last.map { "observed '\($0.observation)'" }
        } else {
            verdict = "unverifiable"
            reason = "conflicting observations for the property"
        }
        return ProbeVerdictSpec(
            claim: assertion.claimID,
            verdict: verdict,
            confidence: confidenceName(confidence),
            reason: reason
        )
    }

    private func matches(_ item: ProbeEvidenceSpec, _ assertion: ProbeAssertionSpec) -> Bool? {
        let base: Bool?
        switch item.observationType {
        case "bool":
            guard let observed = parseBool(item.observation),
                  let expected = parseBool(assertion.value)
            else { return nil }
            base = observed == expected
        case "number":
            guard let observed = Double(item.observation),
                  let expected = Double(assertion.value)
            else { return nil }
            switch assertion.operator {
            case ">": base = observed > expected
            case ">=": base = observed >= expected
            case "<": base = observed < expected
            case "<=": base = observed <= expected
            case "!=": base = observed != expected
            default: base = abs(observed - expected) < 0.000_001
            }
        case "text", "enum":
            switch assertion.operator {
            case "~": base = item.observation.contains(assertion.value)
            case "!=": base = item.observation != assertion.value
            default: base = item.observation == assertion.value
            }
        default:
            return nil
        }
        guard let base else { return nil }
        return assertion.negated ? !base : base
    }

    private func confidenceOf(_ claim: String, positive: Bool, evidence: [ProbeEvidenceSpec]) -> Int {
        let items: [ProbeEvidenceSpec]
        if positive {
            items = evidence.filter { $0.supports.contains(claim) && $0.contradicts.isEmpty }
        } else {
            items = evidence.filter { $0.contradicts.contains(claim) && $0.supports.isEmpty }
        }
        return confidenceFromItems(items)
    }

    private func confidenceFromItems(_ items: [ProbeEvidenceSpec]) -> Int {
        if items.contains(where: { $0.evidenceClass == .authoritative }) {
            return 3
        }
        let sources = Set(items.filter { $0.evidenceClass != .authoritative }.map { $0.source.rawValue })
        let hasSystem = items.contains { $0.evidenceClass == .system }
        let heuristics = items.filter { $0.evidenceClass == .heuristic }.count
        if sources.count >= 2 && hasSystem {
            return 2
        }
        if hasSystem || heuristics > 0 {
            return 1
        }
        return 0
    }

    private func confidenceName(_ rank: Int) -> String {
        switch rank {
        case 3: return "authoritative"
        case 2: return "corroborated"
        case 1: return "inferred"
        default: return "none"
        }
    }

    private func confidenceRank(_ value: String) -> Int {
        switch value.lowercased() {
        case "authoritative": return 3
        case "corroborated", "": return 2
        case "inferred": return 1
        case "none": return 0
        default: return 2
        }
    }

    private func parseBool(_ value: String) -> Bool? {
        switch value.lowercased() {
        case "true", "1", "yes": return true
        case "false", "0", "no": return false
        default: return nil
        }
    }
}
