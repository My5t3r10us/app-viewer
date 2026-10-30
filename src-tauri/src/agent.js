// Injected by App Viewer into the page of an app, for `appv ui ...`. It is a function expression
// that agent.rs calls as (this file)(action, args) and whose result is JSON-serialised.
// The agent's inputs only ever arrive as the `action` / `args` data: there is no eval of
// agent-supplied code here, just a fixed set of actions on elements designated by reference.
(function (action, args) {
  'use strict';
  const MAX_ITEMS = 400;
  const MAX_TEXT = 160;
  const MAX_PAGE_TEXT = 40000;
  // References (e1, e2...) live until the next snapshot, which replaces them.
  const state = (window.__avAgent = window.__avAgent || { refs: new Map(), n: 0 });

  const clip = (s, n = MAX_TEXT) => {
    s = String(s == null ? '' : s).replace(/\s+/g, ' ').trim();
    return s.length > n ? s.slice(0, n - 1) + '…' : s;
  };
  const q = (s) => JSON.stringify(s);

  const INTERACTIVE = [
    'a[href]', 'button', 'input:not([type=hidden])', 'select', 'textarea', 'summary',
    '[contenteditable=""]', '[contenteditable="true"]', '[onclick]', '[tabindex]:not([tabindex="-1"])',
    '[role=button]', '[role=link]', '[role=checkbox]', '[role=radio]', '[role=tab]', '[role=menuitem]',
    '[role=switch]', '[role=option]', '[role=combobox]', '[role=textbox]', '[role=slider]',
  ].join(',');
  const SKIP = new Set(['script', 'style', 'noscript', 'template', 'head', 'svg', 'canvas']);
  const LEAVES = new Set(['a', 'button', 'input', 'select', 'textarea', 'summary']);

  function roleOf(el) {
    const r = el.getAttribute('role');
    if (r) return r;
    const t = el.tagName.toLowerCase();
    if (t === 'a') return 'link';
    if (t === 'button' || t === 'summary') return 'button';
    if (t === 'select') return 'combobox';
    if (t === 'textarea') return 'textbox';
    if (t === 'input') {
      const ty = (el.type || 'text').toLowerCase();
      if (['button', 'submit', 'reset', 'image'].includes(ty)) return 'button';
      if (['checkbox', 'radio', 'range', 'file'].includes(ty)) return ty === 'range' ? 'slider' : ty;
      return 'textbox';
    }
    if (el.isContentEditable) return 'textbox';
    return 'generic';
  }

  function nameOf(el) {
    const aria = el.getAttribute('aria-label');
    if (aria) return clip(aria);
    const by = el.getAttribute('aria-labelledby');
    if (by) {
      const t = by.split(/\s+/).map((id) => (document.getElementById(id) || {}).textContent || '').join(' ');
      if (clip(t)) return clip(t);
    }
    if (el.labels && el.labels.length) return clip([...el.labels].map((l) => l.textContent).join(' '));
    const t = el.tagName.toLowerCase();
    if (t === 'input') {
      const ty = (el.type || '').toLowerCase();
      if (['button', 'submit', 'reset'].includes(ty)) return clip(el.value);
      if (ty === 'image') return clip(el.alt);
      return clip(el.placeholder || el.title || el.name);
    }
    if (t === 'textarea' || t === 'select') return clip(el.placeholder || el.title || el.name);
    return clip(el.innerText || el.textContent || el.title || el.getAttribute('alt') || '');
  }

  function describe(el) {
    return `${roleOf(el)} ${q(nameOf(el))}`;
  }

  function stateOf(el) {
    const bits = [];
    const t = el.tagName.toLowerCase();
    if (t === 'input' || t === 'textarea') {
      const ty = (el.type || 'text').toLowerCase();
      if (ty === 'password') bits.push(`type=password value=(${el.value.length} car., masqué)`);
      else if (ty === 'checkbox' || ty === 'radio') { if (el.checked) bits.push('checked'); }
      else if (!['button', 'submit', 'reset', 'image', 'file'].includes(ty)) bits.push(`value=${q(clip(el.value, 80))}`);
    } else if (t === 'select') {
      const sel = el.selectedOptions && el.selectedOptions[0];
      bits.push(`value=${q(sel ? clip(sel.textContent, 60) : '')}`);
      bits.push(`options=${q([...el.options].slice(0, 12).map((o) => clip(o.textContent, 40)))}`);
    } else if (t === 'a') {
      const href = el.getAttribute('href');
      if (href) bits.push(`→ ${clip(href, 120)}`);
    }
    const aria = el.getAttribute('aria-checked') || el.getAttribute('aria-selected') || el.getAttribute('aria-pressed');
    if (aria === 'true') bits.push('on');
    if (el.getAttribute('aria-expanded')) bits.push(`expanded=${el.getAttribute('aria-expanded')}`);
    if (el.disabled || el.getAttribute('aria-disabled') === 'true') bits.push('disabled');
    return bits.join(' ');
  }

  function snapshot() {
    state.refs = new Map();
    state.n = 0;
    const lines = [];
    let truncated = false;

    const walk = (root) => {
      for (const el of root.children) {
        if (truncated) return;
        const tag = el.tagName.toLowerCase();
        if (SKIP.has(tag)) continue;
        const cs = getComputedStyle(el);
        if (cs.display === 'none') continue;
        const rendered = el.getClientRects().length > 0 && cs.visibility !== 'hidden';
        if (rendered && el.matches(INTERACTIVE)) {
          if (lines.length >= MAX_ITEMS) { truncated = true; return; }
          const ref = `e${++state.n}`;
          state.refs.set(ref, new WeakRef(el));
          lines.push(`[${ref}] ${roleOf(el)} ${q(nameOf(el))} ${stateOf(el)}`.trimEnd());
          if (LEAVES.has(tag)) continue; // its content is its name
        } else if (rendered) {
          const own = clip([...el.childNodes].filter((n) => n.nodeType === 3).map((n) => n.textContent).join(' '));
          if (own) {
            if (lines.length >= MAX_ITEMS) { truncated = true; return; }
            const h = /^h([1-6])$/.exec(tag);
            lines.push(h ? `heading(${h[1]}) ${q(own)}` : `text ${q(own)}`);
          }
        }
        if (el.shadowRoot) walk(el.shadowRoot);
        walk(el);
      }
    };
    walk(document.body || document.documentElement);

    const max = Math.max(0, document.documentElement.scrollHeight - innerHeight);
    const head = [
      `URL : ${location.href}`,
      `Titre : ${document.title}`,
      `Défilement : ${Math.round(scrollY)} / ${Math.round(max)} px`,
    ];
    if (document.querySelector('iframe')) head.push('Note : les iframes ne sont pas parcourues.');
    if (truncated) head.push(`Note : liste tronquée à ${MAX_ITEMS} lignes (utilisez scroll puis un nouveau snapshot).`);
    return { text: head.concat('', lines).join('\n'), refs: state.n, truncated };
  }

  function deref(ref) {
    const w = state.refs.get(String(ref));
    const el = w && w.deref();
    if (!el) throw new Error(`référence ${ref} inconnue : faites d'abord un snapshot`);
    if (!el.isConnected) throw new Error(`l'élément ${ref} n'est plus dans la page : refaites un snapshot`);
    return el;
  }

  function mouseOpts(el) {
    const r = el.getBoundingClientRect();
    return { bubbles: true, cancelable: true, view: window, button: 0, clientX: r.left + r.width / 2, clientY: r.top + r.height / 2 };
  }

  function click(ref) {
    const el = deref(ref);
    if (el.disabled || el.getAttribute('aria-disabled') === 'true') throw new Error(`${ref} est désactivé`);
    el.scrollIntoView({ block: 'center', inline: 'center' });
    const o = mouseOpts(el);
    // Something else on top of it would swallow a real click: say so.
    const top = document.elementFromPoint(o.clientX, o.clientY);
    const covered = top && top !== el && !el.contains(top) && !top.contains(el) ? clip(`${top.tagName.toLowerCase()} ${top.id || top.className || ''}`, 60) : null;
    el.dispatchEvent(new PointerEvent('pointerdown', o));
    el.dispatchEvent(new MouseEvent('mousedown', o));
    if (el.focus) el.focus();
    el.dispatchEvent(new PointerEvent('pointerup', o));
    el.dispatchEvent(new MouseEvent('mouseup', o));
    el.click();
    return { desc: describe(el), covered };
  }

  // Where to click (centre of the element, viewport CSS pixels) so the backend can send a real,
  // trusted mouse event; scrolls the element into view first.
  function locate(ref) {
    const el = deref(ref);
    if (el.disabled || el.getAttribute('aria-disabled') === 'true') throw new Error(`${ref} est désactivé`);
    el.scrollIntoView({ block: 'center', inline: 'center' });
    const r = el.getBoundingClientRect();
    const x = r.left + r.width / 2, y = r.top + r.height / 2;
    const top = document.elementFromPoint(x, y);
    const covered = top && top !== el && !el.contains(top) && !top.contains(el) ? clip(`${top.tagName.toLowerCase()} ${top.id || top.className || ''}`, 60) : null;
    return { x, y, desc: describe(el), covered };
  }

  // Focuses a text field with its content selected (or the caret at the end), ready for real typing.
  function focusField(ref, append) {
    const el = deref(ref);
    const t = el.tagName.toLowerCase();
    const textual = t === 'textarea' || (t === 'input' && !['checkbox', 'radio', 'button', 'submit', 'reset', 'file', 'image', 'range', 'color'].includes((el.type || 'text').toLowerCase()));
    if (!textual && !el.isContentEditable) throw new Error(`${ref} n'est pas un champ de saisie`);
    el.scrollIntoView({ block: 'center' });
    el.focus();
    try {
      if (t === 'input' || t === 'textarea') {
        if (append) { const n = el.value.length; el.setSelectionRange(n, n); } else el.select();
      } else {
        const range = document.createRange();
        range.selectNodeContents(el);
        if (append) range.collapse(false);
        const sel = getSelection();
        sel.removeAllRanges();
        sel.addRange(range);
      }
    } catch (e) { /* types without a selection API (email, number): the focus is enough */ }
    return { desc: describe(el), secret: el.type === 'password', hasText: !!(el.value || el.textContent) };
  }

  function key(el, type, spec) {
    return el.dispatchEvent(new KeyboardEvent(type, spec));
  }

  function pressEnter(el) {
    const spec = { key: 'Enter', code: 'Enter', keyCode: 13, which: 13, bubbles: true, cancelable: true };
    const go = key(el, 'keydown', spec);
    key(el, 'keypress', spec);
    key(el, 'keyup', spec);
    // Synthetic keys don't trigger the browser's default action: do it unless the page handled the key.
    if (go) {
      if (el.form && /^(input|select)$/i.test(el.tagName)) el.form.requestSubmit();
      else if (/^(button|a|summary)$/i.test(el.tagName)) el.click();
    }
  }

  function typeInto(ref, text, append, submit) {
    const el = deref(ref);
    const t = el.tagName.toLowerCase();
    el.scrollIntoView({ block: 'center' });
    if (el.focus) el.focus();
    if (t === 'input' || t === 'textarea') {
      const ty = (el.type || 'text').toLowerCase();
      if (['checkbox', 'radio', 'button', 'submit', 'reset', 'file', 'image', 'range', 'color'].includes(ty)) {
        throw new Error(`${ref} n'est pas un champ de texte (${ty})`);
      }
      // The native setter, so frameworks that track the value (React...) see the change.
      const proto = t === 'textarea' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, append ? el.value + text : text);
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
    } else if (el.isContentEditable) {
      el.textContent = append ? el.textContent + text : text;
      el.dispatchEvent(new Event('input', { bubbles: true }));
    } else {
      throw new Error(`${ref} n'est pas un champ de saisie`);
    }
    if (submit) pressEnter(el);
    return { desc: describe(el), secret: el.type === 'password' };
  }

  function selectOption(ref, wanted) {
    const el = deref(ref);
    if (el.tagName.toLowerCase() !== 'select') throw new Error(`${ref} n'est pas une liste déroulante`);
    const w = String(wanted).trim().toLowerCase();
    const opt = [...el.options].find((o) => o.value.toLowerCase() === w || o.textContent.trim().toLowerCase() === w);
    if (!opt) throw new Error(`option ${q(wanted)} introuvable (options : ${[...el.options].slice(0, 12).map((o) => clip(o.textContent, 30)).join(', ')})`);
    el.value = opt.value;
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    return { desc: describe(el), value: clip(opt.textContent, 60) };
  }

  const KEYS = { Enter: 13, Escape: 27, Tab: 9, ' ': 32, Space: 32, Backspace: 8, Delete: 46, ArrowUp: 38, ArrowDown: 40, ArrowLeft: 37, ArrowRight: 39, Home: 36, End: 35, PageUp: 33, PageDown: 34 };

  function focusables() {
    return [...document.querySelectorAll('a[href],button,input:not([type=hidden]),select,textarea,[tabindex]:not([tabindex="-1"])')]
      .filter((e) => !e.disabled && e.getClientRects().length > 0);
  }

  function press(combo) {
    const parts = String(combo).split('+');
    const name = parts.pop() || '+';
    const mods = parts.map((p) => p.toLowerCase());
    const k = name === 'Space' ? ' ' : name;
    const el = document.activeElement && document.activeElement !== document.body ? document.activeElement : document.body;
    if (k === 'Enter' && !mods.length) { pressEnter(el); return { desc: 'Enter' }; }
    const spec = {
      key: k, code: k.length === 1 ? (/[a-z]/i.test(k) ? `Key${k.toUpperCase()}` : k) : k, keyCode: KEYS[k] || k.toUpperCase().charCodeAt(0),
      ctrlKey: mods.includes('ctrl') || mods.includes('control'), shiftKey: mods.includes('shift'),
      altKey: mods.includes('alt'), metaKey: mods.includes('meta'), bubbles: true, cancelable: true,
    };
    const go = key(el, 'keydown', spec);
    key(el, 'keyup', spec);
    if (go && k === 'Tab') {
      const list = focusables();
      const i = list.indexOf(el);
      const next = list[(i + (spec.shiftKey ? -1 : 1) + list.length) % list.length];
      if (next) next.focus();
    }
    return { desc: combo };
  }

  function scroll(a) {
    if (a.to === 'top') window.scrollTo(0, 0);
    else if (a.to === 'bottom') window.scrollTo(0, document.documentElement.scrollHeight);
    else if (a.to) deref(a.to).scrollIntoView({ block: 'center' });
    else window.scrollBy(0, Number(a.by) || Math.round(innerHeight * 0.8));
    const max = Math.max(0, document.documentElement.scrollHeight - innerHeight);
    return { y: Math.round(scrollY), max: Math.round(max) };
  }

  switch (action) {
    case 'snapshot': return snapshot();
    case 'click': return click(args.ref);
    case 'locate': return locate(args.ref);
    case 'focus-field': return focusField(args.ref, !!args.append);
    case 'type': return typeInto(args.ref, String(args.text == null ? '' : args.text), !!args.append, !!args.submit);
    case 'select': return selectOption(args.ref, args.value);
    case 'press': return press(args.key);
    case 'scroll': return scroll(args);
    case 'text': return { text: clip0() };
    case 'has-text': return { found: ((document.body && document.body.innerText) || '').includes(String(args.text)) };
    case 'info': return { url: location.href, title: document.title, ready: document.readyState };
    default: throw new Error(`action inconnue : ${action}`);
  }

  function clip0() {
    const t = (document.body && document.body.innerText) || '';
    return t.length > MAX_PAGE_TEXT ? t.slice(0, MAX_PAGE_TEXT) + '\n[… tronqué]' : t;
  }
})
