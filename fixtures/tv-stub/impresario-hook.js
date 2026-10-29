/*!
 * Impresario in-app test hook — web SDK (debug builds only).
 *
 * Add this file before the application script, then report the real player
 * state whenever it changes:
 *
 *   ImpresarioHook.player({ state: "Playing", positionSeconds: 12.5,
 *                           durationSeconds: 120, rate: 1, volume: 1,
 *                           muted: false, widthPx: 1920, heightPx: 1080,
 *                           decoded: true, source: "https://…/movie.m3u8" });
 *   ImpresarioHook.drm({ enabled: true, provider: "Widevine",
 *                        license: "active", keyStatus: "usable",
 *                        securityLevel: "L1" });
 *   ImpresarioHook.showing({ decoded: true, source: "…", piece: "player" });
 *
 * The connector reads `window.__impresarioHook.snapshot` and labels it
 * AUTHORITATIVE (`source: app_hook`). Missing fields stay UNVERIFIABLE; they
 * are never guessed. The hook does not change playback behavior.
 *
 * ES5 only: it must run on old TV Chromium builds.
 */
(function (global) {
    'use strict';
    var state = { version: 1, snapshot: { media: null, drm: null, showing: null } };

    function merge(key, value) {
        if (!value || typeof value !== 'object') {
            return;
        }
        var current = state.snapshot[key] || {};
        for (var name in value) {
            if (Object.prototype.hasOwnProperty.call(value, name)) {
                current[name] = value[name];
            }
        }
        state.snapshot[key] = current;
        global.__impresarioHook = state;
    }

    var api = {
        version: 1,
        player: function (value) {
            merge('media', value);
        },
        media: function (value) {
            merge('media', value);
        },
        drm: function (value) {
            merge('drm', value);
        },
        showing: function (value) {
            merge('showing', value);
        },
        clear: function () {
            state.snapshot = { media: null, drm: null, showing: null };
            global.__impresarioHook = state;
        },
        snapshot: function () {
            return state.snapshot;
        }
    };

    global.ImpresarioHook = api;
    global.__impresarioHook = state;
}(typeof window !== 'undefined' ? window : this));
