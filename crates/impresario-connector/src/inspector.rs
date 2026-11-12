//! Connector-side inspector acquisition (TASK-INSPECTOR section 4).
//!
//! The connector advertises `Capability::INSPECTOR` and serves the additive
//! unary `Inspector` RPCs. Acquisition is viewport-only and happens inside the
//! page: an ES5 script walks the composed DOM, computes clipped visibility and
//! drops off-screen subtrees before the JSON ever crosses the wire.
//!
//! Input is deliberately small: tap/scroll/type/key/swipe map onto the same
//! CDP surfaces the command executor already uses. Nothing here needs root,
//! jailbreak or an external runtime tool.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};

use impresario_core::platforms::Platform;
use impresario_locator::role::role_from_web_tag;
use impresario_proto::v1 as pb;
use pb::inspector_server::Inspector;

use crate::backends::{MouseEvent, MouseEventType};
use crate::errors::ConnectorError;
use crate::exec::Executor;
use crate::server::{authorize_token, session_from_metadata, SessionManager};

/// Default hard cap when `max_nodes` is left at zero.
pub const DEFAULT_MAX_NODES: u32 = 5_000;
/// Default depth cap when `max_depth` is left at zero.
pub const DEFAULT_MAX_DEPTH: u32 = 100;

/// The ES5-only inspector helper. Assigns `window.__impresarioInspector`
/// exactly once and exposes `snapshot`, `tap`, `scroll` and `prepareType`.
///
/// The script must run on Chromium 38 (old TV browsers): no arrow functions,
/// block scoping, template strings, `async`/`await`, optional chaining or
/// spread. `inspector_script_is_es5` enforces this in CI.
pub const INSPECTOR_SCRIPT: &str = r##"window.__impresarioInspector = (function () {
  if (window.__impresarioInspector && window.__impresarioInspector.version >= 1) {
    return window.__impresarioInspector;
  }
  var state = { nextId: 1, byId: {}, includeRaw: false };

  function trim(value) { return String(value == null ? '' : value).replace(/^\s+|\s+$/g, ''); }
  function lower(value) { return String(value == null ? '' : value).toLowerCase(); }
  function num(value) { var n = Number(value); return isFinite(n) ? n : 0; }
  function styleOf(el) { try { return window.getComputedStyle(el); } catch (error) { return null; } }
  function rectOf(el) {
    if (!el || !el.getBoundingClientRect) { return { x: 0, y: 0, w: 0, h: 0 }; }
    var r = el.getBoundingClientRect();
    return { x: num(r.left), y: num(r.top), w: num(r.width), h: num(r.height) };
  }
  function intersect(a, b) {
    var x1 = Math.max(a.x, b.x);
    var y1 = Math.max(a.y, b.y);
    var x2 = Math.min(a.x + a.w, b.x + b.w);
    var y2 = Math.min(a.y + a.h, b.y + b.h);
    return { x: x1, y: y1, w: Math.max(0, x2 - x1), h: Math.max(0, y2 - y1) };
  }
  function fractionOf(rect, clip) {
    if (rect.w <= 0 || rect.h <= 0) { return 0; }
    var c = intersect(rect, clip);
    return (c.w * c.h) / (rect.w * rect.h);
  }
  function composedParent(node) {
    if (!node) { return null; }
    if (node.parentNode) { return node.parentNode; }
    if (node.host) { return node.host; }
    return null;
  }
  function idOf(el) {
    if (!el.__impresarioInspectorId) {
      el.__impresarioInspectorId = state.nextId;
      state.nextId += 1;
    }
    return el.__impresarioInspectorId;
  }
  function activeElementDeep() {
    var el = document.activeElement;
    while (el && el.shadowRoot && el.shadowRoot.activeElement) {
      el = el.shadowRoot.activeElement;
    }
    return el || document.body;
  }
  function isHidden(el) {
    if (!el) { return true; }
    if (el.hidden === true) { return true; }
    if (el.getAttribute && el.getAttribute('aria-hidden') === 'true') { return true; }
    var s = styleOf(el);
    if (!s) { return false; }
    if (s.display === 'none' || s.visibility === 'hidden' || s.visibility === 'collapse') { return true; }
    if (parseFloat(s.opacity) === 0) { return true; }
    return false;
  }
  function isEnabled(el) {
    if (!el) { return false; }
    if (el.disabled === true) { return false; }
    if (el.getAttribute && el.getAttribute('aria-disabled') === 'true') { return false; }
    return true;
  }
  function isSecure(el) {
    if (!el) { return false; }
    var tag = lower(el.tagName);
    if (tag !== 'input') { return false; }
    return lower(el.type || 'text') === 'password';
  }
  function isReadOnly(el) {
    if (!el) { return false; }
    if (el.readOnly === true) { return true; }
    return !!(el.getAttribute && el.getAttribute('aria-readonly') === 'true');
  }
  function isRequired(el) {
    if (!el) { return false; }
    if (el.required === true) { return true; }
    return !!(el.getAttribute && el.getAttribute('aria-required') === 'true');
  }
  function ariaBool(el, name) {
    if (!el || !el.getAttribute) { return null; }
    var value = el.getAttribute(name);
    if (value === 'true') { return true; }
    if (value === 'false') { return false; }
    return null;
  }
  function checkedOf(el) {
    if (!el) { return null; }
    var tag = lower(el.tagName);
    if (tag === 'input') {
      var type = lower(el.type || 'text');
      if (type === 'checkbox' || type === 'radio') { return el.checked === true; }
    }
    return ariaBool(el, 'aria-checked');
  }
  function labelTextFor(el) {
    if (!el) { return ''; }
    var id = el.id;
    if (id) {
      var labels = null;
      try {
        labels = document.querySelectorAll('label[for="' + String(id).replace(/\\/g, '\\\\').replace(/"/g, '\\"') + '"]');
      } catch (error) { labels = null; }
      if (labels && labels.length) {
        var parts = [];
        for (var i = 0; i < labels.length; i++) {
          parts.push(trim(labels[i].textContent));
        }
        var joined = trim(parts.join(' '));
        if (joined) { return joined; }
      }
    }
    var current = el.parentNode;
    while (current) {
      if (current.nodeType === 1 && lower(current.tagName) === 'label') {
        return trim(current.textContent);
      }
      current = composedParent(current);
    }
    return '';
  }
  function labelledBy(el) {
    if (!el || !el.getAttribute) { return ''; }
    var ids = trim(el.getAttribute('aria-labelledby'));
    if (!ids) { return ''; }
    var list = ids.split(/\s+/);
    var parts = [];
    for (var i = 0; i < list.length; i++) {
      var target = document.getElementById(list[i]);
      if (target) { parts.push(trim(target.textContent)); }
    }
    return trim(parts.join(' '));
  }
  function nameOf(el) {
    if (!el) { return ''; }
    var aria = trim(el.getAttribute ? el.getAttribute('aria-label') : '');
    if (aria) { return aria; }
    var labelled = labelledBy(el);
    if (labelled) { return labelled; }
    var tag = lower(el.tagName);
    if (tag === 'img') {
      return trim(el.getAttribute('alt')) || trim(el.getAttribute('title'));
    }
    if (tag === 'input' || tag === 'textarea' || tag === 'select') {
      var label = labelTextFor(el);
      if (label) { return label; }
      var placeholder = trim(el.getAttribute('placeholder'));
      if (placeholder) { return placeholder; }
      return trim(el.getAttribute('title'));
    }
    if (tag === 'button' || tag === 'a' || /^h[1-6]$/.test(tag)) {
      var text = trim(el.innerText != null ? el.innerText : el.textContent);
      return text.length > 200 ? text.substring(0, 200) : text;
    }
    return trim(el.getAttribute ? el.getAttribute('title') : '');
  }
  function textOf(el) {
    if (!el) { return { text: '', truncated: false }; }
    var tag = lower(el.tagName);
    if (tag === 'input' || tag === 'textarea' || tag === 'select') {
      return { text: '', truncated: false };
    }
    var raw = el.innerText != null ? el.innerText : el.textContent;
    var text = trim(raw);
    if (text.length > 1000) {
      return { text: text.substring(0, 1000), truncated: true };
    }
    return { text: text, truncated: false };
  }
  function valueOf(el) {
    if (!el || isSecure(el)) { return ''; }
    var tag = lower(el.tagName);
    if (tag === 'input' || tag === 'textarea' || tag === 'select') {
      return el.value == null ? '' : String(el.value);
    }
    if (el.isContentEditable === true) {
      return trim(el.innerText != null ? el.innerText : el.textContent);
    }
    return '';
  }
  function testIdOf(el) {
    if (!el || !el.getAttribute) { return ''; }
    var names = ['data-testid', 'data-test-id', 'data-test', 'data-qa', 'data-cy'];
    for (var i = 0; i < names.length; i++) {
      var value = trim(el.getAttribute(names[i]));
      if (value) { return value; }
    }
    return '';
  }
  function cssEscape(value) {
    return String(value).replace(/([^a-zA-Z0-9_-])/g, '\\$1');
  }
  function cssOf(el) {
    if (!el || !el.tagName) { return ''; }
    try {
      if (el.getRootNode && el.getRootNode() !== document) { return ''; }
    } catch (error) { return ''; }
    var id = el.id;
    if (id) {
      var byId = null;
      try { byId = document.querySelectorAll('#' + cssEscape(id)); } catch (error) { byId = null; }
      if (byId && byId.length === 1) { return '#' + cssEscape(id); }
    }
    var parts = [];
    var current = el;
    while (current && current.nodeType === 1 && current !== document.documentElement) {
      var tag = lower(current.tagName);
      var parent = current.parentNode;
      if (!parent) { break; }
      var index = 1;
      var siblings = parent.children || [];
      for (var i = 0; i < siblings.length; i++) {
        if (siblings[i] === current) { break; }
        if (lower(siblings[i].tagName) === tag) { index += 1; }
      }
      parts.unshift(tag + ':nth-of-type(' + index + ')');
      current = parent;
    }
    if (!parts.length) { return ''; }
    return parts.join(' > ');
  }
  function xpathOf(el) {
    if (!el || !el.tagName) { return ''; }
    try {
      if (el.getRootNode && el.getRootNode() !== document) { return ''; }
    } catch (error) { return ''; }
    var parts = [];
    var current = el;
    while (current && current.nodeType === 1 && current !== document.documentElement) {
      var tag = lower(current.tagName);
      var parent = current.parentNode;
      if (!parent) { break; }
      var index = 1;
      var siblings = parent.children || [];
      for (var i = 0; i < siblings.length; i++) {
        if (siblings[i] === current) { break; }
        if (lower(siblings[i].tagName) === tag) { index += 1; }
      }
      parts.unshift(tag + '[' + index + ']');
      current = parent;
    }
    if (!parts.length) { return ''; }
    return '//' + parts.join('/');
  }
  function scrollOf(el) {
    if (!el) { return null; }
    var sh = el.scrollHeight || 0;
    var ch = el.clientHeight || 0;
    var sw = el.scrollWidth || 0;
    var cw = el.clientWidth || 0;
    var vertical = sh > ch + 4;
    var horizontal = sw > cw + 4;
    if (!vertical && !horizontal) { return null; }
    return {
      canScrollBackward: (vertical && el.scrollTop > 0) || (horizontal && el.scrollLeft > 0),
      canScrollForward: (vertical && el.scrollTop + ch < sh - 1) || (horizontal && el.scrollLeft + cw < sw - 1),
      axis: vertical && horizontal ? 'both' : (vertical ? 'vertical' : 'horizontal')
    };
  }
  function clipOf(el) {
    var clip = { x: 0, y: 0, w: window.innerWidth || 0, h: window.innerHeight || 0 };
    var escapes = false;
    var own = styleOf(el);
    if (own && (own.position === 'fixed' || own.position === 'sticky')) { escapes = true; }
    var current = composedParent(el);
    var guard = 0;
    while (current && current !== document.documentElement && guard < 256) {
      if (current.nodeType === 1) {
        var s = styleOf(current);
        if (s) {
          if (s.position === 'fixed' || s.position === 'sticky') { escapes = true; }
          if (s.transform && s.transform !== 'none') { escapes = true; }
          if (!escapes && s.overflow && s.overflow !== 'visible') {
            var r = current.getBoundingClientRect();
            clip = intersect(clip, { x: num(r.left), y: num(r.top), w: num(r.width), h: num(r.height) });
          }
        }
      }
      current = composedParent(current);
      guard += 1;
    }
    return { clip: clip, escapes: escapes };
  }
  function rawOf(el, tag, inputType) {
    var raw = { tag: tag };
    if (inputType) { raw.type = inputType; }
    if (el.id) { raw.id = String(el.id); }
    if (el.className && typeof el.className === 'string') { raw.classes = trim(el.className); }
    if (el.getAttribute) {
      var name = trim(el.getAttribute('name'));
      if (name) { raw.name = name; }
      var href = trim(el.getAttribute('href'));
      if (href) { raw.href = href; }
      var src = trim(el.getAttribute('src'));
      if (src) { raw.src = src; }
      var aria = trim(el.getAttribute('role'));
      if (aria) { raw.ariaRole = aria; }
    }
    return raw;
  }
  function buildEntry(el, parentId, depth) {
    var tag = lower(el.tagName);
    var inputType = tag === 'input' ? lower(el.type || 'text') : '';
    var rect = rectOf(el);
    var clipInfo = clipOf(el);
    var textInfo = textOf(el);
    var scroll = scrollOf(el);
    var entry = {
      id: idOf(el),
      parent: parentId,
      depth: depth,
      tag: tag,
      inputType: inputType,
      role: el.getAttribute ? trim(el.getAttribute('role')) : '',
      name: nameOf(el),
      text: textInfo.text,
      textTruncated: textInfo.truncated,
      value: valueOf(el),
      label: labelTextFor(el),
      description: trim(el.getAttribute ? el.getAttribute('aria-description') : ''),
      placeholder: tag === 'input' || tag === 'textarea' ? trim(el.getAttribute('placeholder')) : '',
      alt: tag === 'img' ? trim(el.getAttribute('alt')) : '',
      title: trim(el.getAttribute ? el.getAttribute('title') : ''),
      testId: testIdOf(el),
      nativeId: trim(el.id),
      css: cssOf(el),
      xpath: xpathOf(el),
      x: rect.x,
      y: rect.y,
      w: rect.w,
      h: rect.h,
      fraction: fractionOf(rect, clipInfo.clip),
      focused: activeElementDeep() === el,
      enabled: isEnabled(el),
      selected: ariaBool(el, 'aria-selected') === true,
      checked: checkedOf(el),
      expanded: ariaBool(el, 'aria-expanded'),
      pressed: ariaBool(el, 'aria-pressed'),
      readonly: isReadOnly(el),
      required: isRequired(el),
      hidden: isHidden(el),
      secure: isSecure(el),
      systemUi: false,
      scrollable: scroll !== null,
      canScrollBackward: scroll ? scroll.canScrollBackward : false,
      canScrollForward: scroll ? scroll.canScrollForward : false,
      scrollAxis: scroll ? scroll.axis : '',
      escapesClip: clipInfo.escapes,
      childrenHiddenCount: 0,
      raw: state.includeRaw ? rawOf(el, tag, inputType) : {}
    };
    return entry;
  }
  function skipTag(tag) {
    return tag === 'head' || tag === 'script' || tag === 'style' || tag === 'meta' ||
      tag === 'link' || tag === 'title' || tag === 'noscript' || tag === 'template';
  }
  function collect(maxDepth) {
    var entries = [];
    var visited = 0;
    var truncated = false;
    state.byId = {};
    function walk(root, parentId, depth) {
      if (depth > maxDepth) { return; }
      var children = root.children || [];
      for (var i = 0; i < children.length; i++) {
        if (visited >= 20000) { truncated = true; return; }
        var el = children[i];
        var tag = lower(el.tagName);
        if (skipTag(tag)) { continue; }
        visited += 1;
        var entry = buildEntry(el, parentId, depth);
        entries.push(entry);
        state.byId[entry.id] = el;
        if (el.shadowRoot) {
          walk(el.shadowRoot, entry.id, depth + 1);
        } else {
          walk(el, entry.id, depth + 1);
        }
      }
    }
    walk(document.documentElement, 0, 0);
    return { entries: entries, truncated: truncated };
  }
  function snapshot(options) {
    options = options || {};
    var minFraction = num(options.minVisibleFraction);
    var maxDepth = num(options.maxDepth);
    if (maxDepth <= 0) { maxDepth = 100; }
    state.includeRaw = options.includeRaw === true;
    var collected = collect(maxDepth);
    var entries = collected.entries;
    var byId = {};
    var i;
    for (i = 0; i < entries.length; i++) { byId[entries[i].id] = entries[i]; }
    var keep = {};
    var focusedId = 0;
    for (i = 0; i < entries.length; i++) {
      if (entries[i].focused) { focusedId = entries[i].id; }
    }
    for (i = 0; i < entries.length; i++) {
      var entry = entries[i];
      var kept = false;
      if (entry.focused) {
        kept = true;
      } else if (!entry.hidden && entry.fraction > 0 && (minFraction <= 0 || entry.fraction >= minFraction)) {
        kept = true;
      }
      if (kept) {
        keep[entry.id] = true;
        var parent = entry.parent;
        var guard = 0;
        while (parent && !keep[parent] && guard < 512) {
          keep[parent] = true;
          parent = byId[parent] ? byId[parent].parent : 0;
          guard += 1;
        }
      }
    }
    for (i = 0; i < entries.length; i++) { entries[i].childrenHiddenCount = 0; }
    for (i = 0; i < entries.length; i++) {
      var child = entries[i];
      if (child.parent && keep[child.parent] && !keep[child.id] && byId[child.parent]) {
        byId[child.parent].childrenHiddenCount += 1;
      }
    }
    var out = [];
    var registry = {};
    for (i = 0; i < entries.length; i++) {
      if (keep[entries[i].id]) {
        out.push(entries[i]);
        registry[entries[i].id] = state.byId[entries[i].id];
      }
    }
    state.byId = registry;
    return {
      ok: true,
      w: num(window.innerWidth),
      h: num(window.innerHeight),
      dpr: num(window.devicePixelRatio || 1),
      nodes: out,
      focused: focusedId,
      truncated: collected.truncated
    };
  }
  function elementById(id) { return state.byId[id] || null; }
  function scrollIntoView(el) {
    if (!el || !el.scrollIntoView) { return false; }
    try { el.scrollIntoView({ block: 'center', inline: 'center' }); }
    catch (error) { try { el.scrollIntoView(); } catch (error2) { return false; } }
    return true;
  }
  function clickElement(el) {
    if (!el) { return { ok: false, error: 'the node is no longer present' }; }
    scrollIntoView(el);
    try { if (el.focus) { el.focus(); } } catch (error) { }
    try {
      if (el.click) { el.click(); return { ok: true }; }
    } catch (error) { }
    try {
      var event = null;
      try { event = new MouseEvent('click', { bubbles: true, cancelable: true, view: window }); }
      catch (error) { event = document.createEvent('MouseEvents'); event.initEvent('click', true, true); }
      el.dispatchEvent(event);
      return { ok: true };
    } catch (error) { return { ok: false, error: String(error) }; }
  }
  function tap(id, x, y) {
    var el = null;
    if (id) { el = elementById(id); }
    if (!el && x !== null && y !== null) { el = document.elementFromPoint(x, y); }
    return clickElement(el);
  }
  function scroll(direction, id) {
    var el = id ? elementById(id) : null;
    var helper = window.__impresario;
    var target = el || (helper ? helper.primaryScrollable() : null);
    var vertical = direction === 'up' || direction === 'down';
    var amount = 0;
    if (target) {
      amount = 0.8 * (vertical ? target.clientHeight : target.clientWidth);
    } else {
      amount = 0.8 * (vertical ? (window.innerHeight || 0) : (window.innerWidth || 0));
    }
    var forward = direction === 'down' || direction === 'right';
    if (target) {
      if (vertical) { target.scrollTop = target.scrollTop + (forward ? amount : -amount); }
      else { target.scrollLeft = target.scrollLeft + (forward ? amount : -amount); }
    } else if (vertical) {
      window.scrollBy(0, forward ? amount : -amount);
    } else {
      window.scrollBy(forward ? amount : -amount, 0);
    }
    return { ok: true };
  }
  function prepareType(id) {
    var el = id ? elementById(id) : activeElementDeep();
    if (!el) { return { ok: false, error: 'the node is no longer present' }; }
    scrollIntoView(el);
    try { if (el.focus) { el.focus(); } } catch (error) { }
    try {
      if (el.setSelectionRange && el.value != null) {
        var length = String(el.value).length;
        el.setSelectionRange(length, length);
      } else if (el.isContentEditable) {
        var range = document.createRange();
        range.selectNodeContents(el);
        range.collapse(false);
        var selection = window.getSelection();
        if (selection) {
          selection.removeAllRanges();
          selection.addRange(range);
        }
      }
    } catch (error) { }
    return { ok: true };
  }
  return {
    version: 1,
    snapshot: snapshot,
    tap: tap,
    scroll: scroll,
    prepareType: prepareType
  };
})();"##;

/// The idempotent injection expression.
#[must_use]
pub fn inject_expression() -> String {
    format!(
        "if (!window.__impresarioInspector || window.__impresarioInspector.version < 1) {{ {INSPECTOR_SCRIPT} }} true"
    )
}

/// The tonic `Inspector` service. It shares the `Agent` service's
/// [`SessionManager`], so an inspector attaches to a live handshake session
/// with the same token and session-id metadata.
pub struct InspectorService {
    manager: std::sync::Arc<SessionManager>,
    token: Option<String>,
}

impl InspectorService {
    #[must_use]
    pub fn new(manager: std::sync::Arc<SessionManager>, token: Option<String>) -> Self {
        Self {
            manager,
            token: token.filter(|value| !value.is_empty()),
        }
    }
}

#[tonic::async_trait]
impl Inspector for InspectorService {
    async fn snapshot(
        &self,
        request: Request<pb::InspectorSnapshotRequest>,
    ) -> Result<Response<pb::InspectorSnapshotResponse>, Status> {
        let metadata = request.metadata().clone();
        authorize_token(self.token.as_deref(), &metadata, "")?;
        let handle = session_from_metadata(&self.manager, &metadata)?;
        let response = match handle.session.inspector_snapshot(request.get_ref()).await {
            Ok(response) => response,
            Err(error) => pb::InspectorSnapshotResponse {
                rev: 0,
                device_name: handle.session.device.model.clone(),
                platform: platform_to_proto(handle.session.platform) as i32,
                error: Some(error.to_proto()),
                ..pb::InspectorSnapshotResponse::default()
            },
        };
        Ok(Response::new(response))
    }

    async fn input(
        &self,
        request: Request<pb::InspectorInputRequest>,
    ) -> Result<Response<pb::InspectorInputResponse>, Status> {
        let metadata = request.metadata().clone();
        authorize_token(self.token.as_deref(), &metadata, "")?;
        let handle = session_from_metadata(&self.manager, &metadata)?;
        let response = match handle.session.inspector_input(request.get_ref()).await {
            Ok(response) => response,
            Err(error) => pb::InspectorInputResponse {
                ok: false,
                error: Some(error.to_proto()),
                rev: 0,
            },
        };
        Ok(Response::new(response))
    }
}

/// Acquire one viewport-only snapshot for `executor`.
pub async fn snapshot(
    executor: &Executor,
    platform: Platform,
    device: &pb::DeviceInfo,
    rev: &AtomicU64,
    request: &pb::InspectorSnapshotRequest,
) -> Result<pb::InspectorSnapshotResponse, ConnectorError> {
    let started = std::time::Instant::now();
    let cancel = CancellationToken::new();
    executor.inject_helper(&cancel).await?;
    executor
        .evaluate_json::<serde_json::Value>(&inject_expression(), &cancel)
        .await?;

    let options = serde_json::json!({
        "minVisibleFraction": f64::from(request.min_visible_fraction.clamp(0.0, 1.0)),
        "maxDepth": request.max_depth,
        "includeRaw": request.include_raw,
    });
    let expression = format!("window.__impresarioInspector.snapshot({options})");
    let raw: RawSnapshot = executor.evaluate_json(&expression, &cancel).await?;
    if !raw.ok {
        return Err(ConnectorError::internal(
            raw.error
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| "the page inspector returned an error".to_string()),
        ));
    }

    let revision = rev.fetch_add(1, Ordering::SeqCst).saturating_add(1);
    let mut response = normalize(raw, platform, device, revision, request);
    if request.include_screenshot {
        match executor.screenshot(&cancel).await {
            Ok(output) => {
                let (width, height) =
                    crate::png::png_dimensions(&output.screenshot_png).unwrap_or((0, 0));
                response.screenshot_png = output.screenshot_png;
                response.screenshot_width = width;
                response.screenshot_height = height;
            }
            Err(error) => response
                .degraded
                .push(format!("SCREENSHOT_FAILED: {}", error.message)),
        }
    }
    response.acquisition_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
    Ok(response)
}

/// Execute one inspector input action, then re-acquire a snapshot so the UI
/// can immediately render the new revision.
pub async fn input(
    executor: &Executor,
    platform: Platform,
    device: &pb::DeviceInfo,
    rev: &AtomicU64,
    request: &pb::InspectorInputRequest,
) -> Result<pb::InspectorInputResponse, ConnectorError> {
    let cancel = CancellationToken::new();
    executor.inject_helper(&cancel).await?;
    executor
        .evaluate_json::<serde_json::Value>(&inject_expression(), &cancel)
        .await?;

    let kind = pb::InspectorInputKind::try_from(request.kind).map_err(|_| {
        ConnectorError::protocol(format!("unknown inspector input kind {}", request.kind))
    })?;
    match kind {
        pb::InspectorInputKind::Unspecified => {
            return Err(ConnectorError::protocol(
                "inspector input kind is unspecified",
            ))
        }
        pb::InspectorInputKind::Tap => {
            let expression = if request.node_id > 0 {
                format!(
                    "window.__impresarioInspector.tap({}, null, null)",
                    request.node_id
                )
            } else {
                let x = serde_json::json!(request.x);
                let y = serde_json::json!(request.y);
                format!("window.__impresarioInspector.tap(0, {x}, {y})")
            };
            let result: serde_json::Value = executor.evaluate_json(&expression, &cancel).await?;
            ensure_js_ok(&result)?;
        }
        pb::InspectorInputKind::Scroll => {
            let direction = direction_name(request.direction)?;
            let id = request.node_id;
            let count = request.count.max(1);
            for _ in 0..count {
                let expression = format!(
                    "window.__impresarioInspector.scroll({}, {id})",
                    serde_json::json!(direction)
                );
                let result: serde_json::Value =
                    executor.evaluate_json(&expression, &cancel).await?;
                ensure_js_ok(&result)?;
            }
        }
        pb::InspectorInputKind::Key => {
            let press = pb::KeyPress {
                key: request.key,
                repeat: request.count.max(1),
                interval_ms: 100,
            };
            executor.key_press(&press, &cancel).await?;
        }
        pb::InspectorInputKind::Type => {
            let expression = format!(
                "window.__impresarioInspector.prepareType({})",
                request.node_id
            );
            let result: serde_json::Value = executor.evaluate_json(&expression, &cancel).await?;
            ensure_js_ok(&result)?;
            if !request.text.is_empty() {
                executor
                    .backend()
                    .insert_text(&request.text)
                    .await
                    .map_err(ConnectorError::from)?;
            }
        }
        pb::InspectorInputKind::Swipe => {
            swipe(executor, request, &cancel).await?;
        }
    }

    let revision = match snapshot(
        executor,
        platform,
        device,
        rev,
        &pb::InspectorSnapshotRequest::default(),
    )
    .await
    {
        Ok(snapshot) => snapshot.rev,
        Err(_) => 0,
    };
    Ok(pb::InspectorInputResponse {
        ok: true,
        error: None,
        rev: revision,
    })
}

async fn swipe(
    executor: &Executor,
    request: &pb::InspectorInputRequest,
    cancel: &CancellationToken,
) -> Result<(), ConnectorError> {
    let steps = 8u32;
    let start = MouseEvent {
        kind: MouseEventType::Moved,
        x: f64::from(request.x),
        y: f64::from(request.y),
        button: "none",
        buttons: 0,
        click_count: 0,
    };
    executor
        .backend()
        .dispatch_mouse(&start)
        .await
        .map_err(ConnectorError::from)?;
    let pressed = MouseEvent {
        kind: MouseEventType::Pressed,
        x: f64::from(request.x),
        y: f64::from(request.y),
        button: "left",
        buttons: 1,
        click_count: 1,
    };
    executor
        .backend()
        .dispatch_mouse(&pressed)
        .await
        .map_err(ConnectorError::from)?;
    for step in 1..=steps {
        cancel_slot(cancel)?;
        let progress = f64::from(step) / f64::from(steps);
        let x = f64::from(request.x) + (f64::from(request.end_x) - f64::from(request.x)) * progress;
        let y = f64::from(request.y) + (f64::from(request.end_y) - f64::from(request.y)) * progress;
        let event = MouseEvent {
            kind: MouseEventType::Moved,
            x,
            y,
            button: "left",
            buttons: 1,
            click_count: 1,
        };
        executor
            .backend()
            .dispatch_mouse(&event)
            .await
            .map_err(ConnectorError::from)?;
    }
    let released = MouseEvent {
        kind: MouseEventType::Released,
        x: f64::from(request.end_x),
        y: f64::from(request.end_y),
        button: "left",
        buttons: 0,
        click_count: 1,
    };
    executor
        .backend()
        .dispatch_mouse(&released)
        .await
        .map_err(ConnectorError::from)?;
    Ok(())
}

fn ensure_js_ok(value: &serde_json::Value) -> Result<(), ConnectorError> {
    if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(());
    }
    let message = value
        .get("error")
        .and_then(serde_json::Value::as_str)
        .filter(|message| !message.is_empty())
        .unwrap_or("the page rejected the inspector input");
    Err(ConnectorError::not_interactable(message.to_string()))
}

fn direction_name(value: i32) -> Result<&'static str, ConnectorError> {
    match pb::Direction::try_from(value) {
        Ok(pb::Direction::Up) => Ok("up"),
        Ok(pb::Direction::Down) => Ok("down"),
        Ok(pb::Direction::Left) => Ok("left"),
        Ok(pb::Direction::Right) => Ok("right"),
        Ok(pb::Direction::Unspecified) => Err(ConnectorError::protocol(
            "inspector scroll needs a direction",
        )),
        Err(_) => Err(ConnectorError::protocol(format!(
            "unknown direction value {value}"
        ))),
    }
}

fn cancel_slot(cancel: &CancellationToken) -> Result<(), ConnectorError> {
    if cancel.is_cancelled() {
        Err(ConnectorError::cancelled())
    } else {
        Ok(())
    }
}

fn platform_to_proto(platform: Platform) -> pb::Platform {
    match platform {
        Platform::Web => pb::Platform::Web,
        Platform::WebOs => pb::Platform::Webos,
        Platform::Tizen => pb::Platform::Tizen,
        Platform::Android => pb::Platform::Android,
        Platform::AndroidTv => pb::Platform::AndroidTv,
        Platform::Ios => pb::Platform::Ios,
        Platform::Tvos => pb::Platform::Tvos,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RectF {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

fn clip_rect(rect: RectF, clip: RectF) -> RectF {
    let x1 = rect.x.max(clip.x);
    let y1 = rect.y.max(clip.y);
    let x2 = (rect.x + rect.w).min(clip.x + clip.w);
    let y2 = (rect.y + rect.h).min(clip.y + clip.h);
    RectF {
        x: x1,
        y: y1,
        w: (x2 - x1).max(0.0),
        h: (y2 - y1).max(0.0),
    }
}

fn clip_pb_rect(rect: pb::InspectorRect, viewport: RectF) -> pb::InspectorRect {
    let bounds = RectF {
        x: f64::from(rect.x),
        y: f64::from(rect.y),
        w: f64::from(rect.w),
        h: f64::from(rect.h),
    };
    let clipped = clip_rect(bounds, viewport);
    pb::InspectorRect {
        x: clipped.x as f32,
        y: clipped.y as f32,
        w: clipped.w as f32,
        h: clipped.h as f32,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSnapshot {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    w: f64,
    #[serde(default)]
    h: f64,
    #[serde(default = "default_pixel_ratio")]
    dpr: f64,
    #[serde(default)]
    nodes: Vec<RawNode>,
    #[serde(default)]
    focused: u64,
    #[serde(default)]
    truncated: bool,
}

fn default_pixel_ratio() -> f64 {
    1.0
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawNode {
    id: u64,
    #[serde(default)]
    parent: u64,
    #[serde(default)]
    depth: u32,
    #[serde(default)]
    tag: String,
    #[serde(default)]
    input_type: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    text_truncated: bool,
    #[serde(default)]
    value: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    placeholder: String,
    #[serde(default)]
    alt: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    test_id: String,
    #[serde(default)]
    native_id: String,
    #[serde(default)]
    css: String,
    #[serde(default)]
    xpath: String,
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
    #[serde(default)]
    w: f64,
    #[serde(default)]
    h: f64,
    #[serde(default)]
    fraction: f64,
    #[serde(default)]
    focused: bool,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    selected: bool,
    #[serde(default)]
    checked: Option<bool>,
    #[serde(default)]
    expanded: Option<bool>,
    #[serde(default)]
    pressed: Option<bool>,
    #[serde(default)]
    readonly: bool,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    secure: bool,
    #[serde(default)]
    system_ui: bool,
    #[serde(default)]
    scrollable: bool,
    #[serde(default)]
    can_scroll_backward: bool,
    #[serde(default)]
    can_scroll_forward: bool,
    #[serde(default)]
    scroll_axis: String,
    #[serde(default)]
    escapes_clip: bool,
    #[serde(default)]
    children_hidden_count: u32,
    #[serde(default)]
    raw: std::collections::BTreeMap<String, String>,
}

fn default_true() -> bool {
    true
}

fn normalize(
    raw: RawSnapshot,
    platform: Platform,
    device: &pb::DeviceInfo,
    revision: u64,
    request: &pb::InspectorSnapshotRequest,
) -> pb::InspectorSnapshotResponse {
    let RawSnapshot {
        ok: _,
        error: _,
        w,
        h,
        dpr,
        nodes,
        focused,
        truncated,
    } = raw;
    let viewport = RectF {
        x: 0.0,
        y: 0.0,
        w,
        h,
    };
    let mut converted: Vec<pb::InspectorNode> = nodes
        .into_iter()
        .map(|node| convert_node(node, request.include_raw))
        .collect();
    let (capped, pruned_focused_id) = prune_nodes(&mut converted, request, viewport);
    let focused_id = if pruned_focused_id == 0 {
        focused
    } else {
        pruned_focused_id
    };
    let mut degraded = Vec::new();
    if truncated {
        degraded.push(
            "PARTIAL: the page tree hit the traversal budget; some nodes may be missing"
                .to_string(),
        );
    }
    if capped {
        degraded.push("PARTIAL: max_nodes truncated the snapshot".to_string());
    }
    pb::InspectorSnapshotResponse {
        rev: revision,
        device_name: device.model.clone(),
        platform: platform_to_proto(platform) as i32,
        logical_width: w as f32,
        logical_height: h as f32,
        viewport_x: 0.0,
        viewport_y: 0.0,
        viewport_width: w as f32,
        viewport_height: h as f32,
        pixel_ratio: dpr as f32,
        settled: true,
        acquisition_ms: 0,
        nodes: converted,
        degraded,
        focused_id,
        timestamp_unix_ms: now_ms(),
        error: None,
        screenshot_png: Vec::new(),
        screenshot_width: 0,
        screenshot_height: 0,
        truncated: truncated || capped,
    }
}

fn convert_node(node: RawNode, include_raw: bool) -> pb::InspectorNode {
    let explicit_role = if node.role.is_empty() {
        None
    } else {
        Some(node.role.as_str())
    };
    let input_type = if node.input_type.is_empty() {
        None
    } else {
        Some(node.input_type.as_str())
    };
    let role = role_from_web_tag(&node.tag, input_type, explicit_role);
    let bounds = pb::InspectorRect {
        x: node.x as f32,
        y: node.y as f32,
        w: node.w as f32,
        h: node.h as f32,
    };
    let raw = if include_raw {
        serde_json::to_string(&node.raw).unwrap_or_default()
    } else {
        String::new()
    };
    pb::InspectorNode {
        id: node.id,
        parent_id: node.parent,
        role: role.as_str().to_string(),
        name: node.name,
        text: node.text,
        test_id: node.test_id,
        raw_id: node.native_id,
        bounds: Some(bounds),
        viewport_bounds: Some(bounds),
        focused: node.focused,
        enabled: node.enabled,
        selected: node.selected,
        checked: node.checked,
        expanded: node.expanded,
        pressed: node.pressed,
        readonly: node.readonly,
        required: node.required,
        hidden: node.hidden,
        secure: node.secure,
        system_ui: node.system_ui,
        scrollable: node.scrollable,
        can_scroll_backward: node.can_scroll_backward,
        can_scroll_forward: node.can_scroll_forward,
        scroll_axis: node.scroll_axis,
        children_hidden_count: node.children_hidden_count,
        raw,
        value: node.value,
        label: node.label,
        description: node.description,
        placeholder: node.placeholder,
        alt: node.alt,
        title: node.title,
        css_selector: node.css,
        xpath: node.xpath,
        visible_fraction: node.fraction as f32,
        escapes_clip: node.escapes_clip,
        depth: node.depth,
        text_truncated: node.text_truncated,
    }
}

fn prune_nodes(
    nodes: &mut Vec<pb::InspectorNode>,
    request: &pb::InspectorSnapshotRequest,
    viewport: RectF,
) -> (bool, u64) {
    let max_depth = if request.max_depth == 0 {
        DEFAULT_MAX_DEPTH
    } else {
        request.max_depth
    };
    let max_nodes = if request.max_nodes == 0 {
        DEFAULT_MAX_NODES
    } else {
        request.max_nodes
    } as usize;
    let min_fraction = f64::from(request.min_visible_fraction.clamp(0.0, 1.0));

    let by_id: HashMap<u64, usize> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id, index))
        .collect();
    let mut keep = vec![false; nodes.len()];
    for (index, node) in nodes.iter().enumerate() {
        if node.system_ui && !request.include_system_ui {
            continue;
        }
        if node.depth > max_depth {
            continue;
        }
        if node.hidden && !node.focused {
            continue;
        }
        let area = node
            .bounds
            .as_ref()
            .map(|bounds| f64::from(bounds.w).max(0.0) * f64::from(bounds.h).max(0.0))
            .unwrap_or(0.0);
        if area <= 0.0 {
            continue;
        }
        if node.focused {
            if let Some(slot) = keep.get_mut(index) {
                *slot = true;
            }
            continue;
        }
        let fraction = f64::from(node.visible_fraction);
        if fraction > 0.0 && (min_fraction <= 0.0 || fraction >= min_fraction) {
            if let Some(slot) = keep.get_mut(index) {
                *slot = true;
            }
        }
    }

    for index in 0..nodes.len() {
        if !keep.get(index).copied().unwrap_or(false) {
            continue;
        }
        let mut parent = nodes.get(index).map_or(0, |node| node.parent_id);
        let mut guard = 0;
        while parent != 0 && guard < 1024 {
            let Some(&parent_index) = by_id.get(&parent) else {
                break;
            };
            if keep.get(parent_index).copied().unwrap_or(false) {
                break;
            }
            if let Some(slot) = keep.get_mut(parent_index) {
                *slot = true;
            }
            parent = nodes.get(parent_index).map_or(0, |node| node.parent_id);
            guard += 1;
        }
    }

    let mut kept_count = keep.iter().filter(|value| **value).count();
    let mut capped = false;
    if kept_count > max_nodes {
        capped = true;
        let mut priority = vec![false; nodes.len()];
        for (index, node) in nodes.iter().enumerate() {
            if node.focused {
                mark_ancestors(index, nodes, &by_id, &mut priority);
            }
        }
        for index in (0..nodes.len()).rev() {
            if kept_count <= max_nodes {
                break;
            }
            if keep.get(index).copied().unwrap_or(false)
                && !priority.get(index).copied().unwrap_or(false)
            {
                if let Some(slot) = keep.get_mut(index) {
                    *slot = false;
                }
                kept_count = kept_count.saturating_sub(1);
            }
        }
        // The focused node is never dropped. If its ancestor chain alone
        // exceeds the cap, drop ancestors from the end; the final pass
        // promotes the focused node to a root.
        for index in (0..nodes.len()).rev() {
            if kept_count <= max_nodes {
                break;
            }
            let focused = nodes.get(index).is_some_and(|node| node.focused);
            if keep.get(index).copied().unwrap_or(false) && !focused {
                if let Some(slot) = keep.get_mut(index) {
                    *slot = false;
                }
                kept_count = kept_count.saturating_sub(1);
            }
        }
    }

    let mut hidden_counts: HashMap<u64, u32> = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        if keep.get(index).copied().unwrap_or(false) {
            continue;
        }
        let parent = node.parent_id;
        if parent != 0
            && by_id
                .get(&parent)
                .is_some_and(|parent_index| keep.get(*parent_index).copied().unwrap_or(false))
        {
            let count = hidden_counts.get(&parent).copied().unwrap_or(0);
            hidden_counts.insert(parent, count.saturating_add(1));
        }
    }

    let mut out = Vec::with_capacity(kept_count);
    for (index, mut node) in nodes.drain(..).enumerate() {
        if !keep.get(index).copied().unwrap_or(false) {
            continue;
        }
        let extra = hidden_counts.get(&node.id).copied().unwrap_or(0);
        node.children_hidden_count = node.children_hidden_count.saturating_add(extra);
        if node.parent_id != 0
            && !by_id
                .get(&node.parent_id)
                .is_some_and(|parent_index| keep.get(*parent_index).copied().unwrap_or(false))
        {
            node.parent_id = 0;
        }
        if let Some(bounds) = node.bounds.clone() {
            node.viewport_bounds = Some(clip_pb_rect(bounds, viewport));
        }
        out.push(node);
    }
    let focused_id = out
        .iter()
        .find(|node| node.focused)
        .map_or(0, |node| node.id);
    *nodes = out;
    (capped, focused_id)
}

fn mark_ancestors(
    index: usize,
    nodes: &[pb::InspectorNode],
    by_id: &HashMap<u64, usize>,
    flags: &mut [bool],
) {
    let mut current = Some(index);
    let mut guard = 0;
    while let Some(node_index) = current {
        match flags.get_mut(node_index) {
            Some(flag) if !*flag => *flag = true,
            _ => break,
        }
        let parent = nodes.get(node_index).map_or(0, |node| node.parent_id);
        if parent == 0 {
            break;
        }
        current = by_id.get(&parent).copied();
        guard += 1;
        if guard > 1024 {
            break;
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_node(id: u64, parent: u64, x: f64, y: f64, w: f64, h: f64) -> RawNode {
        RawNode {
            id,
            parent,
            depth: 0,
            tag: "div".to_string(),
            input_type: String::new(),
            role: String::new(),
            name: String::new(),
            text: String::new(),
            text_truncated: false,
            value: String::new(),
            label: String::new(),
            description: String::new(),
            placeholder: String::new(),
            alt: String::new(),
            title: String::new(),
            test_id: String::new(),
            native_id: String::new(),
            css: String::new(),
            xpath: String::new(),
            x,
            y,
            w,
            h,
            fraction: 1.0,
            focused: false,
            enabled: true,
            selected: false,
            checked: None,
            expanded: None,
            pressed: None,
            readonly: false,
            required: false,
            hidden: false,
            secure: false,
            system_ui: false,
            scrollable: false,
            can_scroll_backward: false,
            can_scroll_forward: false,
            scroll_axis: String::new(),
            escapes_clip: false,
            children_hidden_count: 0,
            raw: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn inspector_script_is_es5() {
        let banned = ["=>", "`", "const ", "let ", "async ", "await ", "?.", "..."];
        for token in banned {
            assert!(
                !INSPECTOR_SCRIPT.contains(token),
                "ES5 violation: inspector script contains {token:?}"
            );
        }
    }

    #[test]
    fn inspector_script_exposes_the_surface() {
        for name in ["snapshot", "tap", "scroll", "prepareType"] {
            assert!(
                INSPECTOR_SCRIPT.contains(&format!("{name}: {name}")),
                "inspector script does not export {name}"
            );
        }
        assert!(inject_expression().contains("window.__impresarioInspector"));
        assert!(inject_expression().ends_with("true"));
    }

    #[test]
    fn normalize_clips_to_the_viewport_and_prunes_off_screen_nodes() {
        let mut offscreen = raw_node(2, 1, 0.0, 900.0, 100.0, 100.0);
        offscreen.fraction = 0.0;
        let raw = RawSnapshot {
            ok: true,
            error: None,
            w: 800.0,
            h: 600.0,
            dpr: 2.0,
            nodes: vec![
                raw_node(1, 0, 0.0, 0.0, 800.0, 600.0),
                raw_node(3, 1, 10.0, 20.0, 100.0, 50.0),
                offscreen,
            ],
            focused: 0,
            truncated: false,
        };
        let request = pb::InspectorSnapshotRequest::default();
        let response = normalize(raw, Platform::Web, &pb::DeviceInfo::default(), 7, &request);
        assert_eq!(response.rev, 7);
        assert_eq!(response.logical_width, 800.0);
        assert_eq!(response.pixel_ratio, 2.0);
        assert_eq!(response.nodes.len(), 2);
        assert!(response.nodes.iter().all(|node| node.id != 2));
        let child = response
            .nodes
            .iter()
            .find(|node| node.id == 3)
            .expect("child");
        let viewport = child.viewport_bounds.as_ref().expect("viewport bounds");
        assert_eq!(
            (viewport.x, viewport.y, viewport.w, viewport.h),
            (10.0, 20.0, 100.0, 50.0)
        );
    }

    #[test]
    fn normalize_maps_roles_and_secure_values() {
        let mut password = raw_node(1, 0, 0.0, 0.0, 100.0, 40.0);
        password.tag = "input".to_string();
        password.input_type = "password".to_string();
        password.secure = true;
        password.value = String::new();
        let raw = RawSnapshot {
            ok: true,
            error: None,
            w: 100.0,
            h: 100.0,
            dpr: 1.0,
            nodes: vec![password],
            focused: 1,
            truncated: false,
        };
        let request = pb::InspectorSnapshotRequest::default();
        let response = normalize(raw, Platform::Web, &pb::DeviceInfo::default(), 1, &request);
        let node = response.nodes.first().expect("node");
        assert_eq!(node.role, "textbox");
        assert!(node.secure);
        assert!(node.value.is_empty());
        assert_eq!(response.focused_id, 1);
    }

    #[test]
    fn max_nodes_keeps_the_focused_chain() {
        let mut focused = raw_node(4, 3, 0.0, 0.0, 10.0, 10.0);
        focused.focused = true;
        let raw = RawSnapshot {
            ok: true,
            error: None,
            w: 100.0,
            h: 100.0,
            dpr: 1.0,
            nodes: vec![
                raw_node(1, 0, 0.0, 0.0, 100.0, 100.0),
                raw_node(2, 1, 0.0, 0.0, 100.0, 100.0),
                raw_node(3, 2, 0.0, 0.0, 100.0, 100.0),
                focused,
                raw_node(5, 1, 0.0, 0.0, 10.0, 10.0),
            ],
            focused: 4,
            truncated: false,
        };
        let request = pb::InspectorSnapshotRequest {
            max_nodes: 2,
            ..pb::InspectorSnapshotRequest::default()
        };
        let response = normalize(raw, Platform::Web, &pb::DeviceInfo::default(), 1, &request);
        let ids: Vec<u64> = response.nodes.iter().map(|node| node.id).collect();
        assert!(ids.contains(&4));
        assert_eq!(response.focused_id, 4);
        assert!(response.truncated);
    }

    #[test]
    fn direction_mapping_is_total() {
        assert_eq!(direction_name(pb::Direction::Up as i32).expect("up"), "up");
        assert_eq!(
            direction_name(pb::Direction::Down as i32).expect("down"),
            "down"
        );
        assert_eq!(
            direction_name(pb::Direction::Left as i32).expect("left"),
            "left"
        );
        assert_eq!(
            direction_name(pb::Direction::Right as i32).expect("right"),
            "right"
        );
        assert!(direction_name(pb::Direction::Unspecified as i32).is_err());
        assert!(direction_name(999).is_err());
    }
}
