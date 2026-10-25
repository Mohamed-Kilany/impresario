//! The embedded CDP helper script (`window.__impresario`, spec 10.3).
//!
//! The script is **ES5 only** (old TV browsers run Chromium 38): no arrow
//! functions, `let`/`const`, template strings, `async`/`await`, optional
//! chaining or spread. [`tests::helper_script_is_es5`] enforces this in CI; an
//! ESLint `ecmaVersion: 5` lint is wired up in `scripts/lint-helper-es5.mjs`
//! for environments that have Node.js.
//!
//! It is injected idempotently before every command (spec 10.3: "check for the
//! object; inject if missing"; `addScriptToEvaluateOnNewDocument` is not
//! used).

/// The helper object. Assigns `window.__impresario` exactly once.
pub const HELPER_SCRIPT: &str = r##"window.__impresario = (function () {
  function trim(value) { return String(value == null ? '' : value).replace(/^\s+|\s+$/g, ''); }
  function collect(root, out) {
    if (root && root.nodeType === 1) { out.push(root); }
    var nodes = root.querySelectorAll('*');
    for (var i = 0; i < nodes.length; i++) {
      var node = nodes[i];
      out.push(node);
      if (node.shadowRoot) { collect(node.shadowRoot, out); }
    }
    return out;
  }
  function elements() { return collect(document.body || document.documentElement, []); }
  function cssList(value) {
    var all = elements();
    var out = [];
    for (var i = 0; i < all.length; i++) {
      if (all[i].matches && all[i].matches(value)) { out.push(all[i]); }
    }
    return out;
  }
  function xpathList(value) {
    var result = document.evaluate(value, document, null, 7, null);
    var out = [];
    for (var i = 0; i < result.snapshotLength; i++) { out.push(result.snapshotItem(i)); }
    return out;
  }
  function textList(value, contains) {
    var nodes = elements();
    var out = [];
    for (var i = 0; i < nodes.length; i++) {
      var text = trim(nodes[i].textContent);
      if (contains ? text.indexOf(value) !== -1 : text === value) { out.push(nodes[i]); }
    }
    var deepest = [];
    for (var a = 0; a < out.length; a++) {
      var hasChildMatch = false;
      for (var b = 0; b < out.length; b++) {
        if (a !== b && out[a] !== out[b] && out[a].contains(out[b])) { hasChildMatch = true; break; }
      }
      if (!hasChildMatch) { deepest.push(out[a]); }
    }
    return deepest;
  }
  function matches(strategy, value) {
    if (strategy === 'css') { return cssList(value); }
    if (strategy === 'xpath') { return xpathList(value); }
    if (strategy === 'text') { return textList(value, false); }
    if (strategy === 'text_contains') { return textList(value, true); }
    return [];
  }
  function find(strategy, value, index) {
    var list = matches(strategy, value);
    return list.length > index ? list[index] : null;
  }
  function count(strategy, value) { return matches(strategy, value).length; }
  function messageOf(error) { return String(error && error.message ? error.message : error); }
  function findSafe(strategy, value, index) {
    try { var list = matches(strategy, value); return { el: list.length > index ? list[index] : null, error: '' }; }
    catch (error) { return { el: null, error: messageOf(error) }; }
  }
  function countSafe(strategy, value) {
    try { return { count: matches(strategy, value).length, error: '' }; }
    catch (error) { return { count: -1, error: messageOf(error) }; }
  }
  function styleOf(el) {
    try { return window.getComputedStyle(el); } catch (error) { return null; }
  }
  function rectOf(el) {
    if (!el || !el.getBoundingClientRect) { return null; }
    var r = el.getBoundingClientRect();
    return { x: r.left, y: r.top, w: r.width, h: r.height };
  }
  function isVisible(el) {
    if (!el || !el.getBoundingClientRect) { return false; }
    var style = styleOf(el);
    if (!style) { return false; }
    if (style.display === 'none' || style.visibility !== 'visible') { return false; }
    if (parseFloat(style.opacity) <= 0) { return false; }
    var r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) { return false; }
    var vw = window.innerWidth || 0;
    var vh = window.innerHeight || 0;
    if (r.right <= 0 || r.bottom <= 0 || r.left >= vw || r.top >= vh) { return false; }
    return true;
  }
  function isEnabled(el) {
    if (!el) { return false; }
    if (el.disabled === true) { return false; }
    if (el.getAttribute && el.getAttribute('aria-disabled') === 'true') { return false; }
    return true;
  }
  function activeElementDeep() {
    var el = document.activeElement;
    while (el && el.shadowRoot && el.shadowRoot.activeElement) { el = el.shadowRoot.activeElement; }
    return el || document.body;
  }
  function isFocused(el) { return !!el && activeElementDeep() === el; }
  function textOf(el) {
    if (!el) { return ''; }
    var tag = String(el.tagName || '').toLowerCase();
    if (tag === 'input' || tag === 'textarea' || tag === 'select') {
      return el.value == null ? '' : String(el.value);
    }
    var text = el.innerText != null ? el.innerText : el.textContent;
    return trim(text);
  }
  function describe(el) {
    if (!el) { return ''; }
    var tag = String(el.tagName || '').toLowerCase();
    var id = el.id ? '#' + el.id : '';
    var cls = '';
    if (el.className && typeof el.className === 'string') {
      cls = '.' + trim(el.className).split(/\s+/).join('.');
    }
    return tag + id + cls;
  }
  function hitTest(el) {
    if (!el || !el.getBoundingClientRect) { return { ok: false, obscured_by: '' }; }
    var r = el.getBoundingClientRect();
    var x = r.left + r.width / 2;
    var y = r.top + r.height / 2;
    var vw = window.innerWidth || 0;
    var vh = window.innerHeight || 0;
    if (x < 0 || y < 0 || x > vw || y > vh) { return { ok: false, obscured_by: 'viewport' }; }
    var hit = document.elementFromPoint(x, y);
    if (!hit) { return { ok: false, obscured_by: '' }; }
    if (hit === el || el.contains(hit) || isComposedAncestor(hit, el)) { return { ok: true, obscured_by: '' }; }
    return { ok: false, obscured_by: describe(hit) };
  }
  function composedParent(node) {
    if (!node) { return null; }
    if (node.parentNode) { return node.parentNode; }
    if (node.host) { return node.host; }
    return null;
  }
  function isComposedAncestor(node, el) {
    var current = el;
    while (current) {
      if (current === node) { return true; }
      current = composedParent(current);
    }
    return false;
  }
  function scrollIntoView(el) {
    if (!el || !el.scrollIntoView) { return false; }
    try { el.scrollIntoView({ block: 'center', inline: 'center' }); }
    catch (error) { try { el.scrollIntoView(); } catch (error2) { return false; } }
    return true;
  }
  function focus(el) {
    if (!el || !el.focus) { return false; }
    try { el.focus(); } catch (error) { return false; }
    return isFocused(el);
  }
  function fire(el, type) {
    var event = null;
    try { event = new Event(type, { bubbles: true }); }
    catch (error) { event = document.createEvent('Event'); event.initEvent(type, true, false); }
    el.dispatchEvent(event);
  }
  function setValue(el, value, mode) {
    if (!el) { return null; }
    var current = el.value == null ? '' : String(el.value);
    var next = mode === 'append' ? current + value : (mode === 'clear' ? '' : value);
    var proto = null;
    if (window.HTMLInputElement && el instanceof window.HTMLInputElement) { proto = window.HTMLInputElement.prototype; }
    else if (window.HTMLTextAreaElement && el instanceof window.HTMLTextAreaElement) { proto = window.HTMLTextAreaElement.prototype; }
    var descriptor = proto ? Object.getOwnPropertyDescriptor(proto, 'value') : null;
    if (descriptor && descriptor.set) { descriptor.set.call(el, next); } else { el.value = next; }
    fire(el, 'input');
    fire(el, 'change');
    return el.value == null ? '' : String(el.value);
  }
  function selectAll(el) {
    if (!el) { return false; }
    if (el.select) { try { el.select(); return true; } catch (error) { } }
    var range = document.createRange();
    range.selectNodeContents(el);
    var selection = window.getSelection();
    if (!selection) { return false; }
    selection.removeAllRanges();
    selection.addRange(range);
    return true;
  }
  function collapseToEnd(el) {
    var range = document.createRange();
    range.selectNodeContents(el);
    range.collapse(false);
    var selection = window.getSelection();
    if (!selection) { return false; }
    selection.removeAllRanges();
    selection.addRange(range);
    return true;
  }
  function clearContent(el) {
    if (!el) { return false; }
    el.textContent = '';
    fire(el, 'input');
    fire(el, 'change');
    return true;
  }
  function editableKind(el) {
    if (!el) { return ''; }
    var tag = String(el.tagName || '').toLowerCase();
    if (tag === 'textarea') { return el.readOnly === true ? '' : 'textarea'; }
    if (tag === 'input') {
      if (el.readOnly === true) { return ''; }
      var type = String(el.type || 'text').toLowerCase();
      var textTypes = ['text', 'search', 'tel', 'url', 'email', 'password', 'number'];
      if (textTypes.indexOf(type) === -1) { return ''; }
      return 'input';
    }
    if (el.isContentEditable === true) { return 'contenteditable'; }
    return '';
  }
  function selectOption(el, text) {
    if (!el || !el.options) { return { found: false, value: null }; }
    for (var i = 0; i < el.options.length; i++) {
      var option = el.options[i];
      if (trim(option.textContent) === text) {
        el.value = option.value;
        fire(el, 'input');
        fire(el, 'change');
        return { found: true, value: option.value };
      }
    }
    return { found: false, value: null };
  }
  function primaryScrollable() {
    var nodes = elements();
    var best = null;
    var bestArea = 0;
    for (var i = 0; i < nodes.length; i++) {
      var el = nodes[i];
      if (el.scrollHeight > el.clientHeight + 4 || el.scrollWidth > el.clientWidth + 4) {
        var r = el.getBoundingClientRect();
        var area = r.width * r.height;
        if (area > bestArea) { bestArea = area; best = el; }
      }
    }
    return best;
  }
  function visibleExtent(target, vertical) {
    if (target) { return vertical ? target.clientHeight : target.clientWidth; }
    return vertical ? (window.innerHeight || 0) : (window.innerWidth || 0);
  }
  function scrollBy(target, direction, amount) {
    var dx = 0;
    var dy = 0;
    if (direction === 'up') { dy = -amount; }
    else if (direction === 'down') { dy = amount; }
    else if (direction === 'left') { dx = -amount; }
    else { dx = amount; }
    if (target) {
      if (dy !== 0) { target.scrollTop = target.scrollTop + dy; }
      if (dx !== 0) { target.scrollLeft = target.scrollLeft + dx; }
    } else {
      window.scrollBy(dx, dy);
    }
    return true;
  }
  function scrollInfo(target) {
    if (target) {
      return {
        top: target.scrollTop,
        max: Math.max(0, target.scrollHeight - target.clientHeight),
        left: target.scrollLeft,
        maxLeft: Math.max(0, target.scrollWidth - target.clientWidth)
      };
    }
    var doc = document.documentElement || document.body;
    var top = window.pageYOffset != null ? window.pageYOffset : doc.scrollTop;
    var left = window.pageXOffset != null ? window.pageXOffset : doc.scrollLeft;
    return {
      top: top,
      max: Math.max(0, doc.scrollHeight - (window.innerHeight || 0)),
      left: left,
      maxLeft: Math.max(0, doc.scrollWidth - (window.innerWidth || 0))
    };
  }
  function activeElement() { return activeElementDeep(); }
  function describeActive() {
    var el = activeElementDeep();
    if (!el || el === document.body) { return { present: false, label: '', frame: null }; }
    return { present: true, label: describe(el), frame: rectOf(el) };
  }
  return {
    find: find,
    count: count,
    findSafe: findSafe,
    countSafe: countSafe,
    isVisible: isVisible,
    isEnabled: isEnabled,
    isFocused: isFocused,
    textOf: textOf,
    rectOf: rectOf,
    hitTest: hitTest,
    scrollIntoView: scrollIntoView,
    focus: focus,
    setValue: setValue,
    selectAll: selectAll,
    collapseToEnd: collapseToEnd,
    clearContent: clearContent,
    editableKind: editableKind,
    selectOption: selectOption,
    primaryScrollable: primaryScrollable,
    visibleExtent: visibleExtent,
    scrollBy: scrollBy,
    scrollInfo: scrollInfo,
    activeElement: activeElement,
    describeActive: describeActive
  };
})();"##;

/// The idempotent injection expression (spec 10.3).
#[must_use]
pub fn inject_expression() -> String {
    format!("if (!window.__impresario) {{ {HELPER_SCRIPT} }} true")
}

/// The idempotent EME/AVPlay hook (TASK-3 section 6): captures key-system
/// requests and key-session lifecycle without changing application behavior.
/// ES5 only: it must run on the oldest supported TV Chromium.
pub const PROBE_HOOK_SCRIPT: &str = r##"(function () {
  if (window.__impresarioProbeHook && window.__impresarioProbeHook.version >= 1) { return true; }
  var state = {
    version: 1,
    requests: [],
    systems: [],
    sessions: []
  };
  window.__impresarioProbeHook = state;

  function providerOf(keySystem) {
    var value = String(keySystem || '');
    if (value.indexOf('widevine') !== -1) { return 'Widevine'; }
    if (value.indexOf('playready') !== -1) { return 'PlayReady'; }
    if (value.indexOf('fps') !== -1 || value.indexOf('fairplay') !== -1) { return 'FairPlay'; }
    if (value.indexOf('clearkey') !== -1) { return 'ClearKey'; }
    return value;
  }
  function robustnessOf(configs) {
    var found = '';
    try {
      for (var i = 0; i < configs.length; i++) {
        var video = configs[i].videoCapabilities || [];
        for (var j = 0; j < video.length; j++) {
          if (video[j].robustness) { found = String(video[j].robustness); }
        }
      }
    } catch (error) { }
    return found;
  }

  if (navigator.requestMediaKeySystemAccess) {
    var origRequest = navigator.requestMediaKeySystemAccess;
    navigator.requestMediaKeySystemAccess = function (keySystem, configs) {
      var robustness = robustnessOf(configs || []);
      var request = { keySystem: String(keySystem || ''), provider: providerOf(keySystem), robustness: robustness };
      state.requests.push(request);
      return origRequest.call(navigator, keySystem, configs).then(function (access) {
        request.granted = true;
        var origCreate = access.createMediaKeys;
        access.createMediaKeys = function () {
          return origCreate.call(access).then(function (keys) {
            var system = { keySystem: String(keySystem || ''), provider: providerOf(keySystem), robustness: robustness };
            state.systems.push(system);
            keys.__impresarioSystem = system;
            var origCreateSession = keys.createSession;
            keys.createSession = function (sessionType) {
              var session = origCreateSession.call(keys, sessionType);
              var record = {
                keySystem: system.keySystem,
                provider: system.provider,
                robustness: robustness,
                sessionType: String(sessionType || ''),
                license: 'none',
                keyStatus: ''
              };
              state.sessions.push(record);
              function readStatuses() {
                var values = [];
                try {
                  if (session.keyStatuses && session.keyStatuses.forEach) {
                    session.keyStatuses.forEach(function (status) { values.push(String(status)); });
                  }
                } catch (error) { }
                record.keyStatus = values.join(',');
                if (values.length) {
                  if (values.indexOf('expired') !== -1) { record.license = 'expired'; }
                  else if (values.indexOf('usable') !== -1) { record.license = 'active'; }
                }
              }
              function wrap(name, before, after) {
                var original = session[name];
                if (typeof original !== 'function') { return; }
                session[name] = function () {
                  if (before) { before(); }
                  var result = original.apply(session, arguments);
                  if (after && result && result.then) { result.then(after, function () { }); }
                  return result;
                };
              }
              wrap('generateRequest', function () { record.license = 'requesting'; }, function () { record.license = 'pending'; });
              wrap('update', function () { }, function () { record.license = 'active'; });
              try {
                session.addEventListener('keystatuseschange', readStatuses);
                session.addEventListener('message', function (event) {
                  if (record.license === 'none') { record.license = 'requesting'; }
                  record.messageType = String(event.messageType || '');
                });
                session.addEventListener('closed', function () { record.license = 'released'; });
              } catch (error) { }
              readStatuses();
              return session;
            };
            return keys;
          });
        };
        return access;
      });
    };
  }
  return true;
})()"##;

/// The probe sample expression: locates the piece once, then reports DOM,
/// media, image, EME and AVPlay state. Values are JSON-safe (infinities become
/// null and the companion booleans carry the semantics).
#[must_use]
pub fn probe_body(sel: &str) -> String {
    format!(
        r#"
var r = {find};
if (r.error) {{ return {{ok:false, invalid_selector:true, error:r.error}}; }}
var el = r.el;
if (!el) {{ return {{ok:true, found:false}}; }}
window.__impresario.scrollIntoView(el);
var tag = String(el.tagName || '').toLowerCase();
var media = null;
if (tag === 'video' || tag === 'audio') {{ media = el; }}
else if (el.querySelector) {{ media = el.querySelector('video,audio'); }}
if (!media && el.closest) {{ media = el.closest('video,audio'); }}
var image = null;
if (tag === 'img') {{ image = el; }}
else if (el.querySelector) {{ image = el.querySelector('img'); }}
var rect = window.__impresario.rectOf(el);
var hit = window.__impresario.hitTest(el);
var background = '';
try {{ background = String(window.getComputedStyle(el).backgroundImage || ''); }} catch (error) {{ }}
var mediaOut = null;
if (media) {{
  var bufferedEnd = 0;
  try {{
    if (media.buffered && media.buffered.length) {{
      bufferedEnd = media.buffered.end(media.buffered.length - 1);
    }}
  }} catch (error) {{ }}
  var quality = null;
  try {{ if (media.getVideoPlaybackQuality) {{ quality = media.getVideoPlaybackQuality(); }} }} catch (error) {{ }}
  var selectedAudio = '';
  try {{
    if (media.audioTracks) {{
      for (var a = 0; a < media.audioTracks.length; a++) {{
        if (media.audioTracks[a].enabled) {{ selectedAudio = String(media.audioTracks[a].label || media.audioTracks[a].language || ''); }}
      }}
    }}
  }} catch (error) {{ }}
  var subtitle = '';
  try {{
    if (media.textTracks) {{
      for (var t = 0; t < media.textTracks.length; t++) {{
        if (media.textTracks[t].mode === 'showing') {{ subtitle = String(media.textTracks[t].label || media.textTracks[t].language || ''); }}
      }}
    }}
  }} catch (error) {{ }}
  mediaOut = {{
    paused: media.paused === true,
    ended: media.ended === true,
    readyState: Number(media.readyState || 0),
    networkState: Number(media.networkState || 0),
    currentTime: Number(media.currentTime || 0),
    duration: (isFinite(media.duration) ? Number(media.duration) : null),
    live: !isFinite(media.duration),
    bufferedEnd: Number(bufferedEnd || 0),
    seeking: media.seeking === true,
    rate: Number(media.playbackRate || 0),
    volume: Number(media.volume == null ? 1 : media.volume),
    muted: media.muted === true,
    width: Number(media.videoWidth || 0),
    height: Number(media.videoHeight || 0),
    droppedFrames: Number(quality ? quality.droppedVideoFrames : 0),
    totalFrames: Number(quality ? quality.totalVideoFrames : 0),
    audioTracks: Number(media.audioTracks ? media.audioTracks.length : 0),
    textTracks: Number(media.textTracks ? media.textTracks.length : 0),
    audioTrack: selectedAudio,
    subtitleTrack: subtitle,
    src: String(media.currentSrc || media.src || ''),
    error: (media.error ? String(media.error.code || '') + ':' + String(media.error.message || '') : ''),
    mediaKeys: media.mediaKeys != null,
    looping: media.loop === true,
    fullscreen: !!(document.fullscreenElement && (document.fullscreenElement === media || (document.fullscreenElement.contains && document.fullscreenElement.contains(media)))),
    objectFit: String(media.style && media.style.objectFit || '')
  }};
}}
var imageOut = null;
if (image) {{
  imageOut = {{
    complete: image.complete === true,
    naturalWidth: Number(image.naturalWidth || 0),
    naturalHeight: Number(image.naturalHeight || 0),
    src: String(image.currentSrc || image.src || '')
  }};
}}
var avplayOut = null;
try {{
  if (window.webapis && window.webapis.avplay) {{
    var av = window.webapis.avplay;
    avplayOut = {{
      present: true,
      state: String(av.getState ? av.getState() : ''),
      currentTime: Number(av.getCurrentTime ? av.getCurrentTime() : 0),
      duration: Number(av.getDuration ? av.getDuration() : 0),
      rate: Number(av.getPlaybackRate ? av.getPlaybackRate() : 0)
    }};
  }}
}} catch (error) {{ avplayOut = null; }}
var hook = window.__impresarioProbeHook || null;
var appHookOut = null;
try {{
  var appHook = window.__impresarioHook;
  if (appHook && appHook.snapshot && typeof appHook.snapshot === 'object') {{
    appHookOut = appHook.snapshot;
  }}
}} catch (error) {{ appHookOut = null; }}
var eme = {{
  available: hook !== null,
  requests: hook ? hook.requests.length : 0,
  systems: hook ? hook.systems.length : 0,
  sessions: hook ? hook.sessions.length : 0,
  keySystem: '',
  provider: '',
  robustness: '',
  keyStatus: '',
  license: 'none',
  enabled: !!(media && media.mediaKeys != null)
}};
if (hook && hook.systems.length) {{
  eme.keySystem = hook.systems[hook.systems.length - 1].keySystem;
  eme.provider = hook.systems[hook.systems.length - 1].provider;
  eme.robustness = hook.systems[hook.systems.length - 1].robustness;
  eme.enabled = true;
}} else if (hook && hook.requests.length) {{
  eme.keySystem = hook.requests[hook.requests.length - 1].keySystem;
  eme.provider = hook.requests[hook.requests.length - 1].provider;
  eme.robustness = hook.requests[hook.requests.length - 1].robustness;
  eme.enabled = true;
}}
if (hook) {{
  for (var i = 0; i < hook.sessions.length; i++) {{
    var session = hook.sessions[i];
    if (session.keyStatus) {{ eme.keyStatus = session.keyStatus; }}
    if (session.license && session.license !== 'none') {{ eme.license = session.license; }}
  }}
}}
if (media && media.mediaKeys) {{ eme.enabled = true; }}
if (eme.robustness) {{
  if (/HW_SECURE_ALL|HW_SECURE_DECODE|HW_SECURE_CRYPTO/.test(eme.robustness)) {{ eme.securityLevel = 'L1'; }}
  else if (/SW_SECURE_CRYPTO|SW_SECURE_DECODE/.test(eme.robustness)) {{ eme.securityLevel = 'L3'; }}
  else {{ eme.securityLevel = eme.robustness; }}
}} else {{ eme.securityLevel = ''; }}
return {{
  ok:true,
  found:true,
  visible:window.__impresario.isVisible(el),
  hit:hit.ok,
  obscured_by:String(hit.obscured_by || ''),
  tag:tag,
  rect:rect,
  viewport:{{ width:Number(window.innerWidth || 0), height:Number(window.innerHeight || 0), dpr:Number(window.devicePixelRatio || 1) }},
  background:background,
  media:mediaOut,
  image:imageOut,
  avplay:avplayOut,
  eme:eme,
  app_hook:appHookOut
}};
"#,
        find = sel
    )
}

/// Wrap a JS body in an IIFE with a top-level `try`/`catch` so no helper
/// failure ever escapes as a CDP exception:
/// `(function(){try{ <body> }catch(e){return {ok:false,error:String(e)}}})()`.
/// The body must `return` an object with an `ok` field.
#[must_use]
pub fn guarded(body: &str) -> String {
    format!("(function(){{try{{{body}}}catch(e){{return {{ok:false,error:String(e)}};}}}})()")
}

/// A JSON string as a JS string literal (for embedding values in expressions).
#[must_use]
pub fn js_string(value: &str) -> String {
    serde_json::Value::String(value.to_string()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_script_is_es5() {
        // Tokens that cannot appear in an ES5-only script. The check is
        // deliberately textual: it guards against the exact features spec
        // 10.3 bans (arrow functions, block scoping, template strings,
        // async/await, optional chaining, spread).
        let banned = ["=>", "`", "const ", "let ", "async ", "await ", "?.", "..."];
        for token in banned {
            assert!(
                !HELPER_SCRIPT.contains(token),
                "ES5 violation: helper script contains {token:?}"
            );
        }
    }

    #[test]
    fn helper_exposes_the_spec_surface() {
        for name in [
            "find",
            "count",
            "isVisible",
            "isEnabled",
            "isFocused",
            "textOf",
            "rectOf",
            "hitTest",
            "scrollIntoView",
            "focus",
            "setValue",
            "selectOption",
            "scrollBy",
        ] {
            assert!(
                HELPER_SCRIPT.contains(&format!("{name}: {name}")),
                "helper script does not export {name}"
            );
        }
    }

    #[test]
    fn injection_is_conditional() {
        let expression = inject_expression();
        assert!(expression.contains("if (!window.__impresario)"));
        assert!(expression.ends_with("true"));
    }

    #[test]
    fn probe_body_reads_the_optional_app_hook() {
        let body = probe_body("window.__impresario.find('css', '#video', 0, false)");
        assert!(body.contains("window.__impresarioHook"));
        assert!(body.contains("app_hook:appHookOut"));
        assert!(body.contains("window.__impresarioProbeHook"));
    }

    #[test]
    fn guarded_wraps_in_try_catch() {
        let expression = guarded("return {ok:true};");
        assert!(expression.starts_with("(function(){try{"));
        assert!(expression.contains("catch(e)"));
    }

    #[test]
    fn js_string_escapes_quotes() {
        assert_eq!(js_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(js_string("x\ny"), "\"x\\ny\"");
    }
}
