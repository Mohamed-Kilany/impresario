'use strict';

/*
 * Impresario Inspector SPA.
 *
 * The server owns acquisition, pruning, locator resolution and scene edits.
 * This client renders the latest normalized snapshot, applies SSE deltas,
 * maps pointer coordinates back to nodes, and calls the JSON API. No user text
 * is ever assigned through innerHTML.
 *
 * Freeze is deliberately client-side (TASK-INSPECTOR 10.4): the server keeps
 * acquiring, while this page stops applying live snapshot/delta events so a
 * transient state can be inspected. Unfreeze fetches one full snapshot.
 */

const TOKEN = new URLSearchParams(window.location.search).get('token') || '';
const ROW_HEIGHT = 24;
const FILTER_DEBOUNCE_MS = 180;

const FALLBACK_KEYS = [
  'ok', 'enter', 'back', 'up', 'down', 'left', 'right', 'home', 'menu',
  'play', 'pause', 'playpause', 'rewind', 'fastforward', 'backspace',
  'volumeup', 'volumedown', 'mute', 'channelup', 'channeldown', 'stop',
  'record', 'info', 'guide', 'exit', 'subtitle', 'audio', 'search',
  'settings', 'red', 'green', 'yellow', 'blue',
  'num0', 'num1', 'num2', 'num3', 'num4', 'num5', 'num6', 'num7', 'num8', 'num9',
];

const state = {
  meta: null,
  status: null,
  snapshot: null,
  nodeIndex: new Map(),
  pollMs: 250,
  frozen: false,
  freezeInitialized: false,
  eventsOk: false,
  events: null,
  selectedId: 0,
  selectedFingerprint: null,
  selectedNode: null,
  selectionStale: false,
  selectionOffscreen: false,
  detailsFor: 0,
  detailsKey: '',
  detailsSeq: 0,
  detailsStale: false,
  suggestions: [],
  hoverId: 0,
  matchIds: new Set(),
  matchSource: '',
  locatorFilter: null,
  collapsed: new Set(),
  filter: '',
  filterTimer: 0,
  filterSeq: 0,
  rows: [],
  pendingRevealId: 0,
  scene: null,
  sceneCell: null,
  edit: null,
  pendingOp: null,
  hasFrame: false,
  frameNote: '',
  screenshotBusy: false,
  screenshotTimer: 0,
  screenshotFailures: 0,
  toastTimer: 0,
  hoverFrame: 0,
  drag: null,
  wheelAt: 0,
  statusTimer: 0,
};

function byId(id) {
  return document.getElementById(id);
}

function clear(element) {
  if (!element) return;
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
      message: 'Request failed with HTTP ' + response.status + '.',
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

async function copyText(text, label) {
  const value = String(text === undefined || text === null ? '' : text);
  if (!value) return;
  try {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      await navigator.clipboard.writeText(value);
    } else {
      const area = document.createElement('textarea');
      area.value = value;
      area.setAttribute('readonly', '');
      area.style.position = 'fixed';
      area.style.left = '-9999px';
      document.body.appendChild(area);
      area.select();
      document.execCommand('copy');
      document.body.removeChild(area);
    }
    toast((label || 'Value') + ' copied.');
  } catch (error) {
    toast('Could not copy: ' + error.message, true);
  }
}

function setInputStatus(text, isError) {
  const element = byId('input-status');
  element.textContent = text || '';
  element.classList.toggle('error', Boolean(isError));
}

// ---------------------------------------------------------------------------
// Status and devices
// ---------------------------------------------------------------------------

function applyStatus(status) {
  const wasAttached = Boolean(state.status && state.status.attached);
  state.status = status || null;
  if (wasAttached && !(status && status.attached)) clearSnapshot();
  renderStatus();
  if (!wasAttached && status && status.attached) scheduleScreenshot();
}

function renderStatus() {
  const status = state.status;
  const meta = state.meta;
  const attached = Boolean(status && status.attached);
  const pill = byId('attach-status');
  pill.textContent = attached ? 'attached' : 'disconnected';
  pill.classList.toggle('on', attached);
  pill.classList.toggle('error', Boolean(status && status.error));
  byId('rev').textContent = status && status.rev ? 'rev ' + status.rev : '';

  let note = state.frozen ? 'frozen' : (state.eventsOk ? 'live · ' + state.pollMs + ' ms' : 'reconnecting…');
  if (state.frameNote) note += ' · ' + state.frameNote;
  byId('fps').textContent = note;

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
    if (state.selectionStale) summary.push('selection stale');
    else if (state.selectionOffscreen) summary.push('selection offscreen');
    if (status.error) summary.push('error: ' + status.error);
  } else {
    summary.push('No device attached');
  }
  byId('status-text').textContent = summary.join('  ·  ');

  const empty = byId('stage-empty');
  empty.hidden = attached && state.hasFrame;
  empty.textContent = attached ? 'Waiting for the first frame…' : 'Attach a device to inspect its screen.';

  byId('attach').disabled = attached;
  byId('detach').disabled = !attached;
  byId('freeze').disabled = !attached;

  const enabled = attached && !state.frozen;
  for (const id of [
    'tap-selected', 'scroll-up', 'scroll-down', 'scroll-left', 'scroll-right',
    'send-key', 'send-type',
  ]) {
    byId(id).disabled = !enabled;
  }
  byId('key-select').disabled = !enabled;
  byId('type-text').disabled = !enabled;
  for (const button of document.querySelectorAll('.dpad button[data-key]')) {
    button.disabled = !enabled;
  }
}

async function refreshStatus() {
  const data = await api('/api/status');
  state.meta = data;
  state.pollMs = data.poll_interval_ms || 250;
  if (!state.freezeInitialized) {
    state.freezeInitialized = true;
    state.frozen = Boolean(data.frozen);
    byId('freeze').checked = state.frozen;
  }
  if (Array.isArray(data.keys) && data.keys.length) populateKeys(data.keys);
  applyStatus(data.status);
}

async function pollStatus() {
  if (document.hidden) return;
  try {
    const data = await api('/api/status');
    state.meta = data;
    state.pollMs = data.poll_interval_ms || state.pollMs;
    applyStatus(data.status);
  } catch (error) {
    state.eventsOk = false;
    renderStatus();
  }
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
    const data = await api('/api/attach', { body: { device: name } });
    applyStatus(data.status);
    if (data.status && data.status.error) toast(data.status.error, true);
    else toast('Attached to ' + name + '.');
    await Promise.all([refreshStatus(), refreshDevices()]);
    await refreshSnapshot();
    loadScreenshot();
  } catch (error) {
    toast(error.message, true);
  }
}

async function detachDevice() {
  try {
    await api('/api/detach', { body: {} });
    clearSnapshot();
    await Promise.all([refreshStatus(), refreshDevices()]);
    toast('Detached.');
  } catch (error) {
    toast(error.message, true);
  }
}

function clearSnapshot() {
  state.snapshot = null;
  state.nodeIndex = new Map();
  state.selectedId = 0;
  state.selectedFingerprint = null;
  state.selectedNode = null;
  state.selectionStale = false;
  state.selectionOffscreen = false;
  state.detailsFor = 0;
  state.detailsKey = '';
  state.detailsSeq += 1;
  state.detailsStale = false;
  state.suggestions = [];
  state.matchIds.clear();
  state.matchSource = '';
  state.locatorFilter = null;
  state.hoverId = 0;
  state.rows = [];
  state.pendingRevealId = 0;
  state.hasFrame = false;
  state.frameNote = '';
  state.screenshotFailures = 0;
  clearTimeout(state.screenshotTimer);
  byId('screenshot').removeAttribute('src');
  byId('details').textContent = 'Nothing selected.';
  clear(byId('suggestions'));
  clear(byId('locator-result'));
  renderTree();
  drawOverlay();
  renderStatus();
}

async function toggleFreeze() {
  const frozen = byId('freeze').checked;
  const previous = state.frozen;
  // Apply the client-side gate immediately: an SSE update can arrive while
  // the freeze request is still in flight.
  state.frozen = frozen;
  if (frozen) clearTimeout(state.screenshotTimer);
  renderStatus();
  try {
    await api('/api/freeze', { body: { frozen } });
    if (frozen) {
      toast('Frozen. Live updates are paused; the inspector keeps tracking.');
    } else {
      await refreshSnapshot();
      loadScreenshot();
      toast('Live updates resumed.');
    }
    renderStatus();
  } catch (error) {
    state.frozen = previous;
    byId('freeze').checked = previous;
    renderStatus();
    toast(error.message, true);
  }
}

// ---------------------------------------------------------------------------
// Snapshot, tree and selection
// ---------------------------------------------------------------------------

function findNode(id) {
  return state.nodeIndex.get(id) || null;
}

function fingerprintOf(node) {
  return {
    role: node.role || '',
    name: node.name || '',
    text: node.text || '',
    test_id: node.test_id || '',
    native_id: node.native_id || '',
  };
}

function nodeKey(node) {
  if (!node) return '';
  const bounds = node.bounds
    ? [Math.round(node.bounds.x), Math.round(node.bounds.y), Math.round(node.bounds.w), Math.round(node.bounds.h)].join(',')
    : '';
  return [
    node.id, node.role, node.name, node.text, node.value, node.test_id, node.native_id,
    bounds, node.visible_fraction,
    node.state ? JSON.stringify(node.state) : '',
    node.scroll ? JSON.stringify(node.scroll) : '',
  ].join('|');
}

function remapSelection(fingerprint) {
  if (!state.snapshot || !fingerprint) return null;
  const candidates = state.snapshot.nodes.filter((node) =>
    (node.role || '') === fingerprint.role &&
    (node.name || '') === fingerprint.name &&
    (node.test_id || '') === fingerprint.test_id
  );
  if (candidates.length === 1) return candidates[0];
  const byText = candidates.filter((node) => (node.text || '') === fingerprint.text);
  if (byText.length === 1) return byText[0];
  const byNative = candidates.filter((node) => (node.native_id || '') === fingerprint.native_id);
  return byNative.length === 1 ? byNative[0] : null;
}

function setSnapshot(snapshot) {
  if (!snapshot || !Array.isArray(snapshot.nodes)) return;
  state.snapshot = snapshot;
  state.nodeIndex = new Map(snapshot.nodes.map((node) => [node.id, node]));
  onSnapshot();
}

function onSnapshot() {
  if (!state.snapshot) {
    state.rows = [];
    renderTree();
    drawOverlay();
    renderStatus();
    return;
  }
  resolveSelection();
  buildRows();
  renderTree();
  drawOverlay();
  renderStatus();
  if (state.pendingRevealId) {
    const wanted = state.pendingRevealId;
    state.pendingRevealId = 0;
    revealRow(wanted);
  }
  maybeLoadDetails();
  // Keep a live locator filter accurate when the tree changes: snapshot ids
  // can shift between acquisitions.
  if (state.locatorFilter && state.snapshot &&
      state.locatorFilter.rev !== state.snapshot.rev &&
      state.filter.trim() && looksLikeLocator(state.filter.trim())) {
    runFilter();
  }
}

function resolveSelection() {
  if (!state.selectedId && !state.selectedFingerprint) return;
  const current = findNode(state.selectedId);
  if (current) {
    state.selectedNode = current;
    state.selectedFingerprint = fingerprintOf(current);
    state.selectionStale = false;
    state.selectionOffscreen = current.visible_fraction === 0 ||
      Boolean(current.state && current.state.hidden_by_a11y);
    state.detailsStale = false;
    return;
  }
  const remapped = remapSelection(state.selectedFingerprint);
  if (remapped) {
    const previous = state.selectedId;
    state.selectedId = remapped.id;
    state.selectedNode = remapped;
    state.selectedFingerprint = fingerprintOf(remapped);
    state.selectionStale = false;
    state.selectionOffscreen = remapped.visible_fraction === 0;
    state.detailsStale = false;
    if (previous) toast('Selection remapped to e' + remapped.id + '.');
    return;
  }
  state.selectionStale = true;
  state.selectionOffscreen = false;
  // Render the stale banner once instead of rebuilding it on every poll.
  if (!state.detailsStale && state.selectedNode) {
    state.detailsStale = true;
    renderDetails(state.selectedNode, { stale: true });
  }
}

function maybeLoadDetails() {
  if (!state.selectedId) return;
  const node = findNode(state.selectedId);
  if (!node) return;
  const key = nodeKey(node);
  if (state.detailsFor === node.id && state.detailsKey === key) return;
  loadDetails(node.id, key);
}

async function refreshSnapshot() {
  try {
    const data = await api('/api/snapshot');
    if (data.status && data.status.status) {
      state.meta = data.status;
      applyStatus(data.status.status);
    }
    setSnapshot(data);
  } catch (error) {
    if (error.code === 'NO_SNAPSHOT') {
      state.snapshot = null;
      state.nodeIndex = new Map();
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

  let keep = null;
  if (state.locatorFilter) {
    keep = new Set(state.locatorFilter.ids);
  } else if (state.filter.trim()) {
    const needle = state.filter.trim().toLowerCase();
    const matching = new Set();
    const textOf = (node) => [node.role, node.name, node.text, node.value, node.test_id, node.native_id]
      .filter(Boolean).join(' ').toLowerCase();
    for (const node of nodes) {
      if (textOf(node).includes(needle)) matching.add(node.id);
    }
    keep = matching;
  }

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
    // A filter must reveal matches even inside collapsed branches.
    const expanded = Boolean(keep) || !state.collapsed.has(node.id);
    rows.push({ node, depth, hasChildren: children.length > 0, expanded });
    if (expanded) {
      for (const child of children) walk(child, depth + 1);
    }
  };
  for (const root of byParent.get(0) || []) walk(root, 0);
  for (const node of nodes) {
    if (!seen.has(node.id)) rows.push({ node, depth: 0, hasChildren: false, expanded: false });
  }

  if (!keep) {
    state.rows = rows;
    return;
  }
  // Keep ancestors so matched nodes stay reachable in the tree.
  const index = new Map(nodes.map((node) => [node.id, node]));
  const visible = new Set(keep);
  for (const id of keep) {
    let current = index.get(id);
    let guard = 0;
    while (current && guard < 400) {
      visible.add(current.id);
      current = index.get(current.parent_id || 0);
      guard += 1;
    }
  }
  state.rows = rows.filter((row) => visible.has(row.node.id));
}

function renderTree() {
  const tree = byId('tree');
  if (!state.rows.length) {
    clear(tree);
    tree.dataset.ready = '';
    if (state.locatorFilter) tree.textContent = 'No nodes match this locator.';
    else if (state.filter.trim()) tree.textContent = 'No nodes match this search.';
    updateTreeCount();
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
  updateTreeCount();
}

function updateTreeCount() {
  const element = byId('tree-count');
  if (!element) return;
  element.classList.remove('error');
  const total = state.snapshot && Array.isArray(state.snapshot.nodes) ? state.snapshot.nodes.length : 0;
  if (state.locatorFilter) {
    element.textContent = state.locatorFilter.count + ' match' +
      (state.locatorFilter.count === 1 ? '' : 'es') + ' · ' + total + ' nodes';
    element.classList.add('ok');
  } else if (state.filter.trim()) {
    element.textContent = state.rows.length + ' shown · ' + total + ' nodes';
    element.classList.remove('ok');
  } else {
    element.textContent = total + ' nodes';
    element.classList.remove('ok');
  }
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
  if (node.visible_fraction === 0) element.classList.add('offscreen');

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

  const bounds = node.bounds
    ? Math.round(node.bounds.x) + ',' + Math.round(node.bounds.y) + ' ' +
      Math.round(node.bounds.w) + '×' + Math.round(node.bounds.h)
    : '';
  element.title = [
    'e' + node.id,
    node.role || 'generic',
    node.name || node.text || '',
    node.test_id ? 'testid=' + node.test_id : '',
    bounds,
  ].filter(Boolean).join('  ');

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

function updateTreeClasses() {
  const tree = byId('tree');
  for (const element of tree.querySelectorAll('.tree-row')) {
    const id = Number(element.dataset.id);
    element.classList.toggle('selected', id === state.selectedId);
    element.classList.toggle('hover', id === state.hoverId);
    element.classList.toggle('match', state.matchIds.has(id));
  }
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
  updateTreeClasses();
  drawOverlay();
}

function selectNode(id, options) {
  const node = findNode(id);
  if (!node) return;
  const changed = state.selectedId !== id;
  state.selectedId = id;
  state.selectedNode = node;
  state.selectedFingerprint = fingerprintOf(node);
  state.selectionStale = false;
  state.selectionOffscreen = node.visible_fraction === 0 ||
    Boolean(node.state && node.state.hidden_by_a11y);
  state.detailsStale = false;
  if (changed) {
    state.detailsFor = 0;
    state.detailsKey = '';
    state.suggestions = [];
    clear(byId('suggestions'));
    byId('details').textContent = 'Loading…';
  }
  if (options && options.reveal) {
    revealRow(id);
  } else {
    updateTreeClasses();
  }
  drawOverlay();
  renderStatus();
  maybeLoadDetails();
}

function revealRow(id) {
  const index = state.rows.findIndex((row) => row.node.id === id);
  if (index < 0) return;
  const tree = byId('tree');
  const top = index * ROW_HEIGHT;
  const height = tree.clientHeight || 320;
  if (top < tree.scrollTop) tree.scrollTop = top;
  else if (top + ROW_HEIGHT > tree.scrollTop + height) tree.scrollTop = top + ROW_HEIGHT - height;
  renderTree();
  updateTreeClasses();
}

function moveSelection(delta) {
  if (!state.rows.length) return;
  const index = state.rows.findIndex((row) => row.node.id === state.selectedId);
  const next = index < 0 ? 0 : Math.max(0, Math.min(state.rows.length - 1, index + delta));
  const row = state.rows[next];
  if (row) selectNode(row.node.id, { reveal: true });
}

async function loadDetails(id, key) {
  const seq = state.detailsSeq + 1;
  state.detailsSeq = seq;
  state.detailsFor = id;
  state.detailsKey = key;
  try {
    const node = await api('/api/node?id=' + id);
    const suggestions = await api('/api/node/suggest?id=' + id).catch(() => null);
    if (seq !== state.detailsSeq || state.selectedId !== id) return;
    state.selectedNode = node;
    if (!findNode(id)) {
      // The node disappeared while the request was in flight; keep the stale
      // banner instead of presenting the response as current.
      state.selectionStale = true;
      state.detailsStale = true;
      renderDetails(node, { stale: true });
      return;
    }
    state.detailsStale = false;
    renderDetails(node, { stale: false });
    if (suggestions) {
      state.suggestions = suggestions.suggestions || [];
      renderSuggestions(suggestions);
    }
  } catch (error) {
    if (seq !== state.detailsSeq || state.selectedId !== id) return;
    // Let the next snapshot retry the details request.
    state.detailsFor = 0;
    state.detailsKey = '';
    if (state.selectedNode) renderDetails(state.selectedNode, { stale: true, error: error.message });
  }
}

function renderDetails(node, options) {
  const details = byId('details');
  clear(details);
  if (!node) {
    details.textContent = 'Nothing selected.';
    return;
  }
  const settings = options || {};
  if (settings.stale) {
    const banner = document.createElement('div');
    banner.className = 'banner warn';
    banner.textContent = state.selectionOffscreen
      ? 'The selected node is outside the viewport. Showing the last known state.'
      : 'The selected node is no longer in the current snapshot. Showing the last known state.';
    details.appendChild(banner);
    const actions = document.createElement('div');
    actions.className = 'row';
    if (state.selectionOffscreen || node.bounds) {
      actions.appendChild(makeButton('Scroll to reveal', scrollToReveal, 'secondary'));
    }
    actions.appendChild(makeButton('Refresh snapshot', refreshSnapshot, 'secondary'));
    details.appendChild(actions);
    if (settings.error) {
      const error = document.createElement('div');
      error.className = 'meta error';
      error.textContent = settings.error;
      details.appendChild(error);
    }
  }

  const list = document.createElement('dl');
  const bounds = node.bounds
    ? Math.round(node.bounds.x) + ',' + Math.round(node.bounds.y) + ' ' +
      Math.round(node.bounds.w) + '×' + Math.round(node.bounds.h)
    : '';
  const fields = [
    ['Ref', node.ref || ('e' + node.id)],
    ['Role', node.role],
    ['Name', node.name],
    ['Text', node.text],
    ['Value', node.secure ? '***' : node.value],
    ['Test id', node.test_id],
    ['Native id', node.native_id],
    ['Bounds', bounds],
    ['Visible', node.visible_fraction === undefined ? '' : Math.round(node.visible_fraction * 100) + '%'],
    ['State', stateSummary(node.state)],
    ['Scroll', scrollSummary(node.scroll)],
    ['CSS', node.css_selector],
    ['XPath', node.xpath],
    ['Children', node.children === undefined ? '' : String(node.children)],
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

  const actions = document.createElement('div');
  actions.className = 'row copy-row';
  actions.appendChild(makeButton('Copy ref', () => copyText(node.ref || ('e' + node.id), 'Ref'), 'secondary copy'));
  actions.appendChild(makeButton('Copy JSON', () => copyText(JSON.stringify(node, null, 2), 'Node JSON'), 'secondary copy'));
  if (state.suggestions.length) {
    actions.appendChild(makeButton('Copy best locator', () => copyText(state.suggestions[0].locator, 'Locator'), 'secondary copy'));
  }
  details.appendChild(actions);

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

async function scrollToReveal() {
  const node = state.selectedNode;
  const screen = state.snapshot && state.snapshot.screen ? state.snapshot.screen : {};
  let direction = 'down';
  if (node && node.bounds && screen.logical_h) {
    if (node.bounds.y + node.bounds.h <= 0) direction = 'up';
    else if (node.bounds.y >= screen.logical_h) direction = 'down';
    else direction = node.bounds.y < screen.logical_h / 2 ? 'up' : 'down';
  }
  await sendScroll(direction, 3);
  await refreshSnapshot();
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
    const row = document.createElement('div');
    row.className = 'suggestion-row';
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
      resolveLocator(suggestion.locator, { reveal: true });
    });
    row.appendChild(button);
    row.appendChild(makeButton('Copy', () => copyText(suggestion.locator, 'Locator'), 'secondary copy'));
    container.appendChild(row);
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
  const drawNode = (node, color, width, dashed) => {
    if (!node || !node.bounds) return;
    const bounds = node.bounds;
    context.save();
    context.strokeStyle = color;
    context.lineWidth = width;
    if (dashed) context.setLineDash([4, 3]);
    context.strokeRect(
      bounds.x * scaleX,
      bounds.y * scaleY,
      Math.max(1, bounds.w * scaleX),
      Math.max(1, bounds.h * scaleY)
    );
    context.restore();
  };
  if (!Array.isArray(snapshot.nodes)) return;
  for (const node of snapshot.nodes) {
    if (state.matchIds.has(node.id)) drawNode(node, '#f2c14e', 1.5);
  }
  for (const node of snapshot.nodes) {
    if (node.state && node.state.focused) drawNode(node, '#c792ea', 1.5);
    if (node.scroll) {
      drawNode(node, '#8d9ab0', 1, true);
      drawScrollBadge(context, node, scaleX, scaleY);
    }
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

function drawScrollBadge(context, node, scaleX, scaleY) {
  if (!node.bounds) return;
  const x = Math.max(0, node.bounds.x * scaleX + 2);
  const y = Math.max(10, node.bounds.y * scaleY + 10);
  context.save();
  context.font = '10px "Segoe UI", system-ui, sans-serif';
  context.fillStyle = 'rgba(10, 14, 20, 0.75)';
  context.fillRect(x - 2, y - 9, 38, 12);
  context.fillStyle = '#8d9ab0';
  context.fillText(node.scroll && node.scroll.axis === 'horizontal' ? '↔ scroll' : '↕ scroll', x, y);
  context.restore();
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

function stagePointerDown(event) {
  if (event.button !== undefined && event.button !== 0) return;
  const point = stagePoint(event);
  if (!point) return;
  state.drag = {
    start: point,
    last: point,
    startX: event.clientX,
    startY: event.clientY,
    pointerId: event.pointerId,
    moved: false,
  };
  const stage = byId('stage');
  if (stage.setPointerCapture) {
    try {
      stage.setPointerCapture(event.pointerId);
    } catch (error) {
      // Pointer capture is a nicety; dragging still works inside the stage.
    }
  }
  stage.classList.add('dragging');
}

function stagePointerMove(event) {
  const drag = state.drag;
  if (drag && drag.pointerId === event.pointerId) {
    const point = stagePoint(event);
    if (point) {
      drag.last = point;
      if (Math.abs(event.clientX - drag.startX) + Math.abs(event.clientY - drag.startY) > 10) {
        drag.moved = true;
      }
    }
    return;
  }
  if (state.hoverFrame) return;
  state.hoverFrame = window.requestAnimationFrame(() => {
    state.hoverFrame = 0;
    const point = stagePoint(event);
    if (!point) return;
    const candidates = nodesAt(point);
    setHover(candidates.length ? candidates[0].id : 0);
  });
}

async function stagePointerUp(event) {
  const drag = state.drag;
  if (!drag || drag.pointerId !== event.pointerId) return;
  state.drag = null;
  byId('stage').classList.remove('dragging');
  if (drag.moved) {
    await sendSwipe(drag.start.x, drag.start.y, drag.last.x, drag.last.y, 0);
    return;
  }
  const point = stagePoint(event) || drag.start;
  const candidates = nodesAt(point);
  if (!candidates.length) return;
  let node = candidates[0];
  if (event.altKey || event.metaKey) {
    const index = candidates.findIndex((candidate) => candidate.id === state.selectedId);
    node = candidates[(index + 1) % candidates.length];
  }
  selectNode(node.id);
}

function stageWheel(event) {
  if (!state.status || !state.status.attached || state.frozen) return;
  event.preventDefault();
  const now = Date.now();
  if (now - state.wheelAt < 120) return;
  state.wheelAt = now;
  const direction = Math.abs(event.deltaY) >= Math.abs(event.deltaX)
    ? (event.deltaY > 0 ? 'down' : 'up')
    : (event.deltaX > 0 ? 'right' : 'left');
  sendScroll(direction, 1);
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

function populateKeys(keys) {
  const select = byId('key-select');
  const previous = select.value;
  clear(select);
  for (const key of keys) {
    const option = document.createElement('option');
    option.value = key;
    option.textContent = key;
    select.appendChild(option);
  }
  if (previous && keys.includes(previous)) select.value = previous;
  if (!select.value && select.options.length > 0) select.value = 'ok';
}

async function sendInput(body, label) {
  if (!state.status || !state.status.attached) {
    toast('Attach a device first.', true);
    return null;
  }
  if (state.frozen) {
    toast('Unfreeze to interact with the device.', true);
    return null;
  }
  try {
    const data = await api('/api/input', { body });
    setInputStatus((label || body.kind) + ' · rev ' + data.rev);
    if (data.status) applyStatus(data.status);
    await refreshSnapshot();
    loadScreenshot();
    return data;
  } catch (error) {
    setInputStatus((label || body.kind) + ' failed', true);
    toast(error.message, true);
    return null;
  }
}

function selectionRef() {
  if (!state.selectedId || state.selectionStale) return 0;
  return state.selectedId;
}

async function tapSelected() {
  if (!state.selectedId) {
    toast('Select an element in the tree or on the screenshot first.', true);
    return;
  }
  if (state.selectionStale) {
    toast('The selected node is stale; refresh the snapshot first.', true);
    return;
  }
  if (state.selectionOffscreen) {
    toast('The selected node is outside the viewport; scroll to reveal it first.', true);
    return;
  }
  await sendInput({ kind: 'tap', ref: 'e' + state.selectedId }, 'tap');
}

async function sendScroll(direction, count) {
  const body = { kind: 'scroll', direction, count: count || 1 };
  const ref = selectionRef();
  if (ref) body.ref = 'e' + ref;
  await sendInput(body, 'scroll ' + direction);
}

async function sendKey(name) {
  await sendInput({ kind: 'key', key: name }, 'key ' + name);
}

async function sendType() {
  const input = byId('type-text');
  const text = input.value;
  if (!text) {
    toast('Enter text to type.', true);
    return;
  }
  const body = { kind: 'type', text };
  const ref = selectionRef();
  if (ref) body.ref = 'e' + ref;
  const result = await sendInput(body, 'type');
  if (result) input.value = '';
}

async function sendSwipe(x1, y1, x2, y2, durationMs) {
  const body = {
    kind: 'swipe',
    x: x1,
    y: y1,
    end_x: x2,
    end_y: y2,
    duration_ms: durationMs || 0,
  };
  await sendInput(body, 'swipe');
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

function looksLikeLocator(value) {
  const text = String(value || '').trim();
  if (!text) return false;
  if (text === 'focused') return true;
  if (text.includes('>>') || text.includes('within(')) return true;
  return /(^|\s)(role|text|name|testid|id|css|xpath|region|state|index|nth|level|value)\s*=/i.test(text);
}

function applyMatches(data) {
  state.matchIds = new Set((data.matches || []).map((match) => parseRef(match.id)));
  state.matchSource = 'panel';
}

async function resolveLocator(locator, options) {
  const settings = options || {};
  const value = (locator === undefined ? byId('locator-input').value : locator).trim();
  if (!value) {
    toast('Enter a locator first.', true);
    return null;
  }
  try {
    const data = await api('/api/locator/resolve', { body: { locator: value } });
    applyMatches(data);
    renderLocatorResult(data);
    if (settings.reveal && data.unique && data.matches && data.matches.length === 1) {
      selectNode(parseRef(data.matches[0].id), { reveal: true });
    }
    renderTree();
    drawOverlay();
    return data;
  } catch (error) {
    state.matchIds.clear();
    renderTree();
    drawOverlay();
    const result = byId('locator-result');
    result.className = 'result error';
    result.textContent = error.message;
    if (!settings.quiet) toast(error.message, true);
    return null;
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
    applyMatches(data);
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
  clear(result);
  const summary = document.createElement('div');
  summary.textContent = 'count: ' + data.count + '  unique: ' + data.unique;
  result.appendChild(summary);
  for (const match of data.matches || []) {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'match-link';
    button.textContent = match.id + '  ' + match.role + '  ' + (match.name || '') +
      (match.visible ? '' : '  (offscreen)');
    button.addEventListener('click', () => selectNode(parseRef(match.id), { reveal: true }));
    result.appendChild(button);
  }
}

// ---------------------------------------------------------------------------
// Tree filter (substring or locator)
// ---------------------------------------------------------------------------

function onFilterInput(event) {
  state.filter = event.target.value;
  clearTimeout(state.filterTimer);
  state.filterTimer = setTimeout(runFilter, FILTER_DEBOUNCE_MS);
}

async function runFilter() {
  const value = state.filter.trim();
  const seq = state.filterSeq + 1;
  state.filterSeq = seq;
  if (!looksLikeLocator(value)) {
    state.locatorFilter = null;
    if (state.matchSource === 'tree') {
      state.matchIds.clear();
      state.matchSource = '';
      drawOverlay();
    }
    buildRows();
    renderTree();
    return;
  }
  try {
    const data = await api('/api/locator/resolve', { body: { locator: value } });
    if (seq !== state.filterSeq) return;
    state.locatorFilter = {
      ids: new Set((data.matches || []).map((match) => parseRef(match.id))),
      count: data.count,
      rev: data.rev,
    };
    state.matchIds = new Set(state.locatorFilter.ids);
    state.matchSource = 'tree';
    buildRows();
    renderTree();
    drawOverlay();
  } catch (error) {
    if (seq !== state.filterSeq) return;
    state.locatorFilter = null;
    if (state.matchSource === 'tree') {
      state.matchIds.clear();
      state.matchSource = '';
    }
    buildRows();
    renderTree();
    drawOverlay();
    const count = byId('tree-count');
    count.textContent = 'invalid locator: ' + error.message;
    count.classList.add('error');
  }
}

function clearMatches() {
  state.matchIds.clear();
  state.matchSource = '';
  state.locatorFilter = null;
  byId('locator-result').textContent = '';
  byId('tree-filter').value = '';
  state.filter = '';
  buildRows();
  renderTree();
  drawOverlay();
}

// ---------------------------------------------------------------------------
// Scene authoring
// ---------------------------------------------------------------------------

function platformKey(value) {
  const key = String(value || '').toLowerCase();
  if (key === 'tizenos') return 'tizen';
  return key;
}

function platformOfSelection() {
  return (state.status && state.status.platform) || undefined;
}

function sceneDocument() {
  return state.edit || state.scene;
}

function selectorFor(piece, platform) {
  if (!piece || !platform) return '';
  const key = platformKey(platform);
  const selector = (piece.selectors || []).find((candidate) => platformKey(candidate.key) === key);
  return selector ? selector.raw : '';
}

async function refreshScenes() {
  const data = await api('/api/scenes');
  const select = byId('scene-select');
  const previous = state.scene ? state.scene.path : select.value;
  clear(select);
  const scenes = data.scenes || [];
  if (!scenes.length) {
    const option = document.createElement('option');
    option.value = '';
    option.textContent = 'No scenes found';
    option.disabled = true;
    select.appendChild(option);
    return;
  }
  for (const scene of scenes) {
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
  if (state.edit && !window.confirm('Discard the previewed edit?')) return;
  try {
    const scene = await api('/api/scenes/open?path=' + encodeURIComponent(wanted));
    state.scene = scene;
    state.edit = null;
    state.sceneCell = null;
    state.pendingOp = null;
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
  if (!scene) {
    meta.textContent = 'No scene open.';
    clear(byId('scene-table'));
    renderSceneActions();
    return;
  }
  const columns = (scene.columns || [])
    .map((column) => column.name + (column.platform ? ' (' + column.platform + ')' : ''))
    .join(', ');
  const diagnostics = scene.diagnostics || [];
  const lines = [
    scene.path,
    'revision: ' + String(scene.revision || '').slice(0, 12),
    'columns: ' + (columns || '(none)'),
    scene.has_errors ? 'diagnostics: ' + diagnostics.join('; ') : 'valid',
  ];
  if (state.edit) lines.push('preview pending (not committed)');
  if (state.sceneCell) lines.push('cell: ' + state.sceneCell.row + ' / ' + state.sceneCell.column);
  meta.textContent = lines.join('\n');
  renderSceneTable();
  renderSceneActions();
}

function renderSceneTable() {
  const container = byId('scene-table');
  const scrollTop = container.scrollTop;
  const scrollLeft = container.scrollLeft;
  clear(container);
  const sceneDoc = sceneDocument();
  if (!sceneDoc) {
    container.textContent = 'No scene open.';
    return;
  }
  container.classList.toggle('pending', Boolean(state.edit));
  // The index column is shown separately; only platform columns are editable.
  const columns = (sceneDoc.columns || []).filter((column) => Boolean(column.platform));
  const pieces = (sceneDoc.scene && sceneDoc.scene.pieces) || [];
  if (!columns.length) {
    container.textContent = 'This scene has no table columns. Add one for the attached platform.';
    return;
  }
  const table = document.createElement('table');
  const thead = document.createElement('thead');
  const headRow = document.createElement('tr');
  for (const label of ['Piece', 'Index']) {
    const th = document.createElement('th');
    th.textContent = label;
    headRow.appendChild(th);
  }
  for (const column of columns) {
    const th = document.createElement('th');
    th.textContent = column.name;
    th.title = column.platform || 'index';
    headRow.appendChild(th);
  }
  thead.appendChild(headRow);
  table.appendChild(thead);

  const tbody = document.createElement('tbody');
  if (!pieces.length) {
    const tr = document.createElement('tr');
    const td = document.createElement('td');
    td.colSpan = columns.length + 2;
    td.className = 'empty-cell';
    td.textContent = 'No pieces yet. Select an element and use “Add selected element”.';
    tr.appendChild(td);
    tbody.appendChild(tr);
  }
  for (const piece of pieces) {
    const tr = document.createElement('tr');
    const nameCell = document.createElement('td');
    const code = document.createElement('code');
    code.textContent = piece.name;
    nameCell.appendChild(code);
    tr.appendChild(nameCell);
    const indexCell = document.createElement('td');
    indexCell.textContent = String(piece.index || 0);
    tr.appendChild(indexCell);
    for (const column of columns) {
      const td = document.createElement('td');
      const raw = selectorFor(piece, column.platform);
      td.className = 'cell' + (raw ? '' : ' empty-cell');
      if (state.sceneCell && state.sceneCell.row === piece.name &&
          state.sceneCell.column === column.name) {
        td.classList.add('picked');
      }
      if (state.edit && state.edit.column === column.name) td.classList.add('pending');
      td.textContent = raw || '—';
      td.title = raw || 'Empty cell — select an element, then click here';
      td.addEventListener('click', () => {
        state.sceneCell = { row: piece.name, column: column.name, platform: column.platform };
        renderScene();
      });
      tr.appendChild(td);
    }
    tbody.appendChild(tr);
  }
  table.appendChild(tbody);
  container.appendChild(table);
  container.scrollTop = scrollTop;
  container.scrollLeft = scrollLeft;
}

function renderSceneActions() {
  const actions = byId('scene-actions');
  clear(actions);
  if (!state.scene) return;
  // Selection-dependent actions stay enabled and validate on click: the
  // action row must not be rebuilt on every poll, or a click can land on a
  // button that is replaced between mousedown and mouseup.
  actions.appendChild(makeButton('Add selected element', addSelectedElement, 'primary'));

  if (state.sceneCell) {
    actions.appendChild(makeButton('Fill ' + state.sceneCell.row + ' / ' + state.sceneCell.column, fillCell, 'primary'));
    actions.appendChild(makeButton('Replace ' + state.sceneCell.row + ' / ' + state.sceneCell.column, replaceCell, 'secondary'));
  } else {
    actions.appendChild(makeButton('Fill cell…', fillCell, 'secondary'));
    actions.appendChild(makeButton('Replace cell…', replaceCell, 'secondary'));
  }

  const platform = platformOfSelection();
  if (platform) {
    actions.appendChild(makeButton('Add ' + platform + ' column', () => {
      previewScene({ kind: 'add_column', platform });
    }, 'secondary'));
  }
  actions.appendChild(makeButton('Validate', validateScene, 'secondary'));
  actions.appendChild(makeButton('Reload from disk', () => openScene(state.scene.path), 'secondary'));
  actions.appendChild(makeButton('Undo last inspector edit', undoScene, 'secondary'));
}

async function createScene() {
  const path = prompt('New scene path (workspace-relative)', 'scenes/new.scene');
  if (!path) return;
  const fallbackName = path.split('/').pop().replace(/\.scene$/i, '');
  const name = prompt('Scene name', fallbackName);
  if (!name) return;
  const platform = platformOfSelection() || 'android';
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

async function bestLocatorForSelection() {
  if (!state.selectedId || state.selectionStale) return '';
  try {
    const data = await api('/api/node/suggest?id=' + state.selectedId);
    const best = (data.suggestions || [])[0];
    return best ? best.locator : '';
  } catch (error) {
    return '';
  }
}

async function addSelectedElement() {
  if (!state.selectedId || state.selectionStale) {
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
    const op = { kind: 'add_element', row: rowName, locator, seed };
    const platform = platformOfSelection();
    if (platform) op.platform = platform;
    await previewScene(op);
  } catch (error) {
    toast(error.message, true);
  }
}

async function fillCell() {
  if (!state.scene) {
    toast('Open a scene first.', true);
    return;
  }
  let row = state.sceneCell && state.sceneCell.row;
  let column = state.sceneCell && state.sceneCell.column;
  if (!row) {
    row = prompt('Piece name to fill');
    if (!row) return;
  }
  if (!column) {
    const locator = byId('locator-input').value.trim() || await bestLocatorForSelection();
    if (!locator) {
      toast('Select an element or enter a locator.', true);
      return;
    }
    const op = { kind: 'fill_cell', row, locator };
    const platform = platformOfSelection();
    if (platform) op.platform = platform;
    await previewScene(op);
    return;
  }
  const locator = byId('locator-input').value.trim() || await bestLocatorForSelection();
  if (!locator) {
    toast('Select an element or enter a locator.', true);
    return;
  }
  await previewScene({ kind: 'fill_cell', row, column, locator });
}

async function replaceCell() {
  if (!state.scene) {
    toast('Open a scene first.', true);
    return;
  }
  let row = state.sceneCell && state.sceneCell.row;
  let column = state.sceneCell && state.sceneCell.column;
  if (!row) {
    row = prompt('Piece name to replace');
    if (!row) return;
  }
  const locator = byId('locator-input').value.trim() || await bestLocatorForSelection();
  if (!locator) {
    toast('Select an element or enter a locator.', true);
    return;
  }
  const op = { kind: 'replace_cell', row, locator, confirm: false };
  if (column) op.column = column;
  else {
    const platform = platformOfSelection();
    if (platform) op.platform = platform;
  }
  await previewScene(op);
}

async function previewScene(op, options) {
  if (!state.scene) {
    toast('Open a scene first.', true);
    return;
  }
  const body = {
    path: state.scene.path,
    base_revision: state.scene.revision,
    op,
  };
  if (options && options.base_revision) body.base_revision = options.base_revision;
  try {
    const edit = await api('/api/scenes/preview', { body });
    state.edit = edit;
    renderEdit(edit);
    renderScene();
  } catch (error) {
    if (error.code === 'SCENE_CONFIRM_REQUIRED' && !op.confirm) {
      const diff = (error.details && error.details.diff) || '';
      if (window.confirm('This cell is occupied. Replace it?\n\n' + diff)) {
        op.confirm = true;
        return previewScene(op, options);
      }
      return;
    }
    if (error.code === 'AMBIGUOUS_COLUMN') {
      const candidates = (error.details && error.details.candidates) || [];
      const chosen = prompt('Choose the column for this element:\n' + candidates.join('\n'), candidates[0] || '');
      if (chosen) {
        op.column = chosen;
        delete op.platform;
        return previewScene(op, options);
      }
      return;
    }
    if (error.code === 'MISSING_COLUMN' && op.kind !== 'add_column') {
      const details = error.details || {};
      const suggested = details.suggested || details.platform;
      if (suggested && window.confirm(error.message + '\n\nAdd column "' + suggested + '"?')) {
        state.pendingOp = { op, options };
        return previewScene({ kind: 'add_column', platform: details.platform }, options);
      }
      return;
    }
    if (error.code === 'SCENE_CONFLICT') {
      if (window.confirm('The scene changed on disk. Reload it and discard this preview?')) {
        state.edit = null;
        await openScene(state.scene.path);
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
  for (const line of String(edit.diff || '(no textual diff)').split('\n')) {
    const span = document.createElement('div');
    span.textContent = line;
    if (line.startsWith('+') && !line.startsWith('+++')) span.className = 'diff-add';
    else if (line.startsWith('-') && !line.startsWith('---')) span.className = 'diff-del';
    pre.appendChild(span);
  }
  container.appendChild(pre);
  const row = document.createElement('div');
  row.className = 'row';
  const commit = makeButton('Commit', () => commitEdit(edit), 'primary');
  commit.disabled = Boolean(edit.has_errors);
  row.appendChild(commit);
  row.appendChild(makeButton('Discard', () => {
    state.edit = null;
    state.pendingOp = null;
    clear(container);
    renderScene();
  }, 'secondary'));
  container.appendChild(row);
}

async function commitEdit(edit) {
  try {
    const result = await api('/api/scenes/commit', { body: { edit_id: edit.edit_id } });
    toast('Committed ' + result.path + ' (' + String(result.revision || '').slice(0, 12) + ').');
    const pending = state.pendingOp;
    state.pendingOp = null;
    state.edit = null;
    clear(byId('scene-edit'));
    await refreshScenes();
    await openScene(result.path);
    if (pending) await previewScene(pending.op, pending.options);
  } catch (error) {
    toast(error.message, true);
  }
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
    const doc = data.document || {};
    toast(doc.has_errors
      ? 'Validation found errors:\n' + (doc.diagnostics || []).join('\n')
      : 'Scene is valid.', doc.has_errors);
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
// Screenshot polling
// ---------------------------------------------------------------------------

async function loadScreenshot() {
  if (state.screenshotBusy) return;
  if (state.frozen || !state.status || !state.status.attached) return;
  state.screenshotBusy = true;
  try {
    const query = '?t=' + Date.now() + (TOKEN ? '&token=' + encodeURIComponent(TOKEN) : '');
    const response = await fetch('/api/screenshot' + query, { headers: authHeaders() });
    if (!response.ok) {
      let message = 'screenshot request failed with HTTP ' + response.status;
      try {
        const payload = await response.json();
        if (payload && payload.error && payload.error.message) message = payload.error.message;
      } catch (error) {
        // Keep the HTTP status message.
      }
      throw new Error(message);
    }
    const blob = await response.blob();
    const objectUrl = URL.createObjectURL(blob);
    const image = byId('screenshot');
    await new Promise((resolve, reject) => {
      image.onload = () => resolve();
      image.onerror = () => reject(new Error('the screenshot could not be decoded'));
      image.src = objectUrl;
    });
    URL.revokeObjectURL(objectUrl);
    state.hasFrame = true;
    state.screenshotFailures = 0;
    state.frameNote = '';
    byId('stage-empty').hidden = true;
    drawOverlay();
    renderStatus();
  } catch (error) {
    state.screenshotFailures = Math.min(6, state.screenshotFailures + 1);
    state.frameNote = state.screenshotFailures > 1 ? 'screenshot unavailable' : '';
    renderStatus();
  } finally {
    state.screenshotBusy = false;
    scheduleScreenshot();
  }
}

function scheduleScreenshot() {
  clearTimeout(state.screenshotTimer);
  if (state.frozen || !state.status || !state.status.attached) return;
  const backoff = Math.min(4000, Math.max(100, state.pollMs) * Math.pow(2, state.screenshotFailures));
  state.screenshotTimer = setTimeout(loadScreenshot, state.screenshotFailures ? backoff : Math.max(100, state.pollMs));
}

// ---------------------------------------------------------------------------
// Live events and boot
// ---------------------------------------------------------------------------

function connectEvents() {
  const query = TOKEN ? '?token=' + encodeURIComponent(TOKEN) : '';
  const source = new EventSource('/api/events' + query);
  state.events = source;
  source.onopen = () => {
    state.eventsOk = true;
    renderStatus();
  };
  source.onerror = () => {
    state.eventsOk = false;
    renderStatus();
  };
  source.addEventListener('hello', (event) => {
    try {
      const data = JSON.parse(event.data);
      if (data.status) applyStatus(data.status);
    } catch (error) {
      // Ignore malformed keep-alive data.
    }
  });
  source.addEventListener('snapshot', (event) => {
    try {
      const data = JSON.parse(event.data);
      // The initial SSE snapshot carries the full status document, whose
      // attachment status is nested under `status`.
      if (data.status) applyStatus(data.status.status || data.status);
      if (state.frozen) return;
      if (data.snapshot) setSnapshot(data.snapshot);
    } catch (error) {
      toast('The inspector sent an unreadable snapshot.', true);
    }
  });
  source.addEventListener('update', (event) => {
    try {
      const data = JSON.parse(event.data);
      if (data.status) applyStatus(data.status);
      if (data.kind === 'status') return;
      if (state.frozen) return;
      if (data.kind === 'full' && data.snapshot) {
        setSnapshot(data.snapshot);
        return;
      }
      if (data.kind === 'delta' && data.delta) {
        if (!applyDelta(data.delta)) {
          refreshSnapshot();
          return;
        }
        if (data.screen) state.snapshot.screen = data.screen;
        if (data.degraded) state.snapshot.degraded = data.degraded;
        if (typeof data.settled === 'boolean') state.snapshot.settled = data.settled;
        state.nodeIndex = new Map(state.snapshot.nodes.map((node) => [node.id, node]));
        onSnapshot();
      }
    } catch (error) {
      refreshSnapshot();
    }
  });
  window.addEventListener('beforeunload', () => source.close());
}

function applyDelta(delta) {
  const snapshot = state.snapshot;
  if (!snapshot || !delta || delta.base_rev !== snapshot.rev) return false;
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
  return true;
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
  byId('tree-filter').addEventListener('input', onFilterInput);
  byId('tree').addEventListener('scroll', () => {
    window.requestAnimationFrame(renderTree);
  });

  byId('resolve').addEventListener('click', () => resolveLocator(undefined, { reveal: false }));
  byId('verify').addEventListener('click', verifyLocator);
  byId('clear-matches').addEventListener('click', clearMatches);
  byId('locator-input').addEventListener('keydown', (event) => {
    if (event.key === 'Enter') resolveLocator(undefined, { reveal: true });
  });
  let locatorTimer = 0;
  byId('locator-input').addEventListener('input', (event) => {
    clearTimeout(locatorTimer);
    const value = event.target.value.trim();
    if (!looksLikeLocator(value)) return;
    locatorTimer = setTimeout(() => resolveLocator(value, { quiet: true }), 300);
  });

  byId('scene-open').addEventListener('click', () => openScene());
  byId('scene-new').addEventListener('click', createScene);

  byId('stage').addEventListener('pointerdown', stagePointerDown);
  byId('stage').addEventListener('pointermove', stagePointerMove);
  byId('stage').addEventListener('pointerup', stagePointerUp);
  byId('stage').addEventListener('pointercancel', () => {
    state.drag = null;
    byId('stage').classList.remove('dragging');
  });
  byId('stage').addEventListener('pointerleave', () => setHover(0));
  byId('stage').addEventListener('wheel', stageWheel, { passive: false });

  byId('tap-selected').addEventListener('click', tapSelected);
  byId('scroll-up').addEventListener('click', () => sendScroll('up', 1));
  byId('scroll-down').addEventListener('click', () => sendScroll('down', 1));
  byId('scroll-left').addEventListener('click', () => sendScroll('left', 1));
  byId('scroll-right').addEventListener('click', () => sendScroll('right', 1));
  byId('send-key').addEventListener('click', () => sendKey(byId('key-select').value));
  byId('send-type').addEventListener('click', sendType);
  byId('type-text').addEventListener('keydown', (event) => {
    if (event.key === 'Enter') {
      event.preventDefault();
      sendType();
    }
  });
  for (const button of document.querySelectorAll('.dpad button[data-key]')) {
    button.addEventListener('click', () => sendKey(button.dataset.key));
  }

  window.addEventListener('resize', drawOverlay);
  if (window.ResizeObserver) {
    const observer = new ResizeObserver(() => drawOverlay());
    observer.observe(byId('screenshot'));
    observer.observe(byId('stage'));
  }
  document.addEventListener('keydown', (event) => {
    const tag = (event.target && event.target.tagName || '').toLowerCase();
    const typing = tag === 'input' || tag === 'select' || tag === 'textarea' ||
      tag === 'button' || tag === 'a';
    if (typing) return;
    if (event.key === '/') {
      event.preventDefault();
      byId('tree-filter').focus();
    } else if (event.key === 'f') {
      byId('freeze').click();
    } else if (event.key === 'Escape') {
      event.preventDefault();
      sendKey('back');
    } else if (event.ctrlKey && event.key === 'Enter') {
      event.preventDefault();
      sendKey('ok');
    } else if (event.ctrlKey && ['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'].includes(event.key)) {
      event.preventDefault();
      sendKey(event.key.slice(5).toLowerCase());
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
  populateKeys(FALLBACK_KEYS);
  connectEvents();
  try {
    await Promise.all([refreshStatus(), refreshDevices(), refreshScenes()]);
  } catch (error) {
    toast(error.message, true);
  }
  await refreshSnapshot();
  if (state.status && state.status.attached) loadScreenshot();
  state.statusTimer = setInterval(pollStatus, 2000);
}

document.addEventListener('DOMContentLoaded', () => {
  init().catch((error) => toast(error.message, true));
});
