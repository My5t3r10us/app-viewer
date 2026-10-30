// Detached terminal: live logs of one app in its own window, closed automatically when the
// command ends normally (kept open on error so the failure can be read).

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const win = window.__TAURI__.window.getCurrentWindow();
// The backend labels this webview "term-<app id>".
const ID = window.__TAURI__.webview.getCurrentWebview().label.replace(/^term-/, '');

const isActive = (s) => s === 'starting' || s === 'running';
// Even "immediate" closing waits a bit: a restart goes through "stopped" for a split second.
const MIN_CLOSE_DELAY_MS = 700;

let app = null;
let settings = {};
let prevStatus = null;
let buf = [];
let pinned = false;
let countdownTimer = 0;

function renderHead() {
  if (!app) return;
  document.title = `Logs — ${app.name}`;
  const av = $('#term-avatar');
  av.style.setProperty('--h', hue(app.name));
  av.innerHTML = `${esc(initials(app.name))}<span class="dot ${app.status}"></span>`;
  $('#term-name').textContent = app.name;
  const badge = $('#term-badge');
  badge.className = `badge ${app.status}`;
  badge.innerHTML = `<span class="dot ${app.status}"></span>${STATUS_LABEL[app.status]}`;
  $('#term-stop').disabled = !isActive(app.status);
  $('#term-restart').disabled = app.status === 'external';
}

// ---------------------------------------------------------------- banner / auto-close
function hideBanner() {
  clearInterval(countdownTimer);
  countdownTimer = 0;
  $('#term-banner').hidden = true;
}

function showBanner(kind, msg, actions) {
  const banner = $('#term-banner');
  banner.className = `term-banner ${kind}`;
  $('#term-msg').textContent = msg;
  $('#term-msg').title = msg;
  $('#term-banner-actions').innerHTML = actions;
  banner.hidden = false;
}

function startCountdown() {
  hideBanner();
  const secs = Math.max(0, Number(settings.terminalAutocloseSecs ?? 3));
  if (secs === 0) {
    countdownTimer = setTimeout(() => win.close(), MIN_CLOSE_DELAY_MS);
    return;
  }
  let left = secs;
  const update = () => showBanner('ok', `■ Commande terminée — fermeture dans ${left} s`,
    `<button class="btn sm" data-banner="keep">Garder ouvert</button>
     <button class="btn sm" data-banner="close">Fermer</button>`);
  update();
  countdownTimer = setInterval(() => {
    left -= 1;
    if (left <= 0) {
      clearInterval(countdownTimer);
      win.close();
    } else {
      update();
    }
  }, 1000);
}

function showError(message) {
  hideBanner();
  showBanner('err', `✖ ${message || 'La commande s\'est arrêtée en erreur'}`,
    `<button class="btn sm primary" data-banner="start">${icon('play')} Relancer</button>
     <button class="btn sm" data-banner="close">Fermer</button>`);
}

$('#term-banner').addEventListener('click', (e) => {
  const btn = e.target.closest('[data-banner]');
  if (!btn) return;
  switch (btn.dataset.banner) {
    case 'keep': return hideBanner();
    case 'close': return win.close();
    case 'start': return invoke('start_app', { id: ID }).catch(() => {});
  }
});

function onState(snap) {
  settings = snap.settings || {};
  const a = snap.apps.find((x) => x.id === ID);
  if (!a) return win.close(); // app deleted
  app = a;
  renderHead();
  const was = prevStatus;
  prevStatus = a.status;
  if (isActive(a.status)) {
    // (Re)started: cancel any pending close.
    if (!isActive(was)) hideBanner();
    return;
  }
  // Only react to the end of a run we were following.
  if (!isActive(was)) return;
  if (a.status === 'error') showError(a.error);
  else startCountdown();
}

// ---------------------------------------------------------------- logs
function onLog({ id, line }) {
  if (id !== ID) return;
  const box = $('#log-lines');
  if (line.startsWith('▶ ')) {
    buf = [line];
    fillLines(box, buf);
    return;
  }
  buf.push(line);
  if (buf.length > MAX_LOG_LINES) buf.shift();
  appendLine(box, line);
}

// ---------------------------------------------------------------- actions
$('#term-pin').innerHTML = icon('pin');
$('#term-clear').innerHTML = icon('trash');
$('#term-restart').innerHTML = icon('restart');
$('#term-stop').innerHTML = icon('stop');
$('#term-dock').innerHTML = icon('dock');

$('#term-pin').addEventListener('click', async () => {
  pinned = !pinned;
  await win.setAlwaysOnTop(pinned);
  $('#term-pin').classList.toggle('on', pinned);
  $('#term-pin').title = pinned ? 'Ne plus garder au premier plan' : 'Garder au premier plan';
});
$('#term-clear').addEventListener('click', () => {
  invoke('clear_logs', { id: ID });
  buf = [];
  fillLines($('#log-lines'), buf);
});
$('#term-stop').addEventListener('click', () => invoke('stop_app', { id: ID }));
$('#term-restart').addEventListener('click', () => invoke('restart_app', { id: ID }).catch(() => {}));
$('#term-dock').addEventListener('click', () => invoke('dock_logs', { id: ID }));

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && !$('#term-banner').hidden) hideBanner();
});

(async () => {
  await listen('state', (e) => onState(e.payload));
  await listen('app-log', (e) => onLog(e.payload));
  await listen('app-stats', (e) => {
    const s = app && (isActive(app.status) || app.status === 'external') ? e.payload[ID] : null;
    const el = $('#term-stats');
    el.hidden = !s;
    if (!s) return;
    el.innerHTML = statsHtml(s);
    el.title = statsTitle(s, app.port);
  });
  buf = await invoke('get_logs', { id: ID });
  fillLines($('#log-lines'), buf);
  onState(await invoke('get_state'));
})();
