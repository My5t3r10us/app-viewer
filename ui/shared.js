// Helpers shared by the main window (app.js) and the detached terminal (terminal.js).

const $ = (s) => document.querySelector(s);

const ICONS = {
  play: '<polygon points="6 3 20 12 6 21 6 3"/>',
  stop: '<rect x="5" y="5" width="14" height="14" rx="2"/>',
  restart: '<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>',
  reload: '<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>',
  home: '<path d="M3 10.5 12 3l9 7.5"/><path d="M5 9.5V21h14V9.5"/>',
  external: '<path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>',
  folder: '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>',
  code: '<polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/>',
  terminal: '<polyline points="4 17 10 11 4 5"/><line x1="12" x2="20" y1="19" y2="19"/>',
  edit: '<path d="M21.17 6.81a1 1 0 0 0-3.98-3.98L3.84 16.17a2 2 0 0 0-.5.83l-1.32 4.35a.5.5 0 0 0 .62.62l4.35-1.32a2 2 0 0 0 .83-.5z"/>',
  plus: '<path d="M5 12h14"/><path d="M12 5v14"/>',
  minus: '<path d="M5 12h14"/>',
  trash: '<path d="M3 6h18"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>',
  x: '<path d="M18 6 6 18"/><path d="m6 6 12 12"/>',
  alert: '<circle cx="12" cy="12" r="10"/><line x1="12" x2="12" y1="8" y2="12"/><line x1="12" x2="12.01" y1="16" y2="16"/>',
  window: '<rect x="2" y="4" width="20" height="16" rx="2"/><path d="M2 9h20"/><path d="M8 9v11"/>',
  sidebar: '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M9 3v18"/>',
  sidebarClose: '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M9 3v18"/><path d="m16 15-3-3 3-3"/>',
  sidebarOpen: '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M9 3v18"/><path d="m14 9 3 3-3 3"/>',
  pin: '<path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/>',
  settings: '<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>',
  copy: '<rect width="14" height="14" x="8" y="8" rx="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>',
  download: '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" x2="12" y1="15" y2="3"/>',
  file: '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/>',
  bot: '<path d="M12 8V4H8"/><rect width="16" height="12" x="4" y="8" rx="2"/><path d="M2 14h2"/><path d="M20 14h2"/><path d="M15 13v2"/><path d="M9 13v2"/>',
  check: '<path d="M20 6 9 17l-5-5"/>',
  back: '<path d="m12 19-7-7 7-7"/><path d="M19 12H5"/>',
  popout: '<rect x="3" y="7" width="14" height="14" rx="2"/><path d="M14 3h7v7"/><path d="m21 3-9 9"/>',
  dock: '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 15h18"/><path d="m9 9 3 3 3-3"/>',
  cpu: '<rect x="4" y="4" width="16" height="16" rx="2"/><rect x="9" y="9" width="6" height="6" rx="1"/><path d="M15 2v2"/><path d="M15 20v2"/><path d="M2 15h2"/><path d="M2 9h2"/><path d="M20 15h2"/><path d="M20 9h2"/><path d="M9 2v2"/><path d="M9 20v2"/>',
  memory: '<path d="M6 19v-3"/><path d="M10 19v-3"/><path d="M14 19v-3"/><path d="M18 19v-3"/><path d="M8 11V9"/><path d="M16 11V9"/><path d="M12 11V9"/><path d="M2 15h20"/><path d="M2 7a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v1.1a2 2 0 0 0 0 3.837V17a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2v-5.1a2 2 0 0 0 0-3.837Z"/>',
  shield: '<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/>',
};
const icon = (name) => `<svg class="i" viewBox="0 0 24 24" aria-hidden="true">${ICONS[name]}</svg>`;

const STATUS_LABEL = {
  stopped: 'Arrêtée',
  starting: 'Démarrage…',
  running: 'En cours',
  external: 'Externe',
  error: 'Erreur',
};

const MAX_LOG_LINES = 2000;

function readPref(k) { try { return localStorage.getItem(k); } catch { return null; } }
function writePref(k, v) { try { localStorage.setItem(k, v); } catch {} }
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

// ---------------------------------------------------------------- CPU / RAM
const fr1 = new Intl.NumberFormat('fr-FR', { minimumFractionDigits: 1, maximumFractionDigits: 1 });
const fr0 = new Intl.NumberFormat('fr-FR', { maximumFractionDigits: 0 });

function formatCpu(cpu) {
  if (cpu == null) return '—';
  return `${cpu < 10 ? fr1.format(cpu) : fr0.format(cpu)} %`;
}

function formatMemory(bytes) {
  const mo = bytes / 1048576;
  return mo < 1000 ? `${fr0.format(mo)} Mo` : `${fr1.format(mo / 1024)} Go`;
}

/** Inner HTML of the CPU / RAM pill, for app stats sent by the backend ("app-stats"). */
function statsHtml(s) {
  const level = s.cpu == null ? '' : s.cpu >= 85 ? 'err' : s.cpu >= 50 ? 'warn' : '';
  return `
    <span class="stat ${level}">${icon('cpu')}<b>${formatCpu(s.cpu)}</b></span>
    <span class="stat">${icon('memory')}<b>${formatMemory(s.memory)}</b></span>`;
}

function statsTitle(s, port) {
  const cpu = 'CPU en % de la machine (tous les cœurs)';
  if (s.scope === 'port') {
    return `${cpu}, mémoire vive (working set).\nProcessus qui écoute le port ${port} : l'app a été lancée hors d'App Viewer, ses éventuels sous-process ne sont pas comptés.`;
  }
  return `${cpu}, mémoire vive (working set).\n${s.processes} processus : tout l'arbre lancé par App Viewer.`;
}

/** Avatar letters: first letters of the first two words, or the first two letters. */
function initials(name) {
  const words = String(name).trim().split(/[\s\-_.]+/).filter(Boolean);
  if (!words.length) return '?';
  const s = words.length > 1 ? words[0][0] + words[1][0] : words[0].slice(0, 2);
  return s.toUpperCase();
}

/** Stable avatar hue derived from the app name. */
function hue(name) {
  let h = 0;
  for (const c of String(name)) h = (h * 31 + c.codePointAt(0)) >>> 0;
  return h % 360;
}

function lineClass(line) {
  if (/^[▶●■]/.test(line)) return line.startsWith('●') ? 'ok' : 'sys';
  if (line.startsWith('✖') || /\b(error|erreur|exception|EADDRINUSE|failed)\b/i.test(line)) return 'err';
  if (line.startsWith('⚠') || /\bwarn(ing)?\b/i.test(line)) return 'warn';
  return '';
}

function lineEl(line) {
  const div = document.createElement('div');
  div.className = `log-line ${lineClass(line)}`;
  div.textContent = line || ' ';
  return div;
}

/** Appends a line to a log box, keeping it scrolled to the bottom if it already was. */
function appendLine(box, line) {
  const atBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 30;
  box.querySelector('.log-empty')?.remove();
  box.appendChild(lineEl(line));
  while (box.childElementCount > MAX_LOG_LINES) box.firstElementChild.remove();
  if (atBottom) box.scrollTop = box.scrollHeight;
}

function fillLines(box, buf) {
  box.replaceChildren();
  if (!buf || !buf.length) {
    box.innerHTML = '<div class="log-empty">Aucune sortie pour le moment.</div>';
    return;
  }
  const frag = document.createDocumentFragment();
  buf.forEach((l) => frag.appendChild(lineEl(l)));
  box.appendChild(frag);
  box.scrollTop = box.scrollHeight;
}

/**
 * Pointer-drag helper for the resize handles. onMove gets the delta from the press point;
 * pointer capture keeps the events coming even when the cursor crosses a native webview.
 */
function dragHandle(el, { onStart, onMove, onEnd, cursor }) {
  el.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    el.setPointerCapture(e.pointerId);
    const x0 = e.clientX, y0 = e.clientY;
    onStart?.();
    document.body.classList.add('dragging');
    document.body.style.cursor = cursor;
    el.classList.add('active');
    const move = (ev) => onMove(ev.clientX - x0, ev.clientY - y0);
    const up = (ev) => {
      el.removeEventListener('pointermove', move);
      el.removeEventListener('pointerup', up);
      el.removeEventListener('pointercancel', up);
      document.body.classList.remove('dragging');
      document.body.style.cursor = '';
      el.classList.remove('active');
      onEnd?.(ev.clientX - x0, ev.clientY - y0);
    };
    el.addEventListener('pointermove', move);
    el.addEventListener('pointerup', up);
    el.addEventListener('pointercancel', up);
  });
}
