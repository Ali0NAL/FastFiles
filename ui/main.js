// FastFiles arayüzü — derleme adımı gerektirmeyen düz JavaScript.
// Tauri içinde `window.__TAURI__` kullanılır; tarayıcıda açılırsa örnek verilerle önizleme modu çalışır.

const T = window.__TAURI__;
const api = T ? {
  invoke: (cmd, args) => T.core.invoke(cmd, args),
  listen: (ev, cb) => T.event.listen(ev, cb),
  onDragDrop: (cb) => T.webview.getCurrentWebview().onDragDropEvent(cb),
  pick: (opts) => (T.dialog ? T.dialog.open(opts) : T.core.invoke('plugin:dialog|open', { options: opts })),
} : createPreviewApi();

const state = {
  device: null,
  ips: [],
  settings: null,
  peers: [],
  transfers: new Map(),
  incoming: [],
  dragPaths: null,
  // Mesajlaşma: sohbetler cihazın sertifika parmak izine göre tutulur.
  chats: new Map(),      // parmak izi → { fingerprint, name, unread, last }
  history: new Map(),    // parmak izi → mesaj dizisi (açılınca yüklenir)
  openChat: null,        // açık sohbetin parmak izi
  tab: 'transfers',
  avatars: new Map(),    // parmak izi → data URL
  shares: [],            // ortak klasör özetleri
  openShare: null,       // açık ortak klasör kimliği
  shareFiles: [],
  polls: [],             // oylamalar
  openPoll: null,        // açık oylama kimliği
};

// ───────────────────────────── Yardımcılar ─────────────────────────────

const $ = (s, root = document) => root.querySelector(s);
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const nf1 = new Intl.NumberFormat('tr-TR', { maximumFractionDigits: 1 });
const nf0 = new Intl.NumberFormat('tr-TR', { maximumFractionDigits: 0 });

function fmtBytes(n) {
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${(n < 10 && i > 0 ? nf1 : nf0).format(n)} ${u[i]}`;
}
const fmtSpeed = (bps) => `${fmtBytes(bps)}/sn`;
function fmtEta(sec) {
  if (!isFinite(sec) || sec < 0) return '';
  if (sec < 60) return `${Math.max(1, Math.round(sec))} sn`;
  if (sec < 3600) return `${Math.round(sec / 60)} dk`;
  return `${Math.floor(sec / 3600)} sa ${Math.round((sec % 3600) / 60)} dk`;
}
const fmtTime = (ms) => new Date(ms).toLocaleTimeString('tr-TR', { hour: '2-digit', minute: '2-digit' });
function initials(name) {
  const parts = String(name).trim().split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  const s = parts.length > 1 ? parts[0][0] + parts[1][0] : (parts[0] || '?').slice(0, 2);
  return s.toLocaleUpperCase('tr-TR');
}
function hue(id) {
  let h = 2166136261;
  for (const c of String(id)) h = Math.imul(h ^ c.charCodeAt(0), 16777619);
  return (h >>> 0) % 360;
}
const grad = (id) => { const h = hue(id); return `linear-gradient(135deg, hsl(${h} 72% 62%), hsl(${(h + 35) % 360} 70% 50%))`; };
/** Profil resmi varsa resim, yoksa baş harfler (avatar kutusunun içi). */
const face = (fp, name) => {
  const url = state.avatars.get(fp);
  return url ? `<img src="${esc(url)}" alt="" draggable="false">` : esc(initials(name));
};
const fpShort = (fp) => (fp || '').slice(0, 16).toUpperCase().match(/.{4}/g)?.join(' ') ?? '';

const ic = {
  up: '<svg viewBox="0 0 24 24"><path d="M12 19V5m0 0-6 6m6-6 6 6"/></svg>',
  down: '<svg viewBox="0 0 24 24"><path d="M12 5v14m0 0-6-6m6 6 6-6"/></svg>',
  file: '<svg viewBox="0 0 24 24"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/></svg>',
  folder: '<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>',
  x: '<svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6 6 18"/></svg>',
  reveal: '<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v1"/><path d="M3 7v10a2 2 0 0 0 2 2h12l4-8H7l-4 8"/></svg>',
  shield: '<svg viewBox="0 0 24 24"><path d="M12 3 4 6v6c0 5 3.5 8 8 9 4.5-1 8-4 8-9V6z"/><path d="m9 12 2 2 4-4"/></svg>',
  trash: '<svg viewBox="0 0 24 24"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"/></svg>',
  check: '<svg viewBox="0 0 24 24"><path d="m5 12 5 5L20 7"/></svg>',
  alert: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/><path d="M12 8v5M12 16h.01"/></svg>',
  windows: '<svg viewBox="0 0 24 24"><path d="M3 5.5 10.5 4.5v7H3zM12 4.3 21 3v8.5h-9zM3 12.5h7.5v7L3 18.5zM12 12.5h9V21l-9-1.3z" fill="currentColor" stroke="none"/></svg>',
  macos: '<svg viewBox="0 0 24 24"><path d="M16 3c-1 .1-2.2.7-2.9 1.5-.6.7-1.2 1.8-1 2.9 1.1 0 2.2-.6 2.9-1.4.7-.8 1.1-1.9 1-3zM19.5 16.6c-.5 1.2-.8 1.7-1.4 2.7-.9 1.4-2.2 3.1-3.8 3.1-1.4 0-1.8-.9-3.7-.9s-2.3.9-3.7.9c-1.6 0-2.8-1.6-3.7-3C.7 15.6.4 11.2 2 8.8c1.1-1.7 2.9-2.7 4.6-2.7 1.7 0 2.8.9 4.2.9 1.4 0 2.2-.9 4.2-.9 1.5 0 3.1.8 4.2 2.2-3.7 2-3.1 7.3.3 8.3z" fill="currentColor" stroke="none"/></svg>',
  linux: '<svg viewBox="0 0 24 24"><path d="M4 17l6-6-6-6M12 19h8"/></svg>',
  chat: '<svg viewBox="0 0 24 24"><path d="M21 12a8 8 0 0 1-11.6 7.1L4 20l1-4.6A8 8 0 1 1 21 12z"/></svg>',
  ban: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/><path d="m5.6 5.6 12.8 12.8"/></svg>',
  clock: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>',
  checks: '<svg viewBox="0 0 24 24"><path d="m3 12 5 5L18 7M13 16l1 1L22 9"/></svg>',
  clipAttach: '<svg viewBox="0 0 24 24"><path d="M20 11.5 12.4 19a5 5 0 0 1-7.1-7.1l7.8-7.8a3.3 3.3 0 0 1 4.7 4.7l-7.8 7.8a1.7 1.7 0 0 1-2.4-2.4l7.1-7.1"/></svg>',
  pause: '<svg viewBox="0 0 24 24"><path d="M9 6v12M15 6v12"/></svg>',
  play: '<svg viewBox="0 0 24 24"><path d="M8 5.5v13l10.5-6.5z"/></svg>',
  retry: '<svg viewBox="0 0 24 24"><path d="M20 11a8 8 0 1 0-2.3 5.7"/><path d="M20 5v6h-6"/></svg>',
  clip: '<svg viewBox="0 0 24 24"><rect x="6" y="4" width="12" height="17" rx="2"/><path d="M9 4.5V3h6v1.5M9 10h6M9 14h4"/></svg>',
  copy: '<svg viewBox="0 0 24 24"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/></svg>',
  users: '<svg viewBox="0 0 24 24"><circle cx="9" cy="8" r="3.2"/><path d="M3 19c0-3.3 2.7-5.5 6-5.5s6 2.2 6 5.5"/><path d="M16 4.8a3.2 3.2 0 0 1 0 6.4M18 13.8c2 .7 3 2.5 3 5.2"/></svg>',
};
const osIcon = (os) => ic[os === 'macos' ? 'macos' : os === 'linux' ? 'linux' : 'windows'];

// ───────────────────────────── Bildirimler ─────────────────────────────

function toast(msg, { type = 'info', action, onAction, timeout = 4500 } = {}) {
  const el = document.createElement('div');
  el.className = `toast ${type}`;
  el.innerHTML = `${type === 'error' ? ic.alert : type === 'success' ? ic.check : ''}<span>${esc(msg)}</span>${action ? `<button>${esc(action)}</button>` : ''}`;
  if (action) el.querySelector('button').onclick = () => { onAction?.(); dismiss(); };
  $('#toasts').appendChild(el);
  const dismiss = () => { el.classList.add('out'); setTimeout(() => el.remove(), 200); };
  setTimeout(dismiss, timeout);
}

async function call(cmd, args) {
  try {
    return await api.invoke(cmd, args);
  } catch (e) {
    toast(String(e), { type: 'error', timeout: 6000 });
    throw e;
  }
}

// ───────────────────────────── Modallar ─────────────────────────────

let modalKind = null;
let modalOnClose = null;

function openModal(kind, html, { onClose } = {}) {
  closeModal();
  modalKind = kind;
  modalOnClose = onClose || null;
  const root = $('#modal-root');
  root.innerHTML = `<div class="modal" role="dialog" aria-modal="true">${html}</div>`;
  const first = root.querySelector('[autofocus]');
  if (first) setTimeout(() => { first.focus(); first.select?.(); }, 30);
  return root.firstElementChild;
}

function closeModal() {
  const cb = modalOnClose;
  const wasOther = modalKind && modalKind !== 'incoming';
  modalKind = null;
  modalOnClose = null;
  $('#modal-root').innerHTML = '';
  cb?.();
  // Bekleyen gelen istek varsa, kapanan pencereden sonra göster (hemen başka pencere
  // açılmadıysa).
  if (wasOther && state.incoming.length) setTimeout(() => { if (!modalKind) showNextIncoming(); }, 0);
}

$('#modal-root').addEventListener('mousedown', (e) => {
  if (e.target === e.currentTarget && modalKind !== 'incoming' && modalKind !== 'welcome') closeModal();
});
document.addEventListener('keydown', (e) => {
  if (e.key !== 'Escape') return;
  if (modalKind && modalKind !== 'incoming' && modalKind !== 'welcome') closeModal();
  else if (!modalKind && chatOpen()) closeChat();
  else if (!modalKind && shareOpen()) closeShare();
  else if (!modalKind && pollOpen()) closePoll();
});

// ───────────────────────────── Üst çubuk ─────────────────────────────

function renderMe() {
  if (!state.settings) return;
  $('#me-name').textContent = state.settings.deviceName;
  const ip = state.ips[0] || '';
  $('#me-ip').textContent = ip;
  $('#empty-ip').textContent = ip || '—';
  $('#me').title = `Bu bilgisayar · ${state.ips.join(', ')} · port ${state.device?.port ?? ''}`;
  const av = $('#me-avatar');
  const fp = state.device?.fingerprint;
  av.style.background = grad(fp);
  av.innerHTML = face(fp, state.settings.deviceName);
}

// ───────────────────────────── Cihazlar ─────────────────────────────

function renderPeers() {
  const list = state.peers;
  $('#peer-count').textContent = list.length;
  $('#peers').hidden = list.length === 0;
  $('#peers-empty').hidden = list.length > 0;
  $('#drop-hint').hidden = list.length === 0;
  $('#btn-batch').hidden = list.length < 2;

  // Kartları yerinde güncelle: yalnızca değişen kartın içeriği yenilenir,
  // yalnızca yeni gelen cihaz animasyonla girer. Böylece liste hiç yanıp sönmez.
  const container = $('#peers');
  const existing = new Map([...container.children].map((el) => [el.dataset.id, el]));
  list.forEach((p, i) => {
    let el = existing.get(p.id);
    existing.delete(p.id);
    if (!el) {
      el = document.createElement('div');
      el.className = 'peer enter';
      el.dataset.id = p.id;
      setTimeout(() => el.classList.remove('enter'), 400);
    }
    const sig = JSON.stringify([p.name, p.ip, p.os, p.version, p.trusted, p.blocked, p.manualAddr, unreadOf(p.fingerprint), state.avatars.get(p.fingerprint)?.length || 0]);
    if (el.dataset.sig !== sig) {
      el.dataset.sig = sig;
      el.innerHTML = peerInner(p);
    }
    if (container.children[i] !== el) container.insertBefore(el, container.children[i] || null);
  });
  existing.forEach((el) => el.remove());
}

function peerInner(p) {
  return `
      <div class="corner">
        <button class="icon-btn sm ${p.trusted ? 'on' : ''}" data-act="trust"
          title="${p.trusted ? 'Güveni kaldır (her gönderimde sorulsun)' : 'Güvenilir yap (gönderdikleri sormadan kabul edilsin)'}">${ic.shield}</button>
        ${p.manualAddr ? `<button class="icon-btn sm" data-act="forget" title="Listeden kaldır">${ic.trash}</button>` : ''}
      </div>
      <div class="avatar" style="background:${grad(p.fingerprint)}">${face(p.fingerprint, p.name)}<span class="os">${osIcon(p.os)}</span></div>
      <div class="name" title="${esc(p.name)}">${esc(p.name)}</div>
      <div class="addr">${esc(p.ip)}${p.version ? ` · v${esc(p.version)}` : ''}</div>
      <div class="badges">
        ${p.blocked ? `<span class="badge blocked">${ic.ban}Engellendi</span>` : ''}
        ${p.trusted ? `<span class="badge trusted">${ic.shield}Güvenilir</span>` : ''}
        ${p.manualAddr ? '<span class="badge manual">Elle eklendi</span>' : ''}
      </div>
      <div class="actions">
        <button class="btn soft small" data-act="files">${ic.file}Dosya</button>
        <button class="btn soft small" data-act="folder">${ic.folder}Klasör</button>
        <button class="btn soft small sq-btn" data-act="clip" title="Panodakini gönder (Ctrl+V): metin, ekran görüntüsü veya kopyalanan dosyalar">${ic.clip}</button>
        <button class="btn soft small chat-btn" data-act="chat" title="Mesaj gönder">${ic.chat}${unreadOf(p.fingerprint) ? `<span class="dot-badge">${unreadOf(p.fingerprint)}</span>` : ''}</button>
      </div>
      <div class="drop-label">Bırak → ${esc(p.name)}</div>`;
}

$('#peers').addEventListener('click', async (e) => {
  const btn = e.target.closest('[data-act]');
  const card = e.target.closest('.peer');
  if (!btn || !card) return;
  const peer = state.peers.find((p) => p.id === card.dataset.id);
  if (!peer) return;
  switch (btn.dataset.act) {
    case 'files': return pickAndSend(peer, false);
    case 'folder': return pickAndSend(peer, true);
    case 'chat': return openChat(peer.fingerprint, peer.name);
    case 'clip': return sendClipboard([peer.id]);
    case 'trust': {
      const trusted = !peer.trusted;
      state.settings = await call('set_trusted', { fingerprint: peer.fingerprint, name: peer.name, trusted });
      toast(trusted ? `${peer.name} artık güvenilir: gönderdikleri sormadan kabul edilecek` : `${peer.name} için artık onay istenecek`, { type: 'success' });
      return;
    }
    case 'forget':
      await call('forget_manual_peer', { address: peer.manualAddr });
      return;
  }
});

async function pickAndSend(peer, directory) {
  const sel = await api.pick({
    multiple: true,
    directory,
    title: directory ? `${peer.name} cihazına gönderilecek klasörler` : `${peer.name} cihazına gönderilecek dosyalar`,
  });
  if (!sel) return;
  sendPaths(peer, Array.isArray(sel) ? sel : [sel]);
}

async function sendPaths(peer, paths) {
  if (!paths?.length) return;
  try {
    await call('send_paths', { peerIds: [peer.id], paths });
  } catch { /* toast gösterildi */ }
}

// ───────────────────────────── Panoyu gönder ─────────────────────────────

// Pano dosyalarının adında kullanılan yerel tarih-saat, ör. "2026-11-05 14.32.10".
function clipStamp() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}.${p(d.getMinutes())}.${p(d.getSeconds())}`;
}

async function sendClipboard(peerIds) {
  if (!peerIds.length) return;
  try {
    const r = await call('send_clipboard', { peerIds, stamp: clipStamp() });
    const who = peerIds.length === 1 ? state.peers.find((p) => p.id === peerIds[0])?.name || '' : `${r.count} kişi`;
    const msg = {
      text: `Panodaki metin mesaj olarak gönderildi → ${who}`,
      image: `Ekran görüntüsü gönderiliyor → ${who}`,
      files: `Kopyalanan dosyalar gönderiliyor → ${who}`,
      textfile: `Uzun metin .txt dosyası olarak gönderiliyor → ${who}`,
    }[r.kind];
    toast(msg || 'Gönderildi', { type: 'success' });
  } catch { /* toast gösterildi */ }
}

// Birden fazla cihaz varken Ctrl+V: alıcı seçimi.
function showClipboardTargets() {
  const selected = new Set();
  const m = openModal('clip', `
    <h3>Panodakini gönder</h3>
    <p class="lead">Kısa metin mesaj olarak, ekran görüntüsü ve kopyalanan dosyalar dosya olarak gider.</p>
    <div class="field">
      <div class="label-row"><label>Alıcılar</label><button class="link-btn" id="c-all">Tümünü seç</button></div>
      <div class="recips" id="c-recips"></div>
    </div>
    <div class="foot">
      <button class="btn ghost" data-close>Vazgeç</button>
      <button class="btn" id="c-send" disabled>Gönder</button>
    </div>`);
  const peers = () => state.peers.filter((p) => !p.blocked);
  const render = () => {
    m.querySelector('#c-recips').innerHTML = peers().map((p) => `
      <label class="recip ${selected.has(p.id) ? 'on' : ''}" data-id="${esc(p.id)}">
        <input type="checkbox" ${selected.has(p.id) ? 'checked' : ''} />
        <span class="avatar" style="background:${grad(p.fingerprint)}">${esc(initials(p.name))}</span>
        <span class="r-name">${esc(p.name)}</span><span class="muted small">${esc(p.ip)}</span>
      </label>`).join('');
    m.querySelector('#c-all').textContent = selected.size === peers().length ? 'Hiçbirini seçme' : 'Tümünü seç';
    const send = m.querySelector('#c-send');
    send.disabled = !selected.size;
    send.textContent = selected.size > 1 ? `${selected.size} kişiye gönder` : 'Gönder';
  };
  m.querySelector('#c-recips').addEventListener('change', (e) => {
    const id = e.target.closest('.recip')?.dataset.id;
    if (!id) return;
    e.target.checked ? selected.add(id) : selected.delete(id);
    render();
  });
  m.querySelector('#c-all').onclick = () => {
    if (selected.size === peers().length) selected.clear();
    else peers().forEach((p) => selected.add(p.id));
    render();
  };
  m.querySelector('[data-close]').onclick = closeModal;
  m.querySelector('#c-send').onclick = () => { const ids = [...selected]; closeModal(); sendClipboard(ids); };
  render();
}

// Ana ekranda (yazı alanı dışında) Ctrl+V: panodakini gönder.
document.addEventListener('paste', (e) => {
  if (e.target.closest?.('input, textarea, [contenteditable]') || modalKind || chatOpen()) return;
  const peers = state.peers.filter((p) => !p.blocked);
  if (!peers.length) return toast('Ağda gönderilecek cihaz yok', { type: 'error' });
  e.preventDefault();
  if (peers.length === 1) sendClipboard([peers[0].id]);
  else showClipboardTargets();
});

// ───────────────────────────── Toplu gönder ─────────────────────────────

const baseName = (p) => String(p).replace(/[\\/]+$/, '').split(/[\\/]/).pop() || p;
let batch = null; // açık "Toplu gönder" penceresi (sürükle-bırak ve cihaz güncellemeleri için)

function showBatch(initialPaths = [], preselected = []) {
  const selected = new Set(preselected);
  let paths = [...new Set(initialPaths)];
  const m = openModal('batch', `
    <h3>Toplu gönder</h3>
    <p class="lead">Aynı dosyaları birden fazla kişiye gönderin. Her alıcı kendi ekranında ayrıca onaylar.</p>
    <div class="field">
      <div class="label-row"><label>Alıcılar</label><button class="link-btn" id="b-all"></button></div>
      <div class="recips" id="b-recips"></div>
    </div>
    <div class="field">
      <label>Gönderilecekler</label>
      <div class="picked" id="b-picked"></div>
      <div class="row">
        <button class="btn ghost small" id="b-files">${ic.file}Dosya ekle</button>
        <button class="btn ghost small" id="b-folder">${ic.folder}Klasör ekle</button>
      </div>
    </div>
    <div class="foot">
      <button class="btn ghost" data-close>Vazgeç</button>
      <button class="btn" id="b-send" disabled>Gönder</button>
    </div>`, { onClose: () => { batch = null; } });

  const available = () => state.peers.filter((p) => !p.blocked);
  const render = () => {
    const peers = available();
    [...selected].forEach((id) => { if (!peers.some((p) => p.id === id)) selected.delete(id); });
    m.querySelector('#b-recips').innerHTML = peers.length
      ? peers.map((p) => `
        <label class="recip ${selected.has(p.id) ? 'on' : ''}" data-id="${esc(p.id)}">
          <input type="checkbox" ${selected.has(p.id) ? 'checked' : ''} />
          <span class="avatar" style="background:${grad(p.fingerprint)}">${face(p.fingerprint, p.name)}</span>
          <span class="r-name">${esc(p.name)}</span><span class="muted small">${esc(p.ip)}</span>
        </label>`).join('')
      : '<span class="muted small">Ağda gönderilebilecek cihaz yok</span>';
    m.querySelector('#b-all').textContent = peers.length && selected.size === peers.length ? 'Hiçbirini seçme' : 'Tümünü seç';
    const picked = m.querySelector('#b-picked');
    picked.classList.toggle('empty-drop', paths.length === 0);
    picked.innerHTML = paths.length
      ? paths.map((p, i) => `
        <div class="f">${ic.file}<span class="n" title="${esc(p)}">${esc(baseName(p))}</span>
        <button class="icon-btn sm" data-rm="${i}" title="Listeden çıkar">${ic.x}</button></div>`).join('')
      : 'Dosya veya klasörleri buraya sürükleyin<br>ya da aşağıdan ekleyin';
    const send = m.querySelector('#b-send');
    send.disabled = !selected.size || !paths.length;
    send.textContent = selected.size ? `${selected.size} kişiye gönder` : 'Gönder';
  };
  const add = (more) => { paths = [...new Set([...paths, ...more])]; render(); };

  m.querySelector('#b-recips').addEventListener('change', (e) => {
    const id = e.target.closest('.recip')?.dataset.id;
    if (!id) return;
    e.target.checked ? selected.add(id) : selected.delete(id);
    render();
  });
  m.querySelector('#b-all').onclick = () => {
    const peers = available();
    if (selected.size === peers.length) selected.clear();
    else peers.forEach((p) => selected.add(p.id));
    render();
  };
  m.querySelector('#b-picked').addEventListener('click', (e) => {
    const rm = e.target.closest('[data-rm]');
    if (rm) { paths.splice(Number(rm.dataset.rm), 1); render(); }
  });
  const pick = async (directory) => {
    const sel = await api.pick({ multiple: true, directory, title: directory ? 'Gönderilecek klasörler' : 'Gönderilecek dosyalar' });
    if (sel) add(Array.isArray(sel) ? sel : [sel]);
  };
  m.querySelector('#b-files').onclick = () => pick(false);
  m.querySelector('#b-folder').onclick = () => pick(true);
  m.querySelector('[data-close]').onclick = closeModal;
  m.querySelector('#b-send').onclick = async () => {
    const ids = [...selected];
    try {
      const started = await call('send_paths', { peerIds: ids, paths });
      closeModal();
      toast(`${started.length} kişiye gönderim başladı`, { type: 'success' });
    } catch { /* toast gösterildi */ }
  };

  batch = { add, render };
  render();
}
$('#btn-batch').onclick = () => showBatch();

function showAddPeer() {
  const m = openModal('add', `
    <h3>IP adresiyle cihaz ekle</h3>
    <p class="lead">Otomatik bulunamayan bir bilgisayarı adresiyle ekleyin. Adresi o bilgisayardaki FastFiles'ın üst çubuğunda görebilirsiniz.</p>
    <div class="field">
      <label for="addr">Adres</label>
      <input class="input" id="addr" placeholder="ör. 192.168.1.40" autofocus spellcheck="false" />
      <span class="help">Farklı bir port kullanıyorsa: 192.168.1.40:53317</span>
      <div class="err" id="add-err"></div>
    </div>
    <div class="foot">
      <button class="btn ghost" data-close>Vazgeç</button>
      <button class="btn" id="add-go">Bağlan ve ekle</button>
    </div>`);
  const input = m.querySelector('#addr');
  const go = m.querySelector('#add-go');
  m.querySelector('[data-close]').onclick = closeModal;
  const submit = async () => {
    const address = input.value.trim();
    if (!address) return input.focus();
    go.disabled = true;
    go.textContent = 'Bağlanılıyor…';
    m.querySelector('#add-err').textContent = '';
    try {
      const peer = await api.invoke('add_manual_peer', { address });
      closeModal();
      toast(`${peer.name} eklendi`, { type: 'success' });
    } catch (e) {
      m.querySelector('#add-err').textContent = String(e);
      go.disabled = false;
      go.textContent = 'Bağlan ve ekle';
    }
  };
  go.onclick = submit;
  input.addEventListener('keydown', (e) => e.key === 'Enter' && submit());
}
$('#btn-add-peer').onclick = showAddPeer;
$('#link-add-peer').onclick = (e) => { e.preventDefault(); showAddPeer(); };

// ───────────────────────────── Aktarımlar ─────────────────────────────

function statusText(t) {
  const pct = t.total > 0 ? Math.floor((t.done / t.total) * 100) : 100;
  switch (t.status) {
    case 'waiting':
      // Kopan bağlantıdan sonra yeniden deneme bekleniyorsa nedeni gösterilir.
      if (t.error) return [t.error, t.done > 0 ? `%${pct} · ${fmtBytes(t.done)} / ${fmtBytes(t.total)}` : ''];
      return [t.direction === 'send' ? 'Karşı tarafın onayı bekleniyor…' : 'Hazırlanıyor…', ''];
    case 'active': {
      const eta = t.speed > 0 ? fmtEta((t.total - t.done) / t.speed) : '';
      const resumed = t.resumed ? ' · kaldığı yerden' : '';
      return [`%${pct} · ${fmtBytes(t.done)} / ${fmtBytes(t.total)}${resumed}`, t.speed > 0 ? `${fmtSpeed(t.speed)}${eta ? ` · ${eta}` : ''}` : ''];
    }
    case 'done': {
      // Kaldığı yerden devam eden aktarımda süre (ve ortalama hız) anlamlı değil.
      const secs = t.finishedAt && t.createdAt && !t.resumed ? (t.finishedAt - t.createdAt) / 1000 : 0;
      const avg = secs > 0.5 ? ` · ort. ${fmtSpeed(t.total / secs)}` : '';
      const label = t.direction === 'send' ? 'Gönderildi' : 'Alındı';
      return [`${label}${t.resumed ? ' · kaldığı yerden devam edildi' : ''}`, `${whenLabel(t.finishedAt || Date.now())}${avg}`];
    }
    case 'paused':
      return [t.error || 'Duraklatıldı · devam ettirince kaldığı yerden sürer', `%${pct} · ${fmtBytes(t.done)} / ${fmtBytes(t.total)}`];
    case 'declined': return [t.error || 'Reddedildi', whenLabel(t.finishedAt)];
    case 'cancelled': return [t.error || 'İptal edildi', whenLabel(t.finishedAt)];
    default: return [t.error || 'Başarısız oldu', whenLabel(t.finishedAt)];
  }
}

// Bugünse saat, değilse "Dün 14:05" / "3 Kas 14:05".
function whenLabel(ms) {
  if (!ms) return '';
  const label = dayLabel(ms);
  if (label === 'Bugün') return fmtTime(ms);
  if (label === 'Dün') return `Dün ${fmtTime(ms)}`;
  return `${new Date(ms).toLocaleDateString('tr-TR', { day: 'numeric', month: 'short' })} ${fmtTime(ms)}`;
}

function transferActions(t) {
  if (t.status === 'waiting' || t.status === 'active') {
    const pause = t.direction === 'send' && t.resumable
      ? `<button class="icon-btn sm" data-act="pause" title="Duraklat (alıcıdaki yarım dosyalar korunur)">${ic.pause}</button>` : '';
    return `${pause}<button class="icon-btn sm" data-act="cancel" title="İptal et">${ic.x}</button>`;
  }
  let html = '';
  if (t.status === 'paused' && t.direction === 'send') {
    html += `<button class="icon-btn sm" data-act="retry" title="Devam ettir">${ic.play}</button>`;
  }
  if (t.status === 'done' && t.direction === 'receive' && t.savePath) {
    html += `<button class="icon-btn sm" data-act="reveal" title="Klasörde göster">${ic.reveal}</button>`;
  }
  if (t.direction === 'send' && t.sources?.length && t.status !== 'paused') {
    const title = t.status === 'done' ? 'Tekrar gönder'
      : t.status === 'failed' ? 'Tekrar dene (alıcıda yarım kaldıysa kaldığı yerden devam eder)' : 'Tekrar dene';
    html += `<button class="icon-btn sm" data-act="retry" title="${title}">${ic.retry}</button>`;
  }
  html += `<button class="icon-btn sm faint" data-act="remove" title="Geçmişten sil">${ic.trash}</button>`;
  return html;
}

function renderTransfer(t) {
  const list = $('#transfers');
  let el = document.getElementById(`t-${t.id}`);
  if (!el) {
    el = document.createElement('div');
    el.id = `t-${t.id}`;
    el.dataset.id = t.id;
    el.innerHTML = `
      <div class="dir"></div>
      <div class="title"></div>
      <div class="t-actions"></div>
      <div class="sub"></div>
      <div class="bar"><i></i></div>
      <div class="status"><span class="st"></span><span class="sp"></span></div>`;
    list.prepend(el);
  }
  el.className = `transfer ${t.direction} ${t.status}`;
  el.querySelector('.dir').innerHTML = t.direction === 'send' ? ic.up : ic.down;
  const title = el.querySelector('.title');
  title.textContent = t.title;
  title.title = t.currentFile ? `Şu an: ${t.currentFile}` : t.title;
  el.querySelector('.sub').textContent =
    `${t.direction === 'send' ? '→' : '←'} ${t.peerName} · ${t.fileCount} dosya · ${fmtBytes(t.total)}`;
  el.querySelector('.bar i').style.width = `${t.total > 0 ? (t.done / t.total) * 100 : 100}%`;
  const [st, sp] = statusText(t);
  el.querySelector('.st').textContent = st;
  el.querySelector('.st').title = st;
  el.querySelector('.sp').textContent = sp;
  if (el.dataset.status !== t.status) {
    el.dataset.status = t.status;
    el.querySelector('.t-actions').innerHTML = transferActions(t);
  }
}

function updateTransfersChrome() {
  const all = [...state.transfers.values()];
  $('#transfers-empty').hidden = all.length > 0;
  $('#btn-clear').hidden = state.tab !== 'transfers' || !all.some((t) => !['waiting', 'active'].includes(t.status));
}

function onTransfer(t) {
  const prev = state.transfers.get(t.id);
  state.transfers.set(t.id, t);
  renderTransfer(t);
  updateTransfersChrome();
  refreshFileBubbles(t.id);
  if (prev?.status === t.status) return;
  if (t.status === 'done') {
    if (t.direction === 'receive') {
      toast(`"${t.title}" alındı · ${t.peerName}`, {
        type: 'success', action: 'Göster', timeout: 7000,
        onAction: () => api.invoke('reveal_path', { path: t.savePath }),
      });
    } else {
      toast(`"${t.title}" gönderildi → ${t.peerName}`, { type: 'success' });
    }
  } else if (t.status === 'declined') {
    toast(t.error || `${t.peerName} gönderimi reddetti`, { type: 'error' });
  } else if (t.status === 'failed') {
    toast(`Aktarım başarısız: ${t.error || 'bilinmeyen hata'}`, { type: 'error', timeout: 7000 });
  }
}

$('#transfers').addEventListener('click', (e) => {
  const btn = e.target.closest('[data-act]');
  const el = e.target.closest('.transfer');
  if (!btn || !el) return;
  const t = state.transfers.get(el.dataset.id);
  if (!t) return;
  if (btn.dataset.act === 'cancel') call('cancel_transfer', { id: t.id });
  if (btn.dataset.act === 'reveal') call('reveal_path', { path: t.savePath });
  if (btn.dataset.act === 'retry') call('retry_transfer', { id: t.id }).catch(() => {});
  if (btn.dataset.act === 'pause') call('pause_transfer', { id: t.id }).catch(() => {});
  if (btn.dataset.act === 'remove') call('remove_transfer', { id: t.id }).then(setTransfers).catch(() => {});
});

$('#btn-clear').onclick = async () => {
  const ok = await confirmDialog('Aktarım geçmişi silinsin mi?',
    'Biten tüm gönderim ve alımlar listeden kaldırılır. Dosyalar silinmez.', 'Geçmişi temizle', true);
  if (!ok) return;
  const list = await call('clear_finished');
  setTransfers(list);
};

function setTransfers(list) {
  state.transfers = new Map(list.map((t) => [t.id, t]));
  $('#transfers').innerHTML = '';
  [...list].reverse().forEach(renderTransfer);
  updateTransfersChrome();
}

// ───────────────────────────── Gelen istekler ─────────────────────────────

const responded = new Set();

function onIncoming(req) {
  req.receivedAt = Date.now();
  state.incoming.push(req);
  if (modalKind === 'incoming') return renderQueueNote();
  // Başka bir pencere açıksa (Toplu gönder, Ayarlar, PIN…) onu kapatıp içindekileri
  // kaybettirme: o pencere kapanınca istek gösterilir.
  if (modalKind) {
    toast(`${req.device.name} dosya göndermek istiyor`, { action: 'Göster', timeout: 10000, onAction: () => { closeModal(); showNextIncoming(); } });
    return;
  }
  showNextIncoming();
}

function onIncomingClosed({ requestId }) {
  const wasShown = state.incoming[0]?.requestId === requestId && modalKind === 'incoming';
  const req = state.incoming.find((r) => r.requestId === requestId);
  state.incoming = state.incoming.filter((r) => r.requestId !== requestId);
  if (req && !responded.has(requestId)) {
    toast(`${req.device.name} gönderimden vazgeçti veya süre doldu`, { type: 'info' });
  }
  responded.delete(requestId);
  if (wasShown) {
    closeModal();
    showNextIncoming();
  } else {
    renderQueueNote();
  }
}

let countdownTimer = null;

function renderQueueNote() {
  const note = $('#queue-note');
  const req = state.incoming[0];
  if (!note || !req) return;
  const left = Math.max(0, 120 - Math.floor((Date.now() - req.receivedAt) / 1000));
  const more = state.incoming.length - 1;
  note.textContent = `${left} sn içinde yanıt verin${more > 0 ? ` · ${more} istek daha bekliyor` : ''}`;
}

function showNextIncoming() {
  clearInterval(countdownTimer);
  const req = state.incoming[0];
  if (!req) return;
  const d = req.device;
  const fileRows = req.files.map((f) => `
    <div class="f">${ic.file}<span class="n" title="${esc(f.path)}">${esc(f.path)}</span><span class="s">${fmtBytes(f.size)}</span></div>`).join('');
  const more = req.fileCount - req.files.length;
  const m = openModal('incoming', `
    <div class="incoming-head">
      <div class="avatar" style="background:${grad(d.fingerprint)}">${face(d.fingerprint, d.name)}</div>
      <div>
        <h3>${esc(d.name)}</h3>
        <div class="muted small">${esc(req.ip)} · Kimlik <span class="fp">${esc(fpShort(d.fingerprint))}</span></div>
      </div>
    </div>
    <p class="lead">${req.group ? `<b>${esc(req.group)}</b> grubuna` : 'Size'} <b>${req.fileCount} dosya</b> göndermek istiyor.</p>
    <div class="summary"><span>${esc(req.title)}</span><span>${fmtBytes(req.total)}</span></div>
    <div class="file-list">${fileRows}${more > 0 ? `<div class="more">+ ${more} dosya daha</div>` : ''}</div>
    <label class="check"><input type="checkbox" id="trust-cb" /> <span>${esc(d.name)} gönderdiğinde bir daha sorma</span></label>
    <div class="foot">
      <span class="queue-note" id="queue-note"></span><span class="spacer"></span>
      <button class="btn ghost" data-r="decline">Reddet</button>
      <button class="btn" data-r="accept" autofocus>${ic.down}Kabul et</button>
    </div>`);
  renderQueueNote();
  countdownTimer = setInterval(renderQueueNote, 1000);
  m.querySelectorAll('[data-r]').forEach((b) => (b.onclick = async () => {
    const accept = b.dataset.r === 'accept';
    const trust = accept && m.querySelector('#trust-cb').checked;
    responded.add(req.requestId);
    state.incoming = state.incoming.filter((r) => r.requestId !== req.requestId);
    clearInterval(countdownTimer);
    closeModal();
    await api.invoke('respond_incoming', { requestId: req.requestId, accept, trust }).catch(() => {});
    showNextIncoming();
  }));
}

// ───────────────────────────── Ayarlar ─────────────────────────────

function limitOptions(current) {
  const values = [...new Set([0, 2, 5, 10, 25, 50, 100, current])].sort((a, b) => a - b);
  return values.map((v) => `<option value="${v}" ${v === current ? 'selected' : ''}>${v ? `${v} MB/sn` : 'Sınırsız'}</option>`).join('');
}

function showSettings() {
  const s = state.settings;
  const m = openModal('settings', `
    <h3>Ayarlar</h3>
    <p class="lead">Bu bilgisayarın ağda nasıl görüneceği ve gelen dosyaların nereye kaydedileceği.</p>
    <div class="field">
      <label>Profil resmi</label>
      <div class="avatar-edit">
        <div class="avatar" id="s-avatar" style="background:${grad(state.device?.fingerprint)}">${face(state.device?.fingerprint, s.deviceName)}</div>
        <div class="row">
          <button class="btn ghost small" id="s-avatar-pick">Kendi resmini seç</button>
          <button class="btn ghost small" id="s-avatar-remove" ${state.avatars.has(state.device?.fingerprint) ? '' : 'hidden'}>Kaldır</button>
        </div>
        <input type="file" id="s-avatar-file" accept="image/*" hidden />
      </div>
      ${(window.FF_PRESET_AVATARS || []).length ? `
      <div class="preset-grid" id="s-presets">
        ${window.FF_PRESET_AVATARS.map((p) => `
          <button class="preset" data-file="${esc(p.file)}" title="${esc(p.title)}"><img src="avatars/${esc(p.file)}" alt="${esc(p.title)}" loading="lazy" draggable="false"></button>`).join('')}
      </div>` : ''}
      <span class="help">Hazır avatarlardan birine tıklayın ya da kendi resminizi seçin. Diğerleri sizi bu resimle görür.</span>
    </div>
    <div class="field">
      <label for="s-name">Cihaz adı</label>
      <input class="input" id="s-name" maxlength="40" value="${esc(s.deviceName)}" autofocus />
      <span class="help">Diğer bilgisayarlarda bu isimle görünürsünüz.</span>
    </div>
    <div class="field">
      <label>İndirme klasörü</label>
      <div class="row">
        <input class="input path" id="s-dir" value="${esc(s.downloadDir)}" readonly />
        <button class="btn ghost small" id="s-dir-pick">Değiştir</button>
      </div>
    </div>
    <label class="check"><input type="checkbox" id="s-tray" ${s.closeToTray ? 'checked' : ''} />
      <span>Pencere kapatılınca sistem tepsisinde çalışmaya devam et<br><span class="muted small">Kapalıyken de dosya alabilmek için önerilir.</span></span></label>
    <hr class="sep" />
    <div class="field">
      <label>Renk teması</label>
      <div class="theme-row" id="s-themes">${THEMES.map((t) => `
        <button class="swatch ${t.id === (s.theme || 'mor') ? 'on' : ''}" data-theme="${t.id}" title="${esc(t.name)}"
          style="background:linear-gradient(135deg, ${t.a}, ${t.b})"></button>`).join('')}</div>
    </div>
    <div class="field">
      <label for="s-appearance">Görünüm</label>
      <select class="input" id="s-appearance">
        ${[['system', 'Sistemle aynı'], ['light', 'Açık'], ['dark', 'Koyu']].map(([v, l]) => `<option value="${v}" ${(s.appearance || 'system') === v ? 'selected' : ''}>${l}</option>`).join('')}
      </select>
    </div>
    <hr class="sep" />
    <div class="field">
      <label>Uygulama kilidi</label>
      <span class="help">${state.lock?.hasPin
        ? 'Açılışta 4 haneli PIN sorulur. Üst çubuktaki kilit düğmesiyle istediğiniz an kilitleyebilirsiniz.'
        : 'İsterseniz açılışta 4 haneli bir PIN sorulsun. PIN unutulursa kilit ekranından sıfırlanabilir; bu durumda mesajlar ve aktarım geçmişi silinir.'}</span>
      <div class="row">
        ${state.lock?.hasPin
          ? '<button class="btn ghost small" id="s-pin-change">PIN\'i değiştir</button><button class="btn ghost small" id="s-pin-remove">PIN\'i kaldır</button>'
          : '<button class="btn soft small" id="s-pin-set">PIN belirle</button>'}
      </div>
    </div>
    <div class="field">
      <label for="s-autolock">Otomatik kilit</label>
      <select class="input" id="s-autolock" ${state.lock?.hasPin ? '' : 'disabled'}>
        ${[[0, 'Kapalı'], [1, '1 dakika işlem yapılmazsa'], [5, '5 dakika işlem yapılmazsa'], [15, '15 dakika işlem yapılmazsa'], [30, '30 dakika işlem yapılmazsa'], [60, '1 saat işlem yapılmazsa']]
          .map(([v, l]) => `<option value="${v}" ${(s.lockMinutes || 0) === v ? 'selected' : ''}>${l}</option>`).join('')}
      </select>
    </div>
    <hr class="sep" />
    <div class="field">
      <label for="s-limit">Gönderim hız sınırı</label>
      <select class="input" id="s-limit">${limitOptions(s.speedLimitMb || 0)}</select>
      <span class="help">Büyük gönderimler ofis ağını tıkamasın diye bu bilgisayarın toplam gönderim hızını sınırlar. Alım hızı etkilenmez.</span>
    </div>
    <label class="check"><input type="checkbox" id="s-shell" ${s.shellMenu !== false ? 'checked' : ''} />
      <span>Gezgin'de sağ tık menüsüne "FastFiles ile gönder" ekle<br><span class="muted small">Dosya ve klasörlere sağ tıklayıp doğrudan gönderin. Windows 11'de "Diğer seçenekleri göster" altında; çok sayıda dosya için "Gönder" menüsünde de bulunur.</span></span></label>
    <label class="check"><input type="checkbox" id="s-compress" ${s.compress !== false ? 'checked' : ''} />
      <span>Uygun dosyaları sıkıştırarak gönder<br><span class="muted small">Metin, log, CSV, kaynak kod gibi dosyalar özellikle Wi-Fi'da birkaç kat hızlı gider. Zip, video, fotoğraf gibi zaten sıkıştırılmış dosyalar olduğu gibi gider. Karşı tarafta FastFiles'ın güncel sürümü olmalı.</span></span></label>
    <hr class="sep" />
    <div class="field">
      <label>Güvenilir cihazlar</label>
      <span class="help">Bu cihazlardan gelen dosyalar onay sorulmadan kabul edilir.</span>
      <div class="trusted-list" id="s-trusted">
        ${s.trusted.length ? s.trusted.map((t) => `
          <div class="t"><span class="badge trusted">${ic.shield}</span><span class="n">${esc(t.name)}</span>
          <span class="fp">${esc(fpShort(t.fingerprint))}</span>
          <button class="icon-btn sm" data-fp="${esc(t.fingerprint)}" data-name="${esc(t.name)}" title="Güveni kaldır">${ic.trash}</button></div>`).join('')
        : '<span class="muted small">Henüz yok. Gelen istek penceresinde “bir daha sorma” seçeneğiyle ekleyebilirsiniz.</span>'}
      </div>
    </div>
    <div class="field">
      <label>Engellenen cihazlar</label>
      <span class="help">Bu cihazlardan gelen mesaj ve dosyalar otomatik olarak reddedilir.</span>
      <div class="trusted-list" id="s-blocked">
        ${(s.blocked || []).length ? s.blocked.map((t) => `
          <div class="t"><span class="badge blocked">${ic.ban}</span><span class="n">${esc(t.name)}</span>
          <span class="fp">${esc(fpShort(t.fingerprint))}</span>
          <button class="btn ghost small" data-bfp="${esc(t.fingerprint)}" data-name="${esc(t.name)}">Engeli kaldır</button></div>`).join('')
        : '<span class="muted small">Engellenen cihaz yok.</span>'}
      </div>
    </div>
    <hr class="sep" />
    <div class="field">
      <label>Güncellemeler</label>
      <div class="version-row">
        <span>Kullandığınız sürüm: <span class="v">v${esc(state.update?.current || '')}</span></span>
        ${state.update?.ready ? `<span class="badge trusted">v${esc(state.update.ready)} kurulmaya hazır</span>` : ''}
        <span class="spacer" style="flex:1"></span>
        <button class="btn ghost small" id="s-import">Güncelleme paketi yükle</button>
      </div>
      <span class="help">Güncellemeler ağdaki diğer FastFiles'lardan otomatik gelir ve yalnızca yayıncının dijital imzası doğrulanırsa kurulur.
        ${state.update?.serving ? `Bu cihaz v${esc(state.update.serving)} paketini ağa dağıtıyor.` : ''}</span>
    </div>
    <hr class="sep" />
    <div class="muted small">
      Bu cihazın kimliği: <span class="fp">${esc(fpShort(state.device?.fingerprint))}</span><br>
      Adres: ${esc(state.ips.join(', ') || '—')} · Port ${esc(state.device?.port)}
    </div>
    <div class="foot">
      <button class="btn ghost" data-close>Vazgeç</button>
      <button class="btn" id="s-save">Kaydet</button>
    </div>`);
  m.querySelector('[data-close]').onclick = closeModal;
  m.querySelector('#s-dir-pick').onclick = async () => {
    const dir = await api.pick({ directory: true, multiple: false, defaultPath: s.downloadDir, title: 'İndirme klasörünü seçin' });
    if (dir) m.querySelector('#s-dir').value = Array.isArray(dir) ? dir[0] : dir;
  };
  m.querySelectorAll('#s-trusted [data-fp]').forEach((b) => (b.onclick = async () => {
    state.settings = await call('set_trusted', { fingerprint: b.dataset.fp, name: b.dataset.name, trusted: false });
    b.closest('.t').remove();
  }));
  const fileInput = m.querySelector('#s-avatar-file');
  const applyAvatar = async (makeUrl) => {
    try {
      const url = await makeUrl();
      await call('set_avatar', { dataUrl: url });
      state.avatars.set(state.device.fingerprint, url);
      m.querySelector('#s-avatar').innerHTML = face(state.device.fingerprint, state.settings.deviceName);
      m.querySelector('#s-avatar-remove').hidden = false;
      renderMe();
      toast('Profil resmi güncellendi', { type: 'success' });
    } catch (e) {
      toast(String(e?.message || e), { type: 'error' });
    }
  };
  m.querySelector('#s-avatar-pick').onclick = () => fileInput.click();
  fileInput.onchange = () => {
    const file = fileInput.files?.[0];
    fileInput.value = '';
    if (file) applyAvatar(() => squareImage(file, 192));
  };
  m.querySelector('#s-presets')?.addEventListener('click', (e) => {
    const b = e.target.closest('[data-file]');
    if (b) applyAvatar(() => presetToDataUrl(b.dataset.file, 160));
  });
  m.querySelector('#s-avatar-remove').onclick = async () => {
    await call('remove_avatar');
    state.avatars.delete(state.device.fingerprint);
    m.querySelector('#s-avatar').innerHTML = face(state.device.fingerprint, state.settings.deviceName);
    m.querySelector('#s-avatar-remove').hidden = true;
    renderMe();
  };
  m.querySelector('#s-import').onclick = async () => {
    const file = await api.pick({ multiple: false, directory: false, title: 'FastFiles güncelleme paketi', filters: [{ name: 'FastFiles güncellemesi', extensions: ['ffupdate'] }] });
    if (!file) return;
    closeModal();
    importUpdate(Array.isArray(file) ? file[0] : file);
  };
  m.querySelectorAll('#s-blocked [data-bfp]').forEach((b) => (b.onclick = async () => {
    state.settings = await call('set_blocked', { fingerprint: b.dataset.bfp, name: b.dataset.name, blocked: false });
    b.closest('.t').remove();
    renderChatHead();
  }));
  const save = async () => {
    try {
      state.settings = await call('update_settings', {
        deviceName: m.querySelector('#s-name').value,
        downloadDir: m.querySelector('#s-dir').value,
        closeToTray: m.querySelector('#s-tray').checked,
        compress: m.querySelector('#s-compress').checked,
        shellMenu: m.querySelector('#s-shell').checked,
        speedLimitMb: Number(m.querySelector('#s-limit').value),
        theme: m.querySelector('.swatch.on')?.dataset.theme || 'mor',
        appearance: m.querySelector('#s-appearance').value,
        lockMinutes: Number(m.querySelector('#s-autolock').value),
      });
      applyTheme(state.settings);
      renderMe();
      closeModal();
      toast('Ayarlar kaydedildi', { type: 'success' });
    } catch { /* toast gösterildi */ }
  };
  m.querySelector('#s-save').onclick = save;
  m.querySelector('#s-name').addEventListener('keydown', (e) => e.key === 'Enter' && save());
  // Tema: tıklayınca önizlenir; kaydedilmezse pencere kapanınca eskisine döner.
  const preview = () => applyTheme({ theme: m.querySelector('.swatch.on')?.dataset.theme, appearance: m.querySelector('#s-appearance').value });
  m.querySelector('#s-themes').addEventListener('click', (e) => {
    const b = e.target.closest('.swatch');
    if (!b) return;
    m.querySelectorAll('.swatch').forEach((x) => x.classList.toggle('on', x === b));
    preview();
  });
  m.querySelector('#s-appearance').onchange = preview;
  modalOnClose = () => applyTheme(state.settings);
  m.querySelector('#s-pin-set')?.addEventListener('click', () => showPinSetup('set'));
  m.querySelector('#s-pin-change')?.addEventListener('click', () => showPinSetup('change'));
  m.querySelector('#s-pin-remove')?.addEventListener('click', () => showPinSetup('remove'));
}

// ───────────────────────────── Renk temaları ─────────────────────────────

const THEMES = [
  { id: 'mor', name: 'Mor', a: '#5b55f6', b: '#22c5e0' },
  { id: 'okyanus', name: 'Okyanus', a: '#0a7cff', b: '#2ec4b6' },
  { id: 'orman', name: 'Orman', a: '#1f9d6b', b: '#9ccc3d' },
  { id: 'gunbatimi', name: 'Gün batımı', a: '#ef6a2e', b: '#f5b52a' },
  { id: 'gul', name: 'Gül', a: '#e0457b', b: '#a26bff' },
  { id: 'grafit', name: 'Grafit', a: '#4b5567', b: '#7f8ba0' },
];

function applyTheme(s) {
  const root = document.documentElement;
  const theme = THEMES.some((t) => t.id === s?.theme) ? s.theme : 'mor';
  const mode = ['light', 'dark'].includes(s?.appearance) ? s.appearance : 'system';
  if (theme === 'mor') delete root.dataset.theme; else root.dataset.theme = theme;
  if (mode === 'system') delete root.dataset.mode; else root.dataset.mode = mode;
  try { localStorage.setItem('ff-theme', JSON.stringify({ theme, appearance: mode })); } catch { /* yok say */ }
}
// Ayarlar yüklenmeden önce son temayı uygula (açılışta renk sıçramasın).
try { applyTheme(JSON.parse(localStorage.getItem('ff-theme') || 'null')); } catch { /* yok say */ }

// ───────────────────────────── Uygulama kilidi ─────────────────────────────

const lockUi = { pin: '', busy: false, timer: null };
const lockVisible = () => !$('#lock-screen').hidden;

function renderPinDots() {
  $('#pin-input').value = lockUi.pin;
  $('#pin-dots').querySelectorAll('i').forEach((d, i) => d.classList.toggle('on', i < lockUi.pin.length));
}

function showLock() {
  closeEmoji();
  lockUi.pin = '';
  renderPinDots();
  $('#pin-err').textContent = '';
  $('#lock-screen').hidden = false;
  $('#lock-forgot-box')?.remove();
  $('#pin-pad').hidden = false;
  setTimeout(() => $('#pin-input').focus(), 50);
}

function waitRetry(seconds) {
  clearInterval(lockUi.timer);
  let left = seconds;
  const pad = $('#pin-pad').querySelectorAll('button');
  const tick = () => {
    pad.forEach((b) => (b.disabled = left > 0));
    $('#pin-err').textContent = left > 0 ? `Çok fazla yanlış deneme. ${left} sn bekleyin` : '';
    if (left-- <= 0) clearInterval(lockUi.timer);
  };
  tick();
  lockUi.timer = setInterval(tick, 1000);
}

async function submitPin() {
  if (lockUi.busy) return;
  lockUi.busy = true;
  const pin = lockUi.pin;
  try {
    await api.invoke('unlock_app', { pin });
    state.lock = { ...(state.lock || {}), locked: false };
    $('#lock-screen').hidden = true;
    lastActivity = Date.now();
    if (!state.booted) init().catch((e) => toast(`Başlatılamadı: ${e}`, { type: 'error', timeout: 15000 }));
  } catch (e) {
    lockUi.pin = '';
    renderPinDots();
    const dots = $('#pin-dots');
    dots.classList.remove('shake');
    void dots.offsetWidth;
    dots.classList.add('shake');
    const ls = await api.invoke('lock_state').catch(() => null);
    if (ls?.retryAfter > 0) waitRetry(ls.retryAfter);
    else $('#pin-err').textContent = String(e);
  } finally {
    lockUi.busy = false;
  }
}

function pinKey(k) {
  if (lockUi.busy || $('#pin-pad').hidden) return;
  if ([...$('#pin-pad').querySelectorAll('button')].some((b) => b.disabled)) return;
  if (k === 'back') lockUi.pin = lockUi.pin.slice(0, -1);
  else if (/^\d$/.test(k) && lockUi.pin.length < 4) lockUi.pin += k;
  $('#pin-err').textContent = '';
  renderPinDots();
  if (lockUi.pin.length === 4) submitPin();
}

// Yapıştırma / ekran klavyesi gibi tuş olayı üretmeyen girişler.
$('#pin-input').addEventListener('input', () => {
  const digits = $('#pin-input').value.replace(/\D/g, '').slice(0, 4);
  if (lockUi.busy || digits === lockUi.pin) return;
  lockUi.pin = digits;
  $('#pin-err').textContent = '';
  renderPinDots();
  if (lockUi.pin.length === 4) submitPin();
});

$('#pin-pad').addEventListener('click', (e) => {
  const b = e.target.closest('[data-k]');
  if (b) pinKey(b.dataset.k);
});
// Kilitliyken klavye yalnızca PIN girer; diğer kısayollar (Esc vb.) çalışmaz.
window.addEventListener('keydown', (e) => {
  if (!lockVisible()) return;
  e.stopImmediatePropagation();
  if (/^\d$/.test(e.key)) pinKey(e.key);
  else if (e.key === 'Backspace') pinKey('back');
  if (e.key !== 'Tab') e.preventDefault();
}, true);

$('#pin-forgot').onclick = () => {
  $('#pin-pad').hidden = true;
  const box = document.createElement('div');
  box.id = 'lock-forgot-box';
  box.innerHTML = `
    <p class="muted small">PIN sıfırlanırsa bu bilgisayardaki <b>tüm mesajlar ve aktarım geçmişi silinir</b>. Grup üyelikleri, ortak klasörler ve indirilmiş dosyalar kalır.</p>
    <div class="row" style="justify-content:center">
      <button class="btn ghost small" id="forgot-no">Vazgeç</button>
      <button class="btn danger small" id="forgot-yes">Sil ve PIN'i kaldır</button>
    </div>`;
  $('#pin-forgot').before(box);
  box.querySelector('#forgot-no').onclick = () => { box.remove(); $('#pin-pad').hidden = false; };
  box.querySelector('#forgot-yes').onclick = async () => {
    await api.invoke('forget_pin').catch(() => {});
    location.reload();
  };
};

$('#btn-lock').onclick = async () => {
  if (!state.lock?.hasPin) return showPinSetup('set', true);
  try {
    await call('lock_app');
    state.lock.locked = true;
    showLock();
  } catch { /* toast gösterildi */ }
};

function updateLockButton() {
  $('#btn-lock').title = state.lock?.hasPin ? 'Uygulamayı kilitle' : 'Uygulama kilidi (PIN belirleyin)';
}

// PIN belirleme / değiştirme / kaldırma penceresi.
function showPinSetup(mode, lockAfter = false) {
  const needCurrent = mode !== 'set';
  const needNew = mode !== 'remove';
  const title = { set: 'PIN belirle', change: 'PIN\'i değiştir', remove: 'PIN\'i kaldır' }[mode];
  const digits = (id, ph) => `<input class="input" id="${id}" type="password" inputmode="numeric" maxlength="4" placeholder="${ph}" autocomplete="off" />`;
  const m = openModal('pin', `
    <h3>${title}</h3>
    <p class="lead">${mode === 'remove' ? 'Uygulama artık açılışta PIN sormayacak.' : 'Uygulama açılırken ve kilitlediğinizde bu 4 haneli PIN sorulur. Unutursanız kilit ekranından sıfırlayabilirsiniz; bu durumda mesajlar ve aktarım geçmişi silinir.'}</p>
    <div class="pin-fields">
      ${needCurrent ? digits('p-cur', 'Mevcut PIN') : ''}
      ${needNew ? digits('p-new', 'Yeni PIN') + digits('p-new2', 'Tekrar') : ''}
    </div>
    <div class="err" id="p-err"></div>
    <div class="foot">
      <button class="btn ghost" data-close>Vazgeç</button>
      <button class="btn ${mode === 'remove' ? 'danger' : ''}" id="p-go">${mode === 'remove' ? 'Kaldır' : 'Kaydet'}</button>
    </div>`);
  m.querySelectorAll('input').forEach((i) => i.addEventListener('input', () => { i.value = i.value.replace(/\D/g, '').slice(0, 4); }));
  setTimeout(() => m.querySelector('input')?.focus(), 30);
  m.querySelector('[data-close]').onclick = closeModal;
  const go = async () => {
    const val = (id) => m.querySelector(`#${id}`)?.value || '';
    const err = (t) => { m.querySelector('#p-err').textContent = t; };
    if (needNew && !/^\d{4}$/.test(val('p-new'))) return err('PIN 4 rakamdan oluşmalı');
    if (needNew && val('p-new') !== val('p-new2')) return err('PIN\'ler aynı değil');
    try {
      state.lock = await api.invoke('set_app_pin', { pin: needNew ? val('p-new') : null, current: needCurrent ? val('p-cur') : null });
      updateLockButton();
      closeModal();
      toast(mode === 'remove' ? 'PIN kaldırıldı' : 'PIN kaydedildi', { type: 'success' });
      if (lockAfter) $('#btn-lock').click();
    } catch (e) {
      err(String(e));
    }
  };
  m.querySelector('#p-go').onclick = go;
  m.querySelectorAll('input').forEach((i) => i.addEventListener('keydown', (e) => e.key === 'Enter' && go()));
}

// Otomatik kilit: ayardaki süre boyunca fare/klavye hareketi yoksa.
let lastActivity = Date.now();
['mousemove', 'mousedown', 'keydown', 'wheel', 'touchstart'].forEach((ev) =>
  window.addEventListener(ev, () => { lastActivity = Date.now(); }, { passive: true, capture: true }));
setInterval(async () => {
  const mins = state.settings?.lockMinutes || 0;
  if (!mins || !state.lock?.hasPin || lockVisible()) return;
  if (Date.now() - lastActivity >= mins * 60000) {
    try {
      await api.invoke('lock_app');
      state.lock.locked = true;
      showLock();
    } catch { /* yok say */ }
  }
}, 10000);
$('#btn-settings').onclick = showSettings;
$('#me').onclick = showSettings;
$('#btn-downloads').onclick = () => call('open_path', { path: state.settings.downloadDir });

function showWelcome() {
  const m = openModal('welcome', `
    <div class="incoming-head">
      <div class="avatar" style="background:linear-gradient(135deg,var(--accent),var(--accent-2))">
        <svg viewBox="0 0 24 24" style="width:24px;height:24px;fill:#fff;stroke:none"><path d="M13.5 2 5 13.2h6L9.8 22 19 10h-6.2z"/></svg>
      </div>
      <div><h3>FastFiles'a hoş geldiniz</h3><div class="muted small">Ofis içi hızlı dosya paylaşımı</div></div>
    </div>
    <p class="lead">Aynı ağdaki bilgisayarlar birbirini otomatik bulur. Dosyalar internete çıkmadan, şifreli olarak doğrudan karşı bilgisayara gider.</p>
    <div class="field">
      <label for="w-name">Diğerleri sizi hangi isimle görsün?</label>
      <input class="input" id="w-name" maxlength="40" value="${esc(state.settings.deviceName)}" autofocus />
      <div class="err" id="w-err"></div>
    </div>
    <div class="foot"><button class="btn" id="w-go">Başla</button></div>`);
  const go = async () => {
    const name = m.querySelector('#w-name').value.trim();
    if (!name) { m.querySelector('#w-err').textContent = 'Lütfen bir isim girin'; return; }
    state.settings = await call('update_settings', { deviceName: name, onboarded: true });
    renderMe();
    closeModal();
  };
  m.querySelector('#w-go').onclick = go;
  m.querySelector('#w-name').addEventListener('keydown', (e) => e.key === 'Enter' && go());
}

// ───────────────────────────── Mesajlaşma ─────────────────────────────
// Güvenlik: mesaj metni her zaman `esc()`/textContent ile basılır (HTML yorumlanmaz),
// bağlantılar tıklanabilir yapılmaz. Arka uç ayrıca görünmez/yön değiştiren karakterleri temizler.

const unreadOf = (fp) => state.chats.get(fp)?.unread || 0;
const peerByFp = (fp) => state.peers.find((p) => p.fingerprint === fp);
const isBlocked = (fp) => !!state.settings?.blocked?.some((b) => b.fingerprint === fp);
const chatOpen = () => !!state.openChat && !$('#chat-view').hidden;

function setTab(tab) {
  state.tab = tab;
  document.querySelectorAll('.tab').forEach((b) => b.classList.toggle('active', b.dataset.tab === tab));
  $('#tab-transfers').hidden = tab !== 'transfers';
  $('#tab-chats').hidden = tab !== 'chats';
  $('#tab-shares').hidden = tab !== 'shares';
  $('#tab-polls').hidden = tab !== 'polls';
  updateTransfersChrome();
}
document.querySelectorAll('.tab').forEach((b) => (b.onclick = () => setTab(b.dataset.tab)));

function dayLabel(ms) {
  const d = new Date(ms);
  const today = new Date();
  const yesterday = new Date(); yesterday.setDate(today.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return 'Bugün';
  if (d.toDateString() === yesterday.toDateString()) return 'Dün';
  return d.toLocaleDateString('tr-TR', { day: 'numeric', month: 'long', year: 'numeric' });
}

function shortTime(ms) {
  const label = dayLabel(ms);
  if (label === 'Bugün') return fmtTime(ms);
  if (label === 'Dün') return 'Dün';
  return new Date(ms).toLocaleDateString('tr-TR', { day: 'numeric', month: 'short' });
}

function renderConvs() {
  const list = [...state.chats.values()].sort((a, b) => (b.last?.at || 0) - (a.last?.at || 0));
  $('#convs-empty').hidden = list.length > 0;
  $('#convs').hidden = list.length === 0;
  $('#convs').innerHTML = list.map((c) => {
    const g = c.group;
    const online = !g && !!peerByFp(c.fingerprint);
    const lastText = c.last ? c.last.text.replace(/\s+/g, ' ') : '';
    const who = !c.last || c.last.system ? '' : c.last.outgoing ? 'Siz: ' : c.last.senderName ? `${c.last.senderName}: ` : '';
    const last = `${who}${lastText}`;
    const avatar = g
      ? `<div class="avatar group" style="background:${grad(c.fingerprint)}">${ic.users}</div>`
      : `<div class="avatar" style="background:${grad(c.fingerprint)}">${face(c.fingerprint, c.name)}</div>`;
    const extra = g ? (c.left ? '<span class="c-count">· ayrıldınız</span>' : `<span class="c-count">· ${g.members.length} kişi</span>`) : '';
    return `
      <div class="conv ${c.unread ? 'unread' : ''} ${c.left ? 'left' : ''}" data-fp="${esc(c.fingerprint)}">
        ${avatar}
        <div class="c-name">${online ? '<span class="online-dot" title="Çevrimiçi"></span>' : ''}<span>${esc(c.name)}</span>${extra}</div>
        <div class="c-time">${c.last ? esc(shortTime(c.last.at)) : ''}</div>
        <div class="c-last">${typingText(c.fingerprint) ? `<span class="typing">${esc(typingText(c.fingerprint))}</span>` : esc(last)}</div>
        <div>${c.unread ? `<span class="dot-badge">${c.unread}</span>` : ''}</div>
      </div>`;
  }).join('');
  const total = list.reduce((s, c) => s + (c.unread || 0), 0);
  $('#chat-badge').hidden = total === 0;
  $('#chat-badge').textContent = total > 99 ? '99+' : total;
}
$('#convs').addEventListener('click', (e) => {
  const el = e.target.closest('.conv');
  if (el) openChat(el.dataset.fp);
});

async function openChat(fp, name) {
  closeEmoji();
  if (!state.chats.has(fp)) {
    state.chats.set(fp, { fingerprint: fp, name: name || peerByFp(fp)?.name || 'Cihaz', unread: 0, last: null });
  }
  state.openChat = fp;
  // Ortak klasör / oylama ekranı açıksa kapat (aynı paneli kullanırlar).
  state.openShare = null;
  $('#share-view').hidden = true;
  state.openPoll = null;
  $('#poll-view').hidden = true;
  $('#chat-view').hidden = false;
  $('#composer').value = '';
  autoGrow();
  renderChatHead();
  $('#messages').innerHTML = '';
  const hist = await api.invoke('chat_history', { fingerprint: fp }).catch(() => []);
  if (state.openChat !== fp) return;
  state.history.set(fp, hist);
  renderMessages(true);
  markRead(fp);
  if (!$('#composer').disabled) $('#composer').focus();
}

function closeChat() {
  state.openChat = null;
  $('#chat-view').hidden = true;
  setTab('chats');
  renderConvs();
}
$('#chat-back').onclick = closeChat;

function markRead(fp) {
  if (lockVisible()) return; // kilitliyken görülmemiş mesajlar okundu sayılmaz
  const c = state.chats.get(fp);
  if (!c?.unread) return;
  c.unread = 0;
  api.invoke('chat_mark_read', { fingerprint: fp }).catch(() => {});
  renderConvs();
  renderPeers();
}
window.addEventListener('focus', () => { if (chatOpen()) markRead(state.openChat); });

const myFp = () => state.device?.fingerprint;
const groupOf = (key) => state.chats.get(key)?.group || null;

// "Yazıyor…": sohbet anahtarı → { name, fp, until }
const typingText = (key) => {
  const t = state.typing?.get(key);
  if (!t || t.until < Date.now()) return '';
  return groupOf(key) ? `${t.name} yazıyor…` : 'yazıyor…';
};

function onTyping({ key, fingerprint, name }) {
  state.typing ||= new Map();
  state.typing.set(key, { name, fp: fingerprint, until: Date.now() + 6000 });
  renderChatHead();
  renderConvs();
  setTimeout(() => { renderChatHead(); renderConvs(); }, 6100);
}

function renderChatHead() {
  renderChatHeadBase();
  const t = state.openChat && typingText(state.openChat);
  if (t) $('#chat-status').innerHTML = `<span class="typing">${esc(t)}</span>`;
}

function renderChatHeadBase() {
  const fp = state.openChat;
  if (!fp) return;
  const conv = state.chats.get(fp);
  const g = conv?.group;
  $('#chat-members').hidden = !g;
  $('#chat-leave').hidden = !g || conv.left;
  $('#chat-block').hidden = !!g;
  if (g) {
    const av = $('#chat-avatar');
    av.className = 'avatar sm group';
    av.style.background = grad(fp);
    av.innerHTML = ic.users;
    $('#chat-name').textContent = conv.name;
    const online = g.members.filter((m) => m.fingerprint !== myFp() && peerByFp(m.fingerprint)).length;
    const st = $('#chat-status');
    st.className = 'chat-status';
    st.textContent = conv.left
      ? (g.deleted ? 'Grup silindi' : 'Bu grubun üyesi değilsiniz')
      : `${g.members.length} kişi · ${online} çevrimiçi`;
    $('#chat-leave').title = g.admin === myFp() ? 'Grubu herkes için sil' : 'Gruptan ayrıl';
    const canSend = !conv.left && !g.deleted;
    const ta = $('#composer');
    ta.disabled = !canSend;
    $('#chat-attach').disabled = !canSend;
    $('#chat-emoji').disabled = !canSend;
    if (!canSend) closeEmoji();
    ta.placeholder = canSend ? 'Gruba mesaj yazın…' : 'Bu gruba mesaj gönderemezsiniz';
    $('#chat-send').disabled = !canSend || !ta.value.trim();
    updateComposerNote();
    return;
  }
  $('#chat-avatar').className = 'avatar sm';
  const peer = peerByFp(fp);
  const name = peer?.name || state.chats.get(fp)?.name || '';
  const blocked = isBlocked(fp);
  const av = $('#chat-avatar');
  av.style.background = grad(fp);
  av.innerHTML = face(fp, name);
  $('#chat-name').textContent = name;
  const st = $('#chat-status');
  st.className = `chat-status${peer && !blocked ? ' on' : ''}`;
  st.innerHTML = blocked ? 'Engellendi' : peer ? `<span class="online-dot"></span>Çevrimiçi · ${esc(peer.ip)}` : 'Çevrimdışı';
  const bb = $('#chat-block');
  bb.innerHTML = ic.ban;
  bb.classList.toggle('on', blocked);
  bb.title = blocked ? 'Engeli kaldır' : 'Engelle (bu cihazdan mesaj ve dosya alma)';
  const canSend = !!peer && !blocked;
  const ta = $('#composer');
  ta.disabled = !canSend;
  $('#chat-attach').disabled = !canSend;
  $('#chat-emoji').disabled = !canSend;
  if (!canSend) closeEmoji();
  ta.placeholder = canSend ? 'Mesaj yazın…' : blocked ? 'Bu cihazı engellediniz' : 'Cihaz çevrimdışı, mesaj gönderilemez';
  $('#chat-send').disabled = !canSend || !ta.value.trim();
  updateComposerNote();
}

function statusMeta(m) {
  // Grup mesajı: kaç kişiye ulaştı? Çevrimdışı olanlara geri gelince otomatik gider.
  if (m.recipients) {
    const waiting = m.pending?.length || 0;
    if (!waiting) return `<span title="Herkese iletildi">${ic.checks}</span>`;
    return `<span title="Çevrimdışı üyelere geri geldiklerinde otomatik iletilecek">${ic.clock}</span> ${m.recipients - waiting}/${m.recipients}`;
  }
  if (m.status === 'sending') return `<span title="Gönderiliyor">${ic.clock}</span>`;
  if (m.status === 'sent') return `<span title="İletildi">${ic.checks}</span>`;
  if (m.status === 'failed') {
    return `· <span class="err" title="${esc(m.error || '')}">Gönderilemedi</span> · <button data-retry="${esc(m.id)}">Tekrar dene</button>`;
  }
  return '';
}

// Dosya balonu: durum ve ilerleme bağlı aktarım kayıtlarından (state.transfers) okunur.
function fileBubbleInner(m) {
  const f = m.file;
  const ts = (f.transfers || []).map((r) => state.transfers.get(r.id)).filter(Boolean);
  const icon = f.count > 1 ? ic.folder : ic.file;
  let status = '';
  let bar = '';
  let actions = '';
  if (m.recipients) {
    // Grup gönderimi: kaç kişiye ulaştı?
    const done = ts.filter((t) => t.status === 'done').length;
    const active = ts.filter((t) => t.status === 'active' || t.status === 'waiting').length;
    const waiting = m.pending?.length || 0;
    const parts = [`${done}/${m.recipients} kişi aldı`];
    if (active) parts.push(`${active} gönderiliyor`);
    if (waiting) parts.push(`${waiting} kişi çevrimdışı · gelince gönderilecek`);
    status = m.error ? `<span class="err">${esc(m.error)}</span>` : esc(parts.join(' · '));
  } else if (ts.length) {
    const t = ts[0];
    const pct = t.total > 0 ? Math.floor((t.done / t.total) * 100) : 100;
    const label = {
      waiting: t.error || (t.direction === 'send' ? 'Onay bekleniyor…' : 'Hazırlanıyor…'),
      active: `%${pct}${t.speed > 0 ? ` · ${fmtSpeed(t.speed)}` : ''}`,
      done: t.direction === 'send' ? 'Gönderildi' : 'Alındı',
      declined: t.error || 'Reddedildi',
      cancelled: t.error || 'İptal edildi',
      paused: 'Duraklatıldı',
      failed: t.error || 'Başarısız oldu',
    }[t.status] || '';
    status = `<span class="fb-st ${esc(t.status)}">${esc(label)}</span>`;
    if (t.status === 'active' || t.status === 'paused') bar = `<div class="fb-bar"><i style="width:${pct}%"></i></div>`;
    if (t.status === 'active' || t.status === 'waiting') {
      actions = `<button class="icon-btn sm" data-fact="cancel" data-tid="${esc(t.id)}" title="İptal et">${ic.x}</button>`;
    } else if (t.status === 'done' && t.direction === 'receive' && t.savePath) {
      actions = `<button class="icon-btn sm" data-fact="reveal" data-tid="${esc(t.id)}" title="Klasörde göster">${ic.reveal}</button>`;
    } else if (t.direction === 'send' && t.sources?.length && ['failed', 'paused', 'declined', 'cancelled'].includes(t.status)) {
      actions = t.status === 'paused'
        ? `<button class="icon-btn sm" data-fact="retry" data-tid="${esc(t.id)}" title="Devam ettir">${ic.play}</button>`
        : `<button class="icon-btn sm" data-fact="retry" data-tid="${esc(t.id)}" title="Tekrar dene">${ic.retry}</button>`;
    }
  }
  return `
    <div class="fb-row">
      <span class="fb-icon">${icon}</span>
      <div class="fb-main">
        <div class="fb-title" title="${esc(f.title)}">${esc(f.title)}</div>
        <div class="fb-sub">${f.count} dosya · ${fmtBytes(f.total)}</div>
      </div>
      ${actions}
    </div>
    ${bar}
    ${status ? `<div class="fb-status">${status}</div>` : ''}`;
}

// Açık sohbetteki, bu aktarıma bağlı dosya balonlarını yeniler.
function refreshFileBubbles(tid) {
  if (!chatOpen()) return;
  for (const m of state.history.get(state.openChat) || []) {
    if (!m.file?.transfers?.some((r) => r.id === tid)) continue;
    const el = document.querySelector(`.msg[data-id="${CSS.escape(m.id)}"] .file-bubble`);
    if (el) el.innerHTML = fileBubbleInner(m);
  }
}

function renderMessages(forceBottom = false) {
  const box = $('#messages');
  const list = state.history.get(state.openChat) || [];
  const nearBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 80;
  if (!list.length) {
    box.innerHTML = `<div class="empty small-empty"><h3>Henüz mesaj yok</h3>
      <p>Mesajlar şifreli olarak doğrudan karşı bilgisayara gider, hiçbir sunucuya uğramaz.</p></div>`;
    return;
  }
  let html = '';
  let lastDay = '';
  let prev = null; // önceki balonun yönü + göndereni (grupta isim tekrarını önlemek için)
  for (const m of list) {
    const day = new Date(m.at).toDateString();
    if (day !== lastDay) {
      html += `<div class="day-sep">${esc(dayLabel(m.at))}</div>`;
      lastDay = day;
      prev = null;
    }
    if (m.system) {
      html += `<div class="sys-msg">${esc(m.text)}</div>`;
      prev = null;
      continue;
    }
    const dir = m.outgoing ? 'out' : 'in';
    const who = `${dir}:${m.sender || ''}`;
    const first = prev !== who;
    const sender = !m.outgoing && m.senderName && first
      ? `<div class="sender" style="color:hsl(${hue(m.sender)} 65% 55%)">${esc(m.senderName)}</div>`
      : '';
    if (m.file) {
      html += `
      <div class="msg ${dir}${first ? ' first' : ''} file-msg" data-id="${esc(m.id)}">
        ${sender}
        <div class="bubble file-bubble">${fileBubbleInner(m)}</div>
        <div class="meta">${esc(fmtTime(m.at))}</div>
      </div>`;
      prev = who;
      continue;
    }
    html += `
      <div class="msg ${dir}${first ? ' first' : ''}${m.status === 'failed' ? ' failed' : ''}" data-id="${esc(m.id)}">
        ${sender}
        <div class="bubble-row"><div class="bubble">${esc(m.text)}</div><button class="copy-btn" data-copy title="Kopyala">${ic.copy}</button></div>
        <div class="meta">${esc(fmtTime(m.at))}${m.outgoing ? statusMeta(m) : ''}</div>
      </div>`;
    prev = who;
  }
  box.innerHTML = html;
  if (forceBottom || nearBottom) box.scrollTop = box.scrollHeight;
}
$('#messages').addEventListener('click', async (e) => {
  const fb = e.target.closest('[data-fact]');
  if (fb) {
    const t = state.transfers.get(fb.dataset.tid);
    if (fb.dataset.fact === 'cancel') call('cancel_transfer', { id: fb.dataset.tid }).catch(() => {});
    if (fb.dataset.fact === 'retry') call('retry_transfer', { id: fb.dataset.tid }).catch(() => {});
    if (fb.dataset.fact === 'reveal' && t?.savePath) call('reveal_path', { path: t.savePath }).catch(() => {});
    return;
  }
  const b = e.target.closest('[data-retry]');
  if (b && state.openChat) call('chat_retry', { fingerprint: state.openChat, id: b.dataset.retry }).catch(() => {});
  if (e.target.closest('[data-copy]')) {
    const id = e.target.closest('.msg')?.dataset.id;
    const m = (state.history.get(state.openChat) || []).find((x) => x.id === id);
    if (!m) return;
    try {
      await navigator.clipboard.writeText(m.text);
      toast('Mesaj panoya kopyalandı', { type: 'success', timeout: 2000 });
    } catch {
      toast('Panoya kopyalanamadı', { type: 'error' });
    }
  }
});

function autoGrow() {
  const ta = $('#composer');
  ta.style.height = 'auto';
  ta.style.height = `${Math.min(ta.scrollHeight, 130)}px`;
}

function updateComposerNote() {
  const n = $('#composer').value.length;
  const note = $('#composer-note');
  note.classList.toggle('warn', n > 3800);
  note.textContent = n > 3500 ? `${n} / 4000 karakter` : $('#composer').disabled ? '' : 'Enter: gönder · Shift+Enter: yeni satır';
}

async function sendChat() {
  closeEmoji();
  const ta = $('#composer');
  const text = ta.value.trim();
  const key = state.openChat;
  if (!text || ta.disabled || !key) return;
  const peer = peerByFp(key);
  if (!groupOf(key) && !peer) return;
  try {
    if (groupOf(key)) await call('group_send', { key, text });
    else await call('chat_send', { peerId: peer.id, text });
    ta.value = '';
    autoGrow();
    updateComposerNote();
    $('#chat-send').disabled = true;
  } catch { /* toast gösterildi */ }
}

let lastTypingSent = 0;
$('#composer').addEventListener('input', () => {
  // Karşı tarafa "yazıyor…" sinyali (en fazla 2,5 sn'de bir).
  if (state.openChat && $('#composer').value.trim() && Date.now() - lastTypingSent > 2500) {
    lastTypingSent = Date.now();
    api.invoke('send_typing', { key: state.openChat }).catch(() => {});
  }
  autoGrow();
  updateComposerNote();
  $('#chat-send').disabled = $('#composer').disabled || !$('#composer').value.trim();
});
$('#composer').addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    sendChat();
  }
});
$('#chat-send').onclick = sendChat;
// Açık sohbete dosya/klasör gönderir: birebir sohbette o kişiye, grupta tüm üyelere
// (çevrimdışı üyelere geri geldiklerinde).
async function sendToChat(paths, target = state.openChat) {
  const key = target;
  if (!key || !paths?.length) return;
  if (groupOf(key)) {
    try {
      await call('group_send_files', { key, paths });
    } catch { /* toast gösterildi */ }
    return;
  }
  const peer = peerByFp(key);
  if (!peer) return toast('Bu cihaz çevrimdışı, dosya gönderilemez', { type: 'error' });
  sendPaths(peer, paths);
}

// ───────── Emoji ─────────
const EMOJI_SETS = [
  ['Yüzler', '😀 😃 😄 😁 😆 😅 😂 🤣 😊 😇 🙂 😉 😍 🥰 😘 😋 😜 🤪 🤗 🤔 🤨 😐 😑 😶 🙄 😏 😬 😌 😔 😴 😷 🤒 🤯 🥳 😎 🤓 😕 🙁 😮 😲 😳 🥺 😢 😭 😱 😤 😡'],
  ['Eller', '👍 👎 👌 ✌️ 🤞 🤝 🙏 👏 🙌 👋 💪 ✍️ 👀 🫡 🤷 🙋'],
  ['Kalpler ve semboller', '❤️ 🧡 💛 💚 💙 💜 🖤 🤍 💯 ✅ ❌ ⚠️ ❗ ❓ ⭐ ✨ 🔥 💡 🎉 🎊 🚀 ⏰ 📌 🔒'],
  ['İş', '📁 📂 📄 📎 📊 📈 📉 🗂️ 🗓️ 📅 ✉️ 📧 💻 🖥️ 🖨️ ⌨️ 📱 ☎️ 💾 🔗 🧾 💼 ☕ 🍕'],
];
function recentEmoji() {
  try { return JSON.parse(localStorage.getItem('ff-emoji') || '[]'); } catch { return []; }
}
function rememberEmoji(e) {
  try { localStorage.setItem('ff-emoji', JSON.stringify([e, ...recentEmoji().filter((x) => x !== e)].slice(0, 16))); } catch { /* yok say */ }
}
function renderEmoji() {
  const recent = recentEmoji();
  const sets = recent.length ? [['Son kullanılanlar', recent.join(' ')], ...EMOJI_SETS] : EMOJI_SETS;
  $('#emoji-pop').innerHTML = sets.map(([title, list]) => `
    <h4>${esc(title)}</h4>
    <div class="emoji-grid">${list.split(' ').filter(Boolean).map((e) => `<button data-emoji="${esc(e)}">${e}</button>`).join('')}</div>`).join('');
}
const emojiOpen = () => !$('#emoji-pop').hidden;
function closeEmoji() { $('#emoji-pop').hidden = true; }
$('#chat-emoji').onclick = (e) => {
  e.stopPropagation();
  if ($('#composer').disabled) return;
  if (emojiOpen()) return closeEmoji();
  renderEmoji();
  $('#emoji-pop').hidden = false;
};
$('#emoji-pop').addEventListener('click', (e) => {
  e.stopPropagation();
  const b = e.target.closest('[data-emoji]');
  if (!b) return;
  const ta = $('#composer');
  if (ta.disabled) return closeEmoji();
  ta.setRangeText(b.dataset.emoji, ta.selectionStart, ta.selectionEnd, 'end');
  ta.dispatchEvent(new Event('input'));
  ta.focus();
  rememberEmoji(b.dataset.emoji);
});
document.addEventListener('click', (e) => { if (emojiOpen() && !e.target.closest('#emoji-pop')) closeEmoji(); });
window.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && emojiOpen()) { closeEmoji(); e.stopImmediatePropagation(); }
}, true);

$('#chat-attach').onclick = async () => {
  if ($('#composer').disabled) return;
  const key = state.openChat; // seçim penceresi açıkken sohbet değişirse yanlış kişiye gitmesin
  const sel = await api.pick({ multiple: true, directory: false, title: 'Sohbete gönderilecek dosyalar' });
  if (sel) sendToChat(Array.isArray(sel) ? sel : [sel], key);
};

// Sohbette ekran görüntüsü veya Gezgin'den kopyalanan dosya yapıştırılırsa dosya olarak gönderilir.
$('#composer').addEventListener('paste', (e) => {
  const types = [...(e.clipboardData?.types || [])];
  if (!types.includes('Files') || types.includes('text/plain')) return;
  const key = state.openChat;
  const peer = key && !groupOf(key) ? peerByFp(key) : null;
  e.preventDefault();
  if (!peer) return toast('Gruba yapıştırarak gönderilemiyor; ataç düğmesini ya da sürükle-bırakı kullanın', { type: 'error' });
  sendClipboard([peer.id]);
});

function confirmDialog(title, text, okLabel, danger = false) {
  return new Promise((resolve) => {
    let answered = false;
    const done = (v) => { answered = true; closeModal(); resolve(v); };
    const m = openModal('confirm', `
      <h3>${esc(title)}</h3>
      <p class="lead">${esc(text)}</p>
      <div class="foot">
        <button class="btn ghost" data-no>Vazgeç</button>
        <button class="btn ${danger ? 'danger' : ''}" data-yes autofocus>${esc(okLabel)}</button>
      </div>`, { onClose: () => { if (!answered) resolve(false); } });
    m.querySelector('[data-no]').onclick = () => done(false);
    m.querySelector('[data-yes]').onclick = () => done(true);
  });
}

$('#chat-block').onclick = async () => {
  const fp = state.openChat;
  if (!fp) return;
  const name = peerByFp(fp)?.name || state.chats.get(fp)?.name || 'Bu cihaz';
  const blocked = isBlocked(fp);
  if (!blocked && !(await confirmDialog(`${name} engellensin mi?`,
    'Bu cihazdan gelen mesajlar ve dosyalar otomatik olarak reddedilir. Engeli istediğiniz zaman kaldırabilirsiniz.', 'Engelle', true))) return;
  state.settings = await call('set_blocked', { fingerprint: fp, name, blocked: !blocked });
  renderChatHead();
  toast(blocked ? `${name} için engel kaldırıldı` : `${name} engellendi`, { type: 'success' });
};

$('#chat-clear').onclick = async () => {
  const fp = state.openChat;
  if (!fp) return;
  if (!(await confirmDialog('Sohbet geçmişi silinsin mi?',
    'Yalnızca bu bilgisayardaki kopya silinir; karşı taraftaki mesajlar etkilenmez.', 'Sil', true))) return;
  await call('chat_clear', { fingerprint: fp });
  const conv = state.chats.get(fp);
  if (conv?.group && !conv.left) {
    // Etkin grupta yalnızca mesajlar silinir, grup kalır.
    state.history.set(fp, (state.history.get(fp) || []).filter((m) => m.outgoing && m.pending?.length));
    conv.last = null;
    conv.unread = 0;
    renderMessages(true);
    renderConvs();
    return;
  }
  state.history.delete(fp);
  state.chats.delete(fp);
  closeChat();
  renderPeers();
};

// ───────────────────────────── Gruplar ─────────────────────────────

/// Gruba eklenebilecek kişiler: çevrimiçi cihazlar + daha önce yazışılan (çevrimdışı) kişiler.
function candidatePeople(exclude = new Set()) {
  const map = new Map();
  state.peers.filter((p) => !p.blocked).forEach((p) => map.set(p.fingerprint, { fingerprint: p.fingerprint, name: p.name, online: true }));
  state.chats.forEach((c) => {
    if (!c.group && !map.has(c.fingerprint) && !isBlocked(c.fingerprint)) {
      map.set(c.fingerprint, { fingerprint: c.fingerprint, name: c.name, online: false });
    }
  });
  return [...map.values()]
    .filter((p) => !exclude.has(p.fingerprint))
    .sort((a, b) => (b.online - a.online) || a.name.localeCompare(b.name, 'tr'));
}

function personRow(p, checked) {
  return `
    <label class="recip ${checked ? 'on' : ''}" data-fp="${esc(p.fingerprint)}">
      <input type="checkbox" ${checked ? 'checked' : ''} />
      <span class="avatar" style="background:${grad(p.fingerprint)}">${face(p.fingerprint, p.name)}</span>
      <span class="r-name">${esc(p.name)}</span>
      ${p.online ? '<span class="online-dot" title="Çevrimiçi"></span>' : '<span class="off">çevrimdışı</span>'}
    </label>`;
}

function showGroupCreate() {
  const selected = new Set();
  const m = openModal('group-new', `
    <h3>Yeni grup</h3>
    <p class="lead">Grup mesajları her üyeye şifreli olarak ayrı ayrı gider. Çevrimdışı üyeler, geri geldiklerinde mesajları otomatik alır.</p>
    <div class="field">
      <label for="g-name">Grup adı</label>
      <input class="input" id="g-name" maxlength="40" placeholder="ör. Proje Ekibi" autofocus />
    </div>
    <div class="field">
      <label>Üyeler</label>
      <div class="recips" id="g-people"></div>
      <div class="err" id="g-err"></div>
    </div>
    <div class="foot">
      <button class="btn ghost" data-close>Vazgeç</button>
      <button class="btn" id="g-create" disabled>Grubu oluştur</button>
    </div>`);
  const people = candidatePeople();
  const render = () => {
    m.querySelector('#g-people').innerHTML = people.length
      ? people.map((p) => personRow(p, selected.has(p.fingerprint))).join('')
      : '<span class="muted small">Eklenecek kimse yok. Ağda başka bir FastFiles görünmeli.</span>';
    const name = m.querySelector('#g-name').value.trim();
    const btn = m.querySelector('#g-create');
    btn.disabled = !name || !selected.size;
    btn.textContent = selected.size ? `Grubu oluştur (${selected.size + 1} kişi)` : 'Grubu oluştur';
  };
  m.querySelector('#g-people').addEventListener('change', (e) => {
    const fp = e.target.closest('.recip')?.dataset.fp;
    if (!fp) return;
    e.target.checked ? selected.add(fp) : selected.delete(fp);
    render();
  });
  m.querySelector('#g-name').addEventListener('input', render);
  m.querySelector('[data-close]').onclick = closeModal;
  m.querySelector('#g-create').onclick = async () => {
    try {
      const summary = await api.invoke('group_create', { name: m.querySelector('#g-name').value, members: [...selected] });
      state.chats.set(summary.fingerprint, summary);
      closeModal();
      setTab('chats');
      renderConvs();
      openChat(summary.fingerprint);
    } catch (e) {
      m.querySelector('#g-err').textContent = String(e);
    }
  };
  render();
}
$('#btn-new-group').onclick = showGroupCreate;

function showGroupMembers() {
  const key = state.openChat;
  const conv = state.chats.get(key);
  const g = conv?.group;
  if (!g) return;
  const isAdmin = g.admin === myFp() && !conv.left && !g.deleted;
  const removing = new Set();
  const adding = new Set();
  const m = openModal('group-members', `
    <h3>${esc(conv.name)}</h3>
    <p class="lead">${isAdmin ? 'Grubun yöneticisi sizsiniz: adı değiştirebilir, üye ekleyip çıkarabilirsiniz.' : 'Grubu yalnızca oluşturan kişi düzenleyebilir.'}</p>
    ${isAdmin ? `<div class="field"><label for="gm-name">Grup adı</label><input class="input" id="gm-name" maxlength="40" value="${esc(conv.name)}" /></div>` : ''}
    <div class="field">
      <label>Üyeler (${g.members.length})</label>
      <div class="member-list" id="gm-list"></div>
    </div>
    ${isAdmin ? '<div class="field"><label>Üye ekle</label><div class="recips" id="gm-add"></div></div>' : ''}
    <div class="err" id="gm-err"></div>
    <div class="foot">
      <button class="btn ghost" data-close>${isAdmin ? 'Vazgeç' : 'Kapat'}</button>
      ${isAdmin ? '<button class="btn" id="gm-save">Kaydet</button>' : ''}
    </div>`);
  const render = () => {
    m.querySelector('#gm-list').innerHTML = g.members.map((mem) => {
      const me = mem.fingerprint === myFp();
      const online = me || !!peerByFp(mem.fingerprint);
      return `
        <div class="member ${removing.has(mem.fingerprint) ? 'removing' : ''}">
          <span class="avatar" style="background:${grad(mem.fingerprint)}">${face(mem.fingerprint, mem.name)}</span>
          <span class="m-name">${esc(mem.name)}${me ? ' <span class="muted small">(siz)</span>' : ''}
            ${mem.fingerprint === g.admin ? '<span class="badge admin">Yönetici</span>' : ''}</span>
          ${online ? '<span class="online-dot" title="Çevrimiçi"></span>' : '<span class="muted small">çevrimdışı</span>'}
          ${isAdmin && !me ? `<button class="icon-btn sm" data-rm="${esc(mem.fingerprint)}" title="${removing.has(mem.fingerprint) ? 'Geri al' : 'Gruptan çıkar'}">${ic.x}</button>` : ''}
        </div>`;
    }).join('');
    if (isAdmin) {
      const people = candidatePeople(new Set(g.members.map((x) => x.fingerprint)));
      m.querySelector('#gm-add').innerHTML = people.length
        ? people.map((p) => personRow(p, adding.has(p.fingerprint))).join('')
        : '<span class="muted small">Eklenebilecek başka kimse yok.</span>';
    }
  };
  m.querySelector('#gm-list').addEventListener('click', (e) => {
    const b = e.target.closest('[data-rm]');
    if (!b) return;
    removing.has(b.dataset.rm) ? removing.delete(b.dataset.rm) : removing.add(b.dataset.rm);
    render();
  });
  m.querySelector('#gm-add')?.addEventListener('change', (e) => {
    const fp = e.target.closest('.recip')?.dataset.fp;
    if (!fp) return;
    e.target.checked ? adding.add(fp) : adding.delete(fp);
    render();
  });
  m.querySelector('[data-close]').onclick = closeModal;
  m.querySelector('#gm-save')?.addEventListener('click', async () => {
    try {
      const summary = await api.invoke('group_edit', {
        key, name: m.querySelector('#gm-name').value, add: [...adding], remove: [...removing],
      });
      state.chats.set(key, { ...state.chats.get(key), ...summary });
      closeModal();
      renderChatHead();
      renderConvs();
    } catch (e) {
      m.querySelector('#gm-err').textContent = String(e);
    }
  });
  render();
}
$('#chat-members').onclick = showGroupMembers;

$('#chat-leave').onclick = async () => {
  const key = state.openChat;
  const conv = state.chats.get(key);
  const g = conv?.group;
  if (!g) return;
  const admin = g.admin === myFp();
  const ok = await confirmDialog(
    admin ? `“${conv.name}” herkes için silinsin mi?` : `“${conv.name}” grubundan ayrılınsın mı?`,
    admin
      ? 'Grubu siz oluşturdunuz. Silerseniz grup tüm üyeler için kapanır; mesaj geçmişi herkesin bilgisayarında kalır.'
      : 'Artık bu gruptan mesaj almazsınız. Geçmiş bu bilgisayarda kalır.',
    admin ? 'Grubu sil' : 'Ayrıl', true);
  if (!ok) return;
  await call('group_leave', { key });
};

function onConversation(summary) {
  const prev = state.chats.get(summary.fingerprint) || {};
  state.chats.set(summary.fingerprint, { ...prev, ...summary });
  renderConvs();
  if (state.openChat === summary.fingerprint) renderChatHead();
}

// ───────────────────────────── Bildirime tıklama ─────────────────────────────

function onNotificationClick(target = {}) {
  // Kilitliyken hiçbir ekran açılmaz; kilit açılınca kullanıcı kendisi bakar.
  if (lockVisible()) return;
  if (modalKind && modalKind !== 'incoming') closeModal();
  if (target.view === 'chat' && target.key) {
    openChat(target.key);
  } else if (target.view === 'polls') {
    if (chatOpen()) closeChat();
    if (shareOpen()) closeShare();
    if (target.id && state.polls.some((p) => p.id === target.id)) openPoll(target.id);
    else setTab('polls');
  } else if (target.view === 'transfers') {
    if (chatOpen()) closeChat();
    setTab('transfers');
    const el = target.transferId && document.getElementById(`t-${target.transferId}`);
    if (el) {
      el.scrollIntoView({ block: 'nearest' });
      el.classList.remove('flash');
      void el.offsetWidth;
      el.classList.add('flash');
    }
  }
}

function onChat({ fingerprint: fp, name, unread, incoming, message }) {
  // Mesaj geldi: o kişinin "yazıyor…" göstergesi kalkar.
  if (incoming && state.typing?.get(fp) && (!message.sender || state.typing.get(fp).fp === message.sender)) state.typing.delete(fp);
  const c = state.chats.get(fp) || { fingerprint: fp, name, unread: 0, last: null };
  c.name = name;
  c.unread = unread;
  if (!c.last || c.last.id === message.id || message.at >= c.last.at) c.last = message;
  state.chats.set(fp, c);
  const hist = state.history.get(fp);
  if (hist) {
    const i = hist.findIndex((m) => m.id === message.id);
    if (i >= 0) hist[i] = message; else hist.push(message);
  }
  const viewing = state.openChat === fp && chatOpen();
  if (viewing) {
    renderMessages(!incoming);
    renderChatHead();
    if (incoming && document.hasFocus()) markRead(fp);
  } else if (incoming) {
    const who = message.senderName ? `${name} · ${message.senderName}` : name;
    toast(`${who}: ${message.text.replace(/\s+/g, ' ').slice(0, 80)}`, {
      action: 'Aç', timeout: 6000, onAction: () => openChat(fp, name),
    });
  }
  renderConvs();
  renderPeers();
}

// ───────────────────────────── Güncelleme ─────────────────────────────

function renderUpdateBar() {
  const ready = state.update?.ready;
  const bar = $('#update-bar');
  bar.hidden = !ready || state.updateDismissed === ready;
  if (ready) {
    $('#update-text').innerHTML = `FastFiles <b>v${esc(ready)}</b> hazır. Aktarım yokken kendiliğinden kurulacak; uygulama birkaç saniyeliğine yeniden başlar.`;
  }
}

$('#update-now').onclick = async () => {
  $('#update-now').disabled = true;
  $('#update-now').textContent = 'Kuruluyor…';
  try {
    await call('install_update');
  } catch {
    $('#update-now').disabled = false;
    $('#update-now').textContent = 'Şimdi güncelle';
  }
};
$('#update-later').onclick = async () => {
  await api.invoke('postpone_update').catch(() => {});
  state.updateDismissed = state.update?.ready;
  renderUpdateBar();
  toast('Güncelleme 2 saat ertelendi', { type: 'info' });
};

async function importUpdate(path) {
  try {
    state.update = await call('import_update', { path });
    renderUpdateBar();
    toast(state.update.ready
      ? `v${state.update.ready} paketi doğrulandı. Ağdaki diğer cihazlara da dağıtılacak.`
      : `v${state.update.serving} paketi doğrulandı ve ağa dağıtılıyor.`, { type: 'success', timeout: 7000 });
  } catch { /* toast gösterildi */ }
}

// ───────────────────────────── Profil resmi ─────────────────────────────

/** Yüklenmiş resmi ortadan kare kırpar, küçültür ve JPEG data URL'sine çevirir (≤ ~90 KB). */
function imageToSquare(img, size) {
  const s = Math.min(img.naturalWidth, img.naturalHeight);
  if (!s) throw new Error('Resim boş görünüyor');
  const canvas = document.createElement('canvas');
  canvas.width = canvas.height = size;
  const ctx = canvas.getContext('2d');
  ctx.fillStyle = '#fff';
  ctx.fillRect(0, 0, size, size);
  ctx.imageSmoothingQuality = 'high';
  ctx.drawImage(img, (img.naturalWidth - s) / 2, (img.naturalHeight - s) / 2, s, s, 0, 0, size, size);
  for (const q of [0.88, 0.75, 0.6, 0.45]) {
    const url = canvas.toDataURL('image/jpeg', q);
    if (url.length * 0.75 < 90 * 1024) return url;
  }
  throw new Error('Resim küçültülemedi');
}

function loadImage(src) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error('Bu resim biçimi desteklenmiyor. JPEG, PNG, WebP, GIF veya BMP kullanın.'));
    img.src = src;
  });
}

/** Seçilen dosyayı profil resmine çevirir. Dosya data: URL olarak okunur
 *  (uygulamanın güvenlik politikası blob: adreslerinden resim yüklemeye izin vermez). */
async function squareImage(file, size) {
  if (file.size > 25 * 1024 * 1024) throw new Error('Resim çok büyük (en fazla 25 MB)');
  const dataUrl = await new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(r.result);
    r.onerror = () => reject(new Error('Dosya okunamadı'));
    r.readAsDataURL(file);
  });
  return imageToSquare(await loadImage(dataUrl), size);
}

/** Hazır avatar (uygulamayla gelen resim) → profil resmi. */
async function presetToDataUrl(file, size) {
  return imageToSquare(await loadImage(`avatars/${file}`), size);
}

// ───────────────────────────── Ortak klasörler ─────────────────────────────

const folderIcon = '<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>';
const lockIcon = '<svg viewBox="0 0 24 24"><rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/></svg>';
const shareById = (id) => state.shares.find((s) => s.id === id);
const shareOpen = () => !!state.openShare && !$('#share-view').hidden;
/** Dosya bırakılınca eklenecek ortak klasör (açık ve kilidi açıksa). */
const shareDropTarget = () => {
  const s = shareOpen() && shareById(state.openShare);
  return s && s.canAccess && !s.left ? s : null;
};

function shareStatus(s) {
  if (s.deleted) return 'Klasör sahibi tarafından kapatıldı';
  if (s.left) return 'Bu klasörden ayrıldınız';
  if (!s.canAccess) return 'Şifreli · kilidi açılmadı';
  const sync = s.syncing ? ` · <span class="sync-tag">${s.syncing} dosya eşitleniyor</span>` : '';
  return `${s.members.length} kişi · ${s.fileCount} dosya · ${fmtBytes(s.totalSize)}${sync}`;
}

function renderShares() {
  const list = state.shares;
  $('#shares-empty').hidden = list.length > 0;
  $('#shares').hidden = list.length === 0;
  $('#shares').innerHTML = list.map((s) => `
    <div class="conv ${s.left ? 'left' : ''}" data-share="${esc(s.id)}">
      <div class="avatar folder" style="background:${grad(s.id)}">${s.protected && !s.canAccess ? lockIcon : folderIcon}</div>
      <div class="c-name"><span>${esc(s.name)}</span>${s.protected ? `<span class="c-count" title="Şifreli">${lockIcon.replace('<svg', '<svg style="width:12px;height:12px"')}</span>` : ''}</div>
      <div class="c-time"></div>
      <div class="c-last">${shareStatus(s)}</div>
      <div><button class="icon-btn sm row-del" data-del="${esc(s.id)}" title="${s.admin === myFp() && !s.left && !s.deleted ? 'Klasörü herkes için sil' : 'Klasörü listeden kaldır'}">${ic.trash}</button></div>
    </div>`).join('');
}
$('#shares').addEventListener('click', (e) => {
  const del = e.target.closest('[data-del]');
  if (del) {
    e.stopPropagation();
    return confirmShareDelete(del.dataset.del);
  }
  const el = e.target.closest('[data-share]');
  if (el) openShare(el.dataset.share);
});

function onShares(list) {
  state.shares = list;
  renderShares();
  if (state.openShare) {
    if (!shareById(state.openShare)) closeShare();
    else renderShareHead();
  }
}

function openShare(id) {
  // Sohbet / oylama açıksa kapat (aynı paneli kullanırlar).
  state.openChat = null;
  $('#chat-view').hidden = true;
  state.openPoll = null;
  $('#poll-view').hidden = true;
  state.openShare = id;
  state.shareFiles = [];
  $('#share-search').value = '';
  $('#share-view').hidden = false;
  renderShareHead();
  loadShareFiles();
}

function closeShare() {
  state.openShare = null;
  $('#share-view').hidden = true;
  setTab('shares');
}
$('#share-back').onclick = closeShare;

function renderShareHead() {
  const s = shareById(state.openShare);
  if (!s) return;
  const icon = $('#share-icon');
  icon.style.background = grad(s.id);
  icon.innerHTML = s.protected && !s.canAccess ? lockIcon : folderIcon;
  $('#share-name').textContent = s.name;
  $('#share-status').innerHTML = s.canAccess && !s.left
    ? `${s.members.length} kişi · ${s.fileCount} dosya${s.syncing ? ` · <span class="sync-tag">${s.syncing} eşitleniyor</span>` : ''}`
    : shareStatus(s);
  const locked = !s.canAccess && !s.left && !s.deleted;
  $('#share-locked').hidden = !locked;
  $('#share-body').hidden = locked;
  const admin = s.admin === myFp();
  $('#share-leave').hidden = false;
  $('#share-leave').title = admin && !s.left ? 'Klasörü herkes için sil' : 'Klasörü sil';
  const canAdd = s.canAccess && !s.left;
  $('#share-add-files').disabled = !canAdd;
  $('#share-add-folder').disabled = !canAdd;
  $('#share-note').textContent = canAdd
    ? 'Dosyaları buraya sürükleyip bırakabilir veya Gezgin\'de klasöre kopyalayabilirsiniz.'
    : s.left ? 'Bu klasör artık eşitlenmiyor; dosyalar bilgisayarınızda kalır.' : '';
}

async function loadShareFiles() {
  const id = state.openShare;
  const s = shareById(id);
  if (!s || (!s.canAccess && !s.left)) {
    state.shareFiles = [];
    renderShareFiles();
    return;
  }
  const files = await api.invoke('share_files', { id }).catch(() => []);
  if (state.openShare !== id) return;
  state.shareFiles = files;
  renderShareFiles();
}

function renderShareFiles() {
  const q = $('#share-search').value.trim().toLocaleLowerCase('tr-TR');
  const files = q ? state.shareFiles.filter((f) => f.path.toLocaleLowerCase('tr-TR').includes(q)) : state.shareFiles;
  const box = $('#share-files');
  if (!files.length) {
    box.innerHTML = `<div class="empty small-empty"><p>${q ? 'Eşleşen dosya yok.' : 'Klasör boş. Eklediğiniz dosyalar üyelerin bilgisayarına otomatik kopyalanır.'}</p></div>`;
    return;
  }
  box.innerHTML = files.slice(0, 500).map((f) => {
    const slash = f.path.lastIndexOf('/');
    const dir = slash >= 0 ? f.path.slice(0, slash + 1) : '';
    const name = f.path.slice(slash + 1);
    return `
      <div class="sfile" data-path="${esc(f.fullPath)}" title="${esc(f.path)} — açmak için tıklayın">
        <div class="ficon">${ic.file}</div>
        <div>
          <div class="fname"><span class="fdir">${esc(dir)}</span>${esc(name)}</div>
          <div class="fmeta">${esc(f.authorName)} · ${esc(shortTime(f.mtime))} · ${fmtBytes(f.size)}</div>
        </div>
        <button class="icon-btn sm fact" data-reveal="${esc(f.fullPath)}" title="Klasörde göster">${ic.reveal}</button>
      </div>`;
  }).join('') + (files.length > 500 ? `<div class="more muted small" style="padding:8px">+ ${files.length - 500} dosya daha (aramayı daraltın)</div>` : '');
}
$('#share-search').addEventListener('input', renderShareFiles);
$('#share-files').addEventListener('click', (e) => {
  const reveal = e.target.closest('[data-reveal]');
  if (reveal) return call('reveal_path', { path: reveal.dataset.reveal });
  const row = e.target.closest('[data-path]');
  if (row) call('open_path', { path: row.dataset.path });
});
$('#share-open').onclick = () => { const s = shareById(state.openShare); if (s) call('open_path', { path: s.dir }); };

async function addToShare(id, paths) {
  if (!paths?.length) return;
  try {
    const n = await call('share_add_files', { id, paths });
    toast(`${n} dosya ortak klasöre eklendi; üyelere gönderiliyor`, { type: 'success' });
    if (state.openShare === id) loadShareFiles();
  } catch { /* toast gösterildi */ }
}
const pickIntoShare = async (directory) => {
  const id = state.openShare;
  const sel = await api.pick({ multiple: true, directory, title: directory ? 'Ortak klasöre eklenecek klasörler' : 'Ortak klasöre eklenecek dosyalar' });
  if (sel) addToShare(id, Array.isArray(sel) ? sel : [sel]);
};
$('#share-add-files').onclick = () => pickIntoShare(false);
$('#share-add-folder').onclick = () => pickIntoShare(true);

async function unlockShare() {
  const id = state.openShare;
  const pass = $('#share-pass').value;
  if (!pass) return $('#share-pass').focus();
  $('#share-unlock').disabled = true;
  $('#share-err').textContent = '';
  try {
    await api.invoke('share_unlock', { id, password: pass });
    $('#share-pass').value = '';
    toast('Kilit açıldı. Dosyalar eşitleniyor…', { type: 'success' });
  } catch (e) {
    $('#share-err').textContent = String(e);
  } finally {
    $('#share-unlock').disabled = false;
  }
}
$('#share-unlock').onclick = unlockShare;
$('#share-pass').addEventListener('keydown', (e) => e.key === 'Enter' && unlockShare());

$('#share-leave').onclick = () => confirmShareDelete(state.openShare);

function confirmShareDelete(id) {
  const s = shareById(id);
  if (!s) return;
  const admin = s.admin === myFp() && !s.left && !s.deleted;
  const active = !s.left && !s.deleted;
  const m = openModal('share-delete', `
    <h3>“${esc(s.name)}” silinsin mi?</h3>
    <p class="lead">${admin
      ? 'Klasörün sahibi sizsiniz. Silerseniz klasör <b>tüm üyeler için kapanır</b> ve eşitleme durur; çevrimdışı olanlara açıldıklarında bildirilir. Diğer üyelerin bilgisayarındaki kopyalar silinmez.'
      : active
        ? 'Klasörden ayrılırsınız ve listenizden kaldırılır. Diğer üyeler kullanmaya devam eder.'
        : 'Klasör listenizden kaldırılır.'}</p>
    <label class="check"><input type="checkbox" id="sd-files" />
      <span>Bu bilgisayardaki dosyaları da sil<br><span class="muted small path">${esc(s.dir)}</span></span></label>
    <div class="err" id="sd-err"></div>
    <div class="foot">
      <button class="btn ghost" data-close>Vazgeç</button>
      <button class="btn danger" id="sd-go">${admin ? 'Herkes için sil' : 'Sil'}</button>
    </div>`);
  m.querySelector('[data-close]').onclick = closeModal;
  m.querySelector('#sd-go').onclick = async () => {
    const deleteFiles = m.querySelector('#sd-files').checked;
    try {
      await api.invoke('share_delete', { id: s.id, deleteFiles });
      state.shares = state.shares.filter((x) => x.id !== s.id);
      closeModal();
      if (state.openShare === s.id) closeShare();
      renderShares();
      toast(deleteFiles ? 'Klasör ve dosyaları silindi' : 'Klasör silindi; dosyalar bilgisayarınızda duruyor', { type: 'success' });
    } catch (e) {
      m.querySelector('#sd-err').textContent = String(e);
    }
  };
}

function passwordFields(prefix) {
  return `
    <div class="row">
      <input class="input" type="password" id="${prefix}-p1" placeholder="Şifre (en az 4 karakter)" autocomplete="new-password" />
      <input class="input" type="password" id="${prefix}-p2" placeholder="Şifre (tekrar)" autocomplete="new-password" />
    </div>`;
}

function readPassword(m, prefix) {
  const p1 = m.querySelector(`#${prefix}-p1`).value;
  const p2 = m.querySelector(`#${prefix}-p2`).value;
  if (!p1 && !p2) return { password: null };
  if (p1 !== p2) return { error: 'Şifreler aynı değil' };
  if (p1.length < 4) return { error: 'Şifre en az 4 karakter olmalı' };
  return { password: p1 };
}

function showShareCreate() {
  const selected = new Set();
  const m = openModal('share-new', `
    <h3>Yeni ortak klasör</h3>
    <p class="lead">Bu klasöre eklenen dosyalar, seçtiğiniz kişilerin bilgisayarına otomatik kopyalanır ve güncel tutulur. Eklemediğiniz kişiler klasörü göremez.</p>
    <div class="field">
      <label for="sh-name">Klasör adı</label>
      <input class="input" id="sh-name" maxlength="40" placeholder="ör. Proje Dosyaları" autofocus />
    </div>
    <div class="field">
      <label>Üyeler</label>
      <div class="recips" id="sh-people"></div>
    </div>
    <div class="field">
      <label class="check" style="padding:0"><input type="checkbox" id="sh-protect" /> <span>Şifre koy (isteğe bağlı)</span></label>
      <div id="sh-pass" hidden>${passwordFields('sh')}
        <span class="help">Üyeler şifreyi bir kez girer; girmeyenlere dosya gönderilmez. Şifre yalnızca sizin bilgisayarınızda (özet olarak) saklanır.</span>
      </div>
      <div class="err" id="sh-err"></div>
    </div>
    <div class="foot">
      <button class="btn ghost" data-close>Vazgeç</button>
      <button class="btn" id="sh-create" disabled>Klasörü oluştur</button>
    </div>`);
  const people = candidatePeople();
  const render = () => {
    m.querySelector('#sh-people').innerHTML = people.length
      ? people.map((p) => personRow(p, selected.has(p.fingerprint))).join('')
      : '<span class="muted small">Eklenecek kimse yok. Klasörü şimdi oluşturup üyeleri sonra ekleyebilirsiniz.</span>';
    m.querySelector('#sh-create').disabled = !m.querySelector('#sh-name').value.trim();
  };
  m.querySelector('#sh-people').addEventListener('change', (e) => {
    const fp = e.target.closest('.recip')?.dataset.fp;
    if (!fp) return;
    e.target.checked ? selected.add(fp) : selected.delete(fp);
    render();
  });
  m.querySelector('#sh-name').addEventListener('input', render);
  m.querySelector('#sh-protect').onchange = (e) => { m.querySelector('#sh-pass').hidden = !e.target.checked; };
  m.querySelector('[data-close]').onclick = closeModal;
  m.querySelector('#sh-create').onclick = async () => {
    let password = null;
    if (m.querySelector('#sh-protect').checked) {
      const r = readPassword(m, 'sh');
      if (r.error || !r.password) { m.querySelector('#sh-err').textContent = r.error || 'Şifre girin'; return; }
      password = r.password;
    }
    try {
      const s = await api.invoke('share_create', { name: m.querySelector('#sh-name').value, members: [...selected], password });
      if (!shareById(s.id)) state.shares = [...state.shares, s];
      closeModal();
      setTab('shares');
      renderShares();
      openShare(s.id);
    } catch (e) {
      m.querySelector('#sh-err').textContent = String(e);
    }
  };
  render();
}
$('#btn-new-share').onclick = showShareCreate;

function showShareSettings() {
  const s = shareById(state.openShare);
  if (!s) return;
  const isAdmin = s.admin === myFp() && !s.left && !s.deleted;
  const removing = new Set();
  const adding = new Set();
  const m = openModal('share-settings', `
    <h3>${esc(s.name)}</h3>
    <p class="lead">${isAdmin ? 'Klasörün sahibi sizsiniz: adı, üyeleri ve şifreyi yönetebilirsiniz.' : 'Klasörü yalnızca oluşturan kişi düzenleyebilir.'}</p>
    ${isAdmin ? `<div class="field"><label for="ss-name">Klasör adı</label><input class="input" id="ss-name" maxlength="40" value="${esc(s.name)}" /></div>` : ''}
    <div class="field">
      <label>Üyeler (${s.members.length})</label>
      <div class="member-list" id="ss-list"></div>
    </div>
    ${isAdmin ? '<div class="field"><label>Üye ekle</label><div class="recips" id="ss-add"></div></div>' : ''}
    ${isAdmin ? `<div class="field">
      <label>Şifre ${s.protected ? '<span class="badge trusted">açık</span>' : ''}</label>
      ${passwordFields('ss')}
      <span class="help">${s.protected ? 'Yeni şifre yazarsanız değişir. Kilidi açmış üyeler erişmeye devam eder.' : 'Şifre koyarsanız, şu anki üyeler erişmeye devam eder; yeni üyeler şifreyi girmelidir.'}</span>
      ${s.protected ? '<button class="link-btn" id="ss-nopass" style="margin-top:6px">Şifreyi kaldır</button>' : ''}
    </div>` : ''}
    <div class="muted small">Yerel klasör: <span class="path">${esc(s.dir)}</span></div>
    <div class="err" id="ss-err"></div>
    <div class="foot">
      <button class="btn ghost" data-close>${isAdmin ? 'Vazgeç' : 'Kapat'}</button>
      ${isAdmin ? '<button class="btn" id="ss-save">Kaydet</button>' : ''}
    </div>`);
  const render = () => {
    m.querySelector('#ss-list').innerHTML = s.members.map((mem) => {
      const me = mem.fingerprint === myFp();
      const online = me || !!peerByFp(mem.fingerprint);
      return `
        <div class="member ${removing.has(mem.fingerprint) ? 'removing' : ''}">
          <span class="avatar" style="background:${grad(mem.fingerprint)}">${face(mem.fingerprint, mem.name)}</span>
          <span class="m-name">${esc(mem.name)}${me ? ' <span class="muted small">(siz)</span>' : ''}
            ${mem.fingerprint === s.admin ? '<span class="badge admin">Sahip</span>' : ''}</span>
          ${online ? '<span class="online-dot" title="Çevrimiçi"></span>' : '<span class="muted small">çevrimdışı</span>'}
          ${isAdmin && !me ? `<button class="icon-btn sm" data-rm="${esc(mem.fingerprint)}" title="${removing.has(mem.fingerprint) ? 'Geri al' : 'Klasörden çıkar'}">${ic.x}</button>` : ''}
        </div>`;
    }).join('');
    if (isAdmin) {
      const people = candidatePeople(new Set(s.members.map((x) => x.fingerprint)));
      m.querySelector('#ss-add').innerHTML = people.length
        ? people.map((p) => personRow(p, adding.has(p.fingerprint))).join('')
        : '<span class="muted small">Eklenebilecek başka kimse yok.</span>';
    }
  };
  m.querySelector('#ss-list').addEventListener('click', (e) => {
    const b = e.target.closest('[data-rm]');
    if (!b) return;
    removing.has(b.dataset.rm) ? removing.delete(b.dataset.rm) : removing.add(b.dataset.rm);
    render();
  });
  m.querySelector('#ss-add')?.addEventListener('change', (e) => {
    const fp = e.target.closest('.recip')?.dataset.fp;
    if (!fp) return;
    e.target.checked ? adding.add(fp) : adding.delete(fp);
    render();
  });
  m.querySelector('[data-close]').onclick = closeModal;
  m.querySelector('#ss-nopass')?.addEventListener('click', async () => {
    if (!(await confirmDialog('Şifre kaldırılsın mı?', 'Klasördeki tüm üyeler şifre girmeden erişebilir.', 'Kaldır', true))) return;
    await call('share_set_password', { id: s.id, password: null });
    toast('Şifre kaldırıldı', { type: 'success' });
  });
  m.querySelector('#ss-save')?.addEventListener('click', async () => {
    const pw = readPassword(m, 'ss');
    if (pw.error) { m.querySelector('#ss-err').textContent = pw.error; return; }
    try {
      await api.invoke('share_edit', { id: s.id, name: m.querySelector('#ss-name').value, add: [...adding], remove: [...removing] });
      if (pw.password) await api.invoke('share_set_password', { id: s.id, password: pw.password });
      closeModal();
      toast('Klasör güncellendi', { type: 'success' });
    } catch (e) {
      m.querySelector('#ss-err').textContent = String(e);
    }
  });
  render();
}
$('#share-members').onclick = showShareSettings;

// ───────────────────────────── Oylamalar ─────────────────────────────

const pollIcon = '<svg viewBox="0 0 24 24"><path d="M4 20V10M10 20V4M16 20v-7M22 20H2"/></svg>';
const pollById = (id) => state.polls.find((p) => p.id === id);
const pollOpen = () => !!state.openPoll && !$('#poll-view').hidden;
/** Oy bekleyen (açık, katılımcısı olduğum, henüz oy vermediğim) oylama mı? */
const needsMyVote = (p) => p.canVote && p.myVote == null;

function pollWinners(p) {
  const max = Math.max(0, ...p.counts);
  return max > 0 ? p.options.map((o, i) => (p.counts[i] === max ? i : -1)).filter((i) => i >= 0) : [];
}

function pollSummary(p) {
  const total = p.voters.length;
  if (p.closed) {
    const w = pollWinners(p);
    if (!w.length) return 'Sonuçlandı · oy kullanılmadı';
    return w.length === 1 ? `Sonuçlandı · Kazanan: ${p.options[w[0]]}` : `Sonuçlandı · Berabere: ${w.map((i) => p.options[i]).join(', ')}`;
  }
  return `${p.voted.length}/${total} kişi oy kullandı`;
}

function renderPolls() {
  const list = state.polls;
  $('#polls-empty').hidden = list.length > 0;
  $('#polls').hidden = list.length === 0;
  $('#polls').innerHTML = list.map((p) => `
    <div class="conv ${needsMyVote(p) ? 'unread' : ''} ${p.closed ? 'left' : ''}" data-poll="${esc(p.id)}">
      <div class="avatar folder" style="background:${grad(p.id)}">${pollIcon}</div>
      <div class="c-name"><span>${esc(p.question)}</span></div>
      <div class="c-time">${esc(shortTime(p.createdAt))}</div>
      <div class="c-last">${esc(p.isOwner ? 'Siz' : p.ownerName)} · ${esc(pollSummary(p))}</div>
      <div>${needsMyVote(p) ? '<span class="badge vote">Oy verin</span>' : ''}</div>
    </div>`).join('');
  const waiting = list.filter(needsMyVote).length;
  $('#poll-badge').hidden = waiting === 0;
  $('#poll-badge').textContent = waiting;
}
$('#polls').addEventListener('click', (e) => {
  const el = e.target.closest('[data-poll]');
  if (el) openPoll(el.dataset.poll);
});

function onPolls(list) {
  const before = new Map(state.polls.map((p) => [p.id, p]));
  state.polls = list;
  renderPolls();
  // Uygulama öndeyken de haber ver: yeni oylama / sonuçlanan oylama.
  for (const p of list) {
    const old = before.get(p.id);
    if (!old && !p.isOwner && !p.closed) {
      toast(`${p.ownerName} oylama başlattı: ${p.question}`, { action: 'Oy ver', timeout: 8000, onAction: () => openPoll(p.id) });
    } else if (old && !old.closed && p.closed && !p.isOwner) {
      toast(`Oylama sonuçlandı: ${p.question}`, { action: 'Gör', timeout: 8000, onAction: () => openPoll(p.id) });
    }
  }
  if (state.openPoll) {
    if (!pollById(state.openPoll)) closePoll();
    else renderPoll();
  }
}

function openPoll(id) {
  state.openChat = null;
  $('#chat-view').hidden = true;
  state.openShare = null;
  $('#share-view').hidden = true;
  state.openPoll = id;
  setTab('polls');
  $('#poll-view').hidden = false;
  renderPoll();
  const p = pollById(id);
  if (p && !p.seen) api.invoke('poll_mark_seen', { id }).catch(() => {});
}

function closePoll() {
  state.openPoll = null;
  $('#poll-view').hidden = true;
  setTab('polls');
}
$('#poll-back').onclick = closePoll;

function renderPoll() {
  const p = pollById(state.openPoll);
  if (!p) return;
  $('#poll-icon').style.background = grad(p.id);
  $('#poll-status').textContent = p.closed ? 'Sonuçlandı' : `${p.voted.length}/${p.voters.length} oy kullanıldı`;
  $('#poll-question').textContent = p.question;
  $('#poll-meta').innerHTML = `
    <span class="avatar" style="background:${grad(p.owner)}">${face(p.owner, p.ownerName)}</span>
    ${esc(p.isOwner ? 'Siz' : p.ownerName)} açtı · ${esc(shortTime(p.createdAt))}
    ${p.anonymous ? ' · <span title="Kimin neye oy verdiği görünmez">gizli oylama</span>' : ''}`;
  const total = p.counts.reduce((a, b) => a + b, 0);
  const winners = p.closed ? pollWinners(p) : [];
  const nameOf = (fp) => p.voters.find((v) => v.fingerprint === fp)?.name || '?';
  $('#poll-options').innerHTML = p.options.map((o, i) => {
    const count = p.counts[i] || 0;
    const pct = total ? Math.round((count / total) * 100) : 0;
    const names = p.anonymous ? [] : p.ballots.filter((b) => b.option === i).map((b) => nameOf(b.fingerprint));
    return `
      <button class="popt ${p.myVote === i ? 'mine' : ''} ${winners.includes(i) ? 'winner' : ''}" data-opt="${i}" ${p.canVote ? '' : 'disabled'}>
        <span class="fill" style="width:${pct}%"></span>
        <span class="radio"></span>
        <span class="otext">${esc(o)}</span>
        <span class="ocount">${count} oy · %${pct}</span>
        ${names.length ? `<span class="onames">${esc(names.join(', '))}</span>` : ''}
      </button>`;
  }).join('');
  const chips = p.voters.map((v) => `
    <span class="chip ${p.voted.includes(v.fingerprint) ? 'done' : ''}" title="${p.voted.includes(v.fingerprint) ? 'Oy kullandı' : 'Henüz oy kullanmadı'}">
      <span class="avatar" style="background:${grad(v.fingerprint)}">${face(v.fingerprint, v.name)}</span>${esc(v.fingerprint === myFp() ? 'Siz' : v.name)}${p.voted.includes(v.fingerprint) ? ' ✓' : ''}
    </span>`).join('');
  $('#poll-voters').innerHTML = `Katılımcılar <div class="chips">${chips}</div>`;
  let note = '';
  if (p.canVote) note = p.votePending ? 'Oyunuz iletiliyor… (oylamayı açan kişi çevrimiçi olunca ulaşır)' : p.myVote == null ? 'Bir seçeneğe tıklayarak oy verin.' : 'Oy verdiniz. Kapanana kadar değiştirebilirsiniz.';
  else if (!p.closed) note = 'Bu oylamada oy kullanmıyorsunuz; sonucu takip edebilirsiniz.';
  else note = 'Oylama sonuçlandı.';
  $('#poll-foot').innerHTML = `<span>${esc(note)}</span><span class="spacer"></span>
    ${p.isOwner && !p.closed ? '<button class="btn small" id="poll-close">Oylamayı bitir</button>' : ''}`;
  $('#poll-close')?.addEventListener('click', async () => {
    if (!(await confirmDialog('Oylama bitirilsin mi?', 'Artık oy verilemez; sonuç tüm katılımcılara gönderilir.', 'Bitir'))) return;
    await call('poll_close', { id: p.id });
  });
  $('#poll-remove').title = p.isOwner && !p.closed ? 'Oylamayı bitir ve listeden kaldır' : 'Listeden kaldır';
}

$('#poll-options').addEventListener('click', async (e) => {
  const b = e.target.closest('[data-opt]');
  const p = pollById(state.openPoll);
  if (!b || !p || !p.canVote) return;
  const option = Number(b.dataset.opt);
  if (p.myVote === option) return;
  await call('poll_vote', { id: p.id, option }).catch(() => {});
});

$('#poll-remove').onclick = async () => {
  const p = pollById(state.openPoll);
  if (!p) return;
  const ok = await confirmDialog(
    'Oylama listeden kaldırılsın mı?',
    p.isOwner && !p.closed ? 'Oylama açık; kaldırırsanız önce bitirilir ve sonuç katılımcılara gönderilir.' : 'Yalnızca sizin listenizden kaldırılır.',
    'Kaldır', true);
  if (!ok) return;
  await call('poll_remove', { id: p.id });
  closePoll();
};

function showPollCreate() {
  const selected = new Set();
  let options = ['', ''];
  const m = openModal('poll-new', `
    <h3>Yeni oylama</h3>
    <p class="lead">Eklediğiniz kişilere bildirim gider. Sonucu tüm katılımcılar canlı olarak görür.</p>
    <div class="field">
      <label for="pl-q">Soru</label>
      <input class="input" id="pl-q" maxlength="200" placeholder="ör. Cuma öğle yemeği nerede?" autofocus />
    </div>
    <div class="field">
      <label>Seçenekler</label>
      <div id="pl-opts"></div>
      <button class="link-btn" id="pl-add">+ Seçenek ekle</button>
    </div>
    <div class="field">
      <div class="label-row"><label>Oy verecek kişiler</label><button class="link-btn" id="pl-all"></button></div>
      <div class="recips" id="pl-people"></div>
    </div>
    <label class="check"><input type="checkbox" id="pl-me" checked /> <span>Ben de oy kullanacağım</span></label>
    <label class="check"><input type="checkbox" id="pl-anon" /> <span>Gizli oylama <span class="muted small">(kimin neye oy verdiği görünmez, yalnızca sayılar)</span></span></label>
    <div class="err" id="pl-err"></div>
    <div class="foot">
      <button class="btn ghost" data-close>Vazgeç</button>
      <button class="btn" id="pl-create">Oylamayı başlat</button>
    </div>`);
  const people = candidatePeople();
  const renderOpts = () => {
    m.querySelector('#pl-opts').innerHTML = options.map((o, i) => `
      <div class="opt-row">
        <input class="input" data-i="${i}" maxlength="80" placeholder="Seçenek ${i + 1}" value="${esc(o)}" />
        ${options.length > 2 ? `<button class="icon-btn sm" data-del="${i}" title="Seçeneği sil">${ic.x}</button>` : ''}
      </div>`).join('');
    m.querySelector('#pl-add').hidden = options.length >= 10;
  };
  const renderPeople = () => {
    m.querySelector('#pl-people').innerHTML = people.length
      ? people.map((p) => personRow(p, selected.has(p.fingerprint))).join('')
      : '<span class="muted small">Eklenecek kimse yok. Ağda başka bir FastFiles görünmeli.</span>';
    m.querySelector('#pl-all').textContent = people.length && selected.size === people.length ? 'Hiçbirini seçme' : 'Tümünü seç';
  };
  m.querySelector('#pl-opts').addEventListener('input', (e) => {
    const i = e.target.dataset.i;
    if (i != null) options[Number(i)] = e.target.value;
  });
  m.querySelector('#pl-opts').addEventListener('click', (e) => {
    const d = e.target.closest('[data-del]');
    if (d) { options.splice(Number(d.dataset.del), 1); renderOpts(); }
  });
  m.querySelector('#pl-add').onclick = () => {
    options.push('');
    renderOpts();
    m.querySelector(`#pl-opts [data-i="${options.length - 1}"]`).focus();
  };
  m.querySelector('#pl-people').addEventListener('change', (e) => {
    const fp = e.target.closest('.recip')?.dataset.fp;
    if (!fp) return;
    e.target.checked ? selected.add(fp) : selected.delete(fp);
    renderPeople();
  });
  m.querySelector('#pl-all').onclick = () => {
    if (selected.size === people.length) selected.clear(); else people.forEach((p) => selected.add(p.fingerprint));
    renderPeople();
  };
  m.querySelector('[data-close]').onclick = closeModal;
  m.querySelector('#pl-create').onclick = async () => {
    const err = m.querySelector('#pl-err');
    const question = m.querySelector('#pl-q').value.trim();
    const opts = options.map((o) => o.trim()).filter(Boolean);
    if (!question) { err.textContent = 'Soruyu yazın'; return; }
    if (opts.length < 2) { err.textContent = 'En az iki seçenek yazın'; return; }
    if (!selected.size) { err.textContent = 'Oy verecek en az bir kişi seçin'; return; }
    try {
      const p = await api.invoke('poll_create', {
        question, options: opts, voters: [...selected],
        includeMe: m.querySelector('#pl-me').checked, anonymous: m.querySelector('#pl-anon').checked,
      });
      if (!pollById(p.id)) state.polls = [p, ...state.polls];
      closeModal();
      renderPolls();
      openPoll(p.id);
    } catch (e) {
      err.textContent = String(e);
    }
  };
  renderOpts();
  renderPeople();
}
$('#btn-new-poll').onclick = showPollCreate;

// ───────────────────────────── Sürükle-bırak ─────────────────────────────

function peerIdAt(pos) {
  if (!pos) return null;
  const r = window.devicePixelRatio || 1;
  const el = document.elementFromPoint(pos.x / r, pos.y / r);
  return el?.closest('.peer')?.dataset.id || null;
}

function updateDragTarget(pos) {
  const id = peerIdAt(pos);
  document.querySelectorAll('.peer.drop-target').forEach((el) => el.dataset.id !== id && el.classList.remove('drop-target'));
  const msg = $('#drop-msg');
  if (id) {
    document.querySelector(`.peer[data-id="${CSS.escape(id)}"]`)?.classList.add('drop-target');
    msg.textContent = `Bırak → ${state.peers.find((p) => p.id === id)?.name ?? ''}`;
  } else if (state.peers.length === 0) {
    msg.textContent = 'Ağda henüz cihaz yok';
  } else if (state.peers.length === 1) {
    msg.textContent = `Bırakınca ${state.peers[0].name} cihazına gönderilecek`;
  } else {
    msg.textContent = 'Bir cihazın üzerine bırakın ya da boş alana bırakıp birden fazla kişi seçin';
  }
}

// Bırakma noktası açık sohbet panelinin üzerinde mi?
function chatDropAt(pos) {
  if (!pos || !chatOpen() || $('#composer').disabled) return false;
  const r = window.devicePixelRatio || 1;
  const el = document.elementFromPoint(pos.x / r, pos.y / r);
  return !!el?.closest('#chat-view');
}

function endDrag() {
  document.body.classList.remove('dragging');
  document.querySelectorAll('.peer.drop-target').forEach((el) => el.classList.remove('drop-target'));
}

// Bırakma noktası açık ortak klasör panelinin üzerinde mi?
function shareDropAt(pos) {
  if (!pos || !shareOpen()) return false;
  const r = window.devicePixelRatio || 1;
  return !!document.elementFromPoint(pos.x / r, pos.y / r)?.closest('#share-view');
}

// Bırakma noktası (kullanılamaz durumdaki) sohbet/klasör panelinin üzerinde mi?
function sidePanelAt(pos) {
  if (!pos) return false;
  const r = window.devicePixelRatio || 1;
  return !!document.elementFromPoint(pos.x / r, pos.y / r)?.closest('#chat-view, #share-view');
}

api.onDragDrop((event) => {
  const p = event.payload;
  // Kilitliyken sürükle-bırak hiçbir şey yapmaz (pencereye bırakılan dosya gönderilmesin).
  if (lockVisible()) return endDrag();
  if (modalKind === 'incoming' || modalKind === 'welcome') return;
  if (p.type === 'enter' || p.type === 'over') {
    if (p.paths) state.dragPaths = p.paths;
    document.body.classList.add('dragging');
    if (batch) $('#drop-msg').textContent = 'Listeye eklemek için bırakın';
    else if (chatDropAt(p.position)) $('#drop-msg').textContent = `Bırak → ${state.chats.get(state.openChat)?.name || 'sohbet'}`;
    else if (shareDropTarget() && shareDropAt(p.position)) $('#drop-msg').textContent = `“${shareDropTarget().name}” klasörüne eklemek için bırakın`;
    else updateDragTarget(p.position);
  } else if (p.type === 'leave') {
    endDrag();
  } else if (p.type === 'drop') {
    const id = batch ? null : peerIdAt(p.position);
    const paths = p.paths?.length ? p.paths : state.dragPaths;
    endDrag();
    state.dragPaths = null;
    if (!paths?.length) return;
    // Güncelleme paketi bırakıldıysa gönderme, içe aktar.
    if (paths.length === 1 && /\.ffupdate$/i.test(paths[0])) return importUpdate(paths[0]);
    // "Toplu gönder" penceresi açıksa bırakılanlar listeye eklenir.
    if (batch) return batch.add(paths);
    // Açık sohbetin üzerine bırakıldıysa o sohbete (kişiye ya da gruba) gider.
    if (!modalKind && chatDropAt(p.position)) return sendToChat(paths);
    // Açık (ve kilidi açık) ortak klasörün üzerine bırakıldıysa oraya eklenir.
    if (!modalKind && shareDropTarget() && shareDropAt(p.position)) return addToShare(shareDropTarget().id, paths);
    // Kullanılamayan bir sohbet/klasör panelinin üzerine bırakıldı (çevrimdışı kişi, ayrılınan
    // grup, kilitli klasör): başka birine gitmesin.
    if (!modalKind && !id && sidePanelAt(p.position)) {
      return toast('Buraya dosya gönderilemiyor (kişi çevrimdışı ya da klasör kullanılamıyor)', { type: 'error' });
    }
    if (modalKind) closeModal();
    const peer = state.peers.find((x) => x.id === id) || (state.peers.length === 1 ? state.peers[0] : null);
    if (peer) sendPaths(peer, paths);
    else if (state.peers.length === 0) toast('Ağda gönderilecek cihaz yok', { type: 'error' });
    else showBatch(paths);
  }
});

// ───────────────────────────── Başlangıç ─────────────────────────────

async function takePendingSend() {
  const paths = await api.invoke('take_pending_send').catch(() => []);
  if (!paths?.length) return;
  openSendBatch(paths);
}

// Gezgin'den gelen yolları "Toplu gönder"de açar; gelen istek / karşılama ekranı / kilit
// açıkken bekler, açık bir Toplu gönder varsa ona ekler.
function openSendBatch(paths) {
  if (lockVisible() || modalKind === 'incoming' || modalKind === 'welcome') {
    return setTimeout(() => openSendBatch(paths), 1500);
  }
  if (batch) return batch.add(paths);
  const peers = state.peers.filter((p) => !p.blocked);
  if (!peers.length) return toast('Ağda gönderilecek cihaz yok', { type: 'error' });
  showBatch(paths, peers.length === 1 ? [peers[0].id] : []);
}

async function refreshSnapshot() {
  const snap = await api.invoke('get_snapshot');
  state.device = snap.device;
  state.ips = snap.ips;
  state.settings = snap.settings;
  state.peers = snap.peers;
  return snap;
}

async function init() {
  state.booted = true;
  const snap = await refreshSnapshot();
  applyTheme(state.settings);
  updateLockButton();
  state.chats = new Map((snap.conversations || []).map((c) => [c.fingerprint, c]));
  state.update = snap.update;
  state.avatars = new Map(Object.entries(snap.avatars || {}));
  state.shares = snap.shares || [];
  renderShares();
  state.polls = snap.polls || [];
  renderPolls();
  renderUpdateBar();
  renderMe();
  renderPeers();
  renderConvs();
  setTransfers(snap.transfers);

  api.listen('peers', (e) => {
    state.peers = e.payload;
    renderPeers();
    renderConvs();
    renderChatHead();
    batch?.render();
  });
  api.listen('chat', (e) => onChat(e.payload));
  api.listen('avatar', (e) => {
    const { fingerprint, url } = e.payload;
    if (url) state.avatars.set(fingerprint, url); else state.avatars.delete(fingerprint);
    renderMe();
    renderPeers();
    renderConvs();
    renderChatHead();
  });
  api.listen('shares', (e) => onShares(e.payload));
  api.listen('polls', (e) => onPolls(e.payload));
  api.listen('share-files', (e) => { if (state.openShare === e.payload.id) loadShareFiles(); });
  api.listen('conversation', (e) => onConversation(e.payload));
  api.listen('notification-click', (e) => onNotificationClick(e.payload));
  api.listen('update-ready', async () => {
    state.update = await api.invoke('update_state').catch(() => state.update);
    renderUpdateBar();
  });
  api.listen('updated', (e) => toast(`FastFiles güncellendi: v${e.payload.from} → v${e.payload.to}`, { type: 'success', timeout: 7000 }));
  api.listen('transfer', (e) => onTransfer(e.payload));
  api.listen('transfers', (e) => setTransfers(e.payload));
  api.listen('typing', (e) => onTyping(e.payload));
  // Karşı taraf gönderimi elle onayladı.
  api.listen('accepted', (e) => toast(`${e.payload.peerName} kabul etti · "${e.payload.title}" gönderiliyor`, { type: 'success' }));
  // Gezgin'de sağ tık → "FastFiles ile gönder".
  api.listen('send-request', () => takePendingSend());
  takePendingSend();

  // IP değişikliklerini (Wi-Fi ↔ ethernet) yakala.
  setInterval(async () => { await refreshSnapshot().catch(() => {}); renderMe(); }, 15000);

  if (!state.settings.onboarded) showWelcome();
}

document.addEventListener('contextmenu', (e) => { if (!e.target.closest('input')) e.preventDefault(); });
// PIN tanımlıysa uygulama kilitli açılır; veriler kilit açılınca yüklenir.
(async () => {
  // Gelen dosya istekleri kilit açılmadan da dinlenir: uygulama kilitli açılsa bile istek
  // kaybolmaz (pencere kilit ekranının arkasında bekler, kilit açılınca görünür).
  api.listen('incoming', (e) => onIncoming(e.payload));
  api.listen('incoming-closed', (e) => onIncomingClosed(e.payload));
  state.lock = await api.invoke('lock_state').catch(() => ({ hasPin: false, locked: false, retryAfter: 0 }));
  if (state.lock.locked) {
    showLock();
    if (state.lock.retryAfter > 0) waitRetry(state.lock.retryAfter);
    return;
  }
  await init();
})().catch((e) => toast(`Başlatılamadı: ${e}`, { type: 'error', timeout: 15000 }));

// ───────────────────────────── Tarayıcı önizlemesi ─────────────────────────────

function createPreviewApi() {
  const now = Date.now();
  const listeners = {};
  const fp = 'a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90';
  const snap = {
    device: { id: 'me', name: 'NUR-PC', port: 53317, fingerprint: fp, os: 'windows' },
    ips: ['192.168.1.107'],
    settings: { deviceName: 'Nur', downloadDir: 'C:\\Users\\Nur\\Downloads\\FastFiles', closeToTray: true, trusted: [{ fingerprint: fp, name: 'Ahmet' }], manualPeers: [], onboarded: true },
    peers: [
      { id: 'p1', name: 'Ahmet', ip: '192.168.1.21', port: 53317, os: 'windows', fingerprint: fp, trusted: true, manualAddr: null },
      { id: 'p2', name: 'Elif (Tasarım)', ip: '192.168.1.34', port: 53317, os: 'macos', fingerprint: 'ff' + fp.slice(2), trusted: false, manualAddr: null },
      { id: 'p3', name: 'Render-Sunucu', ip: '192.168.1.50', port: 53317, os: 'linux', fingerprint: 'ee' + fp.slice(2), trusted: false, manualAddr: '192.168.1.50' },
    ],
    transfers: [
      { id: 't3', direction: 'send', peerId: 'p3', peerName: 'Render-Sunucu', title: '3 öğe', fileCount: 3, total: 5e6, done: 0, speed: 0, status: 'waiting', createdAt: now - 3000 },
      { id: 't1', direction: 'send', peerId: 'p1', peerName: 'Ahmet', title: 'Proje_Final', fileCount: 128, total: 2.4e9, done: 1.62e9, speed: 98e6, status: 'active', resumable: true, createdAt: now - 20000, currentFile: 'Proje_Final/assets/scene.fbx' },
      { id: 't8', direction: 'send', peerId: 'p2', peerName: 'Elif (Tasarım)', title: 'Çekim_RAW', fileCount: 412, total: 38e9, done: 9.1e9, speed: 0, status: 'paused', resumable: true, createdAt: now - 40000, finishedAt: now - 30000, sources: ['E:\\Çekim_RAW'] },
      { id: 't2', direction: 'receive', peerId: 'p2', peerName: 'Elif (Tasarım)', title: 'logo-v3.psd', fileCount: 1, total: 84e6, done: 84e6, speed: 0, status: 'done', createdAt: now - 90000, finishedAt: now - 88000, savePath: 'C:\\x' },
      { id: 't4', direction: 'send', peerId: 'p2', peerName: 'Elif (Tasarım)', title: 'sunum.pptx', fileCount: 1, total: 12e6, done: 0, speed: 0, status: 'declined', createdAt: now - 300000, finishedAt: now - 290000, sources: ['C:\\x\\sunum.pptx'] },
      { id: 't5', direction: 'send', peerId: 'p1', peerName: 'Ahmet', title: 'Render_4K.mov', fileCount: 1, total: 18e9, done: 11.2e9, speed: 0, status: 'waiting', error: 'Bağlantı koptu · yeniden bağlanılıyor (2/6)', createdAt: now - 600000, sources: ['D:\\Render_4K.mov'] },
      { id: 't6', direction: 'receive', peerId: 'p1', peerName: 'Ahmet', title: 'Arşiv 2025', fileCount: 3120, total: 6.4e9, done: 6.4e9, speed: 0, status: 'done', resumed: true, createdAt: now - 2 * 86400e3, finishedAt: now - 2 * 86400e3 + 600e3, savePath: 'C:\\y' },
      { id: 't7', direction: 'send', peerId: 'p3', peerName: 'Render-Sunucu', title: 'sahne.blend', fileCount: 1, total: 2.1e9, done: 0.9e9, speed: 0, status: 'failed', error: 'Bağlantı koptu', createdAt: now - 26 * 3600e3, finishedAt: now - 26 * 3600e3 + 90e3, sources: ['D:\\sahne.blend'] },
    ],
  };
  const fp2 = snap.peers[1].fingerprint;
  const fp1 = snap.peers[0].fingerprint;
  const history = {
    [fp2]: [
      { id: 'h1', outgoing: false, text: 'Selam! Dünkü sunumun son halini atabilir misin?', at: now - 26 * 3600e3, status: 'received' },
      { id: 'h2', outgoing: true, text: 'Tabii, birazdan gönderiyorum.', at: now - 26 * 3600e3 + 60e3, status: 'sent' },
      { id: 'h3', outgoing: false, text: 'Logo dosyaları geldi, teşekkürler 🙏\nRenkleri de kontrol ettim, sorun yok.', at: now - 25 * 60e3, status: 'received' },
      { id: 'h4', outgoing: true, text: 'Süper. Akşam toplantıda konuşuruz.', at: now - 24 * 60e3, status: 'sent' },
      { id: 'h5', outgoing: true, text: 'Bir de font klasörünü unutma lütfen', at: now - 2 * 60e3, status: 'failed', error: 'Bağlantı koptu' },
      { id: 'h6', outgoing: false, text: '<b>bu HTML olarak yorumlanmamalı</b> <img src=x onerror=alert(1)>', at: now - 60e3, status: 'received' },
      { id: 'f-t2', outgoing: false, text: '📎 logo-v3.psd', at: now - 50e3, status: 'received', file: { title: 'logo-v3.psd', count: 1, total: 84e6, transfers: [{ to: 'me', id: 't2' }] } },
      { id: 'f-t8', outgoing: true, text: '📎 Çekim_RAW', at: now - 40e3, status: 'sent', file: { title: 'Çekim_RAW', count: 412, total: 38e9, transfers: [{ to: 'x', id: 't8' }] } },
      { id: 'f-t4', outgoing: true, text: '📎 sunum.pptx', at: now - 30e3, status: 'sent', file: { title: 'sunum.pptx', count: 1, total: 12e6, transfers: [{ to: 'x', id: 't4' }] } },
    ],
  };
  snap.conversations = [
    { fingerprint: fp2, name: 'Elif (Tasarım)', unread: 2, last: history[fp2].at(-1) },
    { fingerprint: fp1, name: 'Ahmet', unread: 0, last: { id: 'a1', outgoing: true, text: 'Proje dosyalarını gönderdim', at: now - 3 * 3600e3, status: 'sent' } },
    { fingerprint: 'cc' + fp.slice(2), name: 'Mehmet (İzinde)', unread: 0, last: { id: 'b1', outgoing: false, text: 'Pazartesi görüşürüz', at: now - 5 * 86400e3, status: 'received' } },
  ];
  // Örnek grup (bu cihaz yönetici). Önizlemede cihazın kendi parmak izi diğerlerinden farklı olmalı.
  snap.device.fingerprint = 'dd' + fp.slice(2);
  const me = snap.device.fingerprint;
  const fpM = 'cc' + fp.slice(2);
  const gkey = 'g:grp-1';
  const group = {
    id: 'grp-1', name: 'Proje Ekibi', admin: me, rev: 2, deleted: false,
    members: [{ fingerprint: me, name: 'Nur' }, { fingerprint: fp1, name: 'Ahmet' }, { fingerprint: fp2, name: 'Elif (Tasarım)' }, { fingerprint: fpM, name: 'Mehmet (İzinde)' }],
  };
  history[gkey] = [
    { id: 'g0', system: true, text: 'Grubu oluşturdunuz', at: now - 50 * 60e3 },
    { id: 'g1', outgoing: false, sender: fp1, senderName: 'Ahmet', text: 'Build bu akşam hazır olur mu?', at: now - 40 * 60e3, status: 'received' },
    { id: 'g2', outgoing: false, sender: fp2, senderName: 'Elif (Tasarım)', text: 'Arayüz görselleri tamam, klasörü gönderdim.', at: now - 38 * 60e3, status: 'received' },
    { id: 'g3', outgoing: false, sender: fp2, senderName: 'Elif (Tasarım)', text: 'Bir de ikonlar var, onları yarın atarım.', at: now - 37 * 60e3, status: 'received' },
    { id: 'g4', system: true, text: 'Mehmet (İzinde) gruba eklendi', at: now - 30 * 60e3 },
    { id: 'g5', outgoing: true, text: 'Evet, 18:00 gibi paylaşırım 👍', at: now - 20 * 60e3, status: 'sending', recipients: 3, pending: [fpM] },
    { id: 'g6', outgoing: false, sender: fp1, senderName: 'Ahmet', text: 'Süper, teşekkürler', at: now - 5 * 60e3, status: 'received' },
    { id: 'g7', outgoing: true, text: '📎 Toplantı notları', at: now - 3 * 60e3, status: 'sending', recipients: 3, pending: [fpM], file: { title: 'Toplantı notları', count: 6, total: 4.2e6, transfers: [{ to: fp1, id: 't2' }, { to: fp2, id: 't3' }] } },
  ];
  snap.conversations.unshift({ fingerprint: gkey, name: 'Proje Ekibi', unread: 1, last: history[gkey].at(-1), group, left: false });

  // Örnek profil resimleri (önizleme için tuvalde çizilir).
  const mockPhoto = (h1, h2) => {
    const c = document.createElement('canvas');
    c.width = c.height = 96;
    const x = c.getContext('2d');
    const g = x.createLinearGradient(0, 0, 96, 96);
    g.addColorStop(0, `hsl(${h1} 70% 60%)`);
    g.addColorStop(1, `hsl(${h2} 70% 40%)`);
    x.fillStyle = g; x.fillRect(0, 0, 96, 96);
    x.fillStyle = 'rgba(255,255,255,.85)';
    x.beginPath(); x.arc(48, 38, 17, 0, Math.PI * 2); x.fill();
    x.beginPath(); x.ellipse(48, 92, 32, 26, 0, 0, Math.PI * 2); x.fill();
    return c.toDataURL('image/jpeg', 0.9);
  };
  snap.avatars = { [fp2]: mockPhoto(20, 340), [me]: mockPhoto(200, 260) };

  // Örnek ortak klasörler: biri bizim (açık), biri Ahmet'in (şifreli, kilidi açılmamış).
  const shareFilesMock = {
    sh1: [
      { path: 'Sunum/Final_v4.pptx', size: 18.2e6, mtime: now - 4 * 60e3, author: fp2, authorName: 'Elif (Tasarım)' },
      { path: 'Bütçe 2026.xlsx', size: 410e3, mtime: now - 50 * 60e3, author: me, authorName: 'Siz' },
      { path: 'Görseller/logo.png', size: 1.3e6, mtime: now - 3 * 3600e3, author: fp1, authorName: 'Ahmet' },
      { path: 'Görseller/arka-plan.jpg', size: 4.8e6, mtime: now - 26 * 3600e3, author: fp2, authorName: 'Elif (Tasarım)' },
      { path: 'Notlar.txt', size: 3.1e3, mtime: now - 4 * 86400e3, author: me, authorName: 'Siz' },
    ].map((f) => ({ ...f, fullPath: `C:\\Ortak\\${f.path}` })),
  };
  snap.shares = [
    { id: 'sh1', name: 'Proje Dosyaları', admin: me, members: [{ fingerprint: me, name: 'Nur' }, { fingerprint: fp1, name: 'Ahmet' }, { fingerprint: fp2, name: 'Elif (Tasarım)' }],
      protected: false, canAccess: true, left: false, deleted: false, dir: 'C:\\Users\\Nur\\Downloads\\FastFiles\\Ortak\\Proje Dosyaları', fileCount: 5, totalSize: 24.7e6, syncing: 2 },
    { id: 'sh2', name: 'Muhasebe', admin: fp1, members: [{ fingerprint: fp1, name: 'Ahmet' }, { fingerprint: me, name: 'Nur' }],
      protected: true, canAccess: false, left: false, deleted: false, dir: 'C:\\Users\\Nur\\Downloads\\FastFiles\\Ortak\\Muhasebe', fileCount: 0, totalSize: 0, syncing: 0 },
  ];

  // Örnek oylamalar: biri bizim (açık), biri Ahmet'in (oyumuz bekleniyor), biri sonuçlanmış.
  const voters = [{ fingerprint: me, name: 'Nur' }, { fingerprint: fp1, name: 'Ahmet' }, { fingerprint: fp2, name: 'Elif (Tasarım)' }, { fingerprint: fpM, name: 'Mehmet (İzinde)' }];
  snap.polls = [
    { id: 'pl2', owner: fp1, ownerName: 'Ahmet', question: 'Ekip yemeği hangi gün olsun?', options: ['Perşembe', 'Cuma', 'Gelecek hafta'],
      voters, anonymous: false, closed: false, rev: 3, createdAt: now - 15 * 60e3, counts: [1, 1, 0], voted: [fp1, fp2],
      ballots: [{ fingerprint: fp1, option: 0 }, { fingerprint: fp2, option: 1 }], myVote: null, votePending: false, canVote: true, isOwner: false, seen: false },
    { id: 'pl1', owner: me, ownerName: 'Nur', question: 'Yeni logo için hangi renk?', options: ['Mor', 'Turkuaz', 'Turuncu'],
      voters, anonymous: true, closed: false, rev: 5, createdAt: now - 2 * 3600e3, counts: [2, 1, 0], voted: [me, fp1, fp2],
      ballots: [], myVote: 0, votePending: false, canVote: true, isOwner: true, seen: true },
    { id: 'pl0', owner: fp2, ownerName: 'Elif (Tasarım)', question: 'Ofis müziği açık mı kalsın?', options: ['Evet', 'Hayır'],
      voters: voters.slice(0, 3), anonymous: false, closed: true, rev: 9, createdAt: now - 3 * 86400e3, counts: [2, 1], voted: [me, fp1, fp2],
      ballots: [{ fingerprint: me, option: 0 }, { fingerprint: fp1, option: 1 }, { fingerprint: fp2, option: 0 }], myVote: 0, votePending: false, canVote: false, isOwner: false, seen: true },
  ];
  const tallyMock = (p) => {
    p.counts = p.options.map((_, i) => p.ballots.filter((b) => b.option === i).length);
    p.voted = p.ballots.map((b) => b.fingerprint);
  };

  snap.update = location.hash.includes('update')
    ? { current: '1.2.0', ready: '1.2.1', serving: '1.2.1' }
    : { current: '1.2.0', ready: null, serving: null };
  snap.peers[0].version = '1.2.0';
  snap.peers[1].version = '1.1.0';
  const emit = (ev, payload) => (listeners[ev] || []).forEach((cb) => cb({ payload }));
  // Önizlemede olay tetiklemek için: __preview.emit('peers', [...])
  window.__preview = { emit, snap };
  setTimeout(() => {
    if (location.hash.includes('chat')) document.querySelector('.peer[data-id="p2"] [data-act="chat"]')?.click();
    if (location.hash.includes('group')) openChat(gkey);
    if (location.hash.includes('share')) { setTab('shares'); openShare('sh1'); }
    if (location.hash.includes('locked')) { setTab('shares'); openShare('sh2'); }
    if (location.hash.includes('poll')) openPoll(location.hash.includes('poll-closed') ? 'pl0' : 'pl2');
    if (location.hash.includes('batch')) {
      showBatch(['C:\\Projeler\\Sunum_v4.pptx', 'C:\\Projeler\\Marka Kiti', 'D:\\Render\\final_cut.mp4'], ['p1', 'p2']);
    }
    if (!location.hash.includes('incoming')) return;
    (listeners.incoming || []).forEach((cb) => cb({ payload: {
      requestId: 'r1', device: snap.peers[1], ip: '192.168.1.34', title: 'Marka Kiti', fileCount: 14, total: 356e6,
      files: [{ path: 'Marka Kiti/logo.ai', size: 12e6 }, { path: 'Marka Kiti/renkler.pdf', size: 2e6 }, { path: 'Marka Kiti/fontlar/Inter.zip', size: 340e6 }],
    } }));
  }, 400);
  return {
    invoke: async (cmd, args = {}) => {
      if (cmd === 'get_snapshot') return location.hash.includes('empty') ? { ...snap, peers: [], transfers: [], conversations: [] } : snap;
      if (cmd === 'chat_history') return structuredClone(history[args.fingerprint] || []);
      if (cmd === 'share_files') return structuredClone(shareFilesMock[args.id] || []);
      if (cmd === 'share_unlock') {
        if (args.password !== '1234') throw 'Şifre yanlış';
        const s = snap.shares.find((x) => x.id === args.id);
        s.canAccess = true;
        setTimeout(() => emit('shares', structuredClone(snap.shares)), 50);
        return null;
      }
      if (cmd === 'share_create') {
        const s = { id: `sh${Date.now()}`, name: args.name.trim(), admin: me, members: [{ fingerprint: me, name: 'Nur' }],
          protected: !!args.password, canAccess: true, left: false, deleted: false, dir: `C:\\Ortak\\${args.name}`, fileCount: 0, totalSize: 0, syncing: 0 };
        snap.shares.push(s);
        setTimeout(() => emit('shares', structuredClone(snap.shares)), 50);
        return s;
      }
      if (cmd === 'set_avatar') return null;
      if (cmd === 'poll_vote') {
        const p = snap.polls.find((x) => x.id === args.id);
        p.ballots = p.ballots.filter((b) => b.fingerprint !== me).concat([{ fingerprint: me, option: args.option }]);
        tallyMock(p);
        p.myVote = args.option;
        if (p.anonymous) p.ballots = p.ballots; // önizleme: gizli oylamada da yerel sayım
        setTimeout(() => emit('polls', structuredClone(snap.polls)), 30);
        return null;
      }
      if (cmd === 'poll_close') {
        const p = snap.polls.find((x) => x.id === args.id);
        p.closed = true; p.canVote = false;
        setTimeout(() => emit('polls', structuredClone(snap.polls)), 30);
        return null;
      }
      if (cmd === 'poll_create') {
        const p = { id: `pl${Date.now()}`, owner: me, ownerName: 'Nur', question: args.question, options: args.options,
          voters: [...(args.includeMe ? [{ fingerprint: me, name: 'Nur' }] : []), ...args.voters.map((f) => ({ fingerprint: f, name: snap.peers.find((x) => x.fingerprint === f)?.name || 'Kişi' }))],
          anonymous: args.anonymous, closed: false, rev: 1, createdAt: Date.now(), counts: args.options.map(() => 0), voted: [], ballots: [],
          myVote: null, votePending: false, canVote: args.includeMe, isOwner: true, seen: true };
        snap.polls.unshift(p);
        return structuredClone(p);
      }
      if (cmd === 'poll_remove') {
        snap.polls = snap.polls.filter((x) => x.id !== args.id);
        setTimeout(() => emit('polls', structuredClone(snap.polls)), 30);
        return null;
      }
      if (cmd === 'send_paths') return args.peerIds;
      if (cmd === 'chat_send') {
        const peer = snap.peers.find((p) => p.id === args.peerId);
        const msg = { id: `m${Date.now()}`, outgoing: true, text: args.text.trim(), at: Date.now(), status: 'sending' };
        (history[peer.fingerprint] ||= []).push(msg);
        emit('chat', { fingerprint: peer.fingerprint, name: peer.name, unread: 0, incoming: false, message: msg });
        setTimeout(() => emit('chat', { fingerprint: peer.fingerprint, name: peer.name, unread: 0, incoming: false, message: { ...msg, status: 'sent' } }), 500);
        return msg;
      }
      if (cmd === 'group_send') {
        const g = snap.conversations.find((c) => c.fingerprint === args.key).group;
        const others = g.members.filter((m) => m.fingerprint !== me).map((m) => m.fingerprint);
        const msg = { id: `gm${Date.now()}`, outgoing: true, text: args.text.trim(), at: Date.now(), status: 'sending', recipients: others.length, pending: others };
        history[args.key].push(msg);
        emit('chat', { fingerprint: args.key, name: g.name, unread: 0, incoming: false, message: msg });
        setTimeout(() => emit('chat', { fingerprint: args.key, name: g.name, unread: 0, incoming: false, message: { ...msg, pending: [fpM] } }), 400);
        return msg;
      }
      if (cmd === 'group_create') {
        const g = { id: `grp-${Date.now()}`, name: args.name.trim(), admin: me, rev: 1, deleted: false,
          members: [{ fingerprint: me, name: 'Nur' }, ...args.members.map((f) => ({ fingerprint: f, name: snap.peers.find((p) => p.fingerprint === f)?.name || 'Kişi' }))] };
        const key = `g:${g.id}`;
        history[key] = [{ id: 's1', system: true, text: 'Grubu oluşturdunuz', at: Date.now() }];
        return { fingerprint: key, name: g.name, unread: 0, last: history[key][0], group: g, left: false };
      }
      if (cmd === 'group_edit') {
        const c = snap.conversations.find((x) => x.fingerprint === args.key);
        if (args.name) c.name = c.group.name = args.name;
        c.group.members = c.group.members.filter((m) => !args.remove.includes(m.fingerprint));
        args.add.forEach((f) => c.group.members.push({ fingerprint: f, name: snap.peers.find((p) => p.fingerprint === f)?.name || 'Kişi' }));
        return structuredClone(c);
      }
      if (cmd === 'group_leave') {
        const c = snap.conversations.find((x) => x.fingerprint === args.key);
        c.left = true; c.group.deleted = c.group.admin === me;
        emit('conversation', structuredClone(c));
        return null;
      }
      if (cmd === 'set_blocked') {
        snap.settings.blocked = args.blocked ? [...(snap.settings.blocked || []), { fingerprint: args.fingerprint, name: args.name }] : (snap.settings.blocked || []).filter((b) => b.fingerprint !== args.fingerprint);
        return snap.settings;
      }
      if (cmd === 'clear_finished') return snap.transfers.filter((t) => ['waiting', 'active'].includes(t.status));
      if (cmd === 'remove_transfer') return (snap.transfers = snap.transfers.filter((t) => t.id !== args.id));
      if (cmd === 'retry_transfer') return args.id;
      if (cmd === 'pause_transfer') return null;
      if (cmd === 'lock_state') return { hasPin: location.hash.includes('applock') || !!window.__pin, locked: location.hash.includes('applock') && !window.__unlocked, retryAfter: 0 };
      if (cmd === 'unlock_app') { if (args.pin !== '1234') throw 'PIN yanlış'; window.__unlocked = true; return null; }
      if (cmd === 'lock_app') { window.__unlocked = false; return null; }
      if (cmd === 'set_app_pin') { window.__pin = args.pin; return { hasPin: !!args.pin, locked: false, retryAfter: 0 }; }
      if (cmd === 'forget_pin' || cmd === 'send_typing') return null;
      if (cmd === 'take_pending_send') return location.hash.includes('sendto') ? ['C:\\Projeler\\Teklif.docx', 'C:\\Projeler\\Görseller'] : [];
      if (cmd === 'group_send_files') return null;
      if (cmd === 'send_clipboard') return { kind: 'image', count: args.peerIds.length };
      return snap.settings;
    },
    listen: async (ev, cb) => { (listeners[ev] ||= []).push(cb); },
    onDragDrop: () => {},
    pick: async () => null,
  };
}
