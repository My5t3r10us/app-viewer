const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let state = { apps: [], active: null, settings: {}, detached: [], agentControl: [] };
const logs = new Map(); // id -> string[] (loaded lazily, then kept up to date by events)
let stats = {}; // id -> { cpu, memory, processes, scope }, refreshed by the backend every 1.5 s
let modalOpen = false;
let settingsOpen = false;
let downloadsOpen = false;
let downloads = []; // newest first, pushed by the backend ("downloads")
const pageOpen = () => settingsOpen || downloadsOpen; // a full page replaces the app view

// Layout preferences (per machine, kept in localStorage).
const SIDEBAR_DEFAULT = 248, SIDEBAR_MIN = 180, SIDEBAR_MAX = 420, SIDEBAR_SNAP = 120;
const LOGS_DEFAULT = 240, LOGS_MIN = 120;
const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
let sidebarW = clamp(Number(readPref('sidebarW')) || SIDEBAR_DEFAULT, SIDEBAR_MIN, SIDEBAR_MAX);
let sidebarHidden = readPref('sidebarHidden') === '1';
let peeking = false;
let logsH = Math.max(LOGS_MIN, Number(readPref('logsH')) || LOGS_DEFAULT);
let logsOpen = readPref('logsOpen') === '1';

const activeApp = () => state.apps.find((a) => a.id === state.active) || null;
const isUp = (a) => a && (a.status === 'running' || a.status === 'external');
const isNative = (a) => a && a.kind === 'native';
const isBusy = (a) => a && (isUp(a) || a.status === 'starting');
const isDetached = (id) => state.detached?.includes(id);

// ---------------------------------------------------------------- viewport
// The hosted apps are native webviews laid over #viewport; tell the backend where it is.
let lastViewport = '';
let viewportQueued = false;
function reportViewport() {
  if (viewportQueued) return;
  viewportQueued = true;
  requestAnimationFrame(() => {
    viewportQueued = false;
    const r = $('#viewport').getBoundingClientRect();
    let x = r.left;
    // The native webview sits above the UI, so a revealed (peeking) sidebar would be hidden
    // under it: slide the webview right instead, keeping its width so the app doesn't reflow.
    if (sidebarHidden) x = Math.max(x, $('#sidebar').getBoundingClientRect().right);
    const visible = isUp(activeApp()) && !modalOpen && !pageOpen();
    const args = { x, y: r.top, width: r.width, height: r.height, visible };
    const key = JSON.stringify(args);
    if (key === lastViewport) return;
    lastViewport = key;
    invoke('set_viewport', args);
  });
}

/** Keeps the webview glued to the sidebar while it slides (CSS transitions don't trigger ResizeObserver). */
let trackUntil = 0;
function trackViewport(ms = 240) {
  const running = performance.now() < trackUntil;
  trackUntil = performance.now() + ms;
  if (running) return;
  const tick = () => {
    reportViewport();
    if (performance.now() < trackUntil) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

// ---------------------------------------------------------------- layout
function applyLayout() {
  const shell = $('#shell');
  shell.style.setProperty('--sidebar-w', `${sidebarW}px`);
  shell.style.setProperty('--logs-h', `${logsH}px`);
  shell.classList.toggle('side-hidden', sidebarHidden);
  shell.classList.toggle('side-peek', sidebarHidden && peeking);
  $('#pin-btn').hidden = !(sidebarHidden && peeking);
  $('#hide-btn').hidden = sidebarHidden && peeking;
}

function toggleSidebar(hidden = !sidebarHidden) {
  sidebarHidden = hidden;
  peeking = false;
  writePref('sidebarHidden', hidden ? '1' : '0');
  applyLayout();
  renderToolbar();
  trackViewport();
}

let peekTimer = 0, unpeekTimer = 0;
function setPeek(on) {
  clearTimeout(peekTimer);
  clearTimeout(unpeekTimer);
  if (peeking === on || (on && !sidebarHidden)) return;
  peeking = on;
  applyLayout();
  trackViewport();
}

const rail = $('#sidebar-rail');
rail.addEventListener('mouseenter', (e) => {
  // Short dwell so crossing the strip on the way to the window's resize border doesn't pop it.
  if (!sidebarHidden || !state.settings.revealSidebarOnHover || e.buttons) return;
  clearTimeout(unpeekTimer);
  peekTimer = setTimeout(() => setPeek(true), 120);
});
rail.addEventListener('mouseleave', () => clearTimeout(peekTimer));
rail.addEventListener('click', () => toggleSidebar(false));

const sidebar = $('#sidebar');
sidebar.addEventListener('mouseenter', () => clearTimeout(unpeekTimer));
sidebar.addEventListener('mouseleave', () => {
  if (!peeking || document.body.classList.contains('dragging') || modalOpen) return;
  unpeekTimer = setTimeout(() => setPeek(false), 300);
});

// Sidebar width: drag the right edge; drop it below SIDEBAR_SNAP to hide the sidebar.
let dragW0 = 0;
dragHandle($('#sidebar-resizer'), {
  cursor: 'col-resize',
  onStart: () => { dragW0 = sidebarW; },
  onMove: (dx) => {
    const raw = dragW0 + dx;
    sidebarW = clamp(raw, SIDEBAR_MIN, SIDEBAR_MAX);
    $('#shell').classList.toggle('collapse-hint', raw < SIDEBAR_SNAP);
    applyLayout();
    reportViewport();
  },
  onEnd: (dx) => {
    $('#shell').classList.remove('collapse-hint');
    if (dragW0 + dx < SIDEBAR_SNAP) {
      sidebarW = dragW0;
      if (peeking) setPeek(false); else toggleSidebar(true);
    }
    writePref('sidebarW', String(sidebarW));
    applyLayout();
    reportViewport();
  },
});
$('#sidebar-resizer').addEventListener('dblclick', () => {
  sidebarW = SIDEBAR_DEFAULT;
  writePref('sidebarW', String(sidebarW));
  applyLayout();
  reportViewport();
});

// Terminal height: drag the top edge of the panel.
let dragH0 = 0;
const maxLogsH = () => Math.max(LOGS_MIN, document.querySelector('.main').clientHeight - 160);
dragHandle($('#logs-resizer'), {
  cursor: 'row-resize',
  onStart: () => { dragH0 = logsH; },
  onMove: (_, dy) => {
    logsH = clamp(dragH0 - dy, LOGS_MIN, maxLogsH());
    applyLayout();
    reportViewport();
  },
  onEnd: () => writePref('logsH', String(logsH)),
});
$('#logs-resizer').addEventListener('dblclick', () => {
  logsH = clamp(LOGS_DEFAULT, LOGS_MIN, maxLogsH());
  writePref('logsH', String(logsH));
  applyLayout();
  reportViewport();
});

// ---------------------------------------------------------------- logs
async function ensureLogs(id) {
  if (!id || logs.has(id)) return;
  logs.set(id, await invoke('get_logs', { id }));
  renderLogs();
  renderStage();
}

function onLog({ id, line }) {
  // A launch starts with a "▶ command" line and the backend clears the buffer at that point.
  const reset = line.startsWith('▶ ');
  if (reset) logs.set(id, []);
  const buf = logs.get(id);
  if (!buf) return;
  buf.push(line);
  if (buf.length > MAX_LOG_LINES) buf.shift();
  if (id !== state.active) return;
  if (!$('#logs').hidden) {
    if (reset) fillLines($('#log-lines'), buf);
    else appendLine($('#log-lines'), line);
  }
  const a = activeApp();
  if (a && (a.status === 'starting' || a.status === 'error')) renderTail();
}

let logsKey = '';
function renderLogs() {
  const panel = $('#logs');
  const a = activeApp();
  panel.hidden = !logsOpen || !a || isDetached(a.id) || pageOpen();
  if (panel.hidden) { logsKey = ''; return; }
  $('#logs-title').textContent = `Logs — ${a.name}`;
  const detach = $('#logs-detach');
  detach.disabled = !(a.status === 'starting' || a.status === 'running');
  detach.title = detach.disabled
    ? 'Détacher (disponible pendant que la commande tourne)'
    : 'Détacher dans une fenêtre (se ferme à la fin de la commande)';
  // Only rebuild the lines when switching app, not on every status change.
  const key = `${a.id}|${logs.has(a.id)}`;
  if (key === logsKey) return;
  logsKey = key;
  fillLines($('#log-lines'), logs.get(a.id));
}

function toggleLogs(force) {
  const a = activeApp();
  // A detached terminal is shown in its own window: bring it to front instead.
  if (a && isDetached(a.id) && force !== false) return invoke('detach_logs', { id: a.id });
  logsOpen = force ?? !logsOpen;
  writePref('logsOpen', logsOpen ? '1' : '0');
  logsKey = '';
  renderLogs();
  renderToolbar();
  reportViewport();
}

// ---------------------------------------------------------------- render
function render() {
  renderSidebar();
  renderToolbar();
  renderStage();
  renderLogs();
  renderSettings();
  renderDownloads();
  reportViewport();
  ensureLogs(state.active);
}

function renderSidebar() {
  $('#app-list').innerHTML = state.apps.map((a, i) => {
    const busy = isBusy(a);
    return `
    <li class="app-item ${a.id === state.active ? 'active' : ''}" data-id="${esc(a.id)}" title="${esc(a.path)}">
      <span class="avatar" style="--h:${hue(a.name)}">${esc(initials(a.name))}<span class="dot ${a.status}"></span></span>
      <span class="meta">
        <span class="name">${esc(a.name)}</span>
        <span class="sub">${isNative(a) ? 'native' : `:${a.port}`} <span class="st ${a.status}">· ${STATUS_LABEL[a.status]}</span></span>
      </span>
      ${i < 9 ? `<kbd class="key" title="Ctrl+${i + 1}">${i + 1}</kbd>` : ''}
      <span class="quick">
        ${busy
          ? `<button class="icon-btn xs stop" data-quick="stop" title="${a.status === 'external' ? 'Détacher' : 'Arrêter'}">${icon('stop')}</button>`
          : `<button class="icon-btn xs go" data-quick="start" title="Démarrer">${icon('play')}</button>`}
        <button class="icon-btn xs" data-quick="restart" title="Redémarrer" ${a.status === 'external' ? 'disabled' : ''}>${icon('restart')}</button>
      </span>
    </li>`;
  }).join('') || `
    <li class="side-empty">
      <p>Aucune application pour le moment.</p>
      <button class="btn sm" data-act="add">${icon('plus')} Ajouter</button>
    </li>`;

  const up = state.apps.filter(isBusy).length;
  $('#side-count').textContent = state.apps.length || '';
  $('#side-summary').innerHTML = state.apps.length
    ? `<span class="dot ${up ? 'running' : 'stopped'}"></span>${up ? `${up} en cours` : 'Tout est arrêté'}`
    : '';
}

function renderToolbar() {
  const bar = $('#toolbar');
  const a = activeApp();
  const sideBtn = sidebarHidden
    ? `<button class="icon-btn" data-act="sidebar" title="Afficher la sidebar (Ctrl+B)">${icon('sidebarOpen')}</button>`
    : '';
  if (!a) { bar.innerHTML = sideBtn; toolbarKey = ''; return; }
  // A native app has no page here: the web actions stay off.
  const web = !isNative(a);
  const up = isUp(a) && web;
  const detached = isDetached(a.id);
  const agentOn = isAgentOn(a.id);
  const html = `
    ${sideBtn}
    <div class="title">
      <h1>${esc(a.name)}</h1>
      <span class="badge ${a.status}"><span class="dot ${a.status}"></span>${STATUS_LABEL[a.status]}</span>
      ${a.status === 'external' ? `<button class="btn sm danger" id="takeover-btn" data-act="takeover" title="Arrête le process qui occupe le port ${a.port} et relance l'app depuis App Viewer">Reprendre la main</button>` : ''}
    </div>
    <div class="addr" id="addr">
      <input id="addr-input" class="mono" list="addr-history" spellcheck="false" autocomplete="off"
        title="${ADDR_TITLE}" ${up ? '' : 'readonly'}>
      <button class="icon-btn sm" data-act="copy-url" id="copy-url" title="Copier l'URL">${icon('copy')}</button>
    </div>
    <span class="stats" id="stats" hidden></span>
    <span class="zoom" id="zoom" hidden></span>
    <span class="agent-pill" id="agent-pill" hidden title="Un agent agit sur cette page (appv ui). Détails dans les logs.">${icon('bot')} Agent en contrôle</span>
    <div class="actions">
      <button class="icon-btn" data-act="home" title="Page d'accueil de l'app" ${up ? '' : 'disabled'}>${icon('home')}</button>
      <button class="icon-btn" data-act="reload" title="Recharger la page (Ctrl+R)" ${up ? '' : 'disabled'}>${icon('reload')}</button>
      <span class="sep"></span>
      ${isBusy(a)
        ? `<button class="icon-btn stop" data-act="stop" title="${a.status === 'external' ? 'Détacher (process non géré par App Viewer)' : 'Arrêter'}">${icon('stop')}</button>`
        : `<button class="icon-btn go" data-act="start" title="Démarrer">${icon('play')}</button>`}
      <button class="icon-btn" data-act="restart" title="Redémarrer" ${a.status === 'external' ? 'disabled' : ''}>${icon('restart')}</button>
      <span class="sep"></span>
      <button class="icon-btn" data-act="browser" title="Ouvrir dans le navigateur" ${web ? '' : 'disabled'}>${icon('external')}</button>
      <button class="icon-btn" data-act="folder" title="Ouvrir le dossier">${icon('folder')}</button>
      <button class="icon-btn" data-act="devtools" title="Outils de développement" ${up ? '' : 'disabled'}>${icon('code')}</button>
      <span class="sep"></span>
      <button class="icon-btn ${agentOn ? 'on' : ''}" data-act="agent" title="${agentOn ? 'Contrôle par agent AUTORISÉ pour cette app (jusqu\'à la fermeture d\'App Viewer). Cliquer pour retirer.' : 'Autoriser un agent IA à piloter cette app avec appv ui (clic, saisie, lecture). Désactivé par défaut, valable jusqu\'à la fermeture d\'App Viewer.'}">${icon('bot')}</button>
      <button class="icon-btn ${logsOpen || detached ? 'on' : ''}" data-act="logs" title="${detached ? 'Terminal détaché : afficher sa fenêtre' : 'Logs (Ctrl+L)'}">${icon(detached ? 'popout' : 'terminal')}</button>
      <button class="icon-btn" data-act="edit" title="Modifier">${icon('edit')}</button>
    </div>`;
  // Every state event re-renders: only rebuild when something changed, so typing an
  // address isn't interrupted; when it must rebuild, carry the edit over.
  const key = a.id + html;
  if (key !== toolbarKey) {
    const old = $('#addr-input');
    const editing = old && document.activeElement === old && old.dataset.app === a.id
      ? { value: old.value, start: old.selectionStart, end: old.selectionEnd } : null;
    bar.innerHTML = html;
    toolbarKey = key;
    const input = $('#addr-input');
    input.dataset.app = a.id;
    if (editing) {
      input.value = editing.value;
      input.focus();
      input.setSelectionRange(editing.start, editing.end);
    }
  }
  renderAddress();
  renderStats();
  renderZoom();
  renderAgent();
}

// ---------------------------------------------------------------- agent control
// The user's opt-in for `appv ui`: per app, off by default, session only (the backend keeps it in
// memory). The pill shows while an agent acts on the page; every action is also in the app's logs.
const isAgentOn = (id) => state.agentControl?.includes(id);
const agentSeen = {}; // id -> time of the last agent action ("agent-activity")
const AGENT_PILL_MS = 4000;
let agentTimer = 0;

function renderAgent() {
  const el = $('#agent-pill');
  if (!el) return;
  const a = activeApp();
  const left = a ? AGENT_PILL_MS - (Date.now() - (agentSeen[a.id] || 0)) : 0;
  el.hidden = !(left > 0 && isAgentOn(a.id));
  clearTimeout(agentTimer);
  if (!el.hidden) agentTimer = setTimeout(renderAgent, left + 50);
}

// ---------------------------------------------------------------- zoom
// WebView2 handles Ctrl+wheel / Ctrl +/-/0 inside the app itself; the backend remembers the factor
// per app and tells us ("app-zoom"). The pill only shows up when the page is not at 100 %.
const ZOOM_STEPS = [0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3, 4, 5];
const zoomOf = (id) => state.settings?.zoom?.[id] ?? 1;

function renderZoom() {
  const el = $('#zoom');
  if (!el) return;
  const a = activeApp();
  const f = a ? zoomOf(a.id) : 1;
  el.hidden = !a || !isUp(a) || Math.abs(f - 1) < 0.005;
  if (el.hidden) return;
  el.innerHTML = `
    <button class="icon-btn xs" data-act="zoom-out" title="Dézoomer (Ctrl+-)">${icon('minus')}</button>
    <button class="zoom-val" data-act="zoom-reset" title="Remettre à 100 % (Ctrl+0)">${Math.round(f * 100)} %</button>
    <button class="icon-btn xs" data-act="zoom-in" title="Zoomer (Ctrl++)">${icon('plus')}</button>`;
}

function stepZoom(dir) {
  const a = activeApp();
  if (!a || !isUp(a)) return;
  const f = zoomOf(a.id);
  const next = dir > 0 ? ZOOM_STEPS.find((z) => z > f + 0.005) : [...ZOOM_STEPS].reverse().find((z) => z < f - 0.005);
  if (next) invoke('set_zoom', { id: a.id, factor: next }).catch(() => {});
}

function onZoom({ id, factor }) {
  state.settings = { ...state.settings, zoom: { ...state.settings.zoom, [id]: factor } };
  if (id === state.active) renderZoom();
}

// ---------------------------------------------------------------- address bar
let toolbarKey = '';
const currentUrls = {}; // id -> address of its webview, pushed by the backend ("app-url")
const HISTORY_MAX = 40;
const ADDR_TITLE = 'Adresse de la page (F6) · un chemin (/admin) ou une URL, puis Entrée';
const currentUrl = (a) => (isUp(a) && (currentUrls[a.id] || a.currentUrl)) || a.url;

/** Shows the page's address, unless the user is typing one. */
function renderAddress() {
  const input = $('#addr-input');
  const a = activeApp();
  if (!input || !a || document.activeElement === input) return;
  input.value = currentUrl(a);
  input.classList.remove('invalid');
  input.title = ADDR_TITLE;
}

/** Paths visited in an app (most recent first), offered as suggestions in the address bar. */
function urlHistory(id) {
  try { return JSON.parse(readPref(`urlHistory:${id}`)) || []; } catch { return []; }
}

function rememberUrl(id, url) {
  let path;
  try { const u = new URL(url); path = u.pathname + u.search + u.hash; } catch { return; }
  const list = [path, ...urlHistory(id).filter((p) => p !== path)].slice(0, HISTORY_MAX);
  writePref(`urlHistory:${id}`, JSON.stringify(list));
}

function onUrl({ id, url }) {
  currentUrls[id] = url;
  rememberUrl(id, url);
  if (id === state.active) renderAddress();
}

async function navigateTo(target) {
  const a = activeApp();
  const input = $('#addr-input');
  if (!a || !input || !target.trim()) return renderAddress();
  try {
    await invoke('navigate_app', { id: a.id, target });
    input.blur();
  } catch (err) {
    input.classList.add('invalid');
    input.title = String(err);
  }
}

async function copyUrl() {
  const a = activeApp();
  if (!a) return;
  const url = currentUrl(a);
  try {
    await navigator.clipboard.writeText(url);
  } catch {
    const t = Object.assign(document.createElement('textarea'), { value: url });
    document.body.appendChild(t);
    t.select();
    document.execCommand('copy');
    t.remove();
  }
  const btn = $('#copy-url');
  if (!btn) return;
  btn.innerHTML = icon('check');
  btn.classList.add('done');
  clearTimeout(copyUrl.timer);
  copyUrl.timer = setTimeout(() => {
    const b = $('#copy-url');
    if (b) { b.innerHTML = icon('copy'); b.classList.remove('done'); }
  }, 1200);
}

$('#toolbar').addEventListener('focusin', (e) => {
  if (e.target.id !== 'addr-input') return;
  const a = activeApp();
  $('#addr-history').innerHTML = a ? urlHistory(a.id).map((p) => `<option value="${esc(p)}">`).join('') : '';
  e.target.select();
});
$('#toolbar').addEventListener('focusout', (e) => {
  if (e.target.id === 'addr-input') setTimeout(renderAddress);
});
$('#toolbar').addEventListener('input', (e) => {
  if (e.target.id === 'addr-input') e.target.classList.remove('invalid');
});
$('#toolbar').addEventListener('keydown', (e) => {
  if (e.target.id !== 'addr-input') return;
  if (e.key === 'Enter') { e.preventDefault(); if (!e.target.readOnly) navigateTo(e.target.value); }
  else if (e.key === 'Escape') { e.preventDefault(); e.target.blur(); }
});

function renderStats() {
  const el = $('#stats');
  if (!el) return;
  const a = activeApp();
  const s = a && isBusy(a) ? stats[a.id] : null;
  el.hidden = !s;
  if (!s) return;
  el.innerHTML = statsHtml(s);
  el.title = statsTitle(s, a.port);
}

let stageKey = '';
function renderStage() {
  const vp = $('#viewport');
  const a = activeApp();
  const key = a ? `${a.id}|${a.status}|${a.error}|${a.command}|${a.path}|${a.port}` : `none|${state.apps.length}`;
  if (key === stageKey) return;
  stageKey = key;

  if (!a) {
    vp.innerHTML = `
      <div class="placeholder">
        <div class="big-icon">${icon('window')}</div>
        <h2>${state.apps.length ? 'Sélectionnez une application' : 'Aucune application'}</h2>
        <p>Ajoutez vos mini-apps web locales : dossier, commande de lancement et port. App Viewer les démarre et les affiche ici.</p>
        <div class="buttons"><button class="btn primary lg" data-act="add">${icon('plus')} Ajouter une application</button></div>
      </div>`;
    return;
  }

  if (isNative(a) && a.status === 'running') {
    vp.innerHTML = `
      <div class="placeholder">
        <div class="spinner"></div>
        <h2>Ouverture de ${esc(a.name)}…</h2>
        <p>La fenêtre de l'app s'affichera ici dès qu'elle apparaît.</p>
        <div class="buttons"><button class="btn" data-act="logs-open">${icon('terminal')} Voir les logs</button></div>
      </div>`;
    return;
  }

  switch (a.status) {
    case 'running':
    case 'external':
      vp.innerHTML = `<div class="placeholder"><div class="spinner"></div><p>Chargement de ${esc(a.url)}…</p></div>`;
      break;
    case 'starting':
      vp.innerHTML = `
        <div class="placeholder">
          <div class="spinner"></div>
          <h2>Démarrage de ${esc(a.name)}…</h2>
          <p>En attente du port ${a.port}</p>
          <div class="tail" id="tail"></div>
        </div>`;
      renderTail();
      break;
    case 'error':
      vp.innerHTML = `
        <div class="placeholder error">
          <div class="big-icon">${icon('alert')}</div>
          <h2>${esc(a.name)} ne tourne pas</h2>
          <p>${esc(a.error || 'Erreur inconnue')}</p>
          <div class="tail" id="tail"></div>
          <div class="buttons">
            <button class="btn primary" data-act="start">${icon('play')} Relancer</button>
            <button class="btn" data-act="logs-open">${icon('terminal')} Voir les logs</button>
            <button class="btn" data-act="edit">${icon('edit')} Modifier</button>
          </div>
        </div>`;
      renderTail();
      break;
    default:
      vp.innerHTML = `
        <div class="placeholder">
          <div class="big-icon">${icon('window')}</div>
          <h2>${esc(a.name)}</h2>
          <div class="cmd">${esc(a.path)}&gt; ${esc(a.command)}</div>
          <p>${isNative(a) ? 'App native : sa fenêtre s\'affichera ici.' : `L'application est arrêtée. Elle sera affichée sur le port ${a.port}.`}</p>
          <div class="buttons"><button class="btn primary lg" data-act="start">${icon('play')} Démarrer</button></div>
        </div>`;
  }
}

function renderTail() {
  const tail = $('#tail');
  if (!tail) return;
  const buf = logs.get(state.active) || [];
  tail.replaceChildren(...buf.slice(-12).map(lineEl));
  tail.hidden = !buf.length;
}

// ---------------------------------------------------------------- settings
function renderSettings() {
  $('.main').classList.toggle('show-page', pageOpen());
  $('#settings').hidden = !settingsOpen;
  $('#settings-btn').classList.toggle('on', settingsOpen);
  if (!settingsOpen) return;
  for (const input of document.querySelectorAll('[data-setting]')) {
    const v = state.settings[input.dataset.setting];
    if (input.type === 'checkbox') input.checked = !!v;
    else if (document.activeElement !== input) input.value = v ?? '';
  }
}

function toggleSettings(open = !settingsOpen) {
  settingsOpen = open;
  if (open) downloadsOpen = false;
  render();
}

// ---------------------------------------------------------------- take over an external process
// First click names the process that will be ended, second click confirms.
const TAKEOVER_LABEL = 'Reprendre la main';
let takeoverTimer = 0;

function disarmTakeover() {
  clearTimeout(takeoverTimer);
  const btn = $('#takeover-btn');
  if (!btn) return;
  btn.classList.remove('armed');
  btn.disabled = false;
  btn.textContent = TAKEOVER_LABEL;
}

async function takeOver() {
  const a = activeApp();
  const btn = $('#takeover-btn');
  if (!a || !btn) return;
  if (!btn.classList.contains('armed')) {
    const info = await invoke('external_process', { id: a.id }).catch(() => null);
    const who = info ? `${info.name || 'process inconnu'} (PID ${info.pid})` : 'le process';
    btn.classList.add('armed');
    btn.dataset.pid = info ? info.pid : '';
    btn.textContent = `Arrêter ${who} ?`;
    clearTimeout(takeoverTimer);
    takeoverTimer = setTimeout(disarmTakeover, 6000);
    return;
  }
  clearTimeout(takeoverTimer);
  btn.disabled = true;
  btn.textContent = 'Arrêt en cours…';
  try {
    await invoke('take_over_app', { id: a.id, expectedPid: btn.dataset.pid ? Number(btn.dataset.pid) : null });
  } catch (err) {
    btn.classList.remove('armed');
    btn.disabled = false;
    btn.textContent = String(err);
    takeoverTimer = setTimeout(disarmTakeover, 6000);
  }
}

// ---------------------------------------------------------------- downloads
const DL_STATE = { downloading: 'En cours', done: 'Terminé', failed: 'Échec' };

function formatBytes(n) {
  if (n == null) return '';
  if (n < 1024) return `${n} o`;
  if (n < 1048576) return `${fr0.format(n / 1024)} Ko`;
  if (n < 1073741824) return `${fr1.format(n / 1048576)} Mo`;
  return `${fr1.format(n / 1073741824)} Go`;
}

const dlTime = (ms) => new Date(ms).toLocaleString('fr-FR', { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' });

function renderDownloads() {
  const running = downloads.filter((d) => d.status === 'downloading').length;
  const btn = $('#downloads-btn');
  btn.classList.toggle('on', downloadsOpen);
  btn.innerHTML = icon('download') + (running ? `<span class="badge-count">${running}</span>` : '');
  $('#downloads').hidden = !downloadsOpen;
  if (!downloadsOpen) return;
  $('#downloads-clear').disabled = !downloads.some((d) => d.status !== 'downloading');
  $('#dl-list').innerHTML = downloads.map((d) => {
    const sub = [d.appName, dlTime(d.startedAt), d.status === 'done' ? formatBytes(d.size) : ''].filter(Boolean).join(' · ');
    return `
    <div class="dl-item ${d.status}" data-id="${esc(d.id)}">
      <span class="dl-ico">${icon(d.status === 'done' ? 'file' : 'download')}</span>
      <span class="dl-meta">
        <span class="dl-name" title="${esc(d.path)}">${esc(d.fileName)}</span>
        <span class="dl-sub" title="${esc(d.url)}">${esc(sub)}</span>
        ${d.status === 'downloading' ? '<span class="dl-bar"></span>' : ''}
      </span>
      <span class="dl-state">${DL_STATE[d.status]}</span>
      <span class="dl-actions">
        <button class="icon-btn sm" data-dl="open" title="Ouvrir le fichier" ${d.status === 'done' ? '' : 'disabled'}>${icon('external')}</button>
        <button class="icon-btn sm" data-dl="reveal" title="Afficher dans le dossier" ${d.status === 'downloading' ? 'disabled' : ''}>${icon('folder')}</button>
        <button class="icon-btn sm" data-dl="remove" title="Retirer de la liste" ${d.status === 'downloading' ? 'disabled' : ''}>${icon('x')}</button>
      </span>
    </div>`;
  }).join('') || '<div class="dl-empty">Aucun téléchargement pour le moment.</div>';
}

function toggleDownloads(open = !downloadsOpen) {
  downloadsOpen = open;
  if (open) settingsOpen = false;
  render();
}

$('#dl-list').addEventListener('click', (e) => {
  const btn = e.target.closest('[data-dl]');
  if (!btn || btn.disabled) return;
  const id = btn.closest('.dl-item').dataset.id;
  const cmd = { open: 'open_download', reveal: 'reveal_download', remove: 'remove_download' }[btn.dataset.dl];
  invoke(cmd, { id }).catch((err) => console.error(err));
});

function saveSetting(key, value) {
  const settings = { ...state.settings, [key]: value };
  state.settings = settings; // optimistic; the backend echoes the state back
  invoke('save_settings', { settings }).catch((err) => console.error(err));
}

document.querySelectorAll('[data-setting]').forEach((input) => {
  input.addEventListener('change', () => {
    const key = input.dataset.setting;
    if (input.type === 'checkbox') return saveSetting(key, input.checked);
    const n = clamp(Math.round(Number(input.value) || 0), 0, 60);
    input.value = n;
    saveSetting(key, n);
  });
});
$('#settings-back').addEventListener('click', () => toggleSettings(false));

// ---------------------------------------------------------------- actions
async function act(name, id = state.active) {
  const a = state.apps.find((x) => x.id === id);
  switch (name) {
    case 'add': return openEditor(null);
    case 'edit': return a && openEditor(a);
    case 'logs': return toggleLogs();
    case 'logs-open': return toggleLogs(true);
    case 'sidebar': return toggleSidebar();
    case 'start': return invoke('start_app', { id }).catch(() => {});
    case 'stop': return invoke('stop_app', { id });
    case 'restart': return invoke('restart_app', { id }).catch(() => {});
    case 'reload': return invoke('reload_app', { id });
    case 'home': return invoke('home_app', { id });
    case 'browser': return invoke('open_in_browser', { id });
    case 'folder': return invoke('open_folder', { id });
    case 'devtools': return invoke('open_devtools', { id });
    case 'copy-url': return copyUrl();
    case 'takeover': return takeOver();
    case 'agent': return a && invoke('set_agent_control', { id: a.id, enabled: !isAgentOn(a.id) });
    case 'zoom-in': return stepZoom(1);
    case 'zoom-out': return stepZoom(-1);
    case 'zoom-reset': return a && invoke('set_zoom', { id: a.id, factor: 1 }).catch(() => {});
  }
}

function select(id) {
  settingsOpen = false;
  downloadsOpen = false;
  if (id === state.active) return render();
  state.active = id; // optimistic; the backend echoes the state back
  render();
  invoke('select_app', { id });
}

document.addEventListener('click', (e) => {
  const quick = e.target.closest('[data-quick]');
  if (quick) {
    e.stopPropagation();
    if (!quick.disabled) act(quick.dataset.quick, quick.closest('.app-item').dataset.id);
    return;
  }
  const item = e.target.closest('.app-item');
  if (item) return select(item.dataset.id);
  const btn = e.target.closest('[data-act]');
  if (btn && !btn.disabled) act(btn.dataset.act);
});

document.addEventListener('keydown', (e) => {
  if (modalOpen) return;
  if (e.key === 'Escape' && pageOpen()) { e.preventDefault(); settingsOpen = downloadsOpen = false; return render(); }
  if (e.key === 'F6' && $('#addr-input')) { e.preventDefault(); return $('#addr-input').focus(); }
  if (e.ctrlKey && !e.shiftKey && !e.altKey) {
    const k = e.key.toLowerCase();
    if (k === 'n') { e.preventDefault(); act('add'); }
    else if (k === 'l') { e.preventDefault(); toggleLogs(); }
    else if (k === 'r') { e.preventDefault(); act('reload'); }
    else if (k === 'b') { e.preventDefault(); toggleSidebar(); }
    else if (k === ',') { e.preventDefault(); toggleSettings(); }
    else if (k === 'j') { e.preventDefault(); toggleDownloads(); }
    else if (k === '=' || k === '+') { e.preventDefault(); stepZoom(1); }
    else if (k === '-') { e.preventDefault(); stepZoom(-1); }
    else if (k === '0') { e.preventDefault(); act('zoom-reset'); }
    else if (/^[1-9]$/.test(k)) {
      const a = state.apps[Number(k) - 1];
      if (a) { e.preventDefault(); select(a.id); }
    }
  }
});

// ---------------------------------------------------------------- editor
const dialog = $('#editor');
const form = $('#editor-form');
let editing = null;

function setModal(open) {
  modalOpen = open;
  reportViewport();
}

function openEditor(app) {
  editing = app;
  form.reset();
  $('#editor-title').textContent = app ? `Modifier ${app.name}` : 'Nouvelle application';
  $('#delete-btn').hidden = !app;
  $('#delete-btn').classList.remove('armed');
  $('#delete-btn').textContent = 'Supprimer';
  $('#form-error').hidden = true;
  if (app) {
    form.path.value = app.path;
    form.name.value = app.name;
    form.command.value = app.command;
    form.port.value = app.port;
    form.urlPath.value = app.urlPath === '/' ? '' : app.urlPath;
    form.autostart.checked = app.autostart;
    form.setPortEnv.checked = app.setPortEnv;
    form.native.checked = isNative(app);
  }
  syncKind();
  setModal(true);
  dialog.showModal();
  (app ? form.name : form.path).focus();
}

dialog.addEventListener('close', () => setModal(false));
$('#cancel-btn').addEventListener('click', () => dialog.close());

function showError(msg) {
  const el = $('#form-error');
  el.textContent = msg;
  el.hidden = false;
}

async function suggestFromFolder(path) {
  if (!path) return;
  const info = await invoke('inspect_folder', { path });
  if (!form.name.value && info.name) form.name.value = info.name;
  if (!form.command.value && info.command) form.command.value = info.command;
  if (!form.port.value && info.port) form.port.value = info.port;
  folderInfo = info;
  if (form.native.checked && info.nativeCommand && !form.command.value) form.command.value = info.nativeCommand;
}

let folderInfo = null;
// Port and PORT only mean something for a web app.
function syncKind() {
  const native = form.native.checked;
  form.querySelectorAll('.web-only').forEach((el) => { el.hidden = native; });
  form.command.placeholder = native ? 'dist\\mon-app.exe  ou  npm run dev' : 'node server.js';
}
form.native.addEventListener('change', () => {
  syncKind();
  // Swap the guessed command for the native one when the user hasn't typed their own.
  if (form.native.checked && folderInfo?.nativeCommand && (!form.command.value || form.command.value === folderInfo.command)) {
    form.command.value = folderInfo.nativeCommand;
  }
});

$('#browse-btn').addEventListener('click', async () => {
  const path = await invoke('pick_folder');
  if (!path) return;
  form.path.value = path;
  suggestFromFolder(path);
});
form.path.addEventListener('change', () => suggestFromFolder(form.path.value.trim()));

form.addEventListener('submit', async (e) => {
  e.preventDefault();
  const native = form.native.checked;
  const port = native ? 0 : Number(form.port.value);
  if (!form.path.value.trim()) return showError('Indiquez le dossier de l\'application.');
  if (!form.name.value.trim()) return showError('Indiquez un nom.');
  if (!form.command.value.trim()) return showError('Indiquez la commande de lancement.');
  if (!native && (!Number.isInteger(port) || port < 1 || port > 65535)) return showError('Port invalide (1 – 65535).');
  const config = {
    id: editing?.id || '',
    name: form.name.value,
    path: form.path.value,
    command: form.command.value,
    port,
    urlPath: form.urlPath.value.trim() || '/',
    autostart: form.autostart.checked,
    setPortEnv: form.setPortEnv.checked,
    kind: native ? 'native' : 'web',
  };
  try {
    const saved = await invoke('save_app', { config });
    state.active = saved.id;
    settingsOpen = downloadsOpen = false;
    dialog.close();
  } catch (err) {
    showError(String(err));
  }
});

$('#delete-btn').addEventListener('click', async (e) => {
  const btn = e.currentTarget;
  if (!btn.classList.contains('armed')) {
    btn.classList.add('armed');
    btn.textContent = 'Confirmer la suppression';
    return;
  }
  const id = editing.id;
  dialog.close();
  logs.delete(id);
  await invoke('delete_app', { id });
});

// ---------------------------------------------------------------- window controls
const appWindow = window.__TAURI__.window.getCurrentWindow();
const winControls = $('#win-controls');
let winHideTimer = 0;

function showWinControls() {
  clearTimeout(winHideTimer);
  winControls.classList.add('show');
}
function hideWinControlsSoon() {
  clearTimeout(winHideTimer);
  winHideTimer = setTimeout(() => winControls.classList.remove('show'), 350);
}
$('#win-hot').addEventListener('mouseenter', showWinControls);
winControls.addEventListener('mouseenter', showWinControls);
winControls.addEventListener('mouseleave', hideWinControlsSoon);
$('#win-hot').addEventListener('mouseleave', hideWinControlsSoon);

winControls.addEventListener('click', (e) => {
  const btn = e.target.closest('[data-win]');
  if (!btn) return;
  winControls.classList.remove('show');
  if (btn.dataset.win === 'minimize') appWindow.minimize();
  else if (btn.dataset.win === 'maximize') appWindow.toggleMaximize();
  else appWindow.close(); // the backend hides to the tray or quits, depending on the settings
});

async function syncMaximizeGlyph() {
  const max = await appWindow.isMaximized();
  const btn = $('#win-max');
  btn.innerHTML = max ? '&#xE923;' : '&#xE922;';
  btn.title = max ? 'Restaurer' : 'Agrandir';
}
appWindow.onResized(syncMaximizeGlyph);
syncMaximizeGlyph();

// ---------------------------------------------------------------- boot
$('#add-btn').innerHTML = icon('plus');
$('#hide-btn').innerHTML = icon('sidebarClose');
$('#pin-btn').innerHTML = icon('pin');
$('#settings-btn').innerHTML = `${icon('settings')}<span>Réglages</span>`;
$('#downloads-back').innerHTML = icon('back');
$('#downloads-clear').innerHTML = `${icon('trash')} Vider la liste`;
$('#settings-back').innerHTML = icon('back');
$('#security-icon').innerHTML = icon('shield');
$('#agent-icon').innerHTML = icon('bot');
$('#logs-detach').innerHTML = icon('popout');
$('#logs-clear').innerHTML = icon('trash');
$('#logs-close').innerHTML = icon('x');

$('#add-btn').addEventListener('click', () => act('add'));
$('#hide-btn').addEventListener('click', () => toggleSidebar(true));
$('#pin-btn').addEventListener('click', () => toggleSidebar(false));
$('#settings-btn').addEventListener('click', () => toggleSettings());
$('#downloads-btn').addEventListener('click', () => toggleDownloads());
$('#downloads-back').addEventListener('click', () => toggleDownloads(false));
$('#downloads-clear').addEventListener('click', () => invoke('clear_downloads'));
$('#logs-close').addEventListener('click', () => toggleLogs(false));
$('#logs-detach').addEventListener('click', () => {
  if (state.active && !$('#logs-detach').disabled) invoke('detach_logs', { id: state.active });
});
$('#logs-clear').addEventListener('click', () => {
  if (!state.active) return;
  invoke('clear_logs', { id: state.active });
  logs.set(state.active, []);
  logsKey = '';
  renderLogs();
});

applyLayout();
// No slide animation for the layout restored at launch.
requestAnimationFrame(() => requestAnimationFrame(() => $('#shell').classList.remove('boot')));
new ResizeObserver(reportViewport).observe($('#viewport'));
window.addEventListener('resize', reportViewport);

(async () => {
  await listen('state', (e) => { state = e.payload; render(); });
  await listen('app-log', (e) => onLog(e.payload));
  await listen('app-stats', (e) => { stats = e.payload; renderStats(); });
  await listen('app-url', (e) => onUrl(e.payload));
  await listen('app-zoom', (e) => onZoom(e.payload));
  // The backend (screenshots, agent actions) needs this app on screen: close any page over it.
  await listen('show-app', (e) => { if (state.apps.some((x) => x.id === e.payload)) select(e.payload); });
  await listen('agent-activity', (e) => { agentSeen[e.payload.id] = Date.now(); renderAgent(); });
  await listen('downloads', (e) => { downloads = e.payload; renderDownloads(); });
  // "Rattacher" in a detached terminal: show that app with its docked panel.
  await listen('terminal-docked', (e) => {
    const id = e.payload;
    if (!state.apps.some((a) => a.id === id)) return;
    state.detached = (state.detached || []).filter((d) => d !== id);
    select(id);
    toggleLogs(true);
  });
  state = await invoke('get_state');
  downloads = await invoke('get_downloads');
  render();
})();
