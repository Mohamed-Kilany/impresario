'use strict';

/*
 * Impresario Inspector SPA.
 *
 * The server owns acquisition, pruning, locator resolution and scene edits.
 * This client only renders the latest normalized snapshot, applies SSE deltas,
 * maps pointer coordinates back to nodes, and calls the JSON API. No user text
 * is ever assigned through innerHTML.
 */

const TOKEN = new URLSearchParams(window.location.search).get('token') || '';
const ROW_HEIGHT = 24;

const state = {
  meta: null,
  status: null,
  snapshot: null,
  pollMs: 250,
  frozen: false,
  selectedId: 0,
  selectedFingerprint: null,
  detailsFor: 0,
  detailsAt: 0,
  hoverId: 0,
  matchIds: new Set(),
  collapsed: new Set(),
  filter: '',
  rows: [],
  scene: null,
  edit: null,
  screenshotBusy: false,
  screenshotTimer: 0,
  toastTimer: 0,
  hoverFrame: 0,
};

function byId(id) {
  return document.getElementById(id);
}

function clear(element) {
  while (element.firstChild) element.removeChild(element.firstChild);
}

function authHeaders() {
  return TOKEN ? { Authorization: 'Bearer ' + TOKEN } : {};
}

class ApiError extends Error {
  constructor(error) {
    super((error && error.message) || 'The inspector rejected the request.');
    this.name = 'ApiError';
    this.code = (error && error.code) || 'ERROR';
    this.details = (error && error.details) || null;
  }
}

async function api(path, options) {
  const settings = options || {};
  const headers = Object.assign({}, authHeaders(), settings.headers || {});
  const request = {
    method: settings.method || (settings.body !== undefined ? 'POST' : 'GET'),
    headers,
  };
  if (settings.body !== undefined) {
    headers['Content-Type'] = 'application/json';
    request.body = JSON.stringify(settings.body);
  }
  let response;
  try {
    response = await fetch(path, request);
  } catch (error) {
    throw new ApiError({
      code: 'INSPECTOR_UNREACHABLE',
      message: 'Cannot reach the inspector: ' + error.message,
    });
  }
  let payload;
  try {
    payload = await response.json();
  } catch (error) {
    throw new ApiError({
      code: 'BAD_RESPONSE',
      message: 'The inspector returned a non-JSON response (HTTP ' + response.status + ').',
    });
  }
  if (!payload || payload.ok !== true) {
    throw new ApiError((payload && payload.error) || {
      code: 'HTTP_' + response.status,
      message: 'Request failed.',
    });
  }
  return payload.data;
}

function toast(message, isError) {
  const element = byId('toast');
  element.textContent = message;
  element.classList.toggle('error', Boolean(isError));
  element.hidden = false;
  clearTimeout(state.toastTimer);
  state.toastTimer = setTimeout(() => {
    element.hidden = true;
  }, isError ? 8000 : 4000);
}

function makeButton(label, onClick, className) {
  const button = document.createElement('button');
  button.type = 'button';
  button.textContent = label;
  if (className) button.className = className;
  button.addEventListener('click', onClick);
  return button;
}

// ---------------------------------------------------------------------------
// Status and devices
// ---------------------------------------------------------------------------

async function refreshStatus() {
  const data = await api('/api/status');
  state.meta = data;
  state.status = data.status;
  state.pollMs = data.poll_interval_ms || 250;
  state.frozen = Boolean(data.frozen);
  byId('freeze').checked = state.frozen;
  renderStatus();
}

function renderStatus() {
  const status = state.status;
  const meta = state.meta;
  const pill = byId('attach-status');
  const attached = Boolean(status && status.attached);
  pill.textContent = attached ? 'attached' : 'disconnected';
  pill.classList.toggle('on', attached);
  pill.classList.toggle('error', Boolean(status && status.error));
  byId('rev').textContent = status && status.rev ? 'rev ' + status.rev : '';
  byId('fps').textContent = state.frozen ? 'frozen' : state.pollMs + ' ms poll';

  const lines = [];
  if (meta) {
    lines.push('workspace: ' + meta.workspace);
    lines.push('url: ' + meta.url);
  }
  if (attached) {
    lines.push('device: ' + (status.device || '?') + ' (' + (status.platform || '?') + ')');
    if (status.session_id) lines.push('session: ' + status.session_id);
    lines.push('nodes: ' + (status.nodes || 0) + '  settled: ' + Boolean(status.settled));
    lines.push('acquisition: ' + (status.acquisition_ms || 0) + ' ms');
    if (status.truncated) lines.push('snapshot is partial (node or depth budget)');
  } else {
    lines.push('No device attached.');
  }
  if (status && status.error) lines.push('error: ' + status.error);
  byId('device-meta').textContent = lines.join('\n');

  const summary = [];
  if (attached) {
    summary.push((status.device || 'device') + ' (' + (status.platform || '?') + ')');
    summary.push('rev ' + (status.rev || 0));
    summary.push((status.nodes || 0) + ' nodes');
    summary.push(status.settled ? 'settled' : 'updating');
    if (status.error) summary.push('error: ' + status.error);
  } else {
    summary.push('No device attached');
  }
  byId('status-text').textContent = summary.join('  ·  ');
  byId('stage-empty').hidden = attached && Boolean(state.snapshot);
  byId('attach').disabled = attached;
  byId('detach').disabled = !attached;
}

async function refreshDevices() {
  const data = await api('/api/devices');
  const select = byId('device-select');
  const previous = select.value;
  clear(select);
  for (const device of data.devices || []) {
    const option = document.createElement('option');
    option.value = device.name;
    option.textContent = device.name + ' · ' + device.platform + (device.attached ? ' · attached' : '');
    select.appendChild(option);
  }
  const wanted = data.attached || previous;
  if (wanted) select.value = wanted;
  if (!select.value && select.options.length > 0) select.selectedIndex = 0;
}

async function attachDevice() {
  const name = byId('device-select').value;
  if (!name) {
    toast('Choose a device first.', true);
    return;
  }
  try {
    const status = await api('/api/attach', { body: { device: name } });
    toast('Attached to ' + name + '.');
    if (status && status.error) toast(status.error, true);
    await refreshStatus();
    await refreshDevices();
    await refreshSnapshot();
    loadScreenshot();
  } catch (error) {
    toast(error.message, true);
  }
}

async function detachDevice() {
  try {
    await api('/api/detach', { body: {} });
    state.snapshot = null;
    state.selectedId = 0;
    state.selectedFingerprint = null;
    state.matchIds.clear();
    state.hoverId = 0;
    state.rows = [];
    clearTimeout(state.screenshotTimer);
    byId('screenshot').removeAttribute('src');
    renderTree();
    drawOverlay();
    byId('details').textContent = 'Nothing selected.';
    clear(byId('suggestions'));
    await refreshStatus();
    await refreshDevices();
    toast('Detached.');
  } catch (error) {
    toast(error.message, true);
  }
}

async function toggleFreeze() {
  const frozen = byId('freeze').checked;
  try {
    await api('/api/freeze', { body: { frozen } });
    state.frozen = frozen;
    if (frozen) {
      clearTimeout(state.screenshotTimer);
    } else {
      loadScreenshot();
    }
    renderStatus();
  } catch (error) {
    byId('freeze').checked = state.frozen;
    toast(error.message, true);
  }
}

// ---------------------------------------------------------------------------
// Snapshot, tree and selection
// ---------------------------------------------------------------------------

function findNode(id) {
  if (!state.snapshot || !Array.isArray(state.snapshot.nodes)) return null;
  return state.snapshot.nodes.find((node) => node.id === id) || null;
}

function fingerprintOf(node) {
  return {
    role: node.role || '',
    name: node.name || '',
    text: node.text || '',
    test_id: node.test_id || '',
  };
}

function remapSelection(fingerprint) {
  if (!state.snapshot || !fingerprint) return null;
  const candidates = state.snapshot.nodes.filter((node) =>
    (node.role || '') === fingerprint.role &&
    (node.name || '') === fingerprint.name &&
    (node.test_id || '') === fingerprint.test_id
  );
  return candidates.length === 1 ? candidates[0] : null;
}

function applyDelta(delta) {
  const snapshot = state.snapshot;
  if (!snapshot || !delta || delta.base_rev !== snapshot.rev) {
    refreshSnapshot();
    return;
  }
  let nodes = Array.isArray(snapshot.nodes) ? snapshot.nodes.slice() : [];
  const ops = Array.isArray(delta.ops) ? delta.ops : [];
  for (const op of ops) {
    if (!op || !op.op) continue;
    if (op.op === 'remove') {
      nodes = nodes.filter((node) => node.id !== op.id);
    } else if (op.op === 'update' && op.node) {
      const index = nodes.findIndex((node) => node.id === op.node.id);
      if (index >= 0) nodes[index] = op.node;
    } else if (op.op === 'add' && op.node) {
      nodes.push(op.node);
    } else if (op.op === 'move') {
      const node = nodes.find((candidate) => candidate.id === op.id);
      if (node) node.parent_id = op.parent_id;
    } else if (op.op === 'reorder') {
      const index = new Map(nodes.map((node) => [node.id, node]));
      nodes = (op.ids || []).map((id) => index.get(id)).filter(Boolean);
    }
  }
  snapshot.nodes = nodes;
  snapshot.rev = delta.rev;
}

function onSnapshot() {
  if (!state.snapshot) {
    state.rows = [];
    renderTree();
    drawOverlay();
    renderStatus();
    return;
  }
  if (state.selectedFingerprint) {
    const current = findNode(state.selectedId);
    if (!current) {
      const remapped = remapSelection(state.selectedFingerprint);
      if (remapped) {
        state.selectedId = remapped.id;
        state.selectedFingerprint = fingerprintOf(remapped);
        toast('Selection remapped to e' + remapped.id + '.');
        loadDetails(remapped.id);
      } else {
        state.selectedId = 0;
        state.selectedFingerprint = null;
        byId('details').textContent = 'The selected node is no longer in the snapshot.';
        clear(byId('suggestions'));
      }
    }
  }
  buildRows();
  renderTree();
  drawOverlay();
  renderStatus();
  if (state.selectedId) {
    const now = Date.now();
    if (state.detailsFor !== state.selectedId || now - state.detailsAt > 1500) {
      state.detailsFor = state.selectedId;
      state.detailsAt = now;
      loadDetails(state.selectedId);
    }
  }
}

async function refreshSnapshot() {
  try {
    state.snapshot = await api('/api/snapshot');
    onSnapshot();
  } catch (error) {
    if (error.code === 'NO_SNAPSHOT') {
      state.snapshot = null;
      onSnapshot();
      return;
    }
    toast(error.message, true);
  }
}

function buildRows() {
  state.rows = [];
  if (!state.snapshot || !Array.isArray(state.snapshot.nodes)) return;
  const nodes = state.snapshot.nodes;
  const byParent = new Map();
  for (const node of nodes) {
    const parent = node.parent_id || 0;
    if (!byParent.has(parent)) byParent.set(parent, []);
    byParent.get(parent).push(node);
  }
  const rows = [];
  const seen = new Set();
  const walk = (node, depth) => {
    if (seen.has(node.id)) return;
    seen.add(node.id);
    const children = byParent.get(node.id) || [];
    const expanded = !state.collapsed.has(node.id);
    rows.push({ node, depth, hasChildren: children.length > 0, expanded });
    if (expanded) {
      for (const child of children) walk(child, depth + 1);
    }
  };
  for (const root of byParent.get(0) || []) walk(root, 0);
  for (const node of nodes) {
    if (!seen.has(node.id)) rows.push({ node, depth: 0, hasChildren: false, expanded: false });
  }
  if (!state.filter.trim()) {
    state.rows = rows;
    return;
  }
  const needle = state.filter.trim().toLowerCase();
  const matching = new Set();
  const index = new Map(nodes.map((node) => [node.id, node]));
  const textOf = (node) => [node.role, node.name, node.text, node.value, node.test_id, node.native_id]
    .filter(Boolean).join(' ').toLowerCase();
  for (const node of nodes) {
    if (!textOf(node).includes(needle)) continue;
    let current = node;
    let guard = 0;
    while (current && guard < 200) {
      matching.add(current.id);
      current = index.get(current.parent_id || 0);
      guard += 1;
    }
  }
  state.rows = rows.filter((row) => matching.has(row.node.id));
}

function renderTree() {
  const tree = byId('tree');
  if (!state.rows.length) {
    clear(tree);
    tree.dataset.ready = '';
    return;
  }
  if (tree.dataset.ready !== '1') {
    clear(tree);
    const spacer = document.createElement('div');
    spacer.className = 'tree-spacer';
    const layer = document.createElement('div');
    layer.className = 'tree-layer';
    tree.appendChild(spacer);
    tree.appendChild(layer);
    tree.dataset.ready = '1';
  }
  const spacer = tree.querySelector('.tree-spacer');
  const layer = tree.querySelector('.tree-layer');
  if (!spacer || !layer) return;
  spacer.style.height = (state.rows.length * ROW_HEIGHT) + 'px';
  const height = tree.clientHeight || 320;
  const start = Math.max(0, Math.floor(tree.scrollTop / ROW_HEIGHT) - 8);
  const end = Math.min(state.rows.length, Math.ceil((tree.scrollTop + height) / ROW_HEIGHT) + 8);
  clear(layer);
  layer.style.transform = 'translateY(' + (start * ROW_HEIGHT) + 'px)';
  const fragment = document.createDocumentFragment();
  for (let i = start; i < end; i += 1) {
    fragment.appendChild(rowElement(state.rows[i]));
  }
  layer.appendChild(fragment);
}

function rowElement(row) {
  const node = row.node;
  const element = document.createElement('div');
  element.className = 'tree-row';
  element.setAttribute('role', 'treeitem');
  element.dataset.id = String(node.id);
  element.style.paddingLeft = (6 + row.depth * 14) + 'px';
  if (node.id === state.selectedId) element.classList.add('selected');
  if (node.id === state.hoverId) element.classList.add('hover');
  if (state.matchIds.has(node.id)) element.classList.add('match');

  const twisty = document.createElement('span');
  twisty.className = 'twisty';
  twisty.textContent = row.hasChildren ? (row.expanded ? '▾' : '▸') : '·';
  if (row.hasChildren) {
    twisty.addEventListener('click', (event) => {
      event.stopPropagation();
      toggleCollapsed(node.id);
    });
  }
  element.appendChild(twisty);

  const role = document.createElement('span');
  role.className = 'role';
  role.textContent = node.role || 'generic';
  element.appendChild(role);

  const label = document.createElement('span');
  label.className = 'name';
  const name = node.name || node.text || node.label || node.placeholder || node.value || '';
  label.textContent = name.length > 90 ? name.slice(0, 90) + '…' : name;
  label.title = name;
  element.appendChild(label);

  const badges = [];
  if (node.state && node.state.focused) badges.push('focused');
  if (node.state && node.state.selected) badges.push('selected');
  if (node.state && node.state.checked === true) badges.push('checked');
  if (node.state && node.state.disabled) badges.push('disabled');
  if (node.secure) badges.push('secure');
  if (node.children_hidden_count) badges.push('+' + node.children_hidden_count);
  if (badges.length) {
    const badge = document.createElement('span');
    badge.className = 'muted';
    badge.textContent = '[' + badges.join(' ') + ']';
    element.appendChild(badge);
  }

  const reference = document.createElement('span');
  reference.className = 'ref';
  reference.textContent = 'e' + node.id;
  element.appendChild(reference);

  element.addEventListener('click', (event) => {
    event.stopPropagation();
    selectNode(node.id);
  });
  element.addEventListener('dblclick', (event) => {
    event.stopPropagation();
    toggleCollapsed(node.id);
  });
  element.addEventListener('mouseenter', () => setHover(node.id));
  element.addEventListener('mouseleave', () => setHover(0));
  return element;
}

function toggleCollapsed(id) {
  if (state.collapsed.has(id)) state.collapsed.delete(id);
  else state.collapsed.add(id);
  buildRows();
  renderTree();
}

function expandAll() {
  state.collapsed.clear();
  buildRows();
  renderTree();
}

function collapseAll() {
  if (!state.snapshot) return;
  for (const node of state.snapshot.nodes) state.collapsed.add(node.id);
  buildRows();
  renderTree();
}

function setHover(id) {
  if (state.hoverId === id) return;
  state.hoverId = id;
  renderTree();
  drawOverlay();
}

function selectNode(id) {
  const node = findNode(id);
  if (!node) return;
  state.selectedId = id;
  state.selectedFingerprint = fingerprintOf(node);
  renderTree();
  drawOverlay();
  loadDetails(id);
}

function moveSelection(delta) {
  if (!state.rows.length) return;
  const index = state.rows.findIndex((row) => row.node.id === state.selectedId);
  const next = index < 0 ? 0 : Math.max(0, Math.min(state.rows.length - 1, index + delta));
  const row = state.rows[next];
  if (row) {
    selectNode(row.node.id);
    const element = byId('tree').querySelector('[data-id="' + row.node.id + '"]');
    if (element) element.scrollIntoView({ block: 'nearest' });
  }
}

async function loadDetails(id) {
  try {
    const [node, suggestions] = await Promise.all([
      api('/api/node?id=' + id),
      api('/api/node/suggest?id=' + id),
    ]);
    if (state.selectedId !== id) return;
    state.detailsFor = id;
    state.detailsAt = Date.now();
    renderDetails(node);
    renderSuggestions(suggestions);
  } catch (error) {
    if (state.selectedId === id) {
      byId('details').textContent = error.message;
      clear(byId('suggestions'));
    }
  }
}

function renderDetails(node) {
  const details = byId('details');
  clear(details);
  const list = document.createElement('dl');
  const bounds = node.bounds
    ? Math.round(node.bounds.x) + ',' + Math.round(node.bounds.y) + ' ' +
      Math.round(node.bounds.w) + '×' + Math.round(node.bounds.h)
    : '';
  const fields = [
    ['Ref', 'e' + node.id],
    ['Role', node.role],
    ['Name', node.name],
    ['Text', node.text],
    ['Value', node.value],
    ['Test id', node.test_id],
    ['Native id', node.native_id],
    ['Bounds', bounds],
    ['Visible', node.visible_fraction === undefined ? '' : Math.round(node.visible_fraction * 100) + '%'],
    ['State', stateSummary(node.state)],
    ['Scroll', scrollSummary(node.scroll)],
    ['CSS', node.css_selector],
    ['XPath', node.xpath],
    ['Hidden children', node.children_hidden_count ? String(node.children_hidden_count) : ''],
    ['Secure', node.secure ? 'yes (value masked)' : ''],
  ];
  for (const [label, value] of fields) {
    if (value === undefined || value === null || value === '') continue;
    const term = document.createElement('dt');
    term.textContent = label;
    const definition = document.createElement('dd');
    definition.textContent = String(value);
    list.appendChild(term);
    list.appendChild(definition);
  }
  details.appendChild(list);
  if (node.raw && Object.keys(node.raw).length) {
    const pre = document.createElement('pre');
    pre.textContent = JSON.stringify(node.raw, null, 2);
    details.appendChild(pre);
  }
}

function stateSummary(nodeState) {
  if (!nodeState) return '';
  const names = [
    'focused', 'selected', 'checked', 'expanded', 'pressed',
    'disabled', 'readonly', 'required', 'hidden_by_a11y',
  ];
  return names
    .filter((name) => nodeState[name] === true)
    .join(', ');
}

function scrollSummary(scroll) {
  if (!scroll) return '';
  const parts = [];
  if (scroll.axis) parts.push(scroll.axis);
  if (scroll.can_scroll_forward) parts.push('more forward');
  if (scroll.can_scroll_backward) parts.push('more backward');
  return parts.join(', ');
}

function renderSuggestions(data) {
  const container = byId('suggestions');
  clear(container);
  const suggestions = (data && data.suggestions) || [];
  if (!suggestions.length) {
    container.textContent = 'No locator candidates.';
    return;
  }
  for (const suggestion of suggestions) {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'suggestion';
    button.title = (suggestion.notes || []).join('; ');
    const code = document.createElement('code');
    code.textContent = suggestion.locator;
    button.appendChild(code);
    const score = document.createElement('span');
    score.className = 'score';
    score.textContent = suggestion.score + (suggestion.stable ? ' ★' : '');
    button.appendChild(score);
    button.addEventListener('click', () => {
      byId('locator-input').value = suggestion.locator;
      resolveLocator(suggestion.locator);
    });
    container.appendChild(button);
  }
}

// ---------------------------------------------------------------------------
// Overlay and pointer selection
// ---------------------------------------------------------------------------

function drawOverlay() {
  const image = byId('screenshot');
  const canvas = byId('overlay');
  const snapshot = state.snapshot;
  if (!snapshot || !image.complete || !image.naturalWidth) {
    canvas.width = 1;
    canvas.height = 1;
    return;
  }
  const rect = image.getBoundingClientRect();
  if (rect.width < 1 || rect.height < 1) return;
  const ratio = window.devicePixelRatio || 1;
  canvas.width = Math.max(1, Math.round(rect.width * ratio));
  canvas.height = Math.max(1, Math.round(rect.height * ratio));
  canvas.style.width = rect.width + 'px';
  canvas.style.height = rect.height + 'px';
  const context = canvas.getContext('2d');
  if (!context) return;
  context.setTransform(ratio, 0, 0, ratio, 0, 0);
  context.clearRect(0, 0, rect.width, rect.height);
  const screen = snapshot.screen || {};
  const scaleX = rect.width / (screen.logical_w || 1);
  const scaleY = rect.height / (screen.logical_h || 1);
  const drawNode = (node, color, width) => {
    if (!node || !node.bounds) return;
    const bounds = node.bounds;
    context.strokeStyle = color;
    context.lineWidth = width;
    context.strokeRect(
      bounds.x * scaleX,
      bounds.y * scaleY,
      Math.max(1, bounds.w * scaleX),
      Math.max(1, bounds.h * scaleY)
    );
  };
  if (!Array.isArray(snapshot.nodes)) return;
  for (const node of snapshot.nodes) {
    if (state.matchIds.has(node.id)) drawNode(node, '#f2c14e', 1.5);
  }
  for (const node of snapshot.nodes) {
    if (node.state && node.state.focused) drawNode(node, '#c792ea', 1.5);
    if (node.scroll) drawNode(node, '#8d9ab0', 1);
  }
  const hover = findNode(state.hoverId);
  if (hover) drawNode(hover, '#5cc8ff', 1.5);
  const selected = findNode(state.selectedId);
  if (selected) {
    drawNode(selected, '#57d38c', 2.5);
    if (selected.bounds) {
      const label = (selected.role || 'generic') + ' ' +
        (selected.name || selected.text || '').slice(0, 48);
      context.font = '11px "Segoe UI", system-ui, sans-serif';
      const metrics = context.measureText(label);
      const x = Math.max(0, selected.bounds.x * scaleX);
      const y = Math.max(14, selected.bounds.y * scaleY - 4);
      context.fillStyle = 'rgba(10, 14, 20, 0.85)';
      context.fillRect(x, y - 13, metrics.width + 8, 16);
      context.fillStyle = '#57d38c';
      context.fillText(label, x + 4, y - 1);
    }
  }
}

function stagePoint(event) {
  const image = byId('screenshot');
  const rect = image.getBoundingClientRect();
  if (rect.width < 1 || rect.height < 1 || !state.snapshot) return null;
  const screen = state.snapshot.screen || {};
  return {
    x: (event.clientX - rect.left) / rect.width * (screen.logical_w || 1),
    y: (event.clientY - rect.top) / rect.height * (screen.logical_h || 1),
  };
}

function contains(bounds, x, y) {
  return Boolean(bounds) && x >= bounds.x && y >= bounds.y &&
    x <= bounds.x + bounds.w && y <= bounds.y + bounds.h;
}

function area(node) {
  return node.bounds ? Math.max(1, node.bounds.w * node.bounds.h) : Number.MAX_VALUE;
}

function nodesAt(point) {
  if (!state.snapshot || !Array.isArray(state.snapshot.nodes)) return [];
  return state.snapshot.nodes
    .filter((node) => node.visible_fraction > 0 && contains(node.bounds, point.x, point.y))
    .sort((left, right) => area(left) - area(right));
}

function stageClick(event) {
  const point = stagePoint(event);
  if (!point) return;
  const candidates = nodesAt(point);
  if (!candidates.length) return;
  let node = candidates[0];
  if (event.altKey || event.metaKey) {
    const index = candidates.findIndex((candidate) => candidate.id === state.selectedId);
    node = candidates[(index + 1) % candidates.length];
  }
  selectNode(node.id);
}

function stageMove(event) {
  if (state.hoverFrame) return;
  state.hoverFrame = window.requestAnimationFrame(() => {
    state.hoverFrame = 0;
    const point = stagePoint(event);
    if (!point) return;
    const candidates = nodesAt(point);
    setHover(candidates.length ? candidates[0].id : 0);
  });
}

// ---------------------------------------------------------------------------
// Screenshot polling
// ---------------------------------------------------------------------------

async function loadScreenshot() {
  if (state.screenshotBusy) return;
  if (state.frozen || !state.status || !state.status.attached) return;
  state.screenshotBusy = true;
  try {
    const query = '?t=' + Date.now() + (TOKEN ? '&token=' + encodeURIComponent(TOKEN) : '');
    const response = await fetch('/api/screenshot' + query, { headers: authHeaders() });
    if (!response.ok) throw new Error('screenshot request failed with HTTP ' + response.status);
    const blob = await response.blob();
    const objectUrl = URL.createObjectURL(blob);
    const image = byId('screenshot');
    await new Promise((resolve, reject) => {
      image.onload = () => resolve();
      image.onerror = () => reject(new Error('the screenshot could not be decoded'));
      image.src = objectUrl;
    });
    URL.revokeObjectURL(objectUrl);
    byId('stage-empty').hidden = true;
    drawOverlay();
  } catch (error) {
    // Keep the last good frame; acquisition errors are surfaced in status.
  } finally {
    state.screenshotBusy = false;
    scheduleScreenshot();
  }
}

function scheduleScreenshot() {
  clearTimeout(state.screenshotTimer);
  if (state.frozen || !state.status || !state.status.attached) return;
  state.screenshotTimer = setTimeout(loadScreenshot, Math.max(100, state.pollMs));
}

// ---------------------------------------------------------------------------
// Locators
// ---------------------------------------------------------------------------

function parseRef(value) {
  if (typeof value === 'number') return value;
  const text = String(value || '').trim();
  const digits = text.startsWith('e') ? text.slice(1) : text;
  const id = Number(digits);
  return Number.isFinite(id) ? id : 0;
}

async function resolveLocator(locator) {
  const value = (locator === undefined ? byId('locator-input').value : locator).trim();
  if (!value) {
    toast('Enter a locator first.', true);
    return;
  }
  try {
    const data = await api('/api/locator/resolve', { body: { locator: value } });
    state.matchIds = new Set((data.matches || []).map((match) => parseRef(match.id)));
    renderLocatorResult(data);
    renderTree();
    drawOverlay();
  } catch (error) {
    state.matchIds.clear();
    renderTree();
    drawOverlay();
    byId('locator-result').className = 'result error';
    byId('locator-result').textContent = error.message;
  }
}

async function verifyLocator() {
  const value = byId('locator-input').value.trim();
  if (!value) {
    toast('Enter a locator first.', true);
    return;
  }
  try {
    const data = await api('/api/locator/verify', { body: { locator: value } });
    state.matchIds = new Set((data.matches || []).map((match) => parseRef(match.id)));
    renderTree();
    drawOverlay();
    const result = byId('locator-result');
    result.className = 'result ' + (data.stable ? 'ok' : 'error');
    result.textContent = 'count: ' + data.count + '\nunique: ' + data.unique +
      '\nvisible: ' + data.visible + '\nstable: ' + data.stable;
  } catch (error) {
    byId('locator-result').className = 'result error';
    byId('locator-result').textContent = error.message;
  }
}

function renderLocatorResult(data) {
  const result = byId('locator-result');
  result.className = 'result ' + (data.unique ? 'ok' : '');
  const lines = ['count: ' + data.count + '  unique: ' + data.unique];
  for (const match of data.matches || []) {
    lines.push('  ' + match.id + '  ' + match.role + '  ' + (match.name || '') +
      (match.visible ? '' : '  (offscreen)'));
  }
  result.textContent = lines.join('\n');
}

// ---------------------------------------------------------------------------
// Scene authoring
// ---------------------------------------------------------------------------

async function refreshScenes() {
  const data = await api('/api/scenes');
  const select = byId('scene-select');
  const previous = select.value;
  clear(select);
  for (const scene of data.scenes || []) {
    const option = document.createElement('option');
    option.value = scene.path;
    option.textContent = scene.path + '  (' + scene.bytes + ' B)';
    select.appendChild(option);
  }
  if (previous) select.value = previous;
  if (!select.value && select.options.length > 0) select.selectedIndex = 0;
}

async function openScene(path) {
  const wanted = path || byId('scene-select').value;
  if (!wanted) {
    toast('Choose a scene first.', true);
    return;
  }
  try {
    const scene = await api('/api/scenes/open?path=' + encodeURIComponent(wanted));
    state.scene = scene;
    state.edit = null;
    clear(byId('scene-edit'));
    renderScene();
    byId('scene-select').value = scene.path;
  } catch (error) {
    toast(error.message, true);
  }
}

function renderScene() {
  const scene = state.scene;
  const meta = byId('scene-meta');
  const actions = byId('scene-actions');
  clear(actions);
  if (!scene) {
    meta.textContent = 'No scene open.';
    return;
  }
  const columns = (scene.columns || []).map((column) => column.name).join(', ');
  const diagnostics = scene.diagnostics || [];
  meta.textContent = [
    scene.path,
    'revision: ' + String(scene.revision || '').slice(0, 12),
    'columns: ' + (columns || '(none)'),
    scene.has_errors ? 'diagnostics: ' + diagnostics.join('; ') : 'valid',
  ].join('\n');
  actions.appendChild(makeButton('Add selected element', addSelectedElement, 'primary'));
  actions.appendChild(makeButton('Fill cell', fillCell));
  actions.appendChild(makeButton('Replace cell', replaceCell));
  actions.appendChild(makeButton('Validate', validateScene, 'secondary'));
  actions.appendChild(makeButton('Undo last inspector edit', undoScene, 'secondary'));
}

async function createScene() {
  const path = prompt('New scene path (workspace-relative)', 'scenes/new.scene');
  if (!path) return;
  const fallbackName = path.split('/').pop().replace(/\.scene$/i, '');
  const name = prompt('Scene name', fallbackName);
  if (!name) return;
  const platform = (state.status && state.status.platform) || 'android';
  const raw = prompt('Platform columns (comma separated)', platform);
  if (raw === null) return;
  const platforms = raw.split(',').map((value) => value.trim()).filter(Boolean);
  try {
    await api('/api/scenes/create', { body: { path, name, platforms } });
    toast('Created ' + path + '.');
    await refreshScenes();
    await openScene(path);
  } catch (error) {
    toast(error.message, true);
  }
}

function platformOfSelection() {
  return (state.status && state.status.platform) || undefined;
}

async function previewScene(op, options) {
  if (!state.scene) {
    toast('Open a scene first.', true);
    return;
  }
  const body = { path: state.scene.path, op };
  if (options && options.base_revision) body.base_revision = options.base_revision;
  try {
    const edit = await api('/api/scenes/preview', { body });
    state.edit = edit;
    renderEdit(edit);
  } catch (error) {
    if (error.code === 'SCENE_CONFIRM_REQUIRED' && !op.confirm) {
      const diff = (error.details && error.details.diff) || '';
      if (window.confirm('This cell is occupied. Replace it?\n\n' + diff)) {
        op.confirm = true;
        return previewScene(op, options);
      }
      return;
    }
    if (error.code === 'MISSING_COLUMN' && op.kind !== 'add_column') {
      const details = error.details || {};
      if (details.platform && window.confirm(error.message + '\n\nAdd column "' +
          (details.suggested || details.platform) + '"?')) {
        return previewScene({ kind: 'add_column', platform: details.platform }, options);
      }
      return;
    }
    toast(error.message, true);
  }
}

function renderEdit(edit) {
  const container = byId('scene-edit');
  clear(container);
  const meta = document.createElement('div');
  meta.className = 'meta';
  meta.textContent = 'Preview ' + String(edit.revision || '').slice(0, 12) +
    (edit.column ? ' · column ' + edit.column : '') +
    (edit.has_errors ? ' · diagnostics: ' + (edit.diagnostics || []).join('; ') : '');
  container.appendChild(meta);
  const pre = document.createElement('pre');
  pre.textContent = edit.diff || '(no textual diff)';
  container.appendChild(pre);
  const row = document.createElement('div');
  row.className = 'row';
  const commit = makeButton('Commit', () => commitEdit(edit), 'primary');
  commit.disabled = Boolean(edit.has_errors);
  row.appendChild(commit);
  row.appendChild(makeButton('Discard', () => {
    state.edit = null;
    clear(container);
  }, 'secondary'));
  container.appendChild(row);
}

async function commitEdit(edit) {
  try {
    const result = await api('/api/scenes/commit', { body: { edit_id: edit.edit_id } });
    toast('Committed ' + result.path + ' (' + String(result.revision || '').slice(0, 12) + ').');
    state.edit = null;
    clear(byId('scene-edit'));
    await refreshScenes();
    await openScene(result.path);
  } catch (error) {
    toast(error.message, true);
  }
}

async function addSelectedElement() {
  if (!state.selectedId) {
    toast('Select an element in the tree or on the screenshot first.', true);
    return;
  }
  try {
    const suggestions = await api('/api/node/suggest?id=' + state.selectedId);
    const best = (suggestions.suggestions || [])[0];
    if (!best) {
      toast('No locator candidate for this element.', true);
      return;
    }
    const node = findNode(state.selectedId);
    const seed = (node && (node.name || node.text || node.role)) || 'Element';
    const rowName = prompt('Piece name', seed.replace(/[^A-Za-z0-9 _.-]/g, '').trim() || 'Element');
    if (!rowName) return;
    const locator = prompt('Locator', best.locator);
    if (!locator) return;
    await previewScene({
      kind: 'add_element',
      row: rowName,
      platform: platformOfSelection(),
      locator,
      seed,
    });
  } catch (error) {
    toast(error.message, true);
  }
}

async function fillCell() {
  if (!state.scene) {
    toast('Open a scene first.', true);
    return;
  }
  const rowName = prompt('Piece name to fill');
  if (!rowName) return;
  const locator = prompt('Locator', byId('locator-input').value || '');
  if (!locator) return;
  await previewScene({
    kind: 'fill_cell',
    row: rowName,
    platform: platformOfSelection(),
    locator,
  });
}

async function replaceCell() {
  if (!state.scene) {
    toast('Open a scene first.', true);
    return;
  }
  const rowName = prompt('Piece name to replace');
  if (!rowName) return;
  const locator = prompt('Locator', byId('locator-input').value || '');
  if (!locator) return;
  await previewScene({
    kind: 'replace_cell',
    row: rowName,
    platform: platformOfSelection(),
    locator,
    confirm: false,
  });
}

async function validateScene() {
  if (!state.scene) {
    toast('Open a scene first.', true);
    return;
  }
  try {
    const data = await api('/api/scenes/preview', {
      body: { path: state.scene.path, op: { kind: 'validate' } },
    });
    const document = data.document || {};
    toast(document.has_errors
      ? 'Validation found errors:\n' + (document.diagnostics || []).join('\n')
      : 'Scene is valid.', document.has_errors);
  } catch (error) {
    toast(error.message, true);
  }
}

async function undoScene() {
  if (!state.scene) {
    toast('Open a scene first.', true);
    return;
  }
  try {
    const result = await api('/api/scenes/undo', { body: { path: state.scene.path } });
    toast('Restored ' + result.path + ' (' + String(result.revision || '').slice(0, 12) + ').');
    await refreshScenes();
    await openScene(result.path);
  } catch (error) {
    toast(error.message, true);
  }
}

// ---------------------------------------------------------------------------
// Live events and boot
// ---------------------------------------------------------------------------

function connectEvents() {
  const query = TOKEN ? '?token=' + encodeURIComponent(TOKEN) : '';
  const source = new EventSource('/api/events' + query);
  source.addEventListener('hello', (event) => {
    try {
      const data = JSON.parse(event.data);
      if (data.status) {
        state.status = data.status;
        renderStatus();
      }
    } catch (error) {
      // Ignore malformed keep-alive data.
    }
  });
  source.addEventListener('snapshot', (event) => {
    try {
      const data = JSON.parse(event.data);
      state.snapshot = data.snapshot;
      if (data.status) state.status = data.status;
      onSnapshot();
    } catch (error) {
      toast('The inspector sent an unreadable snapshot.', true);
    }
  });
  source.addEventListener('update', (event) => {
    try {
      const data = JSON.parse(event.data);
      if (data.kind === 'full') state.snapshot = data.snapshot;
      else if (data.kind === 'delta') applyDelta(data.delta);
      if (data.status) state.status = data.status;
      onSnapshot();
    } catch (error) {
      refreshSnapshot();
    }
  });
  source.addEventListener('error', () => {
    // EventSource reconnects on its own; the screenshot poll keeps the UI live.
  });
  window.addEventListener('beforeunload', () => source.close());
}

function bindUi() {
  byId('attach').addEventListener('click', attachDevice);
  byId('detach').addEventListener('click', detachDevice);
  byId('freeze').addEventListener('change', toggleFreeze);
  byId('refresh').addEventListener('click', async () => {
    await refreshStatus();
    await refreshSnapshot();
    loadScreenshot();
  });
  byId('expand-all').addEventListener('click', expandAll);
  byId('collapse-all').addEventListener('click', collapseAll);
  byId('tree-filter').addEventListener('input', (event) => {
    state.filter = event.target.value;
    buildRows();
    renderTree();
  });
  byId('resolve').addEventListener('click', () => resolveLocator());
  byId('verify').addEventListener('click', verifyLocator);
  byId('clear-matches').addEventListener('click', () => {
    state.matchIds.clear();
    byId('locator-result').textContent = '';
    renderTree();
    drawOverlay();
  });
  byId('locator-input').addEventListener('keydown', (event) => {
    if (event.key === 'Enter') resolveLocator();
  });
  byId('scene-open').addEventListener('click', () => openScene());
  byId('scene-new').addEventListener('click', createScene);
  byId('tree').addEventListener('scroll', () => {
    window.requestAnimationFrame(renderTree);
  });
  byId('stage').addEventListener('click', stageClick);
  byId('stage').addEventListener('mousemove', stageMove);
  byId('stage').addEventListener('mouseleave', () => setHover(0));
  window.addEventListener('resize', drawOverlay);
  document.addEventListener('keydown', (event) => {
    const tag = (event.target && event.target.tagName || '').toLowerCase();
    if (tag === 'input' || tag === 'select' || tag === 'textarea') return;
    if (event.key === '/') {
      event.preventDefault();
      byId('tree-filter').focus();
    } else if (event.key === 'f') {
      byId('freeze').click();
    } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      moveSelection(event.key === 'ArrowDown' ? 1 : -1);
    } else if (event.key === 'Enter' && state.selectedId) {
      event.preventDefault();
      toggleCollapsed(state.selectedId);
    }
  });
}

async function init() {
  bindUi();
  connectEvents();
  try {
    await Promise.all([refreshStatus(), refreshDevices(), refreshScenes()]);
  } catch (error) {
    toast(error.message, true);
  }
  await refreshSnapshot();
  if (state.status && state.status.attached) loadScreenshot();
}

document.addEventListener('DOMContentLoaded', () => {
  init().catch((error) => toast(error.message, true));
});
