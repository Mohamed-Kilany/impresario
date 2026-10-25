package com.impresario.agent

import impresario.v1.Impresario.ClaimVerdict
import impresario.v1.Impresario.EvidenceClass
import impresario.v1.Impresario.ProbeAssertion
import impresario.v1.Impresario.ProbeCommand
import impresario.v1.Impresario.ProbeConfidence
import impresario.v1.Impresario.ProbeEvidence
import impresario.v1.Impresario.ProbeKind
import impresario.v1.Impresario.ProbeResult
import impresario.v1.Impresario.VerdictState
import kotlinx.coroutines.delay
import kotlin.math.max

/**
 * Evidence collection for TASK-3 probes on Android. The verdict math here is
 * only a provisional wire value; the master recomputes every verdict with the
 * Rust core (exec.rs `canonicalize_probe`), so this file must never invent
 * support that the evidence does not carry.
 *
 * Channels: the accessibility tree (showing), screenshot pixels (coverage),
 * MediaSession/audio state (player), MediaDrm (DRM). Channels the agent
 * cannot reach leave the claim without evidence, which becomes UNVERIFIABLE
 * rather than a false PASS.
 */
class ProbeCollector(
    private val driver: UiDriver,
    private val extras: DriverExtras?,
    private val now: () -> Long,
) {
    private val evidence = mutableListOf<Evidence>()
    private val result: ProbeResult.Builder = ProbeResult.newBuilder()

    suspend fun collect(command: ProbeCommand, timeoutMs: Long): ProbeResult {
        val window = if (command.windowMs == 0) 3000 else command.windowMs.toLong()
        val kind = command.kind
        if (kind == ProbeKind.PROBE_KIND_UNSPECIFIED) {
            throw AgentException.protocol("probe kind is unspecified")
        }
        if (!command.hasSelector()) {
            throw AgentException.protocol("probe requires a selector")
        }
        val spec = SelectorSpec.from(command.selector)
        val findBudget = timeoutMs.coerceIn(250, 2000)
        val element = find(spec, findBudget)
        val rect = element?.let { runCatching { driver.bounds(it) }.getOrNull() }
        val visible = element != null && runCatching { driver.isVisible(element) }.getOrDefault(false)

        when (kind) {
            ProbeKind.PROBE_KIND_SHOWING -> collectShowing(command, element, visible, rect, window)
            ProbeKind.PROBE_KIND_PLAYER,
            ProbeKind.PROBE_KIND_DRM,
            -> collectPlayer(command, element, visible, window, timeoutMs)
            else -> throw AgentException.protocol("unknown probe kind '${kind.name}'")
        }

        finish(command)
        return result.build()
    }

    private suspend fun find(spec: SelectorSpec, budgetMs: Long): ElementHandle? {
        val deadline = now() + budgetMs
        while (true) {
            val matches = runCatching { driver.find(spec, 0) }.getOrDefault(emptyList())
            matches.getOrNull(spec.index)?.let { return it }
            if (now() >= deadline) return null
            delay(50)
        }
    }

    // ------------------------------------------------------------------
    // showing
    // ------------------------------------------------------------------

    private fun collectShowing(
        command: ProbeCommand,
        element: ElementHandle?,
        visible: Boolean,
        rect: Rect?,
        window: Long,
    ) {
        val found = element != null
        val cls = if (found) EvidenceClass.EVIDENCE_CLASS_SYSTEM else EvidenceClass.EVIDENCE_CLASS_AUTHORITATIVE
        claimEvidence(
            source = "a11y_tree",
            kind = "visibility",
            observationType = "bool",
            observation = visible.toString(),
            cls = cls,
            claim = "showing=present",
            truth = visible,
            windowMs = window.toInt(),
        )

        var coverage: Double? = null
        if (found && rect != null && !rect.isEmpty) {
            val png = runCatching { driver.screenshotPng() }.getOrNull()
            if (png != null && png.isNotEmpty()) {
                coverage = extras?.regionCoverage(png, rect)
            }
        }
        if (coverage != null) {
            claimEvidence(
                source = "screenshot_region",
                kind = "painted_region",
                observationType = "number",
                observation = coverage.toString(),
                cls = EvidenceClass.EVIDENCE_CLASS_AUTHORITATIVE,
                claim = "showing=present",
                truth = coverage > 0.5,
                windowMs = window.toInt(),
            )
        }
        // The optional in-app hook can report the app's own decode/source
        // state; without it those claims stay UNVERIFIABLE rather than guessed.
        val hook = extras?.showingSnapshot()
        for (claim in command.claimsList) {
            when {
                claim.claim == "showing.decoded=true" -> {
                    val decoded = hook?.decoded ?: continue
                    claimEvidence(
                        source = "app_hook",
                        kind = "decoded",
                        observationType = "bool",
                        observation = decoded.toString(),
                        cls = EvidenceClass.EVIDENCE_CLASS_AUTHORITATIVE,
                        claim = claim.claim,
                        truth = decoded,
                        windowMs = window.toInt(),
                    )
                }
                claim.claim.startsWith("showing.source~") -> {
                    val source = hook?.source ?: continue
                    val expected = claim.claim.removePrefix("showing.source~")
                    claimEvidence(
                        source = "app_hook",
                        kind = "media_source",
                        observationType = "text",
                        observation = source,
                        cls = EvidenceClass.EVIDENCE_CLASS_AUTHORITATIVE,
                        claim = claim.claim,
                        truth = source.contains(expected),
                        windowMs = window.toInt(),
                    )
                }
            }
        }
        if (coverage != null) {
            for (claim in command.claimsList) {
                val prefix = "showing.coverage>="
                if (claim.claim.startsWith(prefix) && claim.claim.endsWith("%")) {
                    val percent = claim.claim.removePrefix(prefix).removeSuffix("%").toDoubleOrNull()
                    if (percent != null) {
                        claimEvidence(
                            source = "screenshot_region",
                            kind = "coverage",
                            observationType = "number",
                            observation = coverage.toString(),
                            cls = EvidenceClass.EVIDENCE_CLASS_AUTHORITATIVE,
                            claim = claim.claim,
                            truth = coverage >= percent,
                            windowMs = window.toInt(),
                        )
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // player / DRM
    // ------------------------------------------------------------------

    private suspend fun collectPlayer(
        command: ProbeCommand,
        element: ElementHandle?,
        visible: Boolean,
        window: Long,
        timeoutMs: Long,
    ) {
        val first = extras?.mediaSnapshot()
        val drm = extras?.drmSnapshot()
        val pause = window.coerceIn(250, 1000).coerceAtMost(timeoutMs.coerceAtLeast(250)) / 2
        if (command.kind == ProbeKind.PROBE_KIND_PLAYER && pause > 0) {
            delay(pause)
        }
        val second = extras?.mediaSnapshot() ?: first
        val media = second ?: first
        val found = element != null

        val stateClaims = command.claimsList.filter { it.claim.startsWith("state=") }
        if (!found && stateClaims.isNotEmpty()) {
            // The selected piece does not exist: on this platform there is no
            // media for it, so every state claim is contradicted.
            for (claim in stateClaims) {
                claimEvidence(
                    source = "a11y_tree",
                    kind = "media_state",
                    observationType = "bool",
                    observation = "false",
                    cls = EvidenceClass.EVIDENCE_CLASS_AUTHORITATIVE,
                    claim = claim.claim,
                    truth = false,
                    windowMs = window.toInt(),
                )
            }
        } else {
            val statuses = linkedMapOf<String, Pair<String, EvidenceClass>>()
            val mediaSource = media?.origin ?: "media_session"
            media?.state?.let { statuses[mediaSource] = it to EvidenceClass.EVIDENCE_CLASS_AUTHORITATIVE }
            if (media?.activeAudio == true && media.state == null) {
                statuses["audio_playback"] = "Playing" to EvidenceClass.EVIDENCE_CLASS_SYSTEM
            }
            if (statuses.isEmpty() && stateClaims.isNotEmpty()) {
                // The piece exists but no observable media state is
                // available; claims stay UNVERIFIABLE (do not guess Paused).
            }
            for (claim in stateClaims) {
                for ((source, status) in statuses) {
                    claimEvidence(
                        source = source,
                        kind = "media_state",
                        observationType = "enum",
                        observation = status.first,
                        cls = status.second,
                        claim = claim.claim,
                        truth = claim.claim == "state=${status.first}",
                        windowMs = window.toInt(),
                    )
                }
            }
        }

        for (claim in command.claimsList) {
            when {
                claim.claim.startsWith("drm.enabled=") -> {
                    val expected = claim.claim.removePrefix("drm.enabled=").toBooleanStrictOrNull() ?: continue
                    val enabled = drm?.enabled ?: continue
                    claimEvidence(
                        source = drm.origin ?: "media_drm",
                        kind = "drm_enabled",
                        observationType = "bool",
                        observation = enabled.toString(),
                        cls = EvidenceClass.EVIDENCE_CLASS_AUTHORITATIVE,
                        claim = claim.claim,
                        truth = enabled == expected,
                        windowMs = window.toInt(),
                    )
                }
                claim.claim.startsWith("drm.provider=") -> {
                    val expected = claim.claim.removePrefix("drm.provider=")
                    val provider = drm?.provider ?: continue
                    claimEvidence(
                        source = drm.origin ?: "media_drm",
                        kind = "drm_provider",
                        observationType = "enum",
                        observation = provider,
                        cls = EvidenceClass.EVIDENCE_CLASS_AUTHORITATIVE,
                        claim = claim.claim,
                        truth = provider.equals(expected, ignoreCase = true),
                        windowMs = window.toInt(),
                    )
                }
            }
        }

        val fullscreen = if (rectOf(element) != null) isFullscreen(rectOf(element)!!) else null
        var t1: Double? = null
        var t2: Double? = null
        for (assertion in command.assertionsList) {
            val observed = observe(assertion.property, media, drm, fullscreen) ?: continue
            if (assertion.property == "position") {
                t1 = first?.positionSeconds ?: (observed.first as? Double)
                t2 = observed.first as? Double
            }
            addEvidence(
                Evidence(
                    source = observed.second,
                    kind = "property:${assertion.property}",
                    observationType = observed.third,
                    observation = observed.first.toString(),
                    cls = observed.fourth,
                    supports = setOf(assertion.claimId()),
                ),
                window.toInt(),
            )
        }
        // No delta/framerate observation: MediaSession does not expose frame
        // counters. The core turns the missing property into UNVERIFIABLE.
        if (t1 != null && t2 != null && t1 <= t2) {
            // Keep the samples meaningful for the report; no synthesized
            // framerate claim is added.
        }
    }

    private fun rectOf(element: ElementHandle?): Rect? =
        element?.let { runCatching { driver.bounds(it) }.getOrNull() }

    private fun isFullscreen(rect: Rect): Boolean {
        val info = runCatching { driver.deviceInfo() }.getOrNull() ?: return false
        val width = info.screenWidthPx
        val height = info.screenHeightPx
        if (width <= 0 || height <= 0) return false
        return rect.width >= width * 0.98 && rect.height >= height * 0.98
    }

    private data class Observation(
        val first: Any,
        val second: String,
        val third: String,
        val fourth: EvidenceClass,
    )

    private fun observe(
        property: String,
        media: MediaSnapshot?,
        drm: DrmSnapshot?,
        fullscreen: Boolean?,
    ): Observation? {
        val source = media?.origin ?: "media_session"
        val drmSource = drm?.origin ?: "media_drm"
        val auth = EvidenceClass.EVIDENCE_CLASS_AUTHORITATIVE
        return when (property) {
            "position" -> media?.positionSeconds?.let { Observation(it, source, "number", auth) }
            "duration" -> media?.durationSeconds?.let { Observation(it, source, "number", auth) }
            "buffered" -> null
            "rate" -> media?.rate?.let { Observation(it, source, "number", auth) }
            "volume" -> media?.volume?.let { Observation(it, source, "number", auth) }
            "muted" -> media?.muted?.let { Observation(it, source, "bool", auth) }
            "live" -> null
            "looping" -> null
            "fullscreen" -> fullscreen?.let {
                Observation(it, "a11y_tree", "bool", EvidenceClass.EVIDENCE_CLASS_HEURISTIC)
            }
            "width" -> media?.widthPx?.let { Observation(it, source, "number", auth) }
            "height" -> media?.heightPx?.let { Observation(it, source, "number", auth) }
            "resolution" -> media?.heightPx?.takeIf { it > 0 }?.let { Observation(it, source, "number", auth) }
            "bitrate" -> null
            "framerate" -> null
            "droppedFrames" -> null
            "videoCodec" -> null
            "audioCodec" -> null
            "decoder" -> null
            "secure" -> drm?.enabled?.let { Observation(it, drmSource, "bool", auth) }
            "audioTrack" -> media?.audioTrack?.let { Observation(it, source, "text", auth) }
            "subtitleTrack" -> media?.subtitleTrack?.let { Observation(it, source, "text", auth) }
            "error" -> media?.error?.let { Observation(it, source, "text", auth) }
            "license" -> drm?.license?.let { Observation(it, drmSource, "text", auth) }
            "keyStatus" -> drm?.keyStatus?.let { Observation(it, drmSource, "text", auth) }
            "securityLevel" -> drm?.securityLevel?.let { Observation(it, drmSource, "text", auth) }
            "keySystem" -> drm?.provider?.let { Observation(it, drmSource, "text", auth) }
            "hdcp" -> drm?.hdcp?.let { Observation(it, drmSource, "text", auth) }
            else -> null
        }
    }

    // ------------------------------------------------------------------
    // evidence + verdicts
    // ------------------------------------------------------------------

    private data class Evidence(
        val source: String,
        val kind: String,
        val observationType: String,
        val observation: String,
        val cls: EvidenceClass,
        val supports: Set<String> = emptySet(),
        val contradicts: Set<String> = emptySet(),
    )

    private fun claimEvidence(
        source: String,
        kind: String,
        observationType: String,
        observation: String,
        cls: EvidenceClass,
        claim: String,
        truth: Boolean,
        windowMs: Int,
    ) {
        addEvidence(
            Evidence(
                source = source,
                kind = kind,
                observationType = observationType,
                observation = observation,
                cls = cls,
                supports = if (truth) setOf(claim) else emptySet(),
                contradicts = if (truth) emptySet() else setOf(claim),
            ),
            windowMs,
        )
    }

    private fun addEvidence(item: Evidence, windowMs: Int) {
        evidence += item
        result.addEvidence(
            ProbeEvidence.newBuilder()
                .setSource(item.source)
                .setKind(item.kind)
                .setObservationType(item.observationType)
                .setObservation(item.observation)
                .setAtUnixMs(now())
                .setWindowMs(windowMs)
                .setClass_(item.cls)
                .addAllSupports(item.supports)
                .addAllContradicts(item.contradicts),
        )
    }

    private fun finish(command: ProbeCommand) {
        val required = requiredConfidence(command)
        val verdicts = mutableListOf<ClaimVerdict>()
        for (claim in command.claimsList) {
            val support = confidenceOf(claim.claim, positive = true)
            val contra = confidenceOf(claim.claim, positive = false)
            val (expected, opposite) = if (claim.negated) contra to support else support to contra
            val expectedMeets = expected.rank >= required
            val oppositeMeets = opposite.rank >= required
            val verdict = when {
                expectedMeets && !oppositeMeets -> VerdictState.VERDICT_STATE_PASS
                !expectedMeets && oppositeMeets -> VerdictState.VERDICT_STATE_FAIL
                else -> VerdictState.VERDICT_STATE_UNVERIFIABLE
            }
            verdicts += ClaimVerdict.newBuilder()
                .setClaim(claim.claim)
                .setVerdict(verdict)
                .setConfidence(maxConfidence(expected, opposite))
                .setReason(
                    when (verdict) {
                        VerdictState.VERDICT_STATE_PASS -> ""
                        VerdictState.VERDICT_STATE_FAIL -> "evidence contradicts the claim"
                        else -> "evidence is below the required confidence"
                    },
                )
                .build()
        }
        for (assertion in command.assertionsList) {
            verdicts += assertionVerdict(assertion, required)
        }
        for (verdict in verdicts) {
            result.addVerdicts(verdict)
            result.putObserved(
                verdict.claim,
                evidence.firstOrNull {
                    verdict.claim in it.supports || verdict.claim in it.contradicts
                }?.observation ?: verdict.confidence.name,
            )
        }
        val overall = verdicts.map { confidenceOfProto(it.confidence) }.maxByOrNull { it.rank }
            ?: Confidence.NONE
        result.setConfidence(overall.proto)
        result.setUnverifiableReason(
            verdicts.firstOrNull { it.verdict == VerdictState.VERDICT_STATE_UNVERIFIABLE }?.reason ?: "",
        )
        result.windowMs = if (command.windowMs == 0) 3000 else command.windowMs
    }

    private fun assertionVerdict(assertion: ProbeAssertion, required: Int): ClaimVerdict {
        val claim = assertion.claimId()
        val items = evidence.filter { claim in it.supports }
        val confidence = confidenceFromItems(items)
        if (confidence.rank < required) {
            return ClaimVerdict.newBuilder()
                .setClaim(claim)
                .setVerdict(VerdictState.VERDICT_STATE_UNVERIFIABLE)
                .setConfidence(confidence.proto)
                .setReason("the property was not observed at the required confidence")
                .build()
        }
        val item = items.last()
        val matched = matches(item, assertion)
        val passed = if (assertion.negated) !matched else matched
        return ClaimVerdict.newBuilder()
            .setClaim(claim)
            .setVerdict(if (passed) VerdictState.VERDICT_STATE_PASS else VerdictState.VERDICT_STATE_FAIL)
            .setConfidence(confidence.proto)
            .setReason(if (passed) "" else "observed '${item.observation}'")
            .build()
    }

    private fun matches(item: Evidence, assertion: ProbeAssertion): Boolean {
        val expected = assertion.value
        return when (item.observationType) {
            "bool" -> item.observation.toBooleanStrictOrNull() == expected.toBooleanStrictOrNull()
            "number" -> {
                val observed = item.observation.toDoubleOrNull() ?: return false
                val wanted = expected.toDoubleOrNull() ?: return false
                when (assertion.operator.ifEmpty { "=" }) {
                    ">" -> observed > wanted
                    ">=" -> observed >= wanted
                    "<" -> observed < wanted
                    "<=" -> observed <= wanted
                    "!=" -> observed != wanted
                    else -> kotlin.math.abs(observed - wanted) < 0.000001
                }
            }
            "text", "enum" -> when (assertion.operator) {
                "~" -> item.observation.contains(expected)
                "!=" -> item.observation != expected
                else -> item.observation == expected
            }
            else -> false
        }
    }

    private fun requiredConfidence(command: ProbeCommand): Int {
        val number = command.minConfidence.number
        return if (number == 0) 2 else number
    }

    private enum class Confidence(val rank: Int, val proto: ProbeConfidence) {
        NONE(0, ProbeConfidence.PROBE_CONFIDENCE_NONE),
        INFERRED(1, ProbeConfidence.PROBE_CONFIDENCE_INFERRED),
        CORROBORATED(2, ProbeConfidence.PROBE_CONFIDENCE_CORROBORATED),
        AUTHORITATIVE(3, ProbeConfidence.PROBE_CONFIDENCE_AUTHORITATIVE),
    }

    private fun confidenceOf(claim: String, positive: Boolean): Confidence =
        confidenceFromItems(
            evidence.filter {
                if (positive) claim in it.supports && it.contradicts.isEmpty()
                else claim in it.contradicts
            },
        )

    private fun confidenceFromItems(items: List<Evidence>): Confidence {
        if (items.any { it.cls == EvidenceClass.EVIDENCE_CLASS_AUTHORITATIVE }) {
            return Confidence.AUTHORITATIVE
        }
        val sources = items.map { it.source }.distinct().size
        val hasSystem = items.any { it.cls == EvidenceClass.EVIDENCE_CLASS_SYSTEM }
        val heuristics = items.count { it.cls == EvidenceClass.EVIDENCE_CLASS_HEURISTIC }
        return when {
            sources >= 2 && hasSystem -> Confidence.CORROBORATED
            hasSystem || heuristics > 0 -> Confidence.INFERRED
            else -> Confidence.NONE
        }
    }

    private fun maxConfidence(a: Confidence, b: Confidence): ProbeConfidence =
        if (a.rank >= b.rank) a.proto else b.proto

    private fun confidenceOfProto(value: ProbeConfidence): Confidence = when (value) {
        ProbeConfidence.PROBE_CONFIDENCE_AUTHORITATIVE -> Confidence.AUTHORITATIVE
        ProbeConfidence.PROBE_CONFIDENCE_CORROBORATED -> Confidence.CORROBORATED
        ProbeConfidence.PROBE_CONFIDENCE_INFERRED -> Confidence.INFERRED
        else -> Confidence.NONE
    }

    private fun ProbeAssertion.claimId(): String = "property.$property"
}
