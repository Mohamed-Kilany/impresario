import Foundation

// ---------------------------------------------------------------------------
// Probe wire-neutral model (TASK-3 section 5, D-26/D-28).
//
// AgentCore never imports the generated protobuf types: AgentService converts
// `ProbeCommand` into `ProbeCommandSpec` and converts the result back. The
// verdict math here is provisional; the master canonicalizes every verdict
// with the Rust core, so this file must only report what the evidence says.
// ---------------------------------------------------------------------------

public enum ProbeKindSpec: String, Sendable, Equatable {
    case showing
    case player
    case drm
}

public struct ProbeClaimSpec: Sendable, Equatable {
    public let claim: String
    public let negated: Bool

    public init(claim: String, negated: Bool = false) {
        self.claim = claim
        self.negated = negated
    }
}

public struct ProbeAssertionSpec: Sendable, Equatable {
    public let property: String
    public let `operator`: String
    public let value: String
    public let negated: Bool

    public init(property: String, operator: String = "=", value: String, negated: Bool = false) {
        self.property = property
        self.operator = `operator`
        self.value = value
        self.negated = negated
    }

    public var claimID: String { "property.\(property)" }
}

public struct ProbeCommandSpec: Sendable, Equatable {
    public let kind: ProbeKindSpec
    public let selector: SelectorSpec?
    public let claims: [ProbeClaimSpec]
    public let assertions: [ProbeAssertionSpec]
    public let minConfidence: String
    public let windowMs: Int
    public let requireDecoded: Bool
    public let requireSource: Bool
    public let minCoverage: Double

    public init(
        kind: ProbeKindSpec,
        selector: SelectorSpec?,
        claims: [ProbeClaimSpec] = [],
        assertions: [ProbeAssertionSpec] = [],
        minConfidence: String = "corroborated",
        windowMs: Int = 3_000,
        requireDecoded: Bool = false,
        requireSource: Bool = false,
        minCoverage: Double = 0
    ) {
        self.kind = kind
        self.selector = selector
        self.claims = claims
        self.assertions = assertions
        self.minConfidence = minConfidence
        self.windowMs = windowMs
        self.requireDecoded = requireDecoded
        self.requireSource = requireSource
        self.minCoverage = minCoverage
    }
}

/// Evidence channels that a platform can actually observe. The raw values are
/// the stable wire vocabulary; the master maps them to the reference enum.
public enum ProbeEvidenceSourceSpec: String, Sendable, Equatable {
    case accessibilityTree = "a11y_tree"
    case screenshotRegion = "screenshot_region"
    case mediaSession = "media_session"
    case mediaDrm = "media_drm"
    case inAppHook = "app_hook"
}

/// The reference evidence classes (`authoritative`, `system`, `heuristic`).
public enum ProbeEvidenceClassSpec: String, Sendable, Equatable {
    case authoritative
    case system
    case heuristic

    public var rank: Int {
        switch self {
        case .authoritative: return 3
        case .system: return 2
        case .heuristic: return 1
        }
    }
}

public struct ProbeEvidenceSpec: Sendable, Equatable {
    public let source: ProbeEvidenceSourceSpec
    public let kind: String
    public let observationType: String
    public let observation: String
    public let atUnixMs: Int
    public let windowMs: Int
    public let evidenceClass: ProbeEvidenceClassSpec
    public let supports: [String]
    public let contradicts: [String]

    public init(
        source: ProbeEvidenceSourceSpec,
        kind: String,
        observationType: String,
        observation: String,
        atUnixMs: Int = 0,
        windowMs: Int = 0,
        evidenceClass: ProbeEvidenceClassSpec,
        supports: [String] = [],
        contradicts: [String] = []
    ) {
        self.source = source
        self.kind = kind
        self.observationType = observationType
        self.observation = observation
        self.atUnixMs = atUnixMs
        self.windowMs = windowMs
        self.evidenceClass = evidenceClass
        self.supports = supports
        self.contradicts = contradicts
    }
}

public struct ProbeVerdictSpec: Sendable, Equatable {
    public let claim: String
    public let verdict: String
    public let confidence: String
    public let reason: String?

    public init(claim: String, verdict: String, confidence: String, reason: String?) {
        self.claim = claim
        self.verdict = verdict
        self.confidence = confidence
        self.reason = reason
    }
}

public struct ProbeResultSpec: Sendable, Equatable {
    public let verdicts: [ProbeVerdictSpec]
    public let evidence: [ProbeEvidenceSpec]
    public let confidence: String
    public let unverifiableReason: String?
    public let observed: [String: String]
    public let windowMs: Int

    public init(
        verdicts: [ProbeVerdictSpec],
        evidence: [ProbeEvidenceSpec],
        confidence: String,
        unverifiableReason: String?,
        observed: [String: String],
        windowMs: Int
    ) {
        self.verdicts = verdicts
        self.evidence = evidence
        self.confidence = confidence
        self.unverifiableReason = unverifiableReason
        self.observed = observed
        self.windowMs = windowMs
    }
}

/// A platform media snapshot. Every field is optional because no platform
/// exposes all of them; a missing field becomes UNVERIFIABLE, never a guess.
public struct MediaSnapshot: Sendable, Equatable, Codable {
    public let state: String?
    public let positionSeconds: Double?
    public let durationSeconds: Double?
    public let rate: Double?
    public let volume: Double?
    public let muted: Bool?
    public let widthPx: Int?
    public let heightPx: Int?
    public let audioTrack: String?
    public let subtitleTrack: String?
    public let error: String?
    public let decoded: Bool?
    public let source: String?

    public init(
        state: String? = nil,
        positionSeconds: Double? = nil,
        durationSeconds: Double? = nil,
        rate: Double? = nil,
        volume: Double? = nil,
        muted: Bool? = nil,
        widthPx: Int? = nil,
        heightPx: Int? = nil,
        audioTrack: String? = nil,
        subtitleTrack: String? = nil,
        error: String? = nil,
        decoded: Bool? = nil,
        source: String? = nil
    ) {
        self.state = state
        self.positionSeconds = positionSeconds
        self.durationSeconds = durationSeconds
        self.rate = rate
        self.volume = volume
        self.muted = muted
        self.widthPx = widthPx
        self.heightPx = heightPx
        self.audioTrack = audioTrack
        self.subtitleTrack = subtitleTrack
        self.error = error
        self.decoded = decoded
        self.source = source
    }
}

public struct DrmSnapshot: Sendable, Equatable, Codable {
    public let enabled: Bool?
    public let provider: String?
    public let license: String?
    public let keyStatus: String?
    public let securityLevel: String?
    public let hdcp: String?

    public init(
        enabled: Bool? = nil,
        provider: String? = nil,
        license: String? = nil,
        keyStatus: String? = nil,
        securityLevel: String? = nil,
        hdcp: String? = nil
    ) {
        self.enabled = enabled
        self.provider = provider
        self.license = license
        self.keyStatus = keyStatus
        self.securityLevel = securityLevel
        self.hdcp = hdcp
    }
}
