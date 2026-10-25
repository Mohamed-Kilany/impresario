package com.impresario.agent

import impresario.v1.Impresario.ProbeAssertion
import impresario.v1.Impresario.ProbeClaim
import impresario.v1.Impresario.ProbeCommand
import impresario.v1.Impresario.ProbeKind
import impresario.v1.Impresario.ProbeResult
import impresario.v1.Impresario.Selector
import impresario.v1.Impresario.SelectorStrategy
import impresario.v1.Impresario.VerdictState
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ProbeTest {
    private fun selector(value: String = "piece"): Selector =
        Selector.newBuilder()
            .setStrategy(SelectorStrategy.ANDROID_ID)
            .setValue(value)
            .build()

    private fun claim(id: String): ProbeClaim =
        ProbeClaim.newBuilder().setClaim(id).build()

    private fun assertion(property: String, operator: String, value: String): ProbeAssertion =
        ProbeAssertion.newBuilder()
            .setProperty(property)
            .setOperator(operator)
            .setValue(value)
            .build()

    private fun command(
        kind: ProbeKind,
        claims: List<ProbeClaim> = emptyList(),
        assertions: List<ProbeAssertion> = emptyList(),
        windowMs: Int = 300,
    ): ProbeCommand =
        ProbeCommand.newBuilder()
            .setKind(kind)
            .setSelector(selector())
            .addAllClaims(claims)
            .addAllAssertions(assertions)
            .setWindowMs(windowMs)
            .build()

    private fun collect(
        driver: FakeDriver,
        command: ProbeCommand,
    ): ProbeResult = runBlocking {
        ProbeCollector(driver, driver, System::currentTimeMillis)
            .collect(command, timeoutMs = 500)
    }

    private fun verdict(result: ProbeResult, claim: String): VerdictState =
        result.verdictsList.first { it.claim == claim }.verdict

    @Test
    fun playerUsesMediaSessionStateAndProperties() {
        val driver = FakeDriver().apply {
            addNode("piece", selector = "androidId:piece")
            media = MediaSnapshot(
                state = "Playing",
                positionSeconds = 12.5,
                durationSeconds = 120.0,
                rate = 1.0,
                volume = 0.8,
                muted = false,
                widthPx = 1920,
                heightPx = 1080,
            )
        }
        val result = collect(
            driver,
            command(
                ProbeKind.PROBE_KIND_PLAYER,
                claims = listOf(claim("state=Playing")),
                assertions = listOf(
                    assertion("position", ">=", "12"),
                    assertion("resolution", "=", "1080"),
                ),
            ),
        )
        assertEquals(VerdictState.VERDICT_STATE_PASS, verdict(result, "state=Playing"))
        assertEquals(VerdictState.VERDICT_STATE_PASS, verdict(result, "property.position"))
        assertEquals(VerdictState.VERDICT_STATE_PASS, verdict(result, "property.resolution"))
        assertTrue(result.evidenceList.any { it.source == "media_session" })
    }

    @Test
    fun playerWithoutASnapshotNeverInventsState() {
        val driver = FakeDriver().apply {
            addNode("piece", selector = "androidId:piece")
        }
        val result = collect(
            driver,
            command(
                ProbeKind.PROBE_KIND_PLAYER,
                claims = listOf(claim("state=Playing")),
            ),
        )
        assertEquals(VerdictState.VERDICT_STATE_UNVERIFIABLE, verdict(result, "state=Playing"))
        assertTrue(result.evidenceList.none { it.source == "media_session" })
    }

    @Test
    fun missingPieceContradictsStateAuthoritatively() {
        val driver = FakeDriver().apply {
            // The selector intentionally matches nothing.
            media = MediaSnapshot(state = "Playing")
        }
        val result = collect(
            driver,
            command(
                ProbeKind.PROBE_KIND_PLAYER,
                claims = listOf(claim("state=Playing")),
            ),
        )
        assertEquals(VerdictState.VERDICT_STATE_FAIL, verdict(result, "state=Playing"))
        assertTrue(
            result.evidenceList.any {
                it.source == "a11y_tree" &&
                    it.class_ == impresario.v1.Impresario.EvidenceClass.EVIDENCE_CLASS_AUTHORITATIVE
            },
        )
    }

    @Test
    fun drmClaimsUseMediaDrmEvidence() {
        val driver = FakeDriver().apply {
            addNode("piece", selector = "androidId:piece")
            drm = DrmSnapshot(enabled = true, provider = "Widevine")
        }
        val result = collect(
            driver,
            command(
                ProbeKind.PROBE_KIND_DRM,
                claims = listOf(
                    claim("drm.enabled=true"),
                    claim("drm.provider=Widevine"),
                ),
            ),
        )
        assertEquals(VerdictState.VERDICT_STATE_PASS, verdict(result, "drm.enabled=true"))
        assertEquals(VerdictState.VERDICT_STATE_PASS, verdict(result, "drm.provider=Widevine"))
        assertTrue(result.evidenceList.any { it.source == "media_drm" })
    }

    @Test
    fun hookOriginsKeepEvidenceProvenance() {
        val driver = FakeDriver().apply {
            addNode("piece", selector = "androidId:piece")
            media = MediaSnapshot(state = "Playing", origin = "app_hook")
            drm = DrmSnapshot(enabled = true, provider = "Widevine", origin = "app_hook")
        }
        val player = collect(
            driver,
            command(
                ProbeKind.PROBE_KIND_PLAYER,
                claims = listOf(claim("state=Playing")),
            ),
        )
        assertTrue(player.evidenceList.any { it.source == "app_hook" })
        assertTrue(player.evidenceList.none { it.source == "media_session" })

        val drm = collect(
            driver,
            command(
                ProbeKind.PROBE_KIND_DRM,
                claims = listOf(claim("drm.enabled=true"), claim("drm.provider=Widevine")),
            ),
        )
        assertTrue(drm.evidenceList.any { it.source == "app_hook" })
        assertTrue(drm.evidenceList.none { it.source == "media_drm" })
    }

    @Test
    fun showingCombinesCoverageVisibilityAndOptionalHook() {
        val driver = FakeDriver().apply {
            addNode("piece", selector = "androidId:piece", bounds = Rect(0, 0, 100, 100))
            coverage = 72.0
            showing = ShowingSnapshot(
                decoded = true,
                source = "https://cdn.example/movie.m3u8",
            )
        }
        val result = collect(
            driver,
            command(
                ProbeKind.PROBE_KIND_SHOWING,
                claims = listOf(
                    claim("showing=present"),
                    claim("showing.coverage>=50%"),
                    claim("showing.decoded=true"),
                    claim("showing.source~movie.m3u8"),
                ),
            ),
        )
        assertEquals(VerdictState.VERDICT_STATE_PASS, verdict(result, "showing=present"))
        assertEquals(VerdictState.VERDICT_STATE_PASS, verdict(result, "showing.coverage>=50%"))
        assertEquals(VerdictState.VERDICT_STATE_PASS, verdict(result, "showing.decoded=true"))
        assertEquals(VerdictState.VERDICT_STATE_PASS, verdict(result, "showing.source~movie.m3u8"))
        assertTrue(result.evidenceList.any { it.source == "screenshot_region" })
        assertTrue(result.evidenceList.any { it.source == "app_hook" })
    }

    @Test
    fun showingWithoutHookLeavesDecodedUnverifiable() {
        val driver = FakeDriver().apply {
            addNode("piece", selector = "androidId:piece")
            coverage = 60.0
        }
        val result = collect(
            driver,
            command(
                ProbeKind.PROBE_KIND_SHOWING,
                claims = listOf(claim("showing.decoded=true")),
            ),
        )
        assertEquals(VerdictState.VERDICT_STATE_UNVERIFIABLE, verdict(result, "showing.decoded=true"))
    }
}
