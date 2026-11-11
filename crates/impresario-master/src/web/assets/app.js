'use strict';

const ROUTES = [
  ['start', 'Start'],
  ['runs', 'Runs'],
  ['tests', 'Tests'],
  ['coverage', 'Coverage'],
  ['probe', 'Probe'],
  ['groups', 'Groups'],
  ['devices', 'Devices'],
  ['apps', 'Apps'],
  ['config', 'Config'],
  ['resident', 'Resident'],
  ['docs', 'Docs'],
  ['api', 'API'],
];

const state = {
  meta: null,
  workspaces: [],
  route: 'start',
  tests: null,
  devices: [],
  apps: [],
  groups: [],
  config: null,
  resident: null,
  jobs: [],
  runs: [],
  docs: [],
  schema: null,
  selectedJob: null,
  selectedFile: null,
  probe: null,
};

const $ = (selector, root = document) => root.querySelector(selector);

const THEME_KEY = 'impresario.theme';
const THEMES = ['light', 'dark'];

function storedTheme() {
  try {
    const value = localStorage.getItem(THEME_KEY);
    return THEMES.includes(value) ? value : 'light';
  } catch {
    return 'light';
  }
}

function applyTheme(theme, persist = false) {
  const next = THEMES.includes(theme) ? theme : 'light';
  document.documentElement.dataset.theme = next;
  if (persist) {
    try {
      localStorage.setItem(THEME_KEY, next);
    } catch {
      /* Private mode can make storage unavailable; the session still works. */
    }
  }
  const toggle = $('#theme-toggle');
  if (toggle) {
    const dark = next === 'dark';
    toggle.setAttribute('aria-pressed', String(dark));
    toggle.setAttribute('aria-label', dark ? 'Switch to light theme' : 'Switch to dark theme');
    toggle.title = dark ? 'Switch to light theme' : 'Switch to dark theme';
  }
  const meta = document.querySelector('meta[name="theme-color"]');
  if (meta) meta.setAttribute('content', next === 'dark' ? '#0f1615' : '#f6f4ef');
}

applyTheme(storedTheme());

const ICONS = {
  start: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M8 5.5v13l10-6.5-10-6.5Z"/></svg>',
  runs: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M3 12h4l2.5-6 4 12 2.5-6H21"/></svg>',
  tests: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M9 3h6M10 3v6l-5 8.5A2 2 0 0 0 6.7 21h10.6a2 2 0 0 0 1.7-3.5L14 9V3"/><path d="M7.5 15h9"/></svg>',
  coverage: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z"/></svg>',
  probe: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><circle cx="12" cy="12" r="5"/><path d="M12 3v3M12 18v3M3 12h3M18 12h3"/></svg>',
  groups: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M16 20v-1.5a4 4 0 0 0-4-4H7a4 4 0 0 0-4 4V20"/><circle cx="9.5" cy="7.5" r="3.5"/><path d="M17 4.2a3.5 3.5 0 0 1 0 6.6M21 20v-1.5a4 4 0 0 0-3-3.8"/></svg>',
  devices: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>',
  apps: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M12 3 21 7.5v9L12 21 3 16.5v-9L12 3Z"/><path d="M3 7.5 12 12l9-4.5M12 12v9"/></svg>',
  config: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M4 6h8M16 6h4M4 12h2M10 12h10M4 18h8M16 18h4"/><circle cx="14" cy="6" r="2"/><circle cx="8" cy="12" r="2"/><circle cx="14" cy="18" r="2"/></svg>',
  resident: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>',
  docs: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M4 5.5A2.5 2.5 0 0 1 6.5 3H20v16H6.5A2.5 2.5 0 0 0 4 21.5V5.5Z"/><path d="M4 18.5A2.5 2.5 0 0 1 6.5 16H20"/></svg>',
  api: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><rect x="4" y="5" width="16" height="14" rx="2"/><path d="m8 10 3 3-3 3M14 16h3"/></svg>',
};

function layerScope(source) {
  return source === 'global' ? 'global' : 'workspace';
}

class ApiFailure extends Error {
  constructor(message, code, details, status) {
    super(message);
    this.name = 'ApiFailure';
    this.code = code || 'request_failed';
    this.details = details;
    this.status = status;
  }
}

function h(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs || {})) {
    if (value === null || value === undefined || value === false) continue;
    if (key === 'class') node.className = value;
    else if (key === 'text') node.textContent = value;
    else if (key === 'dataset') Object.assign(node.dataset, value);
    else if (key.startsWith('on') && typeof value === 'function') {
      node.addEventListener(key.slice(2), value);
    } else if (key === 'value') node.value = value;
    else if (key === 'checked') node.checked = Boolean(value);
    else if (key === 'selected') node.selected = Boolean(value);
    else if (key === 'disabled') node.disabled = Boolean(value);
    else node.setAttribute(key, value);
  }
  append(node, children);
  return node;
}

function append(node, children) {
  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    if (Array.isArray(child)) append(node, child);
    else if (child instanceof Node) node.appendChild(child);
    else node.appendChild(document.createTextNode(String(child)));
  }
}

function clear(node) {
  while (node.firstChild) node.removeChild(node.firstChild);
}

function toast(message, kind = 'info') {
  const node = h('div', { class: `toast ${kind}`, text: message });
  $('#toasts').appendChild(node);
  window.setTimeout(() => node.remove(), kind === 'bad' ? 8000 : 4500);
}

function pill(text, kind = '') {
  return h('span', { class: `pill ${kind}`.trim(), text });
}

function fmtBytes(bytes) {
  if (!Number.isFinite(bytes)) return '—';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

function statusKind(status) {
  const value = String(status || '').toLowerCase();
  if (['passed', 'ready', 'ok', 'pass', 'running'].includes(value)) {
    return value === 'running' ? 'info' : 'ok';
  }
  if (['failed', 'error', 'stopped', 'interrupted', 'unavailable'].includes(value)) return 'bad';
  if (['queued', 'degraded', 'unverifiable', 'warn'].includes(value)) return 'warn';
  return '';
}

function kv(rows) {
  const list = h('dl', { class: 'kv' });
  for (const [key, value] of rows) {
    list.appendChild(h('dt', { text: key }));
    list.appendChild(h('dd', {}, value instanceof Node ? value : String(value ?? '—')));
  }
  return list;
}

function screenHead(title, subtitle, actions = []) {
  return h('header', { class: 'page-head' },
    h('div', { class: 'page-heading' },
      h('h2', { text: title }),
      subtitle ? h('p', { class: 'page-subtitle', text: subtitle }) : null,
    ),
    h('div', { class: 'page-actions' }, actions),
  );
}

function outputPanel() {
  return h('pre', { class: 'output hidden' });
}

function showResult(target, data) {
  target.classList.remove('hidden');
  const lines = [`$ ${data.action || 'action'}`, `exit: ${data.exit_code}`, `passed: ${data.passed}`];
  if (data.job_id) lines.push(`job: ${data.job_id}`);
  if (data.workspace) lines.push(`workspace: ${data.workspace}`);
  if (data.path) lines.push(`path: ${data.path}`);
  if (data.rolled_back) lines.push('rolled back: yes');
  if (data.stdout) lines.push('', data.stdout.trimEnd());
  if (data.stderr) lines.push('', 'stderr:', data.stderr.trimEnd());
  const extra = { ...data };
  delete extra.action;
  delete extra.exit_code;
  delete extra.passed;
  delete extra.stdout;
  delete extra.stderr;
  delete extra.job_id;
  delete extra.workspace;
  delete extra.path;
  delete extra.rolled_back;
  delete extra.probe;
  if (Object.keys(extra).length > 0) {
    lines.push('', 'details:', JSON.stringify(extra, null, 2));
  }
  target.textContent = lines.join('\n');
}

function showFailure(target, error) {
  target.classList.remove('hidden');
  const lines = [`error: ${error.message}`, `code: ${error.code || 'request_failed'}`];
  if (error.details) lines.push('', JSON.stringify(error.details, null, 2));
  target.textContent = lines.join('\n');
}

async function getJSON(path) {
  const response = await fetch(path, { headers: { Accept: 'application/json' }, cache: 'no-store' });
  let payload = null;
  try {
    payload = await response.json();
  } catch {
    throw new ApiFailure(`invalid JSON response (HTTP ${response.status})`, 'invalid_response', null, response.status);
  }
  if (!response.ok || payload.ok === false) {
    const error = payload && payload.error ? payload.error : {};
    throw new ApiFailure(error.message || `HTTP ${response.status}`, error.code, error.details, response.status);
  }
  return payload.data;
}

async function postAction(name, body = {}) {
  const response = await fetch(`/api/actions/${encodeURIComponent(name)}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
    body: JSON.stringify(body),
  });
  let payload = null;
  try {
    payload = await response.json();
  } catch {
    throw new ApiFailure(`invalid JSON response (HTTP ${response.status})`, 'invalid_response', null, response.status);
  }
  if (!response.ok || payload.ok === false) {
    const error = payload && payload.error ? payload.error : {};
    throw new ApiFailure(error.message || `HTTP ${response.status}`, error.code, error.details, response.status);
  }
  return payload.data;
}

async function act(name, body, target) {
  if (target) {
    target.classList.remove('hidden');
    target.textContent = `$ ${name}\nrunning…`;
  }
  try {
    const data = await postAction(name, body);
    if (target) showResult(target, data);
    toast(`${name}: ${data.passed ? 'ok' : `exit ${data.exit_code}`}`, data.passed ? 'ok' : 'bad');
    return data;
  } catch (error) {
    if (target) showFailure(target, error);
    toast(`${name}: ${error.message}`, 'bad');
    throw error;
  }
}

function option(value, label, selected = false) {
  return h('option', { value, selected }, label);
}

function field(label, control, hint) {
  return h('label', { class: 'field' },
    h('span', { class: 'field-label', text: label }),
    control,
    hint ? h('span', { class: 'field-hint', text: hint }) : null,
  );
}

function button(label, onClick, kind = 'ghost', extra = {}) {
  return h('button', { type: 'button', class: kind, onclick: onClick, ...extra }, label);
}

function renderNav(active) {
  const nav = $('#nav');
  clear(nav);
  for (const [route, label] of ROUTES) {
    const item = h('button', {
      type: 'button',
      class: route === active ? 'active' : '',
      title: label,
      onclick: () => { location.hash = `#/${route}`; },
    });
    const icon = h('span', { class: 'nav-icon', 'aria-hidden': 'true' });
    icon.innerHTML = ICONS[route] || '';
    item.append(icon, h('span', { text: label }));
    if (route === active) item.setAttribute('aria-current', 'page');
    nav.appendChild(item);
  }
}

function renderWorkspaceSelect() {
  const select = $('#workspace-select');
  clear(select);
  for (const workspace of state.workspaces) {
    select.appendChild(option(workspace.path, `${workspace.name}${workspace.active ? ' · active' : ''}`, workspace.active));
  }
  if (state.workspaces.length === 0 && state.meta) {
    select.appendChild(option(state.meta.workspace, state.meta.workspace, true));
  }
}

function setChrome() {
  if (!state.meta) return;
  $('#brand-sub').textContent = `v${state.meta.version} · ${state.meta.workspace_name || 'workspace'}`;
  $('#footer-meta').textContent = `${state.meta.server.bind}:${state.meta.server.port} · ${state.meta.server.loopback ? 'loopback' : 'remote'}`;
  const server = $('#server-pill');
  server.textContent = state.meta.server.loopback ? 'loopback' : 'remote';
  server.className = `pill ${state.meta.server.loopback ? 'ok' : 'warn'}`;
  server.title = state.meta.security.note;
}

async function loadMeta() {
  state.meta = await getJSON('/api/meta');
  state.docs = state.meta.docs || [];
  setChrome();
}

async function loadWorkspaces() {
  const data = await getJSON('/api/workspaces');
  state.workspaces = data.workspaces || [];
  renderWorkspaceSelect();
}

async function switchWorkspace(path) {
  const target = outputPanel();
  try {
    const data = await act('workspace_use', { path }, target);
    if (data.passed) {
      await loadMeta();
      await loadWorkspaces();
      await route();
    }
  } catch {
    /* toast already shown */
  }
}

async function boot() {
  $('#refresh-button').addEventListener('click', () => route());
  $('#theme-toggle').addEventListener('click', () => {
    applyTheme(document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark', true);
  });
  $('#workspace-select').addEventListener('change', (event) => {
    if (event.target.value) switchWorkspace(event.target.value);
  });
  window.addEventListener('hashchange', () => route());
  try {
    await loadMeta();
    await loadWorkspaces();
  } catch (error) {
    const main = $('#app');
    clear(main);
    main.appendChild(h('section', { class: 'card' },
      h('h2', { text: 'Cannot reach the console API' }),
      h('p', { class: 'error-text', text: error.message }),
      h('p', { class: 'muted small', text: 'The server is running but the API request failed. Reload the page or check the terminal.' }),
    ));
    return;
  }
  renderNav(state.route);
  await route();
}

async function route() {
  const raw = location.hash.replace(/^#\/?/, '');
  const [name, ...rest] = raw.split('/');
  const selected = ROUTES.some(([route]) => route === name) ? name : 'start';
  state.route = selected;
  renderNav(selected);
  const main = $('#app');
  clear(main);
  try {
    await SCREENS[selected](rest.join('/'));
  } catch (error) {
    main.appendChild(h('section', { class: 'card' },
      h('h2', { text: 'This screen could not load' }),
      h('p', { class: 'error-text', text: error.message }),
      h('pre', { class: 'output', text: error.details ? JSON.stringify(error.details, null, 2) : (error.stack || '') }),
    ));
  }
}

function errorBanner(error) {
  return h('div', { class: 'banner bad' },
    h('strong', { text: error.message || String(error) }),
    error.details ? h('pre', { class: 'output', text: JSON.stringify(error.details, null, 2) }) : null,
  );
}

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

async function screenStart() {
  const main = $('#app');
  const overview = await getJSON('/api/overview');
  const tests = await getJSON('/api/tests').catch(() => null);
  const devicesData = await getJSON('/api/devices').catch(() => ({ devices: [] }));
  const appsData = await getJSON('/api/apps').catch(() => ({ apps: [] }));
  state.tests = tests;
  state.devices = devicesData.devices || [];
  state.apps = appsData.apps || [];

  const consolePanel = outputPanel();
  const checks = [
    {
      title: 'Workspace selected',
      body: overview.workspace,
      done: true,
      action: null,
    },
    {
      title: 'Workspace validates',
      body: overview.validation.ok ? 'No parse or config errors.' : `${overview.validation.errors.length} error(s) need attention.`,
      done: overview.validation.ok,
      action: () => act('validate', {}, consolePanel).then(() => route()).catch(() => {}),
      label: 'Validate',
    },
    {
      title: 'Devices configured',
      body: overview.counts.devices === 0 ? 'No devices yet. Add one in Devices.' : `${overview.counts.devices} device(s) configured.`,
      done: overview.counts.devices > 0,
      action: () => act('device_check', {}, consolePanel).then(() => route()).catch(() => {}),
      label: 'Check devices',
    },
    {
      title: 'First run',
      body: overview.jobs.total === 0 ? 'Preview a run, then submit it detached.' : `${overview.jobs.total} job(s); ${overview.jobs.passed} passed, ${overview.jobs.failed} failed.`,
      done: overview.jobs.total > 0,
      action: null,
    },
  ];

  const checklist = h('div', { class: 'checklist' });
  for (const check of checks) {
    checklist.appendChild(h('div', { class: `check ${check.done ? 'done' : 'todo'}` },
      h('span', { class: 'dot', text: check.done ? '✓' : '•' }),
      h('div', {},
        h('strong', { text: check.title }),
        h('p', { class: 'muted small wrap', text: check.body }),
      ),
      check.action ? button(check.label, check.action, 'ghost small') : h('span', { class: 'pill ok', text: check.done ? 'done' : 'next' }),
    ));
  }

  const playSelect = h('select', {});
  playSelect.appendChild(option('', 'All plays'));
  for (const play of (tests && tests.plays) || []) playSelect.appendChild(option(play.name, play.name));
  const deviceSelect = h('select', { multiple: '', size: Math.min(6, Math.max(2, state.devices.length)) });
  for (const device of state.devices) deviceSelect.appendChild(option(device.name, `${device.name} (${device.platform})`));
  const appSelect = h('select', {});
  appSelect.appendChild(option('', 'Play/default app'));
  for (const app of state.apps) appSelect.appendChild(option(app.name, app.name));
  const dryRun = h('input', { type: 'checkbox', checked: true });

  const runForm = h('form', { class: 'stack' },
    field('Play', playSelect),
    field('Devices', deviceSelect, 'Leave empty to use the play\'s audience and configured defaults.'),
    field('App', appSelect),
    h('label', { class: 'row' }, dryRun, h('span', { class: 'small', text: 'Preview only (dry run)' })),
    h('div', { class: 'row' },
      button('Run', async (event) => {
        event.preventDefault();
        const body = {};
        if (playSelect.value) body.play = [playSelect.value];
        const devices = Array.from(deviceSelect.selectedOptions).map((entry) => entry.value);
        if (devices.length) body.device = devices;
        if (appSelect.value) body.app = appSelect.value;
        if (dryRun.checked) body.dry_run = true;
        const data = await act('run', body, consolePanel).catch(() => null);
        if (data && data.passed && !dryRun.checked) {
          toast(`Detached job ${data.job_id || ''} submitted`, 'ok');
          location.hash = '#/runs';
        }
      }, ''),
      button('Validate first', () => act('validate', {}, consolePanel).catch(() => {}), 'ghost'),
    ),
  );
  runForm.addEventListener('submit', (event) => event.preventDefault());

  const nextSteps = h('div', { class: 'stack small' },
    h('a', { href: '#/tests', text: 'Edit tests and run one play →' }),
    h('a', { href: '#/probe', text: 'Probe a device and inspect evidence →' }),
    h('a', { href: '#/resident', text: 'Schedule unattended runs and watch for failures →' }),
    h('a', { href: '#/api', text: 'Machine-readable API and agent guide →' }),
  );

  main.appendChild(screenHead('Start', 'A guided path from workspace to evidence', [
    pill(`${overview.counts.scenes} scenes · ${overview.counts.theatres} theatres · ${overview.counts.plays} plays`, 'info'),
    pill(overview.validation.ok ? 'workspace valid' : 'needs attention', overview.validation.ok ? 'ok' : 'bad'),
  ]));
  main.appendChild(h('div', { class: 'grid two stretch' },
    h('section', { class: 'card' },
      h('h3', { text: 'First-run checklist' }),
      h('p', { class: 'muted small', text: 'Every step calls the same CLI the TUI uses.' }),
      checklist,
    ),
    h('section', { class: 'card' },
      h('h3', { text: 'Quick run' }),
      h('p', { class: 'muted small', text: 'Preview by default; clear the checkbox to submit a detached job.' }),
      runForm,
    ),
  ));
  if (!overview.validation.ok) {
    main.appendChild(h('section', { class: 'card' },
      h('h3', { text: 'Validation errors' }),
      h('pre', { class: 'output', text: overview.validation.errors.join('\n') }),
    ));
  }
  main.appendChild(h('div', { class: 'grid two stretch' },
    h('section', { class: 'card' }, h('h3', { text: 'Console' }), consolePanel,
      h('p', { class: 'console-hint', text: 'CLI output from the actions on this screen appears here.' })),
    h('section', { class: 'card' }, h('h3', { text: 'Next steps' }), nextSteps,
      h('p', { class: 'muted small', text: `Jobs: ${overview.jobs.running} running, ${overview.jobs.passed} passed, ${overview.jobs.failed} failed.` }),
      h('p', { class: 'muted small', text: overview.resident && overview.resident.running ? 'Resident runner is running.' : 'Resident runner is stopped.' }),
    ),
  ));
}

// ---------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------

async function screenRuns(arg) {
  const main = $('#app');
  const [jobsData, runsData] = await Promise.all([
    getJSON('/api/jobs?all=1'),
    getJSON('/api/runs').catch(() => ({ runs: [] })),
  ]);
  state.jobs = jobsData.jobs || [];
  state.runs = runsData.runs || [];

  const list = h('div', { class: 'table-wrap jobs' });
  const table = h('table', {},
    h('thead', {}, h('tr', {},
      h('th', { text: 'Job' }), h('th', { text: 'Status' }), h('th', { text: 'Label' }),
      h('th', { text: 'Devices' }), h('th', { text: 'Created' }),
    )),
    h('tbody', {}),
  );
  const body = table.querySelector('tbody');
  if (state.jobs.length === 0) {
    body.appendChild(h('tr', {}, h('td', { colspan: '5', class: 'empty', text: 'No jobs yet.' })));
  }
  for (const job of state.jobs) {
    const row = h('tr', {
      class: `clickable ${state.selectedJob === job.id ? 'selected' : ''}`,
      dataset: { job: job.id },
      onclick: () => showJob(job.id).then(() => {
        const detail = $('#run-detail');
        const card = detail ? detail.closest('.card') : null;
        if (card) card.scrollIntoView({ block: 'start' });
      }),
    },
      h('td', { class: 'mono', text: job.id }),
      h('td', {}, pill(job.status, statusKind(job.status))),
      h('td', { text: job.label || '—' }),
      h('td', { text: (job.devices || []).join(', ') || '—' }),
      h('td', { class: 'nowrap small', text: job.created_at || '—' }),
    );
    body.appendChild(row);
  }
  list.appendChild(table);

  const detail = h('div', { id: 'run-detail', class: 'stack' });
  const reportList = h('div', { class: 'table-wrap' });
  const reportTable = h('table', {},
    h('thead', {}, h('tr', {},
      h('th', { text: 'Report' }), h('th', { text: 'Status' }), h('th', { text: 'Started' }),
      h('th', { text: 'Duration' }), h('th', { text: 'Plays' }),
    )),
    h('tbody', {}),
  );
  const reportBody = reportTable.querySelector('tbody');
  if (state.runs.length === 0) {
    reportBody.appendChild(h('tr', {}, h('td', { colspan: '5', class: 'empty', text: 'No report directories yet.' })));
  }
  for (const run of state.runs) {
    reportBody.appendChild(h('tr', { class: 'clickable', onclick: () => showRun(run.id) },
      h('td', { class: 'mono', text: run.id }),
      h('td', {}, pill(run.status || 'unknown', statusKind(run.status))),
      h('td', { class: 'small nowrap', text: run.started_at || '—' }),
      h('td', { class: 'small', text: run.duration_ms == null ? '—' : `${(run.duration_ms / 1000).toFixed(1)}s` }),
      h('td', { class: 'small', text: String(run.plays ?? 0) }),
    ));
  }
  reportList.appendChild(reportTable);

  main.appendChild(screenHead('Runs', 'Jobs from the supervisor and report directories on disk', [
    button('Refresh', () => route(), 'ghost'),
  ]));
  main.appendChild(h('section', { class: 'card' },
    h('h3', { text: 'Jobs' }),
    h('p', { class: 'muted small', text: 'Newest first. Select one to inspect its log, events, report and artifacts.' }),
    list,
  ));
  main.appendChild(h('section', { class: 'card run-detail-card' }, h('h3', { text: 'Selected job' }), detail));
  main.appendChild(h('section', { class: 'card' },
    h('h3', { text: 'Reports on disk' }),
    h('p', { class: 'muted small', text: `Under ${runsData.report_output || 'report_output'}. Watcher dumps live in Resident.` }),
    reportList,
  ));

  if (state.selectedJob) await showJob(state.selectedJob);
  else detail.appendChild(h('p', { class: 'empty', text: 'Select a job above.' }));
  if (arg) await showRun(arg);
}

async function showJob(id) {
  state.selectedJob = id;
  for (const row of document.querySelectorAll('tr[data-job]')) {
    row.classList.toggle('selected', row.dataset.job === id);
  }
  const panel = $('#run-detail');
  if (!panel) return;
  clear(panel);
  panel.appendChild(h('p', { class: 'muted small', text: 'Loading…' }));
  let data;
  try {
    data = await getJSON(`/api/jobs/${encodeURIComponent(id)}`);
  } catch (error) {
    clear(panel);
    panel.appendChild(errorBanner(error));
    return;
  }
  const job = data.job;
  clear(panel);
  panel.appendChild(h('div', { class: 'row between' },
    h('h4', { class: 'mono', text: job.id }),
    h('div', { class: 'row' },
      pill(job.status, statusKind(job.status)),
      job.exit_code == null ? null : pill(`exit ${job.exit_code}`, job.exit_code === 0 ? 'ok' : 'bad'),
    ),
  ));
  panel.appendChild(kv([
    ['Label', job.label || '—'],
    ['Workspace', job.workspace],
    ['Devices', (job.devices || []).join(', ') || '—'],
    ['Plays', (job.plays || []).join(', ') || '—'],
    ['Created', job.created_at],
    ['Started', job.started_at || '—'],
    ['Finished', job.finished_at || '—'],
    ['Duration', job.duration_ms == null ? '—' : `${(job.duration_ms / 1000).toFixed(1)}s`],
    ['Run dir', job.run_dir],
    ['Message', job.message || '—'],
  ]));
  const output = outputPanel();
  panel.appendChild(h('div', { class: 'row' },
    button('Stop', () => act('job_stop', { id }, output).catch(() => {}), 'danger small', { disabled: ['passed', 'failed', 'error', 'stopped', 'interrupted'].includes(job.status) }),
    button('Remove', () => {
      if (confirm(`Remove job ${job.id}?`)) act('job_remove', { id }, output).then(() => { state.selectedJob = null; route(); }).catch(() => {});
    }, 'danger small'),
  ));
  panel.appendChild(output);

  const tabs = h('div', { class: 'tabs' });
  const tabBody = h('div', { class: 'stack' });
  const tabsDef = [
    ['Log', () => getJSON(`/api/jobs/${encodeURIComponent(id)}/log?limit=200000`).then((value) => value.text)],
    ['Events', () => getJSON(`/api/jobs/${encodeURIComponent(id)}/events?limit=2000`).then((value) => JSON.stringify(value.events, null, 2))],
    ['Report', () => data.report_available ? getJSON(`/api/jobs/${encodeURIComponent(id)}/report`).then((value) => JSON.stringify(value.report, null, 2)) : Promise.reject(new ApiFailure('No report copy is available for this job.', 'report_unavailable'))],
    ['Artifacts', () => getJSON(`/api/jobs/${encodeURIComponent(id)}/artifacts`).then((value) => value.artifacts)],
  ];
  let active = null;
  const selectTab = async (label, loader) => {
    active = label;
    for (const child of tabs.children) child.className = child.textContent === label ? 'active' : '';
    clear(tabBody);
    tabBody.appendChild(h('p', { class: 'muted small', text: 'Loading…' }));
    try {
      const value = await loader();
      clear(tabBody);
      if (label === 'Artifacts') renderArtifactList(tabBody, value, async (path) => {
        const artifact = await getJSON(`/api/jobs/${encodeURIComponent(id)}/artifact?path=${encodeURIComponent(path)}`);
        renderArtifact(tabBody, artifact);
      });
      else tabBody.appendChild(h('pre', { class: 'output', text: value || '(empty)' }));
    } catch (error) {
      clear(tabBody);
      tabBody.appendChild(errorBanner(error));
    }
  };
  for (const [label, loader] of tabsDef) {
    tabs.appendChild(button(label, () => selectTab(label, loader), 'small'));
  }
  panel.appendChild(tabs);
  panel.appendChild(tabBody);
  await selectTab('Log', tabsDef[0][1]);
  if (active === null) return;
}

function renderArtifactList(target, artifacts, onOpen) {
  if (!artifacts || artifacts.length === 0) {
    target.appendChild(h('p', { class: 'empty', text: 'No artifacts in this directory.' }));
    return;
  }
  const list = h('div', { class: 'stack' });
  for (const artifact of artifacts) {
    list.appendChild(h('div', { class: 'row between' },
      h('button', { class: 'ghost small mono', type: 'button', text: artifact.path, onclick: () => onOpen(artifact.path) }),
      h('span', { class: 'tiny muted', text: fmtBytes(artifact.size) }),
    ));
  }
  target.appendChild(list);
}

function renderArtifact(target, artifact) {
  const holder = h('div', { class: 'stack' });
  holder.appendChild(h('div', { class: 'row between' },
    h('strong', { class: 'mono', text: artifact.name }),
    h('span', { class: 'tiny muted', text: `${fmtBytes(artifact.size)} · ${artifact.encoding}` }),
  ));
  if (artifact.encoding === 'text') {
    holder.appendChild(h('pre', { class: 'output', text: artifact.text }));
  } else if (String(artifact.mime || '').startsWith('image/')) {
    holder.appendChild(h('img', { src: `data:${artifact.mime};base64,${artifact.text}`, alt: artifact.name, class: 'artifact-image' }));
  } else {
    holder.appendChild(h('p', { class: 'muted small', text: 'Binary artifact; base64 follows.' }));
    holder.appendChild(h('pre', { class: 'output', text: artifact.text }));
  }
  target.appendChild(holder);
}

async function showRun(id) {
  const detail = $('#run-detail');
  if (!detail) return;
  clear(detail);
  detail.appendChild(h('p', { class: 'muted small', text: 'Loading…' }));
  try {
    const [data, artifacts] = await Promise.all([
      getJSON(`/api/runs/${encodeURIComponent(id)}`),
      getJSON(`/api/runs/${encodeURIComponent(id)}/artifacts`),
    ]);
    clear(detail);
    detail.appendChild(h('h4', { class: 'mono', text: id }));
    const report = data.report || {};
    detail.appendChild(kv([
      ['Status', pill(report.status || 'unknown', statusKind(report.status))],
      ['Exit code', report.exit_code ?? '—'],
      ['Started', report.started_at || '—'],
      ['Duration', report.duration_ms == null ? '—' : `${(report.duration_ms / 1000).toFixed(1)}s`],
      ['Path', data.path],
    ]));
    const tabs = h('div', { class: 'tabs' });
    const body = h('div', { class: 'stack' });
    const setActive = (target) => {
      for (const child of tabs.children) child.classList.toggle('active', child === target);
    };
    const reportButton = button('Report', () => {
      setActive(reportButton);
      clear(body);
      body.appendChild(h('pre', { class: 'output', text: JSON.stringify(report, null, 2) }));
    }, 'small active');
    const artifactsButton = button('Artifacts', () => {
      setActive(artifactsButton);
      clear(body);
      renderArtifactList(body, artifacts.artifacts || [], async (path) => {
        try {
          const artifact = await getJSON(`/api/runs/${encodeURIComponent(id)}/artifact?path=${encodeURIComponent(path)}`);
          renderArtifact(body, artifact);
        } catch (error) {
          body.appendChild(errorBanner(error));
        }
      });
    }, 'small');
    tabs.append(reportButton, artifactsButton);
    detail.appendChild(tabs);
    detail.appendChild(body);
    body.appendChild(h('pre', { class: 'output', text: JSON.stringify(report, null, 2) }));
  } catch (error) {
    clear(detail);
    detail.appendChild(errorBanner(error));
  }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

async function screenTests() {
  const main = $('#app');
  const tree = await getJSON('/api/tests');
  state.tests = tree;
  const devices = await getJSON('/api/devices').catch(() => ({ devices: [] }));
  state.devices = devices.devices || [];

  const treePanel = h('div', { class: 'tree' });
  const addGroup = (title, items) => {
    if (!items || items.length === 0) return;
    const group = h('div', { class: 'tree-group' }, h('div', { class: 'tree-title', text: title }));
    for (const item of items) group.appendChild(item);
    treePanel.appendChild(group);
  };
  addGroup('Scenes', (tree.scenes || []).map((scene) => {
    const row = h('div', { class: 'tree-item', dataset: { file: scene.file }, onclick: () => openTestFile(scene.file) },
      h('span', { class: 'kind', text: 'scene' }),
      h('span', { text: scene.name }),
      h('span', { class: 'count', text: `${(scene.pieces || []).length} pieces` }),
    );
    return row;
  }));
  addGroup('Theatres', (tree.theatres || []).map((theatre) => {
    const row = h('div', { class: 'tree-item', dataset: { file: theatre.file }, onclick: () => openTestFile(theatre.file) },
      h('span', { class: 'kind', text: 'theatre' }),
      h('span', { text: theatre.name }),
      h('span', { class: 'count', text: `${(theatre.stages || []).length} stages` }),
    );
    return row;
  }));
  addGroup('Plays', (tree.plays || []).map((play) => {
    const row = h('div', { class: 'tree-item', dataset: { file: play.file }, onclick: () => openTestFile(play.file) },
      h('span', { class: 'kind', text: 'play' }),
      h('span', { text: play.name }),
      h('span', { class: 'count', text: `${(play.acts || []).length} acts` }),
    );
    return row;
  }));

  const editor = h('div', { class: 'stack' });
  const pathLabel = h('p', { class: 'mono small muted', text: 'Select a file in the tree.' });
  const textarea = h('textarea', { rows: 22, spellcheck: 'false', placeholder: 'Select a file to edit…' });
  const output = outputPanel();
  const saveButton = button('Save + validate', async () => {
    if (!state.selectedFile) return;
    const data = await act('test_save', { path: state.selectedFile, content: textarea.value }, output).catch(() => null);
    if (data && data.passed) await route();
  }, '');
  const runButton = button('Run this play', async () => {
    if (!state.selectedFile || !state.selectedFile.endsWith('.play')) {
      toast('Select a .play file first', 'bad');
      return;
    }
    const data = await act('run', { play: [state.selectedFile] }, output).catch(() => null);
    if (data && data.passed && data.job_id) {
      toast(`Detached job ${data.job_id} submitted`, 'ok');
      location.hash = '#/runs';
    }
  }, 'ghost');
  const newKind = h('select', {}, option('scene', 'scene'), option('theatre', 'theatre'), option('play', 'play'));
  const newName = h('input', { placeholder: 'UpperCamelCase', autocomplete: 'off' });
  const force = h('input', { type: 'checkbox' });
  const createButton = button('Create file', async () => {
    if (!newName.value.trim()) { toast('A name is required', 'bad'); return; }
    const data = await act('test_new', { kind: newKind.value, name: newName.value.trim(), force: force.checked }, output).catch(() => null);
    if (data && data.passed) { newName.value = ''; await route(); }
  }, 'ghost');

  editor.appendChild(pathLabel);
  editor.appendChild(textarea);
  editor.appendChild(h('div', { class: 'row' }, saveButton, runButton));
  editor.appendChild(output);

  const grepInput = h('input', { placeholder: 'Search every workspace DSL file', autocomplete: 'off' });
  const grepResults = h('div', { class: 'stack' });
  const grepButton = button('Search', async () => {
    clear(grepResults);
    if (!grepInput.value.trim()) return;
    try {
      const data = await getJSON(`/api/tests/grep?q=${encodeURIComponent(grepInput.value.trim())}`);
      if (!data.matches || data.matches.length === 0) {
        grepResults.appendChild(h('p', { class: 'empty', text: 'No matches.' }));
        return;
      }
      for (const match of data.matches) {
        grepResults.appendChild(h('div', { class: 'row between' },
          h('button', { class: 'ghost small mono', type: 'button', text: `${match.file}:${match.line}`, onclick: () => openTestFile(match.file) }),
          h('span', { class: 'small wrap', text: match.text }),
        ));
      }
    } catch (error) {
      grepResults.appendChild(errorBanner(error));
    }
  }, 'ghost');

  main.appendChild(screenHead('Tests', 'Browse, edit and run the workspace DSL', [
    pill(`${(tree.scenes || []).length} scenes · ${(tree.theatres || []).length} theatres · ${(tree.plays || []).length} plays`, 'info'),
    button('Refresh', () => route(), 'ghost'),
  ]));
  main.appendChild(h('div', { class: 'split' },
    h('section', { class: 'card' },
      h('h3', { text: 'Workspace tree' }),
      treePanel,
      h('div', { class: 'stack', style: null },
        h('h4', { text: 'Search' }),
        h('div', { class: 'row' }, grepInput, grepButton),
        grepResults,
      ),
    ),
    h('section', { class: 'card' }, h('h3', { text: 'Editor' }), editor),
  ));
  main.appendChild(h('section', { class: 'card' },
    h('h3', { text: 'New test file' }),
    h('div', { class: 'row' },
      field('Kind', newKind),
      field('Name', newName),
      h('label', { class: 'row' }, force, h('span', { class: 'small', text: 'Overwrite' })),
      createButton,
    ),
  ));

  async function openTestFile(path) {
    if (!path) return;
    state.selectedFile = path;
    pathLabel.textContent = path;
    textarea.value = '';
    output.classList.add('hidden');
    for (const item of treePanel.querySelectorAll('.tree-item')) {
      item.classList.toggle('selected', item.dataset.file === path);
    }
    try {
      const data = await getJSON(`/api/tests/file?path=${encodeURIComponent(path)}`);
      textarea.value = data.text;
      textarea.focus();
    } catch (error) {
      showFailure(output, error);
    }
  }

  if (state.selectedFile) await openTestFile(state.selectedFile);
}

// ---------------------------------------------------------------------------
// Coverage
// ---------------------------------------------------------------------------

async function screenCoverage() {
  const main = $('#app');
  const data = await getJSON('/api/coverage');
  const rows = data.rows || [];
  const table = h('table', {},
    h('thead', {}, h('tr', {},
      h('th', { text: 'Stage' }), h('th', { text: 'Theatres' }), h('th', { text: 'Implementations' }),
      h('th', { text: 'Audience' }), h('th', { text: 'Missing devices' }),
    )),
    h('tbody', {}),
  );
  const body = table.querySelector('tbody');
  if (rows.length === 0) {
    body.appendChild(h('tr', {}, h('td', { colspan: '5', class: 'empty', text: 'No stage declarations found.' })));
  }
  for (const row of rows) {
    body.appendChild(h('tr', {},
      h('td', { class: 'mono', text: row.stage }),
      h('td', { class: 'small', text: (row.theatres || []).join(', ') }),
      h('td', { text: String(row.implementations) }),
      h('td', { class: 'small', text: (row.platforms || []).join(', ') || '—' }),
      h('td', { class: 'small' }, (row.missing || []).length === 0
        ? pill('covered', 'ok')
        : h('div', { class: 'row' }, ...(row.missing || []).map((device) => pill(`${device.name} (${device.platform})`, 'warn')))),
    ));
  }
  main.appendChild(screenHead('Coverage', 'Which stages exist and which configured devices have no implementation'));
  main.appendChild(h('section', { class: 'card' },
    h('p', { class: 'muted small', text: data.legend || '' }),
    h('div', { class: 'table-wrap' }, table),
  ));
}

// ---------------------------------------------------------------------------
// Probe
// ---------------------------------------------------------------------------

async function screenProbe() {
  const main = $('#app');
  const [support, devicesData, appsData] = await Promise.all([
    getJSON('/api/probe/support'),
    getJSON('/api/devices').catch(() => ({ devices: [] })),
    getJSON('/api/apps').catch(() => ({ apps: [] })),
  ]);
  state.devices = devicesData.devices || [];
  state.apps = appsData.apps || [];
  const rows = support.devices || [];
  const table = h('table', {},
    h('thead', {}, h('tr', {},
      h('th', { text: 'Device' }), h('th', { text: 'Platform' }), h('th', { text: 'Showing' }),
      h('th', { text: 'Player' }), h('th', { text: 'DRM' }), h('th', { text: 'Strongest' }),
    )),
    h('tbody', {}),
  );
  const body = table.querySelector('tbody');
  for (const row of rows) {
    const mark = (value) => value ? pill('yes', 'ok') : pill('no', 'bad');
    body.appendChild(h('tr', {},
      h('td', { text: row.device }),
      h('td', { class: 'small', text: row.platform }),
      h('td', {}, mark(row.showing)),
      h('td', {}, mark(row.player)),
      h('td', {}, mark(row.drm)),
      h('td', { class: 'small', text: row.strongest || '—' }),
    ));
  }

  const kind = h('select', {}, option('showing', 'showing'), option('player', 'player'), option('drm', 'drm'));
  const device = h('select', {});
  for (const entry of state.devices) device.appendChild(option(entry.name, `${entry.name} (${entry.platform})`));
  const app = h('select', {});
  app.appendChild(option('', 'None'));
  for (const entry of state.apps) app.appendChild(option(entry.name, entry.name));
  const piece = h('input', { placeholder: 'Scene.Piece (optional)', autocomplete: 'off' });
  const selector = h('input', { placeholder: 'raw selector (optional)', autocomplete: 'off' });
  const claims = h('input', { placeholder: 'claims, comma separated (optional)', autocomplete: 'off' });
  const confidence = h('select', {}, option('', 'Default'), option('none', 'none'), option('inferred', 'inferred'), option('corroborated', 'corroborated'), option('authoritative', 'authoritative'));
  const output = outputPanel();
  const result = h('div', { class: 'stack' });
  result.appendChild(h('p', { class: 'console-hint', text: 'Run a live probe to see verdicts, confidence and evidence.' }));

  const form = h('form', { class: 'stack' },
    h('div', { class: 'grid two compact' },
      field('Probe family', kind),
      field('Device', device),
      field('App', app),
      field('Piece', piece),
      field('Selector', selector),
      field('Claims', claims),
    ),
    field('Minimum confidence', confidence),
    h('div', { class: 'row' },
      button('Run live probe', async (event) => {
        event.preventDefault();
        if (!device.value) { toast('Add and select a device first', 'bad'); return; }
        const payload = { kind: kind.value, device: device.value };
        if (app.value) payload.app = app.value;
        if (piece.value.trim()) payload.piece = piece.value.trim();
        if (selector.value.trim()) payload.selector = selector.value.trim();
        const claimList = claims.value.split(',').map((value) => value.trim()).filter(Boolean);
        if (claimList.length) payload.claims = claimList;
        if (confidence.value) payload.confidence = confidence.value;
        clear(result);
        const data = await act('probe', payload, output).catch(() => null);
        if (data) renderProbeResult(result, data);
      }, ''),
      button('Clear', () => { clear(result); output.classList.add('hidden'); }, 'ghost'),
    ),
  );
  form.addEventListener('submit', (event) => event.preventDefault());

  main.appendChild(screenHead('Probe', 'Plan-time support and one live inspection with evidence', [
    pill(`${rows.length} configured devices`, 'info'),
  ]));
  main.appendChild(h('section', { class: 'card' },
    h('h3', { text: 'Plan-time support' }),
    h('p', { class: 'muted small', text: support.legend || '' }),
    h('div', { class: 'table-wrap' }, table),
  ));
  main.appendChild(h('div', { class: 'grid two' },
    h('section', { class: 'card' }, h('h3', { text: 'Live probe' }), form, output),
    h('section', { class: 'card' }, h('h3', { text: 'Evidence' }), result),
  ));
}

function renderProbeResult(target, data) {
  clear(target);
  const probe = data.probe || null;
  target.appendChild(kv([
    ['Action', data.action],
    ['Exit code', data.exit_code],
    ['Passed', data.passed ? pill('yes', 'ok') : pill('no', 'bad')],
    ...(probe ? [
      ['Verdict', pill(probe.verdict || 'unknown', statusKind(probe.verdict))],
      ['Confidence', probe.confidence || '—'],
      ['Window', probe.window_ms == null ? '—' : `${probe.window_ms} ms`],
      ['Run id', probe.run_id || '—'],
      ['Run dir', probe.run_dir || '—'],
    ] : []),
  ]));
  if (!probe) {
    target.appendChild(h('pre', { class: 'output', text: data.stdout || data.stderr || '' }));
    return;
  }
  if (probe.unverifiable_reason) {
    target.appendChild(h('div', { class: 'banner warn', text: probe.unverifiable_reason }));
  }
  if (Array.isArray(probe.verdicts) && probe.verdicts.length) {
    const table = h('table', {},
      h('thead', {}, h('tr', {},
        h('th', { text: 'Claim' }), h('th', { text: 'Verdict' }), h('th', { text: 'Confidence' }), h('th', { text: 'Reason' }),
      )),
      h('tbody', {}),
    );
    const body = table.querySelector('tbody');
    for (const verdict of probe.verdicts) {
      body.appendChild(h('tr', {},
        h('td', { class: 'mono small', text: verdict.claim || '—' }),
        h('td', {}, pill(verdict.verdict || 'unknown', statusKind(verdict.verdict))),
        h('td', { class: 'small', text: verdict.confidence || '—' }),
        h('td', { class: 'small', text: verdict.reason || '—' }),
      ));
    }
    target.appendChild(h('div', { class: 'table-wrap' }, table));
  }
  if (Array.isArray(probe.evidence) && probe.evidence.length) {
    const table = h('table', {},
      h('thead', {}, h('tr', {},
        h('th', { text: 'Source' }), h('th', { text: 'Class' }), h('th', { text: 'Kind' }), h('th', { text: 'Observation' }),
      )),
      h('tbody', {}),
    );
    const body = table.querySelector('tbody');
    for (const evidence of probe.evidence) {
      const observation = evidence.observation && typeof evidence.observation === 'object'
        ? JSON.stringify(evidence.observation)
        : String(evidence.observation ?? '');
      body.appendChild(h('tr', {},
        h('td', { class: 'small mono', text: evidence.source || '—' }),
        h('td', { class: 'small', text: evidence.class || '—' }),
        h('td', { class: 'small', text: evidence.kind || '—' }),
        h('td', { class: 'small wrap', text: observation }),
      ));
    }
    target.appendChild(h('div', { class: 'table-wrap' }, table));
  }
  if (probe.observed && Object.keys(probe.observed).length) {
    target.appendChild(h('h4', { text: 'Observed' }));
    target.appendChild(h('pre', { class: 'output', text: JSON.stringify(probe.observed, null, 2) }));
  }
}

// ---------------------------------------------------------------------------
// Groups
// ---------------------------------------------------------------------------

async function screenGroups() {
  const main = $('#app');
  const data = await getJSON('/api/groups');
  state.groups = data.groups || [];
  const table = h('table', {},
    h('thead', {}, h('tr', {},
      h('th', { text: 'Group' }), h('th', { text: 'Source' }), h('th', { text: 'Members' }),
      h('th', { text: 'Resolved devices' }), h('th', { text: '' }),
    )),
    h('tbody', {}),
  );
  const body = table.querySelector('tbody');
  if (state.groups.length === 0) body.appendChild(h('tr', {}, h('td', { colspan: '5', class: 'empty', text: 'No groups yet.' })));
  for (const group of state.groups) {
    const output = outputPanel();
    body.appendChild(h('tr', {},
      h('td', { class: 'mono', text: group.name }),
      h('td', { class: 'small', text: group.source }),
      h('td', { class: 'small', text: (group.members || []).join(', ') }),
      h('td', { class: 'small' }, group.error
        ? h('span', { class: 'error-text', text: group.error })
        : (group.devices || []).map((device) => pill(`${device.name} (${device.platform})`, 'info'))),
      h('td', {},
        h('div', { class: 'row' },
          button('Set', () => {
            const members = prompt(`Members for ${group.name} (comma separated)`, (group.members || []).join(', '));
            if (members === null) return;
            act('group_set', { name: group.name, members: members.split(',').map((value) => value.trim()).filter(Boolean), scope: layerScope(group.source) }, output).then(() => route()).catch(() => {});
          }, 'ghost small'),
          button('Remove', () => {
            if (confirm(`Remove group ${group.name}?`)) act('group_remove', { name: group.name, scope: layerScope(group.source) }, output).then(() => route()).catch(() => {});
          }, 'danger small'),
        ),
        output,
      ),
    ));
  }
  const name = h('input', { placeholder: 'group-name', autocomplete: 'off' });
  const members = h('input', { placeholder: 'web1, @phones', autocomplete: 'off' });
  const scope = h('select', {}, option('workspace', 'workspace'), option('global', 'global'));
  const output = outputPanel();
  const form = h('form', { class: 'row' },
    field('Name', name),
    field('Members', members),
    field('Scope', scope),
    button('Add group', (event) => {
      event.preventDefault();
      if (!name.value.trim() || !members.value.trim()) { toast('Name and members are required', 'bad'); return; }
      act('group_add', { name: name.value.trim(), members: members.value.split(',').map((value) => value.trim()).filter(Boolean), scope: scope.value }, output).then(() => route()).catch(() => {});
    }, ''),
  );
  form.addEventListener('submit', (event) => event.preventDefault());

  main.appendChild(screenHead('Groups', 'Actor groups resolve devices and nested @groups'));
  main.appendChild(h('section', { class: 'card' },
    h('div', { class: 'table-wrap' }, table),
    h('h3', { text: 'Add group' }),
    form,
    output,
  ));
}

// ---------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------

const DEVICE_TYPES = ['web', 'webOS', 'tizenOS', 'android', 'androidtv', 'iOS', 'tvOS'];

async function screenDevices() {
  const main = $('#app');
  const data = await getJSON('/api/devices');
  state.devices = data.devices || [];
  const table = h('table', {},
    h('thead', {}, h('tr', {},
      h('th', { text: 'Name' }), h('th', { text: 'Type' }), h('th', { text: 'IP' }),
      h('th', { text: 'Platform' }), h('th', { text: 'Transport' }), h('th', { text: 'Source' }),
      h('th', { text: '' }),
    )),
    h('tbody', {}),
  );
  const body = table.querySelector('tbody');
  if (state.devices.length === 0) body.appendChild(h('tr', {}, h('td', { colspan: '7', class: 'empty', text: 'No devices configured.' })));
  for (const device of state.devices) {
    const output = outputPanel();
    body.appendChild(h('tr', {},
      h('td', { class: 'mono', text: device.name }),
      h('td', { class: 'small', text: device.type }),
      h('td', { class: 'small mono', text: device.ip }),
      h('td', { class: 'small', text: device.platform }),
      h('td', { class: 'small', text: device.transport }),
      h('td', { class: 'small', text: device.source || 'effective' }),
      h('td', {},
        h('div', { class: 'row' },
          button('Edit', () => {
            const type = prompt('Device type', device.type);
            if (type === null) return;
            const ip = prompt('Device IP', device.ip);
            if (ip === null) return;
            act('device_edit', { name: device.name, type, ip, scope: layerScope(device.source) }, output).then(() => route()).catch(() => {});
          }, 'ghost small'),
          button('Rename', () => {
            const next = prompt('New device name', device.name);
            if (next && next !== device.name) act('device_rename', { name: device.name, new_name: next, scope: layerScope(device.source) }, output).then(() => route()).catch(() => {});
          }, 'ghost small'),
          button('Remove', () => {
            if (confirm(`Remove device ${device.name}?`)) act('device_remove', { name: device.name, scope: layerScope(device.source) }, output).then(() => route()).catch(() => {});
          }, 'danger small'),
        ),
        output,
      ),
    ));
  }
  const name = h('input', { placeholder: 'device-name', autocomplete: 'off' });
  const type = h('select', {}, ...DEVICE_TYPES.map((value) => option(value, value)));
  const ip = h('input', { placeholder: '127.0.0.1', autocomplete: 'off', value: '127.0.0.1' });
  const scope = h('select', {}, option('workspace', 'workspace'), option('global', 'global'));
  const output = outputPanel();
  const form = h('form', { class: 'row' },
    field('Name', name),
    field('Type', type),
    field('IP', ip),
    field('Scope', scope),
    button('Add device', (event) => {
      event.preventDefault();
      if (!name.value.trim() || !ip.value.trim()) { toast('Name and IP are required', 'bad'); return; }
      act('device_add', { name: name.value.trim(), type: type.value, ip: ip.value.trim(), scope: scope.value }, output).then(() => route()).catch(() => {});
    }, ''),
  );
  form.addEventListener('submit', (event) => event.preventDefault());

  main.appendChild(screenHead('Devices', 'Effective definitions from the global and workspace layers'));
  main.appendChild(h('section', { class: 'card' },
    h('div', { class: 'table-wrap' }, table),
    h('h3', { text: 'Add device' }),
    form,
    output,
  ));
}

// ---------------------------------------------------------------------------
// Apps
// ---------------------------------------------------------------------------

async function screenApps() {
  const main = $('#app');
  const data = await getJSON('/api/apps');
  state.apps = data.apps || [];
  const table = h('table', {},
    h('thead', {}, h('tr', {},
      h('th', { text: 'Name' }), h('th', { text: 'Description' }), h('th', { text: 'Platforms' }),
      h('th', { text: 'Source' }), h('th', { text: '' }),
    )),
    h('tbody', {}),
  );
  const body = table.querySelector('tbody');
  if (state.apps.length === 0) body.appendChild(h('tr', {}, h('td', { colspan: '5', class: 'empty', text: 'No apps configured.' })));
  for (const app of state.apps) {
    const output = outputPanel();
    const platforms = Object.entries(app.platforms || {}).map(([platform, entry]) => {
      const id = entry.id || entry.url || entry.package || '';
      return `${platform}${id ? `=${id}` : ''}`;
    });
    body.appendChild(h('tr', {},
      h('td', { class: 'mono', text: app.name }),
      h('td', { class: 'small', text: app.description || '—' }),
      h('td', { class: 'small' }, platforms.length ? platforms.map((value) => pill(value, 'info')) : '—'),
      h('td', { class: 'small', text: app.source || 'effective' }),
      h('td', {},
        h('div', { class: 'row' },
          button('Rename', () => {
            const next = prompt('New app name', app.name);
            if (next && next !== app.name) act('app_rename', { name: app.name, new_name: next, scope: layerScope(app.source) }, output).then(() => route()).catch(() => {});
          }, 'ghost small'),
          button('Remove', () => {
            if (confirm(`Remove app ${app.name}?`)) act('app_remove', { name: app.name, scope: layerScope(app.source) }, output).then(() => route()).catch(() => {});
          }, 'danger small'),
        ),
        output,
      ),
    ));
  }
  const name = h('input', { placeholder: 'app-name', autocomplete: 'off' });
  const description = h('input', { placeholder: 'Optional description', autocomplete: 'off' });
  const platforms = h('textarea', { rows: 3, placeholder: 'web=https://example.test\nandroid=com.example.app', spellcheck: 'false' });
  const scope = h('select', {}, option('workspace', 'workspace'), option('global', 'global'));
  const output = outputPanel();
  const form = h('form', { class: 'stack' },
    h('div', { class: 'row' },
      field('Name', name),
      field('Description', description),
      field('Scope', scope),
    ),
    field('Platforms', platforms, 'One PLATFORM=ID or web=URL entry per line.'),
    button('Add app', (event) => {
      event.preventDefault();
      if (!name.value.trim()) { toast('A name is required', 'bad'); return; }
      const entries = platforms.value.split('\n').map((value) => value.trim()).filter(Boolean);
      act('app_add', { name: name.value.trim(), description: description.value.trim() || undefined, platforms: entries, scope: scope.value }, output).then(() => route()).catch(() => {});
    }, ''),
  );
  form.addEventListener('submit', (event) => event.preventDefault());

  main.appendChild(screenHead('Apps', `Effective apps from ${data.path || 'the workspace and global layers'}`));
  main.appendChild(h('section', { class: 'card' },
    h('div', { class: 'table-wrap' }, table),
    h('h3', { text: 'Add app' }),
    form,
    output,
  ));
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

async function screenConfig() {
  const main = $('#app');
  const data = await getJSON('/api/config');
  state.config = data;
  const values = data.values || {};
  const definitions = data.definitions || [];
  const filter = h('input', { placeholder: 'Filter settings', autocomplete: 'off' });
  const table = h('table', {},
    h('thead', {}, h('tr', {},
      h('th', { text: 'Key' }), h('th', { text: 'Value' }), h('th', { text: 'Source' }), h('th', { text: '' }),
    )),
    h('tbody', {}),
  );
  const body = table.querySelector('tbody');
  const output = outputPanel();
  const renderRows = () => {
    clear(body);
    const needle = filter.value.trim().toLowerCase();
    for (const definition of definitions) {
      if (needle && !`${definition.key} ${definition.doc}`.toLowerCase().includes(needle)) continue;
      const entry = values[definition.key] || {};
      const value = entry.value;
      const rendered = value === null || value === undefined
        ? h('span', { class: 'muted', text: 'unset' })
        : h('span', { class: 'mono small', text: Array.isArray(value) ? value.join(', ') : String(value) });
      body.appendChild(h('tr', {},
        h('td', { class: 'mono small', title: definition.doc, text: definition.key }),
        h('td', {}, rendered),
        h('td', { class: 'small muted', text: entry.source || definition.default || 'default' }),
        h('td', {}, button('Unset', () => {
          act('config_unset', { key: definition.key }, output).then(() => route()).catch(() => {});
        }, 'ghost small')),
      ));
    }
  };
  filter.addEventListener('input', renderRows);
  renderRows();

  const key = h('select', {}, ...definitions.map((definition) => option(definition.key, definition.key)));
  const value = h('input', { placeholder: 'new value', autocomplete: 'off' });
  const scope = h('select', {}, option('workspace', 'workspace'), option('global', 'global'));
  const form = h('form', { class: 'row' },
    field('Key', key),
    field('Value', value),
    field('Scope', scope),
    button('Set', (event) => {
      event.preventDefault();
      if (!value.value.trim()) { toast('A value is required', 'bad'); return; }
      act('config_set', { key: key.value, value: value.value, scope: scope.value }, output).then(() => route()).catch(() => {});
    }, ''),
  );
  form.addEventListener('submit', (event) => event.preventDefault());

  main.appendChild(screenHead('Config', 'Effective settings, winning layers and the schema', [
    pill(`global: ${data.paths?.global || '—'}`, 'info'),
    pill(`workspace: ${data.paths?.workspace || '—'}`, 'info'),
  ]));
  main.appendChild(h('section', { class: 'card' },
    h('div', { class: 'row between' }, h('h3', { text: 'Settings' }), filter),
    h('div', { class: 'table-wrap' }, table),
  ));
  main.appendChild(h('section', { class: 'card' },
    h('h3', { text: 'Set a setting' }),
    form,
    output,
  ));
}

// ---------------------------------------------------------------------------
// Resident
// ---------------------------------------------------------------------------

async function screenResident() {
  const main = $('#app');
  const [data, notifications] = await Promise.all([
    getJSON('/api/resident'),
    getJSON('/api/notifications?limit=20').catch(() => ({ notifications: [] })),
  ]);
  state.resident = data;
  const status = data.status || {};
  const service = status.state || {};
  const output = outputPanel();
  const serviceButtons = h('div', { class: 'row' },
    button('Start', () => act('service_start', {}, output).then(() => route()).catch(() => {}), 'ghost small'),
    button('Stop', () => act('service_stop', {}, output).then(() => route()).catch(() => {}), 'ghost small'),
    button('Restart', () => act('service_restart', {}, output).then(() => route()).catch(() => {}), 'ghost small'),
    button('Pause', () => act('service_pause', {}, output).then(() => route()).catch(() => {}), 'ghost small'),
    button('Resume', () => act('service_resume', {}, output).then(() => route()).catch(() => {}), 'ghost small'),
    button('Install logon hook', () => act('service_install', {}, output).then(() => route()).catch(() => {}), 'ghost small'),
    button('Uninstall hook', () => {
      if (confirm('Remove the logon hook? History and config are kept.')) act('service_uninstall', {}, output).then(() => route()).catch(() => {});
    }, 'danger small'),
  );

  const scheduleTable = h('table', {},
    h('thead', {}, h('tr', {},
      h('th', { text: 'Schedule' }), h('th', { text: 'Enabled' }), h('th', { text: 'When' }),
      h('th', { text: 'Next fire' }), h('th', { text: 'Runs' }), h('th', { text: 'Failures' }), h('th', { text: '' }),
    )),
    h('tbody', {}),
  );
  const scheduleBody = scheduleTable.querySelector('tbody');
  for (const schedule of status.schedules || []) {
    scheduleBody.appendChild(h('tr', {},
      h('td', { class: 'mono', text: schedule.name }),
      h('td', {}, pill(schedule.enabled ? 'enabled' : 'disabled', schedule.enabled ? 'ok' : 'warn')),
      h('td', { class: 'small', text: schedule.when }),
      h('td', { class: 'small', text: schedule.next_fire || '—' }),
      h('td', { text: String(schedule.runs) }),
      h('td', { text: String(schedule.failures) }),
      h('td', {},
        h('div', { class: 'row' },
          button('Run now', () => act('schedule_run_now', { name: schedule.name }, output).catch(() => {}), 'ghost small'),
          button(schedule.enabled ? 'Disable' : 'Enable', () => act(schedule.enabled ? 'schedule_disable' : 'schedule_enable', { name: schedule.name }, output).then(() => route()).catch(() => {}), 'ghost small'),
          button('Remove', () => {
            if (confirm(`Remove schedule ${schedule.name}?`)) act('schedule_remove', { name: schedule.name }, output).then(() => route()).catch(() => {});
          }, 'danger small'),
        ),
      ),
    ));
  }
  if ((status.schedules || []).length === 0) scheduleBody.appendChild(h('tr', {}, h('td', { colspan: '7', class: 'empty', text: 'No schedules.' })));

  const watcherTable = h('table', {},
    h('thead', {}, h('tr', {},
      h('th', { text: 'Watcher' }), h('th', { text: 'Enabled' }), h('th', { text: 'Paused' }),
      h('th', { text: 'Lease' }), h('th', { text: 'Iterations' }), h('th', { text: 'Failures' }), h('th', { text: 'Last dump' }), h('th', { text: '' }),
    )),
    h('tbody', {}),
  );
  const watcherBody = watcherTable.querySelector('tbody');
  for (const watcher of status.watchers || []) {
    watcherBody.appendChild(h('tr', {},
      h('td', { class: 'mono', text: watcher.name }),
      h('td', {}, pill(watcher.enabled ? 'enabled' : 'disabled', watcher.enabled ? 'ok' : 'warn')),
      h('td', {}, pill(watcher.paused ? 'paused' : 'running', watcher.paused ? 'warn' : 'ok')),
      h('td', { class: 'small', text: watcher.lease }),
      h('td', { text: String(watcher.iterations) }),
      h('td', { text: String(watcher.failures) }),
      h('td', { class: 'small mono', text: watcher.last_dump || '—' }),
      h('td', {},
        h('div', { class: 'row' },
          button('Run now', () => act('watcher_run_now', { name: watcher.name }, output).catch(() => {}), 'ghost small'),
          button(watcher.paused ? 'Resume' : 'Pause', () => act(watcher.paused ? 'watcher_resume' : 'watcher_pause', { name: watcher.name }, output).then(() => route()).catch(() => {}), 'ghost small'),
          button('Start', () => act('watcher_start', { name: watcher.name }, output).then(() => route()).catch(() => {}), 'ghost small'),
          button('Stop', () => act('watcher_stop', { name: watcher.name }, output).then(() => route()).catch(() => {}), 'ghost small'),
          button('Remove', () => {
            if (confirm(`Remove watcher ${watcher.name}?`)) act('watcher_remove', { name: watcher.name }, output).then(() => route()).catch(() => {});
          }, 'danger small'),
        ),
      ),
    ));
  }
  if ((status.watchers || []).length === 0) watcherBody.appendChild(h('tr', {}, h('td', { colspan: '8', class: 'empty', text: 'No watchers.' })));

  const scheduleName = h('input', { placeholder: 'nightly', autocomplete: 'off' });
  const scheduleWhen = h('input', { placeholder: 'kind: daily, at: "02:30"', autocomplete: 'off' });
  const schedulePlays = h('input', { placeholder: 'play names, comma separated', autocomplete: 'off' });
  const scheduleActors = h('input', { placeholder: 'devices or @groups', autocomplete: 'off' });
  const scheduleBusy = h('select', {}, option('queue', 'queue'), option('skip', 'skip'), option('fail', 'fail'));
  const scheduleDisabled = h('input', { type: 'checkbox' });
  const scheduleForm = h('form', { class: 'stack' },
    h('div', { class: 'row' },
      field('Name', scheduleName),
      field('When', scheduleWhen),
      field('Busy policy', scheduleBusy),
      h('label', { class: 'row' }, scheduleDisabled, h('span', { class: 'small', text: 'Disabled' })),
    ),
    h('div', { class: 'row' },
      field('Plays', schedulePlays),
      field('Actors', scheduleActors),
      button('Add schedule', (event) => {
        event.preventDefault();
        if (!scheduleName.value.trim() || !scheduleWhen.value.trim()) { toast('Name and when are required', 'bad'); return; }
        act('schedule_add', {
          name: scheduleName.value.trim(),
          when: scheduleWhen.value.trim(),
          play: schedulePlays.value.split(',').map((value) => value.trim()).filter(Boolean),
          actor: scheduleActors.value.split(',').map((value) => value.trim()).filter(Boolean),
          on_busy: scheduleBusy.value,
          disabled: scheduleDisabled.checked,
        }, output).then(() => route()).catch(() => {});
      }, ''),
    ),
  );
  scheduleForm.addEventListener('submit', (event) => event.preventDefault());

  const watcherName = h('input', { placeholder: 'smoke-watch', autocomplete: 'off' });
  const watcherPlays = h('input', { placeholder: 'play names, comma separated', autocomplete: 'off' });
  const watcherActors = h('input', { placeholder: 'devices or @groups', autocomplete: 'off' });
  const watcherGap = h('input', { placeholder: '30s (optional)', autocomplete: 'off' });
  const watcherLease = h('select', {}, option('gap', 'gap'), option('no-gap', 'no-gap'));
  const watcherDisabled = h('input', { type: 'checkbox' });
  const watcherForm = h('form', { class: 'stack' },
    h('div', { class: 'row' },
      field('Name', watcherName),
      field('Gap', watcherGap),
      field('Lease', watcherLease),
      h('label', { class: 'row' }, watcherDisabled, h('span', { class: 'small', text: 'Disabled' })),
    ),
    h('div', { class: 'row' },
      field('Plays', watcherPlays),
      field('Actors', watcherActors),
      button('Add watcher', (event) => {
        event.preventDefault();
        if (!watcherName.value.trim()) { toast('A name is required', 'bad'); return; }
        act('watcher_add', {
          name: watcherName.value.trim(),
          play: watcherPlays.value.split(',').map((value) => value.trim()).filter(Boolean),
          actor: watcherActors.value.split(',').map((value) => value.trim()).filter(Boolean),
          gap: watcherGap.value.trim() || undefined,
          lease: watcherLease.value,
          disabled: watcherDisabled.checked,
        }, output).then(() => route()).catch(() => {});
      }, ''),
    ),
  );
  watcherForm.addEventListener('submit', (event) => event.preventDefault());

  const dumpsList = h('div', { class: 'stack' });
  for (const dump of data.dumps || []) {
    dumpsList.appendChild(h('div', { class: 'row between' },
      h('span', { class: 'mono small', text: `${dump.watcher}/${dump.name}` }),
      button('Open dump', async () => {
        try {
          const dumpData = await getJSON(`/api/dumps?watcher=${encodeURIComponent(dump.watcher)}`);
          const entry = (dumpData.dumps || []).find((value) => value.watcher === dump.watcher && value.name === dump.name);
          if (!entry) throw new ApiFailure('dump disappeared', 'not_found');
          const artifacts = await getJSON(`/api/dumps/artifact?path=${encodeURIComponent(`${entry.path}/dump.json`)}`);
          showDialog(h('div', { class: 'stack' },
            h('h3', { class: 'mono', text: `${dump.watcher}/${dump.name}` }),
            h('pre', { class: 'output', text: artifacts.text }),
          ));
        } catch (error) {
          toast(error.message, 'bad');
        }
      }, 'ghost small'),
    ));
  }
  if ((data.dumps || []).length === 0) dumpsList.appendChild(h('p', { class: 'empty', text: 'No watcher dumps.' }));

  const notificationList = h('div', { class: 'stack' });
  const events = Array.isArray(notifications.notifications) ? notifications.notifications : [];
  for (const event of events.slice(0, 20)) {
    notificationList.appendChild(h('div', { class: 'banner' },
      h('div', { class: 'row between' },
        h('strong', { text: event.kind || event.channel || 'notification' }),
        h('span', { class: 'tiny muted', text: event.at || event.time || '' }),
      ),
      h('p', { class: 'small wrap', text: event.message || event.text || JSON.stringify(event) }),
    ));
  }
  if (events.length === 0) notificationList.appendChild(h('p', { class: 'empty', text: 'No notifications.' }));

  main.appendChild(screenHead('Resident', 'Service, schedules, watchers, dumps and notifications', [
    pill(status.running ? 'running' : 'stopped', status.running ? 'ok' : 'warn'),
    status.restart_required ? pill('restart required', 'warn') : null,
    status.heartbeat_stale ? pill('stale heartbeat', 'bad') : null,
  ]));
  main.appendChild(h('section', { class: 'card' },
    h('div', { class: 'row between' },
      h('h3', { text: 'Service' }),
      pill(`pid ${service.pid || '—'}`, 'info'),
    ),
    kv([
      ['Running', status.running ? 'yes' : 'no'],
      ['Runner version', status.runner_version || '—'],
      ['Client version', status.client_version || '—'],
      ['Paused', service.paused ? 'yes' : 'no'],
      ['Last heartbeat', service.heartbeat_at || service.updated_at || '—'],
      ['Log', data.log_path || '—'],
    ]),
    serviceButtons,
    output,
  ));
  main.appendChild(h('section', { class: 'card' },
    h('h3', { text: 'Schedules' }),
    h('div', { class: 'table-wrap' }, scheduleTable),
    h('h3', { text: 'Add schedule' }),
    scheduleForm,
  ));
  main.appendChild(h('section', { class: 'card' },
    h('h3', { text: 'Watchers' }),
    h('div', { class: 'table-wrap' }, watcherTable),
    h('h3', { text: 'Add watcher' }),
    watcherForm,
  ));
  main.appendChild(h('div', { class: 'grid two' },
    h('section', { class: 'card' }, h('h3', { text: 'Watcher dumps' }), dumpsList),
    h('section', { class: 'card' }, h('h3', { text: 'Notifications' }), notificationList),
  ));
}

// ---------------------------------------------------------------------------
// Docs
// ---------------------------------------------------------------------------

async function screenDocs(arg) {
  const main = $('#app');
  const data = await getJSON('/api/docs');
  state.docs = data.docs || [];
  const slug = arg || (state.docs[0] && state.docs[0].slug) || 'web';
  const select = h('select', {}, ...state.docs.map((doc) => option(doc.slug, doc.title, doc.slug === slug)));
  const search = h('input', { placeholder: 'Filter documents', autocomplete: 'off' });
  const list = h('div', { class: 'stack' });
  const content = h('pre', { class: 'output', text: 'Loading…' });
  const load = async (value) => {
    clear(content);
    content.textContent = 'Loading…';
    try {
      const doc = await getJSON(`/api/docs/${encodeURIComponent(value)}`);
      content.textContent = doc.text;
      select.value = value;
      for (const child of list.children) child.classList.toggle('selected', child.dataset.slug === value);
      if (location.hash !== `#/docs/${value}`) history.replaceState(null, '', `#/docs/${value}`);
    } catch (error) {
      content.textContent = error.message;
    }
  };
  const renderList = () => {
    clear(list);
    const needle = search.value.trim().toLowerCase();
    for (const doc of state.docs) {
      if (needle && !`${doc.title} ${doc.description}`.toLowerCase().includes(needle)) continue;
      list.appendChild(h('button', {
        type: 'button',
        class: `ghost small doc-item ${doc.slug === slug ? 'selected' : ''}`,
        dataset: { slug: doc.slug },
        onclick: () => load(doc.slug),
      }, h('div', { class: 'stack' },
        h('strong', { text: doc.title }),
        h('span', { class: 'tiny muted', text: doc.description }),
      )));
    }
  };
  search.addEventListener('input', renderList);
  select.addEventListener('change', () => load(select.value));
  renderList();

  main.appendChild(screenHead('Docs', 'The documentation compiled into this binary', [
    button('Open raw Markdown', () => window.open(`/api/docs/${encodeURIComponent(select.value)}/raw`, '_blank', 'noopener'), 'ghost'),
  ]));
  main.appendChild(h('div', { class: 'split' },
    h('section', { class: 'card' }, h('h3', { text: 'Documents' }), search, list),
    h('section', { class: 'card' },
      h('div', { class: 'row between' }, h('h3', { text: 'Reader' }), select),
      content,
    ),
  ));
  await load(slug);
}

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

async function screenApi() {
  const main = $('#app');
  const [schema, guide] = await Promise.all([getJSON('/api/schema'), getJSON('/api/guide')]);
  state.schema = schema;
  const origin = window.location.origin;

  const steps = h('div', { class: 'checklist' });
  for (const step of guide.steps || []) {
    const example = `curl -s -X POST ${origin}/api/actions/${step.action.name} \\\n  -H 'Content-Type: application/json' \\\n  -d '${JSON.stringify(step.action.body)}'`;
    steps.appendChild(h('div', { class: 'check' },
      h('span', { class: 'dot', text: '→' }),
      h('div', {},
        h('strong', { text: step.title }),
        h('p', { class: 'muted small', text: step.body }),
        h('p', { class: 'tiny muted mono', text: step.endpoint }),
      ),
      button('Copy curl', async () => {
        try {
          await navigator.clipboard.writeText(example);
          toast('Copied', 'ok');
        } catch {
          showDialog(h('pre', { class: 'output', text: example }));
        }
      }, 'ghost small'),
    ));
  }

  const endpointTable = h('table', {},
    h('thead', {}, h('tr', {}, h('th', { text: 'Method' }), h('th', { text: 'Path' }), h('th', { text: 'Purpose' }))),
    h('tbody', {}),
  );
  const endpointBody = endpointTable.querySelector('tbody');
  for (const endpoint of schema.endpoints || []) {
    endpointBody.appendChild(h('tr', {},
      h('td', { class: 'small mono', text: endpoint.method }),
      h('td', { class: 'small mono wrap', text: endpoint.path }),
      h('td', { class: 'small', text: endpoint.description }),
    ));
  }

  const actionTable = h('table', {},
    h('thead', {}, h('tr', {}, h('th', { text: 'Action' }), h('th', { text: 'Description' }), h('th', { text: 'Parameters' }))),
    h('tbody', {}),
  );
  const actionBody = actionTable.querySelector('tbody');
  for (const action of schema.actions || []) {
    const params = Object.entries(action.params || {}).map(([name, definition]) => {
      const required = definition.required ? 'required' : 'optional';
      return `${name} (${definition.type}, ${required})`;
    });
    actionBody.appendChild(h('tr', {},
      h('td', { class: 'mono small', text: action.name }),
      h('td', { class: 'small', text: action.description }),
      h('td', { class: 'small wrap', text: params.join('; ') || '—' }),
    ));
  }

  main.appendChild(screenHead('API', 'Machine-readable guidance for agents and scripts', [
    pill(`${(schema.actions || []).length} actions · ${(schema.endpoints || []).length} endpoints`, 'info'),
    button('Open raw schema', () => window.open('/api/schema', '_blank', 'noopener'), 'ghost'),
  ]));
  main.appendChild(h('div', { class: 'grid two' },
    h('section', { class: 'card' },
      h('h3', { text: 'Guided flow' }),
      h('p', { class: 'muted small', text: 'Every step is one HTTP action. The same flow drives the Start screen.' }),
      steps,
    ),
    h('section', { class: 'card' },
      h('h3', { text: 'Envelope and security' }),
      h('pre', { class: 'output', text: JSON.stringify({ envelope: schema.envelope, security: schema.security }, null, 2) }),
      h('p', { class: 'muted small', text: 'Mutations are serialized. Values starting with "-" are rejected, and action names are an allowlist.' }),
    ),
  ));
  main.appendChild(h('section', { class: 'card' },
    h('h3', { text: 'Endpoints' }),
    h('div', { class: 'table-wrap' }, endpointTable),
  ));
  main.appendChild(h('section', { class: 'card' },
    h('h3', { text: 'Actions' }),
    h('div', { class: 'table-wrap' }, actionTable),
  ));
}

function showDialog(content) {
  const dialog = $('#dialog');
  const body = $('#dialog-body');
  clear(body);
  body.appendChild(content);
  body.appendChild(h('div', { class: 'row end' }, button('Close', () => dialog.close(), 'ghost')));
  if (!dialog.open) dialog.showModal();
}

// ---------------------------------------------------------------------------
// Router table
// ---------------------------------------------------------------------------

const SCREENS = {
  start: screenStart,
  runs: screenRuns,
  tests: screenTests,
  coverage: screenCoverage,
  probe: screenProbe,
  groups: screenGroups,
  devices: screenDevices,
  apps: screenApps,
  config: screenConfig,
  resident: screenResident,
  docs: screenDocs,
  api: screenApi,
};

document.addEventListener('DOMContentLoaded', boot);
