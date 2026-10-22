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
