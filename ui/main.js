// FastFiles arayüzü — derleme adımı gerektirmeyen düz JavaScript.
// Tauri içinde `window.__TAURI__` kullanılır; tarayıcıda açılırsa örnek verilerle önizleme modu çalışır.

const T = window.__TAURI__;
const api = T ? {
  invoke: (cmd, args) => T.core.invoke(cmd, args),
  invokeRaw: (cmd, body, headers) => T.core.invoke(cmd, body, { headers }),
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
  games: [],             // oyun odaları (davetler dahil)
  gameCatalog: [],       // oynanabilir oyunlar
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
  else if (!modalKind && !$('#screen-viewer').hidden) closeScreenViewer();
  else if (document.getElementById('status-menu')) closeStatusMenu();
  else if (document.getElementById('react-pop')) closeReactPicker();
  else if (!modalKind && gamesOpen()) {
    // Oyun ekranı diğer her şeyin önünde: önce terfi seçimi, sonra ekranın kendisi kapanır.
    if (!$('#promo').hidden) closePromo();
    else closeGames();
  }
  else if (!modalKind && officeOpen()) {
    if (officeState.mod?.pickMode) setOfficePick(false);
    else closeOffice();
  }
  else if (!modalKind && chatOpen() && !$('#mention-pop').hidden) $('#mention-pop').hidden = true;
  else if (!modalKind && chatOpen() && (chatx.reply || chatx.edit)) cancelCompose();
  else if (!modalKind && chatOpen() && !$('#chat-searchbar').hidden) closeSearch();
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
  $('#me').title = `Durumunuzu değiştirmek için tıklayın · ${state.ips.join(', ')} · port ${state.device?.port ?? ''}`;
  const av = $('#me-avatar');
  const fp = state.device?.fingerprint;
  av.style.background = grad(fp);
  av.innerHTML = face(fp, state.settings.deviceName);
  renderMyStatus();
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
    const sig = JSON.stringify([p.name, p.ip, p.os, p.version, p.trusted, p.blocked, p.manualAddr, p.status, unreadOf(p.fingerprint), state.avatars.get(p.fingerprint)?.length || 0]);
    if (el.dataset.sig !== sig) {
      el.dataset.sig = sig;
      el.innerHTML = peerInner(p);
    }
    if (container.children[i] !== el) container.insertBefore(el, container.children[i] || null);
  });
  existing.forEach((el) => el.remove());
  officeSyncPeople();
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
      ${p.status ? `<div class="peer-status">${statusChip(p.status)}</div>` : ''}
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
  if (e.target.closest?.('input, textarea, [contenteditable]') || modalKind || chatOpen() || gamesOpen()) return;
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
  officeOnTransfer(prev, t);
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
  officeTransfers();
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
    <label class="check"><input type="checkbox" id="s-hotkeys" ${s.hotkeys !== false ? 'checked' : ''} />
      <span>Genel kısayollar<br><span class="muted small"><b>Ctrl+Shift+Boşluk</b>: hangi programda olursanız olun hızlı gönderim · <b>Ctrl+Shift+PrtSc</b>: ekran görüntüsü al, üzerine çiz, gönder. Uygulama içinde <b>Ctrl+K</b> de hızlı gönderimi açar. Başka bir programın kısayoluyla çakışırsa kapatın.</span></span></label>
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
        hotkeys: m.querySelector('#s-hotkeys').checked,
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
  closeViewer(); // kilitliyken resim görünmesin
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
// Ada tıklamak durum menüsünü açar (Ayarlar: dişli düğmesi ya da menünün altı).
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
// Güvenlik: mesaj metni her zaman `esc()`/textContent ile basılır (HTML yorumlanmaz). Bağlantılar
// yalnızca http(s)/www olarak tanınır ve arka uçtaki `open_url` ile tarayıcıda açılır (bkz.
// `messageBody`). Arka uç ayrıca görünmez/yön değiştiren karakterleri temizler.

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
    const lastText = c.last ? (c.last.deleted ? '🚫 Bu mesaj silindi' : c.last.text.replace(/\s+/g, ' ')) : '';
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
  if (gamesOpen()) closeGames();
  if (state.openChat !== fp) resetChatExtras();
  closeEmoji();
  closeViewer();
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
  closeViewer();
  resetChatExtras();
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
  officeState.mod?.typing(fingerprint);
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
  $('#chat-game').hidden = !!g || isBlocked(state.openChat);
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
    ta.placeholder = canSend ? 'Gruba mesaj yazın… (@ ile birinden bahsedin)' : 'Bu gruba mesaj gönderemezsiniz';
    $('#chat-send').disabled = !canSend || !ta.value.trim();
    $('#chat-nudge').hidden = true;
    $('#chat-screen').hidden = !canSend;
    $('#chat-shot').disabled = !canSend;
    $('#chat-attach').title = 'Dosya gönder (ya da dosyaları sohbete sürükleyin)';
    if (chatx.edit) ta.disabled = false;
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
  st.innerHTML = blocked ? 'Engellendi' : peer
    ? `<span class="online-dot st-${esc(peer.status || 'ok')}"></span>${peer.status ? `${statusOf(peer.status).icon} ${esc(statusOf(peer.status).label)}` : 'Çevrimiçi'} · ${esc(peer.ip)}`
    : 'Çevrimdışı';
  const bb = $('#chat-block');
  bb.innerHTML = ic.ban;
  bb.classList.toggle('on', blocked);
  bb.title = blocked ? 'Engeli kaldır' : 'Engelle (bu cihazdan mesaj ve dosya alma)';
  const canSend = !!peer && !blocked;
  const ta = $('#composer');
  ta.disabled = !canSend;
  // Çevrimdışı kişiye de dosya bırakılabilir: çevrimiçi olunca kendiliğinden gider.
  $('#chat-attach').disabled = blocked;
  $('#chat-attach').title = peer ? 'Dosya gönder (ya da dosyaları sohbete sürükleyin)' : 'Dosya bırak: kişi çevrimiçi olunca kendiliğinden gönderilir';
  $('#chat-emoji').disabled = !canSend;
  if (!canSend) closeEmoji();
  ta.placeholder = canSend ? 'Mesaj yazın…' : blocked ? 'Bu cihazı engellediniz' : 'Çevrimdışı · mesaj gönderilemez, ama dosya bırakabilirsiniz';
  $('#chat-send').disabled = !canSend || !ta.value.trim();
  $('#chat-nudge').hidden = !canSend;
  $('#chat-screen').hidden = !canSend;
  $('#chat-shot').disabled = blocked;
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

// ───────────────────────────── Sohbette resimler ─────────────────────────────
// Tek dosyalık resim balonları resmin kendisiyle gösterilir; tıklayınca resim gösterici açılır.
// Resim, mesaj kaydından arka uçta çözülen yoldan okunur (gönderende kaynak, alıcıda kaydedilen
// dosya). Büyük fotoğraflar küçük bir önizlemeye indirgenir; tam boyutu yalnızca gösterici yükler.

const IMG_EXT = /\.(png|jpe?g|jfif|gif|webp|bmp|avif|ico)$/i;
const IMG_MAX = 40 * 1024 * 1024; // arka uçtaki IMAGE_PREVIEW_MAX ile aynı
const THUMB_EDGE = 640; // önizlemenin uzun kenarı (px)
const THUMB_CACHE = 80;
const BUBBLE_W = 280; // balondaki resmin en büyük genişliği / yüksekliği
const BUBBLE_H = 300;
const thumbs = new Map(); // `${sohbet}|${mesaj}` → { url, w, h } | { failed } | {} (yükleniyor)
const thumbQueue = [];
let thumbBusy = 0;

const thumbKey = (chat, id) => `${chat}|${id}`;
const isNearBottom = (box) => box.scrollHeight - box.scrollTop - box.clientHeight < 120;

function isImageMsg(m) {
  return !!m.file && m.file.count === 1 && m.file.total <= IMG_MAX && IMG_EXT.test(m.file.title || '');
}

// Resim hazır mı: gönderende hemen, alıcıda aktarım bitince.
function imageReady(m) {
  if (!isImageMsg(m)) return false;
  if (m.outgoing) return true;
  return (m.file.transfers || []).some((r) => state.transfers.get(r.id)?.status === 'done');
}

// Balonda resim gösteriliyor mu (yüklenemediyse normal dosya balonuna döner).
function imageShown(m) {
  return imageReady(m) && !thumbs.get(thumbKey(state.openChat, m.id))?.failed;
}

// Resmin balondaki boyutu: oran korunur, çok dar/uzun resimler makul sınırlarda kalır.
function bubbleImageSize(m) {
  const t = thumbs.get(thumbKey(state.openChat, m.id));
  if (!t?.w) return { w: 240, h: 170 };
  const scale = Math.min(1, BUBBLE_W / t.w, BUBBLE_H / t.h);
  return { w: Math.max(170, Math.round(t.w * scale)), h: Math.max(60, Math.round(t.h * scale)) };
}

function bubbleWidthStyle(m) {
  return `width:${bubbleImageSize(m).w + 12}px`;
}

function imagePart(m) {
  if (!imageShown(m)) return '';
  const t = thumbs.get(thumbKey(state.openChat, m.id));
  if (!t?.url) queueThumb(state.openChat, m.id);
  const { h } = bubbleImageSize(m);
  return `<button class="fb-img${t?.url ? '' : ' loading'}" data-view="${esc(m.id)}" style="height:${h}px" title="Büyüt">${
    t?.url ? `<img src="${t.url}" alt="${esc(m.file.title)}" draggable="false">` : ''}</button>`;
}

function sizeImageBubble(msgEl, m) {
  const bubble = msgEl.querySelector('.file-bubble');
  if (bubble) bubble.style.width = `${bubbleImageSize(m).w + 12}px`;
}

// Yüklenen (ya da yüklenemeyen) önizlemeyi açık sohbetteki balona yerleştirir; sohbet en
// alttaysa en altta kalır.
function updateImageBubble(chat, id) {
  if (!chatOpen() || state.openChat !== chat) return;
  const m = (state.history.get(chat) || []).find((x) => x.id === id);
  const msg = document.querySelector(`.msg[data-id="${CSS.escape(id)}"]`);
  if (!m || !msg) return;
  const box = $('#messages');
  const stick = isNearBottom(box);
  const old = msg.querySelector('.fb-img');
  const html = imagePart(m);
  if (old) {
    if (html) old.outerHTML = html;
    else old.remove();
  }
  msg.classList.toggle('img-msg', !!html);
  if (html) sizeImageBubble(msg, m);
  else msg.querySelector('.file-bubble')?.style.removeProperty('width');
  const body = msg.querySelector('.fb-body');
  if (body) body.innerHTML = fileBubbleInner(m);
  if (stick) box.scrollTop = box.scrollHeight;
}

// Tauri ham baytları ArrayBuffer olarak döndürür.
async function fetchImageBlob(chat, id) {
  const data = await api.invoke('chat_image', { fingerprint: chat, id });
  const bytes = data instanceof ArrayBuffer || ArrayBuffer.isView(data) ? data : new Uint8Array(data);
  return new Blob([bytes]);
}

function queueThumb(chat, id) {
  const k = thumbKey(chat, id);
  if (thumbs.has(k)) return;
  thumbs.set(k, {});
  thumbQueue.push([chat, id]);
  pumpThumbs();
}

function pumpThumbs() {
  while (thumbBusy < 3 && thumbQueue.length) {
    const [chat, id] = thumbQueue.shift();
    thumbBusy++;
    makeThumb(chat, id).finally(() => {
      thumbBusy--;
      pumpThumbs();
    });
  }
}

async function makeThumb(chat, id) {
  const k = thumbKey(chat, id);
  let entry;
  try {
    const blob = await fetchImageBlob(chat, id);
    const bmp = await createImageBitmap(blob);
    const { width: w, height: h } = bmp;
    const head = new TextDecoder().decode(await blob.slice(0, 3).arrayBuffer());
    let url;
    if (head === 'GIF' || (blob.size < 400 * 1024 && Math.max(w, h) <= THUMB_EDGE * 1.5)) {
      // Küçük resimler ve GIF'ler (hareket kaybolmasın) olduğu gibi gösterilir.
      url = URL.createObjectURL(blob);
    } else {
      const scale = Math.min(1, THUMB_EDGE / Math.max(w, h));
      const c = document.createElement('canvas');
      c.width = Math.max(1, Math.round(w * scale));
      c.height = Math.max(1, Math.round(h * scale));
      const x = c.getContext('2d');
      x.imageSmoothingQuality = 'high';
      x.drawImage(bmp, 0, 0, c.width, c.height);
      const small = await new Promise((res) => c.toBlob(res, 'image/webp', 0.85));
      url = URL.createObjectURL(small || blob);
    }
    bmp.close();
    entry = { url, w, h };
  } catch {
    entry = { failed: true };
  }
  thumbs.delete(k); // en sona taşı (en son kullanılan)
  thumbs.set(k, entry);
  while (thumbs.size > THUMB_CACHE) {
    const [oldKey, old] = thumbs.entries().next().value;
    if (old.url) URL.revokeObjectURL(old.url);
    thumbs.delete(oldKey);
  }
  updateImageBubble(chat, id);
  if (viewer.open && viewer.chat === chat) renderViewerCount();
}

// ───────────────────────────── Resim gösterici ─────────────────────────────

const viewer = { open: false, chat: null, ids: [], index: 0, url: null, w: 0, h: 0, s: 1, x: 0, y: 0, fit: 1, token: 0, drag: null, dragged: false };

function viewerEl() {
  let el = $('#viewer');
  if (el) return el;
  el = document.createElement('div');
  el.id = 'viewer';
  el.hidden = true;
  el.innerHTML = `
    <div class="vw-top">
      <div class="vw-info"><b id="vw-name"></b><span id="vw-sub"></span></div>
      <span id="vw-count"></span>
      <div class="vw-tools">
        <button class="vw-btn" data-vw="out" title="Uzaklaştır (−)"><svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="7"/><path d="m20 20-4-4M8 11h6"/></svg></button>
        <span id="vw-zoom">100%</span>
        <button class="vw-btn" data-vw="in" title="Yakınlaştır (+)"><svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="7"/><path d="m20 20-4-4M8 11h6M11 8v6"/></svg></button>
        <button class="vw-btn" data-vw="fit" title="Ekrana sığdır (0)"><svg viewBox="0 0 24 24"><path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/></svg></button>
        <span class="vw-sep"></span>
        <button class="vw-btn" data-vw="open" title="Varsayılan uygulamada aç"><svg viewBox="0 0 24 24"><path d="M14 4h6v6M20 4l-9 9"/><path d="M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/></svg></button>
        <button class="vw-btn" data-vw="reveal" title="Klasörde göster">${ic.reveal}</button>
        <button class="vw-btn" data-vw="close" title="Kapat (Esc)">${ic.x}</button>
      </div>
    </div>
    <div class="vw-stage"><img id="vw-img" alt="" draggable="false"><div class="vw-msg" id="vw-msg"></div></div>
    <button class="vw-nav prev" data-vw="prev" title="Önceki (←)"><svg viewBox="0 0 24 24"><path d="m15 5-7 7 7 7"/></svg></button>
    <button class="vw-nav next" data-vw="next" title="Sonraki (→)"><svg viewBox="0 0 24 24"><path d="m9 5 7 7-7 7"/></svg></button>`;
  document.body.appendChild(el);
  el.addEventListener('click', (e) => {
    const b = e.target.closest('[data-vw]');
    if (b) return viewerAction(b.dataset.vw);
    // Resmin dışındaki boş alana tıklamak kapatır.
    if (e.target.classList.contains('vw-stage') && !viewer.dragged) closeViewer();
  });
  const stage = el.querySelector('.vw-stage');
  stage.addEventListener('wheel', (e) => {
    e.preventDefault();
    const r = stage.getBoundingClientRect();
    zoomViewer(viewer.s * Math.exp(-e.deltaY * 0.0015), e.clientX - r.left, e.clientY - r.top);
  }, { passive: false });
  stage.addEventListener('dblclick', (e) => {
    const r = stage.getBoundingClientRect();
    const zoomed = viewer.s > viewer.fit * 1.01;
    zoomViewer(zoomed ? viewer.fit : Math.max(1, viewer.fit * 2), e.clientX - r.left, e.clientY - r.top);
  });
  stage.addEventListener('pointerdown', (e) => {
    if (e.button !== 0 || !e.target.closest('#vw-img')) return;
    viewer.drag = { x: e.clientX, y: e.clientY, ox: viewer.x, oy: viewer.y };
    viewer.dragged = false;
    stage.setPointerCapture(e.pointerId);
  });
  stage.addEventListener('pointermove', (e) => {
    if (!viewer.drag) return;
    const dx = e.clientX - viewer.drag.x;
    const dy = e.clientY - viewer.drag.y;
    if (Math.abs(dx) + Math.abs(dy) > 3) viewer.dragged = true;
    viewer.x = viewer.drag.ox + dx;
    viewer.y = viewer.drag.oy + dy;
    applyViewer();
  });
  const endDragView = () => {
    viewer.drag = null;
    setTimeout(() => { viewer.dragged = false; }, 0);
  };
  stage.addEventListener('pointerup', endDragView);
  stage.addEventListener('pointercancel', endDragView);
  window.addEventListener('resize', () => viewer.open && fitViewer());
  return el;
}

// Sohbetteki gösterilebilir resimler (sırayla).
function viewerIds(chat) {
  return (state.history.get(chat) || []).filter((m) => imageShown(m)).map((m) => m.id);
}

function openViewer(id) {
  if (lockVisible() || !state.openChat) return;
  closeEmoji();
  viewer.chat = state.openChat;
  viewer.ids = viewerIds(viewer.chat);
  viewer.index = Math.max(0, viewer.ids.indexOf(id));
  viewer.open = true;
  viewerEl().hidden = false;
  showViewerImage();
}

function closeViewer() {
  if (!viewer.open) return;
  viewer.open = false;
  viewer.token++;
  $('#viewer').hidden = true;
  $('#vw-img').removeAttribute('src');
  if (viewer.url) URL.revokeObjectURL(viewer.url);
  viewer.url = null;
}

function renderViewerCount() {
  const current = viewer.ids[viewer.index];
  viewer.ids = viewerIds(viewer.chat);
  viewer.index = Math.max(0, viewer.ids.indexOf(current));
  const n = viewer.ids.length;
  $('#vw-count').textContent = n > 1 ? `${viewer.index + 1} / ${n}` : '';
  $('#viewer .vw-nav.prev').hidden = viewer.index <= 0;
  $('#viewer .vw-nav.next').hidden = viewer.index >= n - 1;
}

async function showViewerImage() {
  const id = viewer.ids[viewer.index];
  const m = (state.history.get(viewer.chat) || []).find((x) => x.id === id);
  if (!m) return closeViewer();
  const token = ++viewer.token;
  const img = $('#vw-img');
  $('#vw-msg').textContent = '';
  $('#vw-name').textContent = m.file.title;
  const who = m.outgoing ? 'Siz' : (m.senderName || state.chats.get(viewer.chat)?.name || '');
  $('#vw-sub').textContent = `${who} · ${dayLabel(m.at)} ${fmtTime(m.at)} · ${fmtBytes(m.file.total)}`;
  renderViewerCount();
  if (viewer.url) URL.revokeObjectURL(viewer.url);
  viewer.url = null;
  // Önce eldeki önizleme gösterilir, tam boyut gelince değiştirilir.
  const t = thumbs.get(thumbKey(viewer.chat, id));
  if (t?.url) {
    viewer.w = t.w;
    viewer.h = t.h;
    img.src = t.url;
    img.hidden = false;
    fitViewer();
  } else {
    img.hidden = true;
  }
  try {
    const blob = await fetchImageBlob(viewer.chat, id);
    if (token !== viewer.token) return;
    const url = URL.createObjectURL(blob);
    const probe = new Image();
    probe.src = url;
    await probe.decode();
    if (token !== viewer.token) return URL.revokeObjectURL(url);
    viewer.url = url;
    const sameSize = !img.hidden && viewer.w === probe.naturalWidth && viewer.h === probe.naturalHeight;
    viewer.w = probe.naturalWidth;
    viewer.h = probe.naturalHeight;
    img.src = url;
    img.hidden = false;
    if (!sameSize) fitViewer();
  } catch (e) {
    if (token !== viewer.token) return;
    if (!t?.url) $('#vw-msg').textContent = `Resim açılamadı${typeof e === 'string' ? `: ${e}` : ''}`;
  }
}

function fitViewer() {
  const stage = $('#viewer .vw-stage');
  if (!viewer.w || !stage) return;
  viewer.fit = Math.min(1, (stage.clientWidth - 48) / viewer.w, (stage.clientHeight - 32) / viewer.h);
  viewer.s = viewer.fit;
  applyViewer();
}

function zoomViewer(s, cx, cy) {
  if (!viewer.w) return;
  const stage = $('#viewer .vw-stage');
  if (cx == null) {
    cx = stage.clientWidth / 2;
    cy = stage.clientHeight / 2;
  }
  const next = Math.min(Math.max(s, viewer.fit), Math.max(8, viewer.fit));
  viewer.x = cx - ((cx - viewer.x) * next) / viewer.s;
  viewer.y = cy - ((cy - viewer.y) * next) / viewer.s;
  viewer.s = next;
  applyViewer();
}

// Ölçek/konumu uygular: resim alandan küçükse ortalanır, büyükse kenar dışına kaydırılamaz.
function applyViewer() {
  const stage = $('#viewer .vw-stage');
  const W = stage.clientWidth;
  const H = stage.clientHeight;
  const w = viewer.w * viewer.s;
  const h = viewer.h * viewer.s;
  viewer.x = w <= W ? (W - w) / 2 : Math.min(0, Math.max(W - w, viewer.x));
  viewer.y = h <= H ? (H - h) / 2 : Math.min(0, Math.max(H - h, viewer.y));
  const img = $('#vw-img');
  img.style.width = `${viewer.w}px`;
  img.style.height = `${viewer.h}px`;
  img.style.transform = `translate(${viewer.x}px, ${viewer.y}px) scale(${viewer.s})`;
  img.classList.toggle('grab', viewer.s > viewer.fit * 1.01);
  $('#vw-zoom').textContent = `${Math.round(viewer.s * 100)}%`;
}

function stepViewer(d) {
  const i = viewer.index + d;
  if (i < 0 || i >= viewer.ids.length) return;
  viewer.index = i;
  showViewerImage();
}

function viewerAction(a) {
  const id = viewer.ids[viewer.index];
  if (a === 'close') closeViewer();
  else if (a === 'prev') stepViewer(-1);
  else if (a === 'next') stepViewer(1);
  else if (a === 'in') zoomViewer(viewer.s * 1.25);
  else if (a === 'out') zoomViewer(viewer.s / 1.25);
  else if (a === 'fit') fitViewer();
  else if ((a === 'open' || a === 'reveal') && id) {
    api.invoke('chat_image_open', { fingerprint: viewer.chat, id, reveal: a === 'reveal' })
      .catch((e) => toast(String(e), { type: 'error' }));
  }
}

window.addEventListener('keydown', (e) => {
  if (!viewer.open || modalKind || lockVisible()) return;
  const map = { Escape: 'close', ArrowLeft: 'prev', ArrowRight: 'next', '+': 'in', '=': 'in', '-': 'out', '0': 'fit' };
  const a = map[e.key];
  if (!a) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  viewerAction(a);
}, true);

// Dosya balonu: durum ve ilerleme bağlı aktarım kayıtlarından (state.transfers) okunur.
function fileBubbleInner(m) {
  const f = m.file;
  const ts = (f.transfers || []).map((r) => state.transfers.get(r.id)).filter(Boolean);
  const icon = f.count > 1 ? ic.folder : ic.file;
  const showsImage = imageShown(m);
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
      ${showsImage ? '' : `<span class="fb-icon">${icon}</span>`}
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
    const msg = document.querySelector(`.msg[data-id="${CSS.escape(m.id)}"]`);
    const body = msg?.querySelector('.fb-body');
    if (!body) continue;
    // Alınan resim tamamlandıysa önizleme şimdi eklenir.
    if (!msg.querySelector('.fb-img') && imageShown(m)) {
      const stick = isNearBottom($('#messages'));
      body.insertAdjacentHTML('beforebegin', imagePart(m));
      msg.classList.add('img-msg');
      sizeImageBubble(msg, m);
      if (stick) $('#messages').scrollTop = $('#messages').scrollHeight;
    }
    body.innerHTML = fileBubbleInner(m);
  }
}

function renderMessages(forceBottom = false) {
  const box = $('#messages');
  const hitRe = searchRegex();
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
      const img = imagePart(m);
      html += `
      <div class="msg ${dir}${first ? ' first' : ''} file-msg${img ? ' img-msg' : ''}" data-id="${esc(m.id)}">
        ${sender}
        <div class="bubble file-bubble" style="${img ? bubbleWidthStyle(m) : ''}">${img}<div class="fb-body">${fileBubbleInner(m)}</div></div>
        <div class="meta">${esc(fmtTime(m.at))}</div>
      </div>`;
      prev = who;
      continue;
    }
    html += textMessageHtml(m, dir, first, sender, hitRe);
    prev = who;
  }
  box.innerHTML = html;
  if (hitRe) {
    const hits = updateSearchHits();
    hits.forEach((h, i) => h.classList.toggle('hit-cur', i === chatx.hit));
    return;
  }
  if (forceBottom || nearBottom) box.scrollTop = box.scrollHeight;
}
$('#messages').addEventListener('keydown', (e) => {
  const link = e.key === 'Enter' && e.target.closest?.('.msg-link');
  if (link) call('open_url', { url: link.dataset.url }).catch(() => {});
});
$('#messages').addEventListener('click', async (e) => {
  const link = e.target.closest('.msg-link');
  if (link) {
    e.preventDefault();
    call('open_url', { url: link.dataset.url }).catch(() => {});
    return;
  }
  const v = e.target.closest('[data-view]');
  if (v) return openViewer(v.dataset.view);
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
  if (!text || (ta.disabled && !chatx.edit) || !key) return;
  const peer = peerByFp(key);
  if (!groupOf(key) && !peer && !chatx.edit) return;
  try {
    if (chatx.edit) {
      await call('chat_edit', { key, id: chatx.edit.id, text });
      chatx.edit = null;
      renderReplyBar();
      ta.value = '';
      autoGrow();
      updateComposerNote();
      renderChatHead();
      return;
    }
    const replyTo = chatx.reply?.id || null;
    if (groupOf(key)) await call('group_send', { key, text, replyTo });
    else await call('chat_send', { peerId: peer.id, text, replyTo });
    chatx.reply = null;
    renderReplyBar();
    $('#mention-pop').hidden = true;
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
  if (!peer) return queueFiles(key, paths);
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
  if (target.view === 'phone') return openPhone();
  if (target.view === 'screen' && target.session) return watchScreen(target.session);
  if (modalKind && modalKind !== 'incoming') closeModal();
  if (target.view === 'games') {
    if (target.id && gameById(target.id)) openRoom(target.id);
    else openGames();
    return;
  }
  if (gamesOpen()) closeGames();
  if (officeOpen()) closeOffice();
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
  let isNew = true;
  if (hist) {
    const i = hist.findIndex((m) => m.id === message.id);
    isNew = i < 0;
    if (i >= 0) hist[i] = message; else hist.push(message);
  }
  const viewing = state.openChat === fp && chatOpen();
  if (viewing) {
    renderMessages(!incoming && isNew);
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
  officeOnChat(fp, incoming, message);
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
  if (gamesOpen()) closeGames();
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
  if (gamesOpen()) closeGames();
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

// ───────────────────────────── Oyunlar ─────────────────────────────
// Odanın tek yetkilisi daveti gönderen kişidir (arka uç: node/game.rs, kurallar: games/).
// Arayüz yalnızca arka ucun verdiği yasal hamleleri (`legal`) önerir; kural kontrolü arka uçtadır.
// Yeni bir oyun eklenince burada yalnızca o oyunun tahtasını çizen kısım eklenir.

const gameState = {
  open: null,         // açık oda kimliği
  selected: null,     // seçili kare, ör. "e2"
  drag: null,         // sürüklenen taş
  optimistic: null,   // gönderilen ama henüz onaylanmamış hamle (tahtada hemen görünsün diye)
  promo: null,        // terfi seçimi bekleyen hamle
};
const gameById = (id) => state.games.find((g) => g.id === id);
const gamesOpen = () => !$('#games-screen').hidden;
const gameDef = (id) => state.gameCatalog.find((d) => d.id === id);
const opponentsOf = (g) => g.players.filter((p) => p.fingerprint !== myFp());
const vsName = (g) => opponentsOf(g).map((p) => p.name).join(', ') || '—';
const gameOver = (g) => ['finished', 'declined', 'cancelled'].includes(g.status);

// Taşlar: dolu satranç sembolleri; U+FE0E emoji yerine metin olarak çizilmelerini sağlar. Renk CSS'te.
// Beyaz taş: beyaz dolu sembolün üstüne koyu çizgili sembol (data-o) bindirilir.
const PIECE_GLYPH = { k: '♚', q: '♛', r: '♜', b: '♝', n: '♞', p: '♟' };
const OUTLINE_GLYPH = { k: '♔', q: '♕', r: '♖', b: '♗', n: '♘', p: '♙' };
function pieceHtml(p) {
  const t = p.toLowerCase();
  const white = p === p.toUpperCase();
  return `<span class="pc ${white ? 'w' : 'b'}"${white ? ` data-o="${OUTLINE_GLYPH[t]}︎"` : ''}>${PIECE_GLYPH[t]}︎</span>`;
}
// Notasyonda Türkçe taş harfleri: Ş(ah) V(ezir) K(ale) F(il) A(t).
const TR_PIECE = { K: 'Ş', Q: 'V', R: 'K', B: 'F', N: 'A' };
const trSan = (san) => String(san).replace(/[KQRBN]/g, (c) => TR_PIECE[c]);
const RESULT_REASON = {
  checkmate: 'şah mat', stalemate: 'pat', repetition: 'üç kez tekrar', fifty: '50 hamle kuralı', material: 'yetersiz malzeme',
  resign: 'pes etme', agreement: 'anlaşmalı', score: 'en az vuruş', tie: 'eşit vuruş', forfeit: 'diğerleri çekildi',
};
const GAME_ART = {
  chess: `<span class="gc-piece">${PIECE_GLYPH.n}︎</span>`,
  golf: '<span class="gc-golf"><svg viewBox="0 0 24 24"><path d="M8 20V3l8 3.5L8 10"/><path d="M3 20.5c0-1.4 2.5-2.5 6-2.5s6 1.1 6 2.5"/><circle cx="18.5" cy="18.5" r="2"/></svg></span>',
};

function gameResultText(g) {
  const r = g.result;
  if (!r) return 'Oyun bitti';
  const me = myFp();
  const nameOf = (fp) => g.players.find((p) => p.fingerprint === fp)?.name || '?';
  if (r.reason === 'resign') return r.by === me ? 'Pes ettiniz' : `${nameOf(r.by)} pes etti · kazandınız`;
  const why = RESULT_REASON[r.reason] || 'oyun bitti';
  if (!r.winner) return `Berabere · ${why}`;
  return r.winner === me ? `Kazandınız · ${why}` : `${nameOf(r.winner)} kazandı · ${why}`;
}

/** Sonuç benim açımdan: "win" | "lose" | "draw" */
function gameOutcome(g) {
  const r = g.result;
  if (!r) return 'draw';
  if (r.reason === 'resign') return r.by === myFp() ? 'lose' : 'win';
  return !r.winner ? 'draw' : r.winner === myFp() ? 'win' : 'lose';
}

function gameStatusText(g) {
  const me = myFp();
  switch (g.status) {
    case 'invited':
      if (g.isHost) return 'Davet gönderildi · yanıt bekleniyor';
      return g.joined.includes(me) || g.pending ? 'Kabul ettiniz · oda kuruluyor…' : 'Sizi davet ediyor';
    case 'declined': return g.isHost ? 'Davet reddedildi' : 'Daveti reddettiniz';
    case 'cancelled': return 'Davet geri çekildi';
    case 'finished': return gameResultText(g);
  }
  const golf = g.game === 'golf';
  if (golf && g.info?.quit?.[g.mySeat]) return 'Çekildiniz · izliyorsunuz';
  if (g.pending) return golf ? 'Vuruşunuz iletiliyor…' : 'Hamleniz iletiliyor…';
  if (g.drawOffer && g.drawOffer !== me) return 'Beraberlik teklif ediyor';
  if (g.myTurn) return golf ? `Sıra sizde · ${(g.info?.hole ?? 0) + 1}. delik` : g.info?.check ? 'Sıra sizde · Şah!' : 'Sıra sizde';
  return `${g.players[g.turn]?.name || 'Rakip'} ${golf ? 'vuruyor…' : 'düşünüyor…'}`;
}

function renderGames() {
  const list = state.games;
  const waiting = list.filter((g) => g.needsMe).length;
  $('#games-badge').hidden = waiting === 0;
  $('#games-badge').textContent = waiting;
  $('#games-empty').hidden = list.length > 0;
  $('#game-list').hidden = list.length === 0;
  $('#game-list').innerHTML = list.map((g) => {
    const opp = opponentsOf(g)[0];
    const online = opp && !!peerByFp(opp.fingerprint);
    const cls = [g.id === gameState.open ? 'active' : '', g.needsMe ? 'unread' : '', gameOver(g) ? 'left' : ''].join(' ');
    const badge = !g.needsMe ? ''
      : `<span class="badge vote">${g.status === 'invited' ? 'Davet' : g.myTurn ? 'Sıra sizde' : 'Yanıt bekliyor'}</span>`;
    return `
      <div class="conv ${cls}" data-game="${esc(g.id)}">
        <div class="avatar" style="background:${grad(opp?.fingerprint || g.id)}">${opp ? face(opp.fingerprint, opp.name) : ''}</div>
        <div class="c-name">${online ? '<span class="online-dot" title="Çevrimiçi"></span>' : ''}<span>${esc(vsName(g))}</span><span class="c-count">· ${esc(g.title)}</span></div>
        <div class="c-time">${esc(shortTime(g.updatedAt))}</div>
        <div class="c-last">${esc(gameStatusText(g))}</div>
        <div>${badge}</div>
      </div>`;
  }).join('');
}
$('#game-list').addEventListener('click', (e) => {
  const el = e.target.closest('[data-game]');
  if (el) openRoom(el.dataset.game);
});

function renderGameCards() {
  $('#game-cards').innerHTML = state.gameCatalog.map((d) => `
    <button class="game-card" data-game-new="${esc(d.id)}">
      <span class="gc-art">${GAME_ART[d.id] || ''}</span>
      <span class="gc-name">${esc(d.name)}</span>
      <span class="gc-desc">${esc(d.description)}</span>
      <span class="gc-meta">${d.minPlayers === d.maxPlayers ? `${d.maxPlayers} oyuncu` : `${d.minPlayers}–${d.maxPlayers} oyuncu`} · Davet gönder →</span>
    </button>`).join('') + `
    <div class="game-card soon">
      <span class="gc-art"><svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"/></svg></span>
      <span class="gc-name">Yakında</span>
      <span class="gc-desc">Yeni oyunlar eklenecek.</span>
    </div>`;
}
$('#game-cards').addEventListener('click', (e) => {
  const b = e.target.closest('[data-game-new]');
  if (b) showGameCreate({ game: b.dataset.gameNew });
});

function openGames(id) {
  if (officeOpen()) closeOffice();
  closeEmoji();
  closeViewer();
  $('#games-screen').hidden = false;
  document.body.classList.add('games-open');
  if (id !== undefined) gameState.open = id && gameById(id) ? id : null;
  renderGames();
  renderGameCards();
  renderRoom();
}

function closeGames() {
  closePromo();
  cancelDrag();
  $('#games-screen').hidden = true;
  document.body.classList.remove('games-open');
}

function openRoom(id) {
  if (gameState.open !== id) gameState.selected = null;
  closePromo();
  openGames(id);
}

function closeRoom() {
  gameState.open = null;
  gameState.selected = null;
  closePromo();
  renderGames();
  renderRoom();
}

$('#btn-games').onclick = () => (gamesOpen() ? closeGames() : openGames());
$('#games-close').onclick = closeGames;
$('#room-back').onclick = closeRoom;
$('#btn-new-game').onclick = () => showGameCreate();
$('#chat-game').onclick = () => { if (state.openChat) showGameCreate({ fp: state.openChat }); };

/** Arka uç değişen tek odayı gönderir; listeye yerleştirilir. */
function onGame(view) {
  const list = state.games.filter((g) => g.id !== view.id);
  list.push(view);
  list.sort((a, b) => b.updatedAt - a.updatedAt);
  onGames(list);
}

/** Açık odanın ekranda görünen her şeyini belirleyen alanlar; değişmediyse yeniden çizilmez
 *  (gereksiz çizim, sürüklenen satranç taşını bırakıyor ve nişanı bozuyordu). */
const roomSig = (g) => (g ? JSON.stringify([g.rev, g.status, g.moves.length, g.pending, g.myTurn, g.turn, g.drawOffer, g.players.length, g.joined.length]) : '');

function onGames(list) {
  const before = new Map(state.games.map((g) => [g.id, g]));
  const openSig = roomSig(before.get(gameState.open));
  const hadOpt = gameState.optimistic;
  state.games = list;
  maybeClearOptimistic();
  // Uygulama öndeyken de haber ver (arkadayken Windows bildirimi gelir).
  for (const g of list) {
    const old = before.get(g.id);
    if (!old) {
      if (!g.isHost && g.status === 'invited' && g.needsMe) {
        toast(`${g.hostName} sizi ${g.title} oynamaya davet ediyor`, { action: 'Görüntüle', timeout: 12000, onAction: () => openRoom(g.id) });
      }
      continue;
    }
    if (gamesOpen() && gameState.open === g.id) continue;
    if (old.status === 'invited' && g.status === 'active') {
      toast(`${vsName(g)} ile ${g.title} başladı`, { type: 'success', action: 'Oyuna git', timeout: 10000, onAction: () => openRoom(g.id) });
    } else if (old.status === 'invited' && g.status === 'declined' && g.isHost) {
      toast(`${vsName(g)} ${g.title} davetinizi reddetti`, { type: 'error' });
    } else if (old.status !== 'finished' && g.status === 'finished') {
      toast(`${g.title} · ${gameResultText(g)}`, { action: 'Gör', timeout: 8000, onAction: () => openRoom(g.id) });
    } else if (!old.myTurn && g.myTurn) {
      toast(`${g.title} · sıra sizde (${vsName(g)})`, { action: 'Oyuna git', timeout: 8000, onAction: () => openRoom(g.id) });
    } else if (g.drawOffer && g.drawOffer !== myFp() && old.drawOffer !== g.drawOffer) {
      toast(`${vsName(g)} beraberlik teklif ediyor`, { action: 'Gör', timeout: 8000, onAction: () => openRoom(g.id) });
    }
  }
  if (gameState.open && !gameById(gameState.open)) gameState.open = null;
  renderGames();
  if (gamesOpen() && (roomSig(gameById(gameState.open)) !== openSig || hadOpt !== gameState.optimistic)) renderRoom();
  officeGames();
}

// ── Oda ──

function renderRoom() {
  const g = gameById(gameState.open);
  const isGolf = g?.game === 'golf';
  $('#games-screen').classList.toggle('room-open', !!g);
  // Golf sahası geniş: açıkken oyun listesi gizlenir (geri düğmesi listeye döner).
  $('#games-screen').classList.toggle('wide-room', isGolf);
  $('#game-lobby').hidden = !!g;
  $('#game-room').hidden = !g;
  if (!g) return;
  $('#chess-col').hidden = isGolf;
  $('#golf-col').hidden = !isGolf;
  $('#moves-head').textContent = isGolf ? 'Skor tablosu' : 'Hamleler';
  renderRoomPanel(g);
  if (isGolf) {
    renderGolf(g);
    renderScores(g);
    return;
  }
  cancelDrag();
  if (!g.myTurn) { gameState.selected = null; closePromo(); }
  renderBoard(g);
  renderMoves(g);
}

// Satranç tahtası: FEN'den çizilir. Kare adları "a1".."h8"; dizi sırası a1=0 … h8=63.
const sqName = (i) => String.fromCharCode(97 + (i % 8)) + (Math.floor(i / 8) + 1);
const sqIndex = (n) => (n.charCodeAt(0) - 97) + (Number(n[1]) - 1) * 8;

function parseFen(fen) {
  const board = new Array(64).fill(null);
  const [place = '', side = 'w'] = String(fen).split(' ');
  place.split('/').forEach((row, i) => {
    let f = 0;
    for (const c of row) {
      if (/\d/.test(c)) f += Number(c);
      else { board[(7 - i) * 8 + f] = c; f++; }
    }
  });
  return { board, side };
}

/** Onaylanmamış hamleyi tahtada göstermek için (rok, geçerken alma ve terfi dahil). */
function applyLocalMove(board, uci) {
  const b = board.slice();
  const from = sqIndex(uci.slice(0, 2));
  const to = sqIndex(uci.slice(2, 4));
  const p = b[from];
  if (!p) return b;
  const white = p === p.toUpperCase();
  if (p.toLowerCase() === 'p' && from % 8 !== to % 8 && !b[to]) b[to + (white ? -8 : 8)] = null;
  if (p.toLowerCase() === 'k' && Math.abs(to - from) === 2) {
    const [rf, rt] = to > from ? [from + 3, from + 1] : [from - 4, from - 1];
    b[rt] = b[rf];
    b[rf] = null;
  }
  b[to] = uci[4] ? (white ? uci[4].toUpperCase() : uci[4]) : p;
  b[from] = null;
  return b;
}

/** Ekranda görünen tahta (onay bekleyen hamle uygulanmış). */
function shownBoard(g) {
  let { board } = parseFen(g.info?.fen || '');
  const opt = gameState.optimistic?.id === g.id ? gameState.optimistic : null;
  if (opt) board = applyLocalMove(board, opt.uci);
  return { board, last: opt ? opt.uci : g.info?.lastMove, opt };
}

const legalFrom = (g, sq) => (g.myTurn && !gameState.optimistic ? g.legal.filter((m) => m.startsWith(sq)) : []);

function renderBoard(g) {
  const el = $('#board');
  if (!g.info?.fen) { el.innerHTML = ''; return; }
  const { board, last, opt } = shownBoard(g);
  const flip = g.mySeat === 1;
  const canMove = g.myTurn && !opt;
  const sel = gameState.selected;
  const targets = new Set(sel ? legalFrom(g, sel).map((m) => m.slice(2, 4)) : []);
  const lastSq = last ? [last.slice(0, 2), last.slice(2, 4)] : [];
  let checkSq = null;
  if (g.info.check && !opt) {
    const k = g.info.fen.split(' ')[1] === 'w' ? 'K' : 'k';
    const i = board.indexOf(k);
    if (i >= 0) checkSq = sqName(i);
  }
  let html = '';
  for (let row = 0; row < 8; row++) {
    for (let col = 0; col < 8; col++) {
      const r = flip ? row : 7 - row;
      const f = flip ? 7 - col : col;
      const name = sqName(r * 8 + f);
      const p = board[r * 8 + f];
      const cls = ['sq', (r + f) % 2 === 0 ? 'dark' : 'light'];
      if (lastSq.includes(name)) cls.push('last');
      if (sel === name) cls.push('sel');
      if (targets.has(name)) cls.push(p ? 'target cap' : 'target');
      if (checkSq === name) cls.push('check');
      if (canMove && legalFrom(g, name).length) cls.push('movable');
      html += `<div class="${cls.join(' ')}" data-sq="${name}">`
        + (col === 0 ? `<span class="cr">${name[1]}</span>` : '')
        + (row === 7 ? `<span class="cf">${name[0]}</span>` : '')
        + (p ? pieceHtml(p) : '') + '</div>';
    }
  }
  el.innerHTML = html;
  el.classList.toggle('flipped', flip);
  el.classList.toggle('live', canMove);
  const tops = g.players.map((p, seat) => ({ p, seat }));
  const mine = tops.find((x) => x.p.fingerprint === myFp());
  const other = tops.find((x) => x.p.fingerprint !== myFp());
  const mat = material(board);
  $('#pb-top').innerHTML = other ? playerBar(g, other.p, other.seat, mat) : '';
  $('#pb-bottom').innerHTML = mine ? playerBar(g, mine.p, mine.seat, mat) : '';
}

const START_COUNT = { q: 1, r: 2, b: 2, n: 2, p: 8 };
const PIECE_VALUE = { q: 9, r: 5, b: 3, n: 3, p: 1 };
function material(board) {
  const cnt = { w: {}, b: {} };
  const score = { w: 0, b: 0 };
  for (const p of board) {
    if (!p || p.toLowerCase() === 'k') continue;
    const c = p === p.toUpperCase() ? 'w' : 'b';
    const t = p.toLowerCase();
    cnt[c][t] = (cnt[c][t] || 0) + 1;
    score[c] += PIECE_VALUE[t];
  }
  return { cnt, score };
}

/** Oyuncunun aldığı taşlar ve malzeme farkı. Koltuk 0 beyaz, 1 siyah. */
function capturesHtml(mat, seat) {
  const me = seat === 0 ? 'w' : 'b';
  const victim = seat === 0 ? 'b' : 'w';
  let s = '';
  for (const t of ['q', 'r', 'b', 'n', 'p']) {
    const n = Math.max(0, START_COUNT[t] - (mat.cnt[victim][t] || 0));
    s += pieceHtml(victim === 'w' ? t.toUpperCase() : t).repeat(n);
  }
  const diff = mat.score[me] - mat.score[victim];
  return s + (diff > 0 ? `<span class="pb-adv">+${diff}</span>` : '');
}

function playerBar(g, player, seat, mat) {
  const isMe = player.fingerprint === myFp();
  const online = isMe || !!peerByFp(player.fingerprint);
  const turn = g.status === 'active' && g.turn === seat;
  const seatName = gameDef(g.game)?.seats?.[seat] || '';
  return `
    <span class="avatar" style="background:${grad(player.fingerprint)}">${face(player.fingerprint, player.name)}</span>
    <div class="pb-who">
      <div class="pb-name"><span>${esc(player.name)}${isMe ? ' <span class="muted">(siz)</span>' : ''}</span>
        ${isMe ? '' : online ? '<span class="online-dot" title="Çevrimiçi"></span>' : '<span class="off">çevrimdışı</span>'}</div>
      <div class="pb-caps">${capturesHtml(mat, seat)}</div>
    </div>
    ${turn ? '<span class="pb-turn">Sırada</span>' : ''}
    <span class="pb-seat ${seat === 0 ? 'w' : 'b'}" title="${esc(seatName)}">${esc(seatName)}</span>`;
}

/** "1.4.3" < "1.5.0" gibi sürüm karşılaştırması. */
function versionLess(a, b) {
  const pa = String(a).split('.').map(Number);
  const pb = String(b).split('.').map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] || 0) - (pb[i] || 0);
    if (d) return d < 0;
  }
  return false;
}

const roomBtn = (act, label, cls = '', disabled = false) =>
  `<button class="btn small ${cls}" data-room-act="${act}" ${disabled ? 'disabled' : ''}>${esc(label)}</button>`;

function renderRoomPanel(g) {
  const me = myFp();
  const opp = opponentsOf(g)[0];
  const oppName = opp?.name || 'Rakip';
  const oppOnline = !!opp && !!peerByFp(opp.fingerprint);
  const seatName = (gameDef(g.game)?.seats?.[g.mySeat] || '').toLocaleLowerCase('tr-TR');
  const isGolf = g.game === 'golf';
  const many = g.players.length > 2;
  $('#room-name').textContent = `${g.title} · ${vsName(g)}`;
  if (isGolf && g.info?.holes) {
    const h = g.info.holes[g.info.hole];
    $('#room-sub').textContent = `${g.info.hole + 1}/${g.info.holes.length}. delik · ${h.name} · Par ${h.par}`;
  } else {
    $('#room-sub').textContent = seatName ? `Siz ${seatName} taşlarla oynuyorsunuz` : '';
  }
  $('#room-chat').hidden = !opp || many;
  let status = '';
  let tone = '';
  let actions = '';
  switch (g.status) {
    case 'invited':
      if (g.isHost) {
        // Oyunları bilmeyen eski sürüm daveti alamaz; otomatik güncelleme gelince ulaşır.
        const waiting = g.players.filter((p) => !g.joined.includes(p.fingerprint));
        const first = waiting[0] || opp;
        const firstOnline = !!first && !!peerByFp(first.fingerprint);
        const oppVer = first && peerByFp(first.fingerprint)?.version;
        const oldVer = oppVer && state.device?.version && versionLess(oppVer, state.device.version);
        status = `<b>${esc(waiting.map((p) => p.name).join(', ') || oppName)}</b> davetinize henüz yanıt vermedi.`
          + (many && g.joined.length > 1 ? `<div class="small">Kabul eden: ${esc(g.players.filter((p) => g.joined.includes(p.fingerprint) && p.fingerprint !== me).map((p) => p.name).join(', '))}</div>` : '')
          + (!firstOnline ? '<div class="small muted">Çevrimdışı olan, FastFiles açılınca daveti alır.</div>'
            : oldVer ? `<div class="small muted">FastFiles sürümü eski (v${esc(oppVer)}); güncellenince davet ulaşır.</div>` : '');
        actions = roomBtn('cancel', 'Daveti geri çek', 'ghost');
      } else if (g.joined.includes(me) || g.pending) {
        status = many ? 'Daveti kabul ettiniz. Herkes katılınca oyun başlar.' : 'Daveti kabul ettiniz. Oda kuruluyor…';
      } else {
        tone = 'invite';
        status = `<b>${esc(g.hostName)}</b> sizi <b>${esc(g.title)}</b> oynamaya davet ediyor.`
          + (seatName ? `<div class="small">Siz <b>${esc(seatName)}</b> taşlarla oynayacaksınız.</div>` : '')
          + (many ? `<div class="small">Oyuncular: ${esc(g.players.map((p) => (p.fingerprint === me ? 'siz' : p.name)).join(', '))}</div>` : '');
        actions = roomBtn('join', 'Kabul et') + roomBtn('decline', 'Reddet', 'ghost');
      }
      break;
    case 'active': {
      if (isGolf) {
        const cur = g.players[g.turn];
        const curOnline = !!cur && (cur.fingerprint === me || !!peerByFp(cur.fingerprint));
        if (g.info?.quit?.[g.mySeat]) status = 'Oyundan çekildiniz; oyunu izliyorsunuz.';
        else if (g.pending) status = 'Vuruşunuz iletiliyor…';
        else if (g.myTurn) { status = '<b>Sıra sizde.</b> Topu geriye doğru çekip bırakın.'; tone = 'turn'; }
        else if (cur) status = curOnline ? `<b>${esc(cur.name)}</b> vuruyor…` : `<b>${esc(cur.name)}</b> çevrimdışı; döndüğünde oyun sürer.`;
        if (!g.info?.quit?.[g.mySeat]) actions = roomBtn('resign', 'Oyundan çekil', 'ghost danger-text', g.pending);
        break;
      }
      const offered = g.drawOffer && g.drawOffer !== me && !g.pending;
      if (g.pending) status = oppOnline ? 'Hamleniz iletiliyor…' : `<b>${esc(oppName)}</b> çevrimdışı; hamleniz o dönünce iletilecek.`;
      else if (g.myTurn) { status = g.info?.check ? '<b>Şah!</b> Sıra sizde.' : '<b>Sıra sizde.</b> Bir taşa tıklayın ya da sürükleyin.'; tone = g.info?.check ? 'danger' : 'turn'; }
      else status = oppOnline ? `<b>${esc(oppName)}</b> düşünüyor…` : `<b>${esc(oppName)}</b> çevrimdışı. Döndüğünde oyun kaldığı yerden sürer.`;
      if (offered) {
        status += `<div class="offer">${esc(oppName)} beraberlik teklif ediyor.</div>`;
        actions = roomBtn('acceptDraw', 'Beraberliği kabul et') + roomBtn('declineDraw', 'Reddet', 'ghost');
      } else {
        if (g.drawOffer === me) status += '<div class="offer mine">Beraberlik teklifiniz yanıt bekliyor.</div>';
        actions = roomBtn('offerDraw', 'Beraberlik teklif et', 'ghost', g.drawOffer === me || g.pending);
      }
      actions += roomBtn('resign', 'Pes et', 'ghost danger-text', g.pending);
      break;
    }
    case 'finished':
      tone = gameOutcome(g);
      status = `<b>${esc(gameResultText(g))}</b>`;
      actions = roomBtn('rematch', 'Rövanş') ;
      break;
    case 'declined':
      status = g.isHost ? `<b>${esc(oppName)}</b> davetinizi reddetti.` : 'Daveti reddettiniz.';
      actions = roomBtn('rematch', g.isHost ? 'Tekrar davet et' : 'Davet et', 'ghost');
      break;
    case 'cancelled':
      status = g.isHost ? 'Daveti geri çektiniz.' : `<b>${esc(g.hostName)}</b> daveti geri çekti.`;
      actions = roomBtn('rematch', 'Davet et', 'ghost');
      break;
  }
  const st = $('#room-status');
  st.className = `room-status ${tone}`;
  st.innerHTML = status;
  $('#room-actions').innerHTML = actions;
}

function renderMoves(g) {
  const san = g.info?.san || [];
  const optUci = gameState.optimistic?.id === g.id ? gameState.optimistic.uci : null;
  const all = optUci ? [...san, '…'] : san;
  let html = '';
  for (let i = 0; i < all.length; i += 2) {
    const cell = (j) => (j < all.length ? `<span class="mv ${j === all.length - 1 ? 'cur' : ''}">${esc(trSan(all[j]))}</span>` : '<span></span>');
    html += `<div class="mv-row"><span class="mv-no">${i / 2 + 1}.</span>${cell(i)}${cell(i + 1)}</div>`;
  }
  const el = $('#moves');
  el.innerHTML = html || '<div class="mv-empty">Henüz hamle yok</div>';
  el.scrollTop = el.scrollHeight;
}

// ── Mini golf ──
// Fizik arka uçtadır (games/golf.rs): arayüz yalnızca sahayı çizer, nişanı alır ve arka ucun
// verdiği top yolunu (`lastShot.path`, ~40 nokta/sn) canlandırır. Saha 800×500 birimdir.

const golfCanvas = $('#golf-canvas');
const golfView = { room: null, shots: -1, anim: null, aim: null, raf: 0, flashTimer: 0 };
const GOLF_PULL = 220;           // bu kadar geri çekince %100 güç (saha birimi)
const GOLF_POINT_MS = 25;        // yol noktaları arası süre (1/40 sn)
const golfColor = (fp) => `hsl(${hue(fp)} 78% 52%)`;
const shotCount = (g) => g.moves.filter((m) => m[0] === 'a').length;

function renderGolf(g) {
  if (!g.info?.holes) return;
  const n = shotCount(g);
  if (golfView.room !== g.id) {
    // Odaya yeni girildi: eski vuruş canlandırılmaz.
    Object.assign(golfView, { room: g.id, shots: n, anim: null, aim: null });
    $('#golf-flash').hidden = true;
  } else if (n > golfView.shots) {
    golfView.shots = n;
    const s = g.info.lastShot;
    if (s?.path?.length > 1) golfView.anim = { shot: s, start: performance.now() };
  }
  if (!g.myTurn) golfView.aim = null;
  renderGolfHud(g);
  golfLoop();
}

function renderGolfHud(g) {
  const info = g.info;
  const h = info.holes[info.hole];
  const chips = g.players.map((p, seat) => {
    const b = info.balls[seat];
    const quit = info.quit[seat];
    const cur = g.status === 'active' && g.turn === seat;
    const state = quit ? 'çekildi' : b.done ? `⛳ ${b.strokes}` : b.strokes ? `${b.strokes} vuruş` : 'başlamadı';
    return `<span class="gh-chip${cur ? ' cur' : ''}${quit ? ' quit' : ''}" title="${esc(p.name)}">
      <i style="background:${golfColor(p.fingerprint)}"></i>${esc(p.fingerprint === myFp() ? 'Siz' : p.name)}<small>${esc(state)}</small></span>`;
  }).join('');
  $('#golf-hud').innerHTML = `<div class="gh-hole"><b>${info.hole + 1}. delik</b> · ${esc(h.name)} · Par ${h.par}</div><div class="gh-chips">${chips}</div>`;
  let hint = '';
  if (g.status === 'finished') hint = gameResultText(g);
  else if (g.status !== 'active') hint = 'Herkes katılınca oyun başlar.';
  else if (g.myTurn) hint = 'Topu tutup geriye doğru çekin ve bırakın: ne kadar çekerseniz o kadar sert vurur. (Esc: vazgeç)';
  else if (g.pending) hint = 'Vuruşunuz iletiliyor…';
  else hint = `${g.players[g.turn]?.name || ''} vuruyor…`;
  $('#golf-hint').textContent = hint;
}

function golfLoop() {
  if (!golfView.raf) golfView.raf = requestAnimationFrame(golfTick);
}
function golfTick(now) {
  golfView.raf = 0;
  if (drawGolf(now)) golfView.raf = requestAnimationFrame(golfTick);
}
new ResizeObserver(() => golfLoop()).observe(golfCanvas);

/** Yol üzerinde t (0–1) anındaki nokta. */
function pointAlong(path, t) {
  const f = t * (path.length - 1);
  const i = Math.min(path.length - 2, Math.floor(f));
  const k = f - i;
  return [path[i][0] + (path[i + 1][0] - path[i][0]) * k, path[i][1] + (path[i + 1][1] - path[i][1]) * k];
}

/** Bir kare çizer; canlandırma ya da nişan sürüyorsa `true` döner. */
function drawGolf(now) {
  const g = gameById(gameState.open);
  if (!g || g.game !== 'golf' || !gamesOpen() || !g.info?.holes) return false;
  const info = g.info;
  const r = golfCanvas.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const w = Math.max(1, Math.round(r.width * dpr));
  const h = Math.max(1, Math.round(r.height * dpr));
  if (golfCanvas.width !== w || golfCanvas.height !== h) { golfCanvas.width = w; golfCanvas.height = h; }
  const ctx = golfCanvas.getContext('2d');
  const k = w / info.width;
  ctx.setTransform(k, 0, 0, k, 0, 0);
  ctx.clearRect(0, 0, info.width, info.height);

  let holeIdx = info.hole;
  let animPos = null;
  let animating = false;
  const a = golfView.anim;
  if (a) {
    const dur = Math.max(250, (a.shot.path.length - 1) * GOLF_POINT_MS);
    const t = Math.min(1, (now - a.start) / dur);
    holeIdx = a.shot.hole;
    animPos = pointAlong(a.shot.path, t);
    animating = t < 1;
    if (!animating) {
      golfView.anim = null;
      golfFlash(a.shot);
      holeIdx = info.hole;
      animPos = null;
    }
  }
  drawCourse(ctx, info.holes[holeIdx], info);
  const sameHole = holeIdx === info.hole;
  const order = g.players.map((p, seat) => seat).sort((x, y) => (x === g.turn) - (y === g.turn));
  let waiting = 0;
  for (const seat of order) {
    const p = g.players[seat];
    const b = info.balls[seat];
    if (info.quit[seat]) continue;
    let pos = null;
    if (animPos && a.shot.seat === seat) pos = animPos;
    else if (sameHole && !b.done) pos = [b.x, b.y];
    // Henüz vurmamış ve sırası gelmemiş toplar başlangıcın yanında sıraya dizilir (üst üste binmesin).
    if (pos && !b.strokes && seat !== g.turn && !(animPos && a.shot.seat === seat)) {
      pos = [pos[0] - 24 - 16 * (waiting % 3), pos[1] + 18 * (Math.floor(waiting / 3) - 0.5)];
      waiting++;
    }
    if (pos) drawBall(ctx, pos, info.ballR, golfColor(p.fingerprint), !animating && g.status === 'active' && seat === g.turn, now);
  }
  const aiming = !!golfView.aim && !animating && g.myTurn;
  if (aiming) drawAim(ctx, info);
  // Yalnızca canlandırma ve nişan sürerken çizmeye devam edilir (boştayken işlemci harcanmaz).
  return animating || aiming;
}

function drawCourse(ctx, hole, info) {
  const poly = (pts) => {
    ctx.beginPath();
    pts.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
    ctx.closePath();
  };
  // Duvarlar: sınırın dışına taşan kalın çizgi; çim üstüne dökülünce iç yarısı örtülür.
  ctx.lineJoin = 'round';
  poly(hole.bounds);
  ctx.strokeStyle = '#6b4329';
  ctx.lineWidth = 18;
  ctx.stroke();
  ctx.strokeStyle = '#8a5a39';
  ctx.lineWidth = 12;
  ctx.stroke();
  ctx.save();
  poly(hole.bounds);
  ctx.fillStyle = '#3f9d4f';
  ctx.fill();
  ctx.clip();
  ctx.fillStyle = 'rgba(255, 255, 255, .055)';
  for (let x = 0; x < info.width; x += 70) ctx.fillRect(x, 0, 35, info.height);
  for (const [x, y, w, h] of hole.sand) {
    ctx.fillStyle = '#e6d39b';
    ctx.beginPath(); ctx.roundRect(x, y, w, h, 14); ctx.fill();
    ctx.fillStyle = 'rgba(150, 120, 60, .35)';
    for (let i = 0; i < (w * h) / 220; i++) {
      const px = x + 6 + ((i * 37) % (w - 12));
      const py = y + 6 + ((i * 53) % (h - 12));
      ctx.fillRect(px, py, 2, 2);
    }
  }
  for (const [x, y, w, h] of hole.water) {
    ctx.fillStyle = '#3b8fd9';
    ctx.beginPath(); ctx.roundRect(x, y, w, h, 16); ctx.fill();
    ctx.strokeStyle = 'rgba(255, 255, 255, .35)';
    ctx.lineWidth = 2;
    for (let yy = y + 18; yy < y + h - 8; yy += 22) {
      ctx.beginPath();
      for (let xx = x + 10; xx < x + w - 10; xx += 4) ctx.lineTo(xx, yy + Math.sin(xx / 9) * 3);
      ctx.stroke();
    }
  }
  ctx.restore();
  for (const b of hole.blocks) {
    poly(b);
    ctx.fillStyle = '#8a5a39';
    ctx.fill();
    ctx.strokeStyle = '#6b4329';
    ctx.lineWidth = 2;
    ctx.stroke();
  }
  for (const [x, y, r] of hole.posts) {
    ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2);
    ctx.fillStyle = '#9aa3ad'; ctx.fill();
    ctx.lineWidth = 3; ctx.strokeStyle = '#6e7781'; ctx.stroke();
    ctx.beginPath(); ctx.arc(x - r * 0.3, y - r * 0.3, r * 0.35, 0, Math.PI * 2);
    ctx.fillStyle = 'rgba(255, 255, 255, .35)'; ctx.fill();
  }
  // Başlangıç minderi
  const [tx, ty] = hole.tee;
  ctx.fillStyle = 'rgba(255, 255, 255, .28)';
  ctx.beginPath(); ctx.roundRect(tx - 16, ty - 16, 32, 32, 6); ctx.fill();
  // Delik ve bayrak
  const [cx, cy] = hole.cup;
  ctx.beginPath(); ctx.arc(cx, cy, info.cupR, 0, Math.PI * 2);
  ctx.fillStyle = '#16231a'; ctx.fill();
  ctx.lineWidth = 2; ctx.strokeStyle = 'rgba(255, 255, 255, .5)'; ctx.stroke();
  ctx.strokeStyle = '#f2f2f2'; ctx.lineWidth = 2.5;
  ctx.beginPath(); ctx.moveTo(cx + 1, cy); ctx.lineTo(cx + 1, cy - 46); ctx.stroke();
  ctx.fillStyle = '#e5484d';
  ctx.beginPath(); ctx.moveTo(cx + 2, cy - 46); ctx.lineTo(cx + 26, cy - 38); ctx.lineTo(cx + 2, cy - 30); ctx.closePath(); ctx.fill();
}

function drawBall(ctx, [x, y], r, color, active, now) {
  if (active) {
    // Sırası gelen top: sabit çift halka (yanıp sönme sürekli çizim gerektirirdi).
    ctx.lineWidth = 2.5;
    ctx.strokeStyle = color;
    ctx.globalAlpha = 0.6;
    ctx.beginPath(); ctx.arc(x, y, r + 6, 0, Math.PI * 2); ctx.stroke();
    ctx.globalAlpha = 0.25;
    ctx.beginPath(); ctx.arc(x, y, r + 11, 0, Math.PI * 2); ctx.stroke();
    ctx.globalAlpha = 1;
  }
  ctx.beginPath(); ctx.ellipse(x + 2, y + 3, r, r * 0.8, 0, 0, Math.PI * 2);
  ctx.fillStyle = 'rgba(0, 0, 0, .25)'; ctx.fill();
  ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2);
  ctx.fillStyle = '#fff'; ctx.fill();
  ctx.lineWidth = 3; ctx.strokeStyle = color; ctx.stroke();
}

function drawAim(ctx, info) {
  const { ball: [bx, by], cur: [px, py] } = golfView.aim;
  const { angle, power } = aimShot();
  // Lastik: toptan imlece
  ctx.setLineDash([4, 5]);
  ctx.strokeStyle = 'rgba(255, 255, 255, .6)'; ctx.lineWidth = 2;
  ctx.beginPath(); ctx.moveTo(bx, by); ctx.lineTo(px, py); ctx.stroke();
  ctx.setLineDash([]);
  if (power < 0.04) return;
  // Gidiş yönü ve güç
  const len = 40 + power * 240;
  const [dx, dy] = [Math.cos(angle), Math.sin(angle)];
  const color = `hsl(${120 - 120 * power} 85% 50%)`;
  ctx.strokeStyle = color; ctx.lineWidth = 4; ctx.lineCap = 'round';
  ctx.setLineDash([2, 10]);
  ctx.beginPath(); ctx.moveTo(bx + dx * (info.ballR + 6), by + dy * (info.ballR + 6)); ctx.lineTo(bx + dx * len, by + dy * len); ctx.stroke();
  ctx.setLineDash([]);
  const [ex, ey] = [bx + dx * len, by + dy * len];
  ctx.fillStyle = color;
  ctx.beginPath();
  ctx.moveTo(ex + dx * 10, ey + dy * 10);
  ctx.lineTo(ex - dy * 7, ey + dx * 7);
  ctx.lineTo(ex + dy * 7, ey - dx * 7);
  ctx.closePath(); ctx.fill();
  ctx.font = 'bold 16px "Segoe UI", sans-serif';
  ctx.fillStyle = '#fff'; ctx.strokeStyle = 'rgba(0, 0, 0, .55)'; ctx.lineWidth = 3;
  const label = `%${Math.max(1, Math.round(power * 100))}`;
  ctx.strokeText(label, bx + 14, by - 14);
  ctx.fillText(label, bx + 14, by - 14);
}

/** Geri çekme vektöründen vuruş: yön imlecin tersine, güç çekme uzunluğuyla. */
function aimShot() {
  const { ball: [bx, by], cur: [px, py] } = golfView.aim;
  const [dx, dy] = [bx - px, by - py];
  return { angle: Math.atan2(dy, dx), power: Math.min(1, Math.hypot(dx, dy) / GOLF_PULL) };
}

function golfPoint(e) {
  const g = gameById(gameState.open);
  const r = golfCanvas.getBoundingClientRect();
  const k = (g?.info?.width || 800) / r.width;
  return [(e.clientX - r.left) * k, (e.clientY - r.top) * k];
}

golfCanvas.addEventListener('pointerdown', (e) => {
  const g = gameById(gameState.open);
  if (e.button !== 0 || !g || g.game !== 'golf' || !g.myTurn || golfView.anim) return;
  const b = g.info.balls[g.mySeat];
  golfView.aim = { ball: [b.x, b.y], cur: golfPoint(e), pointerId: e.pointerId };
  golfCanvas.setPointerCapture(e.pointerId);
  e.preventDefault();
  golfLoop();
});
golfCanvas.addEventListener('pointermove', (e) => {
  if (!golfView.aim) return;
  golfView.aim.cur = golfPoint(e);
  golfLoop();
});
golfCanvas.addEventListener('pointerup', async (e) => {
  const aim = golfView.aim;
  const g = gameById(gameState.open);
  if (!aim) return;
  aim.cur = golfPoint(e);
  const { angle, power } = aimShot();
  golfView.aim = null;
  golfLoop();
  if (!g?.myTurn || power < 0.04) return;
  const deg10 = ((Math.round((angle * 180 / Math.PI) * 10) % 3600) + 3600) % 3600;
  const mv = `a${deg10}p${Math.max(1, Math.min(100, Math.round(power * 100)))}`;
  await call('game_act', { id: g.id, action: { kind: 'move', mv } }).catch(() => {});
});
golfCanvas.addEventListener('pointercancel', () => { golfView.aim = null; golfLoop(); });
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && golfView.aim) { golfView.aim = null; golfLoop(); e.stopImmediatePropagation(); }
}, true);

function golfFlash(shot) {
  const g = gameById(gameState.open);
  const who = g?.players[shot.seat];
  const name = who?.fingerprint === myFp() ? 'Siz' : who?.name || '';
  const text = shot.result === 'in' ? (shot.strokes === 1 ? `${name}: tek vuruşta delik! 🎉` : `${name}: delikte! ⛳ (${shot.strokes} vuruş)`)
    : shot.result === 'water' ? `${name}: suya düştü! +1 ceza`
    : shot.result === 'max' ? `${name}: vuruş hakkı doldu`
    : '';
  const el = $('#golf-flash');
  if (!text) return;
  el.textContent = text;
  el.className = `golf-flash ${shot.result}`;
  el.hidden = false;
  clearTimeout(golfView.flashTimer);
  golfView.flashTimer = setTimeout(() => { el.hidden = true; }, 1800);
}

function renderScores(g) {
  const info = g.info;
  if (!info?.holes) return;
  const live = g.status === 'active';
  const parAt = (n) => info.holes.slice(0, n).reduce((s, h) => s + h.par, 0);
  const head = `<tr><th></th>${info.holes.map((h, i) => `<th class="${live && i === info.hole ? 'cur' : ''}" title="${esc(h.name)}">${i + 1}</th>`).join('')}<th>Top.</th></tr>`;
  const par = `<tr class="par"><td>Par</td>${info.holes.map((h) => `<td>${h.par}</td>`).join('')}<td>${parAt(info.holes.length)}</td></tr>`;
  const winner = g.result?.winner;
  const rows = g.players.map((p, seat) => {
    const done = info.scores[seat] || [];
    const cells = info.holes.map((h, i) => {
      if (i < done.length) {
        const s = done[i];
        if (s == null) return '<td class="q">—</td>';
        return `<td class="${s < h.par ? 'under' : s > h.par ? 'over' : ''}">${s}</td>`;
      }
      if (live && i === info.hole && !info.quit[seat]) {
        const b = info.balls[seat];
        return `<td class="now">${b.strokes || ''}${b.done ? '' : b.strokes ? '…' : ''}</td>`;
      }
      return '<td></td>';
    }).join('');
    const diff = info.totals[seat] - parAt(done.length);
    const rel = done.length ? (diff === 0 ? 'E' : diff > 0 ? `+${diff}` : String(diff)) : '';
    return `<tr class="${p.fingerprint === winner ? 'win' : ''}${info.quit[seat] ? ' quit' : ''}">
      <td class="who"><i style="background:${golfColor(p.fingerprint)}"></i>${esc(p.fingerprint === myFp() ? 'Siz' : p.name)}</td>
      ${cells}<td class="tot">${info.totals[seat]}<small>${rel}</small></td></tr>`;
  }).join('');
  $('#moves').innerHTML = `<table class="golf-scores">${head}${par}${rows}</table>
    <div class="golf-legend">Her delikte en fazla ${info.maxStrokes} vuruş. Suya düşen top +1 ceza ile geri gelir.</div>`;
}

$('#room-actions').addEventListener('click', async (e) => {
  const b = e.target.closest('[data-room-act]');
  const g = gameById(gameState.open);
  if (!b || !g) return;
  const act = b.dataset.roomAct;
  if (act === 'rematch') return rematch(g);
  if (act === 'resign') {
    const ok = g.game === 'golf'
      ? await confirmDialog('Oyundan çekilmek istiyor musunuz?', 'Diğer oyuncular oyuna devam eder; tek kişi kalırsa o kazanır.', 'Çekil', true)
      : await confirmDialog('Pes etmek istiyor musunuz?', `Oyunu ${vsName(g)} kazanmış sayılır.`, 'Pes et', true);
    if (!ok) return;
  }
  b.disabled = true;
  const kind = act === 'cancel' ? 'decline' : act;
  await call('game_act', { id: g.id, action: { kind } }).catch(() => {});
});

async function rematch(g) {
  const two = g.players.length === 2 && g.mySeat != null;
  try {
    const n = await call('game_invite', { game: g.game, players: opponentsOf(g).map((p) => p.fingerprint), seat: two ? 1 - g.mySeat : null });
    if (!gameById(n.id)) state.games = [n, ...state.games];
    openRoom(n.id);
    toast(`Davet gönderildi → ${vsName(n)}`, { type: 'success' });
  } catch { /* toast gösterildi */ }
}

$('#room-chat').onclick = () => {
  const opp = opponentsOf(gameById(gameState.open) || { players: [] })[0];
  if (!opp) return;
  closeGames();
  openChat(opp.fingerprint, opp.name);
};

$('#room-remove').onclick = async () => {
  const g = gameById(gameState.open);
  if (!g) return;
  const text = g.status === 'active' && g.game === 'golf' ? 'Oyun sürüyor; kaldırırsanız oyundan çekilmiş sayılırsınız.'
    : g.status === 'active' ? 'Oyun sürüyor; kaldırırsanız pes etmiş sayılırsınız.'
    : g.status === 'invited' ? (g.isHost ? 'Davet geri çekilir.' : 'Davet reddedilir.')
    : 'Yalnızca sizin listenizden kaldırılır.';
  if (!(await confirmDialog('Oyun listeden kaldırılsın mı?', text, 'Kaldır', true))) return;
  await call('game_remove', { id: g.id }).catch(() => {});
  state.games = state.games.filter((x) => x.id !== g.id);
  closeRoom();
};

// ── Hamle: tıkla-tıkla ya da sürükle-bırak ──

const boardEl = $('#board');

function sqFromPoint(x, y) {
  const r = boardEl.getBoundingClientRect();
  if (x < r.left || y < r.top || x >= r.right || y >= r.bottom) return null;
  const col = Math.floor((x - r.left) / (r.width / 8));
  const row = Math.floor((y - r.top) / (r.height / 8));
  const flip = boardEl.classList.contains('flipped');
  return sqName((flip ? row : 7 - row) * 8 + (flip ? 7 - col : col));
}

boardEl.addEventListener('pointerdown', (e) => {
  if (e.button !== 0) return;
  const g = gameById(gameState.open);
  const sq = g && sqFromPoint(e.clientX, e.clientY);
  if (!sq) return;
  const sel = gameState.selected;
  if (sel && sel !== sq && legalFrom(g, sel).some((m) => m.slice(2, 4) === sq)) {
    tryMove(g, sel, sq);
    return;
  }
  if (legalFrom(g, sq).length) {
    e.preventDefault();
    gameState.drag = { from: sq, x: e.clientX, y: e.clientY, moved: false, wasSelected: sel === sq, pointerId: e.pointerId };
    gameState.selected = sq;
    boardEl.setPointerCapture(e.pointerId);
    renderBoard(g);
    return;
  }
  if (sel) { gameState.selected = null; renderBoard(g); }
});

boardEl.addEventListener('pointermove', (e) => {
  const d = gameState.drag;
  if (!d) return;
  if (!d.moved) {
    if (Math.hypot(e.clientX - d.x, e.clientY - d.y) < 4) return;
    d.moved = true;
    const src = boardEl.querySelector(`[data-sq="${d.from}"] .pc`);
    if (src) {
      d.ghost = src.cloneNode(true);
      d.ghost.classList.add('ghost');
      d.ghost.style.fontSize = getComputedStyle(src).fontSize;
      document.body.appendChild(d.ghost);
      src.classList.add('lifted');
    }
  }
  if (d.ghost) { d.ghost.style.left = `${e.clientX}px`; d.ghost.style.top = `${e.clientY}px`; }
  const over = sqFromPoint(e.clientX, e.clientY);
  if (over !== d.over) {
    boardEl.querySelector('.sq.hover')?.classList.remove('hover');
    if (over) boardEl.querySelector(`[data-sq="${over}"]`)?.classList.add('hover');
    d.over = over;
  }
});

function endPieceDrag(e, cancelled) {
  const d = gameState.drag;
  if (!d) return;
  gameState.drag = null;
  d.ghost?.remove();
  try { boardEl.releasePointerCapture(d.pointerId); } catch { /* bırakılmış */ }
  const g = gameById(gameState.open);
  if (!g) return;
  if (d.moved && !cancelled) {
    const to = sqFromPoint(e.clientX, e.clientY);
    if (to && to !== d.from && legalFrom(g, d.from).some((m) => m.slice(2, 4) === to)) { tryMove(g, d.from, to); return; }
  } else if (!d.moved && d.wasSelected) {
    gameState.selected = null; // seçili taşa ikinci tıklama seçimi kaldırır
  }
  renderBoard(g);
}
boardEl.addEventListener('pointerup', (e) => endPieceDrag(e, false));
boardEl.addEventListener('pointercancel', (e) => endPieceDrag(e, true));

function cancelDrag() {
  if (!gameState.drag) return;
  gameState.drag.ghost?.remove();
  gameState.drag = null;
}

function tryMove(g, from, to) {
  const cands = g.legal.filter((m) => m.startsWith(from + to));
  if (!cands.length) return;
  gameState.selected = null;
  if (cands.length > 1) { renderBoard(g); showPromo(g, from, to); return; }
  sendMove(g, cands[0]);
}

async function sendMove(g, uci) {
  gameState.optimistic = { id: g.id, uci, base: g.moves.length, inCall: true };
  renderRoom();
  try {
    await api.invoke('game_act', { id: g.id, action: { kind: 'move', mv: uci } });
    if (gameState.optimistic?.uci === uci) gameState.optimistic.inCall = false;
  } catch (e) {
    gameState.optimistic = null;
    toast(String(e), { type: 'error' });
  }
  maybeClearOptimistic();
  renderRoom();
}

/** Hamle odaya işlendiyse ya da sahibi reddettiyse geçici gösterimi kaldır. */
function maybeClearOptimistic() {
  const o = gameState.optimistic;
  if (!o) return;
  const g = gameById(o.id);
  if (!g || g.moves.length !== o.base || (!g.pending && !o.inCall)) gameState.optimistic = null;
}

function showPromo(g, from, to) {
  const color = g.mySeat === 1 ? 'b' : 'w';
  const names = { q: 'Vezir', r: 'Kale', b: 'Fil', n: 'At' };
  const el = $('#promo');
  el.innerHTML = `<div class="promo-title">Piyon neye dönüşsün?</div><div class="promo-row">${['q', 'r', 'b', 'n']
    .map((t) => `<button data-promo="${t}" title="${names[t]}">${pieceHtml(color === 'w' ? t.toUpperCase() : t)}<span>${names[t]}</span></button>`).join('')}</div>`;
  el.hidden = false;
  gameState.promo = { id: g.id, from, to };
}

function closePromo() {
  $('#promo').hidden = true;
  gameState.promo = null;
}

$('#promo').addEventListener('click', (e) => {
  const p = gameState.promo;
  const b = e.target.closest('[data-promo]');
  if (!p) return;
  const g = gameById(p.id);
  closePromo();
  if (b && g) sendMove(g, p.from + p.to + b.dataset.promo);
  else if (g) renderBoard(g);
});

// ── Yeni oyun / davet ──

function showGameCreate({ game, fp } = {}) {
  const defs = state.gameCatalog;
  if (!defs.length) return toast('Oyun listesi yüklenemedi', { type: 'error' });
  let chosen = gameDef(game) || defs[0];
  const selected = new Set(fp && !isBlocked(fp) ? [fp] : []);
  let seat = 'random';
  const m = openModal('game-new', `
    <h3>Yeni oyun</h3>
    <p class="lead">Rakibinize davet gider. Kabul edince oyun odası açılır ve oynamaya başlarsınız.</p>
    <div class="field"><label>Oyun</label><div class="game-pick" id="gn-games"></div></div>
    <div class="field"><label id="gn-people-label">Rakip</label><div class="recips" id="gn-people"></div></div>
    <div class="field" id="gn-seat-field"><label>Taşlarınız</label><div class="seg" id="gn-seat"></div></div>
    <div class="err" id="gn-err"></div>
    <div class="foot">
      <button class="btn ghost" data-close>Vazgeç</button>
      <button class="btn" id="gn-send">Davet gönder</button>
    </div>`);
  const people = candidatePeople();
  const maxOpp = () => chosen.maxPlayers - 1;
  const render = () => {
    const single = maxOpp() === 1;
    m.querySelector('#gn-games').innerHTML = defs.map((d) => `
      <button class="gp ${d.id === chosen.id ? 'on' : ''}" data-g="${esc(d.id)}">${GAME_ART[d.id] || ''}<span>${esc(d.name)}</span></button>`).join('');
    m.querySelector('#gn-people-label').textContent = single ? 'Rakip' : `Rakipler (en fazla ${maxOpp()})`;
    m.querySelector('#gn-people').innerHTML = people.length
      ? people.map((p) => personRow(p, selected.has(p.fingerprint)).replace('type="checkbox"', single ? 'type="radio" name="gn-opp"' : 'type="checkbox"')).join('')
      : '<span class="muted small">Davet edilecek kimse yok. Ağda başka bir FastFiles görünmeli.</span>';
    const seats = chosen.seats || [];
    m.querySelector('#gn-seat-field').hidden = seats.length !== 2;
    m.querySelector('#gn-seat').innerHTML = [['random', 'Rastgele'], ...seats.map((s, i) => [String(i), s])]
      .map(([v, l]) => `<button class="${seat === v ? 'on' : ''}" data-seat="${v}">${esc(l)}</button>`).join('');
  };
  m.querySelector('#gn-games').addEventListener('click', (e) => {
    const b = e.target.closest('[data-g]');
    if (!b) return;
    chosen = gameDef(b.dataset.g) || chosen;
    while (selected.size > maxOpp()) selected.delete([...selected].pop());
    render();
  });
  m.querySelector('#gn-people').addEventListener('change', (e) => {
    const f = e.target.closest('.recip')?.dataset.fp;
    if (!f) return;
    if (maxOpp() === 1) { selected.clear(); selected.add(f); }
    else if (e.target.checked) { if (selected.size < maxOpp()) selected.add(f); }
    else selected.delete(f);
    render();
  });
  m.querySelector('#gn-seat').addEventListener('click', (e) => {
    const b = e.target.closest('[data-seat]');
    if (b) { seat = b.dataset.seat; render(); }
  });
  m.querySelector('[data-close]').onclick = closeModal;
  m.querySelector('#gn-send').onclick = async () => {
    const err = m.querySelector('#gn-err');
    if (selected.size < chosen.minPlayers - 1) {
      err.textContent = maxOpp() === 1 ? 'Bir rakip seçin' : `En az ${chosen.minPlayers - 1} rakip seçin`;
      return;
    }
    try {
      const g = await api.invoke('game_invite', { game: chosen.id, players: [...selected], seat: seat === 'random' ? null : Number(seat) });
      if (!gameById(g.id)) state.games = [g, ...state.games];
      closeModal();
      openRoom(g.id);
      toast(`Davet gönderildi → ${vsName(g)}`, { type: 'success' });
    } catch (e) {
      err.textContent = String(e);
    }
  };
  render();
}

// ───────────────────────────── Durum ─────────────────────────────
// Müsait / Toplantıda / Odakta / Dışarıda. Elle seçilir; "Müsait"teyken 10 dakika işlem
// yapılmazsa arka uç kendiliğinden "Dışarıda" yapar. "Odakta" iken bildirimler susar.

const STATUS = {
  '': { label: 'Müsait', icon: '🟢', help: 'Herkes ulaşabilir' },
  busy: { label: 'Toplantıda', icon: '📅', help: 'Toplantıdayım, geç dönebilirim' },
  focus: { label: 'Odakta', icon: '🎧', help: 'Bildirimler susar (adınız geçince gelir)' },
  away: { label: 'Dışarıda', icon: '☕', help: 'Bilgisayar başında değilim' },
};
const statusOf = (s) => STATUS[s] || STATUS[''];
const myStatus = () => state.myStatus ?? state.device?.status ?? '';

function renderMyStatus() {
  const s = myStatus();
  $('#me').dataset.status = s;
  const el = $('#me-status');
  el.textContent = s ? `${statusOf(s).icon} ${statusOf(s).label}` : '';
  el.hidden = !s;
  el.title = s === 'away' && !state.settings?.status ? '10 dakikadır işlem yapılmadı' : '';
}

function statusChip(s) {
  return s ? `<span class="st-chip st-${esc(s)}">${statusOf(s).icon} ${esc(statusOf(s).label)}</span>` : '';
}

function closeStatusMenu() {
  document.getElementById('status-menu')?.remove();
  document.removeEventListener('mousedown', statusMenuOutside, true);
}
function statusMenuOutside(e) {
  if (!e.target.closest('#status-menu') && !e.target.closest('#me')) closeStatusMenu();
}
function showStatusMenu() {
  closeStatusMenu();
  const cur = state.settings?.status || '';
  const pop = document.createElement('div');
  pop.className = 'status-menu';
  pop.id = 'status-menu';
  pop.innerHTML = `<div class="sm-head">Durumunuz</div>${Object.entries(STATUS).map(([k, v]) => `
    <button data-status="${k}" class="${cur === k ? 'on' : ''}"><span class="sm-ic">${v.icon}</span>
      <span class="sm-txt"><b>${esc(v.label)}</b><small>${esc(v.help)}</small></span></button>`).join('')}
    <div class="sm-sep"></div>
    <button data-settings><span class="sm-ic">⚙️</span><span class="sm-txt"><b>Ayarlar</b></span></button>`;
  document.body.appendChild(pop);
  pop.querySelector('[data-settings]').onclick = () => { closeStatusMenu(); showSettings(); };
  const r = $('#me').getBoundingClientRect();
  pop.style.left = `${Math.max(8, Math.min(window.innerWidth - 270, r.left))}px`;
  pop.style.top = `${r.bottom + 6}px`;
  pop.addEventListener('click', async (e) => {
    const b = e.target.closest('[data-status]');
    if (!b) return;
    closeStatusMenu();
    try {
      state.settings = await call('set_status', { status: b.dataset.status });
      state.myStatus = b.dataset.status;
      renderMyStatus();
      officeSyncPeople();
      toast(`Durumunuz: ${statusOf(b.dataset.status).icon} ${statusOf(b.dataset.status).label}`, { type: 'success', timeout: 2500 });
    } catch { /* toast gösterildi */ }
  });
  setTimeout(() => document.addEventListener('mousedown', statusMenuOutside, true), 0);
}
$('#me').addEventListener('click', () => (document.getElementById('status-menu') ? closeStatusMenu() : showStatusMenu()));
$('#me').addEventListener('keydown', (e) => {
  if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); showStatusMenu(); }
});

// ───────────────────────────── Sohbet: tepki, yanıt, düzenleme, arama, @bahsetme ─────────────────────────────

const REACTIONS = ['👍', '❤️', '😂', '😮', '😢', '🙏', '🎉', '🔥'];
const chatx = { reply: null, edit: null, search: '', hit: -1, mention: null };
const reEsc = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
ic.reply = '<svg viewBox="0 0 24 24"><path d="M10 8 5 12l5 4M5 12h9a5 5 0 0 1 5 5v1"/></svg>';
ic.edit = '<svg viewBox="0 0 24 24"><path d="M4 20h4L19 9l-4-4L4 16z"/><path d="m13.5 6.5 4 4"/></svg>';
ic.smile = '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/><path d="M8.5 14.5s1.3 2 3.5 2 3.5-2 3.5-2M9 9.5h.01M15 9.5h.01"/></svg>';

function resetChatExtras() {
  chatx.reply = null;
  chatx.edit = null;
  chatx.search = '';
  chatx.hit = -1;
  $('#chat-searchbar').hidden = true;
  $('#chat-search-input').value = '';
  $('#mention-pop').hidden = true;
  renderReplyBar();
}

function searchRegex() {
  return chatx.search ? new RegExp(reEsc(chatx.search), 'giu') : null;
}

// Sohbetteki bağlantılar: yalnızca http(s):// ve www. ile başlayanlar. Bağlantının görünen metni
// adresin kendisidir (başka bir yazının arkasına gizlenemez); tıklanınca arka uç adresi yeniden
// denetleyip varsayılan tarayıcıda açar. Sondaki noktalama bağlantıya dahil edilmez.
const LINK_RE = /\b(?:https?:\/\/|www\.)[^\s<>"'`]+/giu;
function trimLink(url) {
  let u = url.replace(/[.,;:!?…'"]+$/u, '');
  // "(bkz. https://a.com/x)" gibi: eşi olmayan kapanış parantezi bağlantıya ait değildir.
  for (const [o, c] of [['(', ')'], ['[', ']'], ['{', '}']]) {
    while (u.endsWith(c) && u.split(c).length > u.split(o).length) u = u.slice(0, -1);
  }
  return u;
}

/** Mesaj metni: bağlantılar tıklanabilir, grupta @ad vurgulanır, aramada eşleşmeler işaretlenir. */
function messageBody(text, key, hitRe) {
  let out = '';
  let last = 0;
  for (const mm of text.matchAll(LINK_RE)) {
    const url = trimLink(mm[0]);
    if (!url.includes('.')) continue;
    out += messageSpan(text.slice(last, mm.index), key, hitRe);
    out += `<a class="msg-link" data-url="${esc(url)}" title="${esc(url)}" tabindex="0">${messageSpan(url, null, hitRe)}</a>`;
    last = mm.index + url.length;
  }
  return out + messageSpan(text.slice(last), key, hitRe);
}

function messageSpan(text, key, hitRe) {
  if (!text) return '';
  if (hitRe) {
    let out = '';
    let last = 0;
    for (const mm of text.matchAll(hitRe)) {
      out += `${esc(text.slice(last, mm.index))}<mark>${esc(mm[0])}</mark>`;
      last = mm.index + mm[0].length;
    }
    return out + esc(text.slice(last));
  }
  const g = groupOf(key);
  if (!g || !text.includes('@')) return esc(text);
  const names = [...new Set([...g.members.map((x) => x.name), 'herkes'])].filter(Boolean).sort((a, b) => b.length - a.length);
  const re = new RegExp(`@(${names.map(reEsc).join('|')})(?![\\p{L}\\p{N}])`, 'giu');
  let out = '';
  let last = 0;
  for (const mm of text.matchAll(re)) {
    out += `${esc(text.slice(last, mm.index))}<span class="mention">${esc(mm[0])}</span>`;
    last = mm.index + mm[0].length;
  }
  return out + esc(text.slice(last));
}

function reactionsHtml(m) {
  if (!m.reactions?.length) return '';
  const me = myFp();
  const by = new Map();
  for (const r of m.reactions) by.set(r.emoji, [...(by.get(r.emoji) || []), r.fp]);
  return `<div class="reactions">${[...by].map(([emo, fps]) => `<button class="react-pill${fps.includes(me) ? ' mine' : ''}" data-react="${esc(emo)}"
    title="${esc(fps.map((f) => (f === me ? 'Siz' : officeName(f))).join(', '))}">${esc(emo)}<span>${fps.length}</span></button>`).join('')}</div>`;
}

function msgTools(m) {
  const own = m.outgoing;
  return `<div class="msg-tools">
    <button data-mt="react" title="Tepki ver">${ic.smile}</button>
    <button data-mt="reply" title="Yanıtla">${ic.reply}</button>
    ${own ? `<button data-mt="edit" title="Düzenle">${ic.edit}</button><button data-mt="delete" title="Herkesten sil">${ic.trash}</button>` : ''}
    <button data-copy title="Kopyala">${ic.copy}</button>
  </div>`;
}

function textMessageHtml(m, dir, first, sender, hitRe) {
  const id = esc(m.id);
  if (m.deleted) {
    return `<div class="msg ${dir}${first ? ' first' : ''} deleted" data-id="${id}">${sender}
      <div class="bubble-row"><div class="bubble"><i class="deleted-text">🚫 Bu mesaj silindi</i></div></div>
      <div class="meta">${esc(fmtTime(m.at))}</div></div>`;
  }
  const quote = m.reply
    ? `<div class="quote" data-jump="${esc(m.reply.id)}"><b>${esc(m.reply.name || '')}</b><span>${esc(m.reply.text || '…')}</span></div>`
    : '';
  const hit = hitRe && new RegExp(hitRe.source, 'iu').test(m.text);
  return `
    <div class="msg ${dir}${first ? ' first' : ''}${m.status === 'failed' ? ' failed' : ''}${m.mention ? ' mentioned' : ''}${hit ? ' hit' : ''}" data-id="${id}">
      ${sender}
      <div class="bubble-row"><div class="bubble">${quote}${messageBody(m.text, state.openChat, hit ? hitRe : null)}</div>${msgTools(m)}</div>
      ${reactionsHtml(m)}
      <div class="meta">${m.edited ? '<span class="edited">düzenlendi</span> · ' : ''}${esc(fmtTime(m.at))}${m.outgoing ? statusMeta(m) : ''}</div>
    </div>`;
}

const msgById = (id) => (state.history.get(state.openChat) || []).find((x) => x.id === id);

function closeReactPicker() {
  document.getElementById('react-pop')?.remove();
  document.removeEventListener('mousedown', reactOutside, true);
}
function reactOutside(e) {
  if (!e.target.closest('#react-pop')) closeReactPicker();
}
function showReactPicker(anchor, id) {
  closeReactPicker();
  const mine = msgById(id)?.reactions?.find((r) => r.fp === myFp())?.emoji;
  const pop = document.createElement('div');
  pop.className = 'react-pop';
  pop.id = 'react-pop';
  pop.innerHTML = REACTIONS.map((e) => `<button data-pick="${e}" class="${e === mine ? 'on' : ''}">${e}</button>`).join('');
  document.body.appendChild(pop);
  const r = anchor.getBoundingClientRect();
  pop.style.left = `${Math.max(8, Math.min(window.innerWidth - pop.offsetWidth - 8, r.left + r.width / 2 - pop.offsetWidth / 2))}px`;
  pop.style.top = `${Math.max(8, r.top - pop.offsetHeight - 6)}px`;
  pop.addEventListener('click', (e) => {
    const b = e.target.closest('[data-pick]');
    if (!b) return;
    closeReactPicker();
    toggleReaction(id, b.dataset.pick);
  });
  setTimeout(() => document.addEventListener('mousedown', reactOutside, true), 0);
}
async function toggleReaction(id, emoji) {
  const key = state.openChat;
  const mine = msgById(id)?.reactions?.find((r) => r.fp === myFp())?.emoji;
  await call('chat_react', { key, id, emoji: mine === emoji ? '' : emoji }).catch(() => {});
}

function startReply(m) {
  if (chatx.edit) $('#composer').value = '';
  chatx.edit = null;
  const name = m.outgoing ? 'Kendinizi' : (m.senderName || state.chats.get(state.openChat)?.name || '');
  chatx.reply = { id: m.id, name, text: m.text };
  renderReplyBar();
  $('#composer').focus();
}
function startEdit(m) {
  chatx.reply = null;
  chatx.edit = { id: m.id };
  const ta = $('#composer');
  ta.value = m.text;
  autoGrow();
  updateComposerNote();
  renderReplyBar();
  ta.disabled = false;
  ta.focus();
  ta.setSelectionRange(ta.value.length, ta.value.length);
  $('#chat-send').disabled = false;
}
function cancelCompose() {
  if (chatx.edit) {
    $('#composer').value = '';
    autoGrow();
    updateComposerNote();
    $('#chat-send').disabled = true;
  }
  chatx.reply = null;
  chatx.edit = null;
  renderReplyBar();
  renderChatHead();
}
function renderReplyBar() {
  const bar = $('#reply-bar');
  const r = chatx.edit
    ? { title: '✎ Mesajı düzenliyorsunuz', text: 'Enter: kaydet · Esc: vazgeç' }
    : chatx.reply ? { title: `↩ ${chatx.reply.name} yanıtlıyorsunuz`, text: chatx.reply.text } : null;
  bar.hidden = !r;
  bar.classList.toggle('editing', !!chatx.edit);
  if (r) {
    $('#reply-title').textContent = r.title;
    $('#reply-text').textContent = r.text.replace(/\s+/g, ' ').slice(0, 180);
  }
}
$('#reply-cancel').onclick = cancelCompose;

function jumpTo(id) {
  const el = document.querySelector(`#messages .msg[data-id="${CSS.escape(id)}"]`);
  if (!el) return toast('Alıntılanan mesaj bu sohbette artık yok', { timeout: 2500 });
  el.scrollIntoView({ block: 'center', behavior: 'smooth' });
  el.classList.remove('flash');
  void el.offsetWidth;
  el.classList.add('flash');
}

$('#messages').addEventListener('click', async (e) => {
  const pill = e.target.closest('[data-react]');
  const msgEl = e.target.closest('.msg');
  if (pill && msgEl) return toggleReaction(msgEl.dataset.id, pill.dataset.react);
  const q = e.target.closest('[data-jump]');
  if (q) return jumpTo(q.dataset.jump);
  const t = e.target.closest('[data-mt]');
  if (!t || !msgEl) return;
  const m = msgById(msgEl.dataset.id);
  if (!m) return;
  if (t.dataset.mt === 'react') showReactPicker(t, m.id);
  if (t.dataset.mt === 'reply') startReply(m);
  if (t.dataset.mt === 'edit') startEdit(m);
  if (t.dataset.mt === 'delete') {
    const ok = await confirmDialog('Mesaj herkesten silinsin mi?', 'Mesaj sizde ve karşı tarafta "Bu mesaj silindi" olarak görünür.', 'Herkesten sil', true);
    if (ok) call('chat_delete', { key: state.openChat, id: m.id }).catch(() => {});
  }
});
$('#messages').addEventListener('dblclick', (e) => {
  // Çift tıklama hızlı 👍
  const el = e.target.closest('.msg:not(.deleted):not(.file-msg)');
  if (el && !e.target.closest('.msg-tools, .reactions, .quote') && !window.getSelection()?.toString()) toggleReaction(el.dataset.id, '👍');
});

// ── Arama ──
function openSearch() {
  if (!chatOpen()) return;
  $('#chat-searchbar').hidden = false;
  const i = $('#chat-search-input');
  i.focus();
  i.select();
}
function closeSearch() {
  $('#chat-searchbar').hidden = true;
  $('#chat-search-input').value = '';
  chatx.search = '';
  chatx.hit = -1;
  renderMessages(true);
}
function updateSearchHits() {
  const hits = [...document.querySelectorAll('#messages .msg.hit')];
  $('#chat-search-count').textContent = chatx.search ? (hits.length ? `${Math.max(0, chatx.hit) + 1}/${hits.length}` : 'Sonuç yok') : '';
  return hits;
}
function stepSearch(d) {
  const hits = [...document.querySelectorAll('#messages .msg.hit')];
  if (!hits.length) return updateSearchHits();
  chatx.hit = chatx.hit < 0 ? hits.length - 1 : (chatx.hit + d + hits.length) % hits.length;
  hits.forEach((h, i) => h.classList.toggle('hit-cur', i === chatx.hit));
  hits[chatx.hit].scrollIntoView({ block: 'center' });
  updateSearchHits();
}
$('#chat-search').onclick = () => ($('#chat-searchbar').hidden ? openSearch() : closeSearch());
$('#chat-search-close').onclick = closeSearch;
$('#chat-search-next').onclick = () => stepSearch(1);
$('#chat-search-prev').onclick = () => stepSearch(-1);
$('#chat-search-input').addEventListener('input', (e) => {
  chatx.search = e.target.value.trim();
  chatx.hit = -1;
  renderMessages();
  stepSearch(0);
});
$('#chat-search-input').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') { e.preventDefault(); stepSearch(e.shiftKey ? -1 : 1); }
});

// ── @bahsetme ──
function mentionQuery() {
  const ta = $('#composer');
  const pos = ta.selectionStart;
  const m = ta.value.slice(0, pos).match(/(^|\s)@([\p{L}\p{N}._-]{0,30})$/u);
  return m ? { start: pos - m[2].length - 1, end: pos, q: m[2] } : null;
}
function updateMentionPop() {
  const pop = $('#mention-pop');
  const g = groupOf(state.openChat);
  const mq = g && !chatx.edit ? mentionQuery() : null;
  if (!mq) { pop.hidden = true; chatx.mention = null; return; }
  const q = mq.q.toLocaleLowerCase('tr');
  const names = [...g.members.filter((x) => x.fingerprint !== myFp()).map((x) => x.name), 'herkes'];
  const opts = names.filter((n) => n.toLocaleLowerCase('tr').includes(q)).slice(0, 6);
  if (!opts.length) { pop.hidden = true; chatx.mention = null; return; }
  const sel = Math.min(chatx.mention?.sel || 0, opts.length - 1);
  chatx.mention = { ...mq, opts, sel };
  pop.innerHTML = opts.map((n, i) => `<button data-mention="${esc(n)}" class="${i === sel ? 'on' : ''}">${n === 'herkes' ? '📣 @herkes' : `@${esc(n)}`}</button>`).join('');
  pop.hidden = false;
}
function insertMention(name) {
  const ta = $('#composer');
  const { start, end } = chatx.mention;
  ta.value = `${ta.value.slice(0, start)}@${name} ${ta.value.slice(end)}`;
  const p = start + name.length + 2;
  ta.setSelectionRange(p, p);
  $('#mention-pop').hidden = true;
  chatx.mention = null;
  ta.focus();
  autoGrow();
}
$('#mention-pop').addEventListener('mousedown', (e) => {
  const b = e.target.closest('[data-mention]');
  if (!b) return;
  e.preventDefault();
  insertMention(b.dataset.mention);
});
$('#composer').addEventListener('input', updateMentionPop);
$('#composer').addEventListener('click', updateMentionPop);
// Yakalama aşamasında: Enter gönderiminden önce çalışsın.
$('#composer').addEventListener('keydown', (e) => {
  const mm = chatx.mention;
  if (mm && !$('#mention-pop').hidden) {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      e.stopImmediatePropagation();
      mm.sel = (mm.sel + (e.key === 'ArrowDown' ? 1 : -1) + mm.opts.length) % mm.opts.length;
      updateMentionPop();
      return;
    }
    if (e.key === 'Enter' || e.key === 'Tab') {
      e.preventDefault();
      e.stopImmediatePropagation();
      insertMention(mm.opts[mm.sel]);
      return;
    }
  }
  if (e.key === 'ArrowUp' && !$('#composer').value && !chatx.edit) {
    // Boş kutuda ↑: son mesajımı düzenle
    const last = [...(state.history.get(state.openChat) || [])].reverse().find((x) => x.outgoing && !x.file && !x.system && !x.deleted);
    if (last) { e.preventDefault(); startEdit(last); }
  }
}, true);

// ── Dürtme ──
$('#chat-nudge').onclick = async () => {
  const fp = state.openChat;
  try {
    await call('chat_nudge', { fingerprint: fp });
    $('#chat-nudge').classList.add('waved');
    setTimeout(() => $('#chat-nudge').classList.remove('waved'), 700);
  } catch { /* toast gösterildi */ }
};
function onNudge({ fingerprint, name }) {
  document.body.classList.remove('nudged');
  void document.body.offsetWidth;
  document.body.classList.add('nudged');
  setTimeout(() => document.body.classList.remove('nudged'), 700);
  if (state.openChat !== fingerprint || !chatOpen()) {
    toast(`👋 ${name} sizi dürttü`, { action: 'Aç', timeout: 8000, onAction: () => openChat(fingerprint, name) });
  }
}

// ── Çevrimdışı kişiye dosya ──
async function queueFiles(fp, paths) {
  const name = state.chats.get(fp)?.name || '';
  try {
    await call('chat_queue_files', { fingerprint: fp, name, paths });
    toast(`${name || 'Kişi'} çevrimiçi olunca dosya kendiliğinden gönderilecek`, { type: 'success' });
  } catch { /* toast gösterildi */ }
}

function onChatRemove({ fingerprint, id }) {
  const hist = state.history.get(fingerprint);
  if (hist) {
    const i = hist.findIndex((m) => m.id === id);
    if (i >= 0) hist.splice(i, 1);
  }
  if (state.openChat === fingerprint && chatOpen()) renderMessages();
}

// ───────────────────────────── Ekran görüntüsü: al, çiz, gönder ─────────────────────────────

let shotBusy = false;
async function startShot(target = null) {
  if (shotBusy || lockVisible()) return;
  shotBusy = true;
  // Uygulama içinden başlatıldıysa pencere görüntüde çıkmasın.
  if (T && document.hasFocus()) {
    try { await T.window.getCurrentWindow().minimize(); } catch { /* yok say */ }
    await new Promise((r) => setTimeout(r, 250));
  }
  let buf;
  try {
    buf = await api.invoke('screen_clip');
  } catch (e) {
    if (!/İptal|alınmadı/.test(String(e))) toast(String(e), { type: 'error' });
    shotBusy = false;
    return;
  }
  shotBusy = false;
  openShotEditor(new Blob([buf], { type: 'image/png' }), target);
}

function shotTargets() {
  const out = [];
  const seen = new Set();
  for (const p of state.peers) {
    if (!p.fingerprint || seen.has(p.fingerprint) || isBlocked(p.fingerprint)) continue;
    seen.add(p.fingerprint);
    out.push({ key: p.fingerprint, label: p.name });
  }
  for (const c of state.chats.values()) {
    if (c.group && !c.left && !c.group.deleted) out.push({ key: c.fingerprint, label: `👥 ${c.name}` });
    else if (!c.group && !seen.has(c.fingerprint) && !isBlocked(c.fingerprint)) {
      seen.add(c.fingerprint);
      out.push({ key: c.fingerprint, label: `${c.name} (çevrimiçi olunca)` });
    }
  }
  return out;
}

function openShotEditor(blob, target) {
  const url = URL.createObjectURL(blob);
  const img = new Image();
  img.onerror = () => { URL.revokeObjectURL(url); toast('Görüntü açılamadı', { type: 'error' }); };
  img.onload = () => {
    const targets = shotTargets();
    const pre = target?.key || (chatOpen() ? state.openChat : '');
    const colors = ['#ef4444', '#f59e0b', '#22c55e', '#3b82f6', '#ffffff', '#111827'];
    const tools = [['pen', '✏️', 'Kalem'], ['arrow', '➡️', 'Ok'], ['rect', '▭', 'Kutu'], ['mark', '🖍️', 'Vurgula'], ['text', 'T', 'Yazı']];
    const m = openModal('shot', `
      <div class="shot-head">
        <h3>Ekran görüntüsü</h3>
        <div class="shot-tools">
          ${tools.map(([k, i, t], n) => `<button class="tool${n === 1 ? ' on' : ''}" data-tool="${k}" title="${t}">${i}</button>`).join('')}
          <span class="tsep"></span>
          ${colors.map((c, n) => `<button class="color${n === 0 ? ' on' : ''}" data-color="${c}" style="--c:${c}" title="${c}"></button>`).join('')}
          <span class="tsep"></span>
          <button class="tool" data-undo title="Geri al (Ctrl+Z)">↶</button>
        </div>
      </div>
      <div class="shot-stage"><canvas id="shot-canvas"></canvas><input class="shot-text" id="shot-text" hidden placeholder="Yazı + Enter"></div>
      <div class="shot-foot">
        <label for="shot-to">Kime</label>
        <select class="input" id="shot-to">
          <option value="">— seçin —</option>
          ${targets.map((t) => `<option value="${esc(t.key)}" ${t.key === pre ? 'selected' : ''}>${esc(t.label)}</option>`).join('')}
        </select>
        <span class="spacer"></span>
        <button class="btn ghost" data-copyimg>Panoya kopyala</button>
        <button class="btn ghost" data-cancel>Vazgeç</button>
        <button class="btn" data-send>Gönder</button>
      </div>`, { onClose: () => URL.revokeObjectURL(url) });
    m.classList.add('shot-modal');
    const cv = m.querySelector('#shot-canvas');
    cv.width = img.naturalWidth;
    cv.height = img.naturalHeight;
    const ctx = cv.getContext('2d');
    const lw = Math.max(4, Math.round(Math.max(cv.width, cv.height) / 240));
    const shapes = [];
    let tool = 'arrow';
    let color = colors[0];
    let cur = null;
    const draw = (s) => {
      ctx.save();
      ctx.strokeStyle = s.color;
      ctx.fillStyle = s.color;
      ctx.lineWidth = lw;
      ctx.lineCap = 'round';
      ctx.lineJoin = 'round';
      if (s.type === 'pen' || s.type === 'mark') {
        if (s.type === 'mark') { ctx.globalAlpha = 0.35; ctx.lineWidth = lw * 6; }
        ctx.beginPath();
        s.pts.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
        ctx.stroke();
      } else if (s.type === 'rect') {
        ctx.strokeRect(Math.min(s.x1, s.x2), Math.min(s.y1, s.y2), Math.abs(s.x2 - s.x1), Math.abs(s.y2 - s.y1));
      } else if (s.type === 'arrow') {
        const a = Math.atan2(s.y2 - s.y1, s.x2 - s.x1);
        const h = lw * 5;
        ctx.beginPath();
        ctx.moveTo(s.x1, s.y1);
        ctx.lineTo(s.x2 - Math.cos(a) * h * 0.6, s.y2 - Math.sin(a) * h * 0.6);
        ctx.stroke();
        ctx.beginPath();
        ctx.moveTo(s.x2, s.y2);
        ctx.lineTo(s.x2 - h * Math.cos(a - 0.45), s.y2 - h * Math.sin(a - 0.45));
        ctx.lineTo(s.x2 - h * Math.cos(a + 0.45), s.y2 - h * Math.sin(a + 0.45));
        ctx.closePath();
        ctx.fill();
      } else if (s.type === 'text') {
        ctx.font = `600 ${lw * 7}px "Segoe UI", system-ui, sans-serif`;
        ctx.textBaseline = 'top';
        ctx.lineWidth = lw * 1.2;
        ctx.strokeStyle = s.color === '#ffffff' ? '#000' : '#fff';
        ctx.strokeText(s.text, s.x, s.y);
        ctx.fillText(s.text, s.x, s.y);
      }
      ctx.restore();
    };
    const redraw = () => {
      ctx.drawImage(img, 0, 0);
      shapes.forEach(draw);
      if (cur) draw(cur);
    };
    redraw();
    const pt = (e) => {
      const r = cv.getBoundingClientRect();
      return [((e.clientX - r.left) / r.width) * cv.width, ((e.clientY - r.top) / r.height) * cv.height];
    };
    const textBox = m.querySelector('#shot-text');
    const commitText = () => {
      const v = textBox.value.trim();
      if (v && textBox.dataset.x) shapes.push({ type: 'text', color, text: v, x: Number(textBox.dataset.x), y: Number(textBox.dataset.y) });
      textBox.hidden = true;
      textBox.value = '';
      redraw();
    };
    textBox.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); commitText(); }
      if (e.key === 'Escape') { e.stopPropagation(); textBox.hidden = true; textBox.value = ''; }
    });
    textBox.addEventListener('blur', commitText);
    cv.addEventListener('pointerdown', (e) => {
      const [x, y] = pt(e);
      if (tool === 'text') {
        const r = cv.getBoundingClientRect();
        const stage = m.querySelector('.shot-stage').getBoundingClientRect();
        textBox.style.left = `${e.clientX - stage.left}px`;
        textBox.style.top = `${e.clientY - stage.top}px`;
        textBox.style.color = color;
        textBox.dataset.x = String(x);
        textBox.dataset.y = String(y);
        textBox.hidden = false;
        setTimeout(() => textBox.focus(), 0);
        void r;
        return;
      }
      cv.setPointerCapture(e.pointerId);
      cur = tool === 'pen' || tool === 'mark' ? { type: tool, color: tool === 'mark' ? '#facc15' : color, pts: [[x, y]] } : { type: tool, color, x1: x, y1: y, x2: x, y2: y };
    });
    cv.addEventListener('pointermove', (e) => {
      if (!cur) return;
      const [x, y] = pt(e);
      if (cur.pts) cur.pts.push([x, y]);
      else { cur.x2 = x; cur.y2 = y; }
      redraw();
    });
    cv.addEventListener('pointerup', () => {
      if (!cur) return;
      const tiny = cur.pts ? cur.pts.length < 2 : Math.hypot(cur.x2 - cur.x1, cur.y2 - cur.y1) < 4;
      if (!tiny) shapes.push(cur);
      cur = null;
      redraw();
    });
    m.addEventListener('click', async (e) => {
      const t = e.target.closest('[data-tool]');
      if (t) {
        tool = t.dataset.tool;
        m.querySelectorAll('[data-tool]').forEach((b) => b.classList.toggle('on', b === t));
        cv.style.cursor = tool === 'text' ? 'text' : 'crosshair';
        return;
      }
      const c = e.target.closest('[data-color]');
      if (c) {
        color = c.dataset.color;
        m.querySelectorAll('[data-color]').forEach((b) => b.classList.toggle('on', b === c));
        return;
      }
      if (e.target.closest('[data-undo]')) { shapes.pop(); redraw(); return; }
      if (e.target.closest('[data-cancel]')) return closeModal();
      const out = () => new Promise((res) => cv.toBlob(res, 'image/png'));
      if (e.target.closest('[data-copyimg]')) {
        try {
          await navigator.clipboard.write([new ClipboardItem({ 'image/png': await out() })]);
          toast('Panoya kopyalandı', { type: 'success', timeout: 2000 });
        } catch { toast('Panoya kopyalanamadı', { type: 'error' }); }
        return;
      }
      if (e.target.closest('[data-send]')) {
        const key = m.querySelector('#shot-to').value;
        if (!key) return toast('Önce kime gönderileceğini seçin', { type: 'error' });
        const btn = e.target.closest('[data-send]');
        btn.disabled = true;
        try {
          const bytes = new Uint8Array(await (await out()).arrayBuffer());
          await api.invokeRaw('send_png', bytes, { 'x-key': key, 'x-stamp': clipStamp() });
          closeModal();
          toast(`Ekran görüntüsü gönderiliyor → ${shotTargets().find((t) => t.key === key)?.label || ''}`, { type: 'success' });
        } catch (err) {
          btn.disabled = false;
          toast(String(err), { type: 'error' });
        }
      }
    });
    m.addEventListener('keydown', (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'z') { e.preventDefault(); shapes.pop(); redraw(); }
    });
  };
  img.src = url;
}
$('#chat-shot').onclick = () => startShot({ key: state.openChat });

// ───────────────────────────── Hızlı gönderim paleti ─────────────────────────────
// Ctrl+Shift+Boşluk (her yerden) ya da uygulamada Ctrl+K: isim yaz, Enter → panodakini gönder.

function paletteItems(q) {
  const ql = q.toLocaleLowerCase('tr');
  const match = (s) => !ql || s.toLocaleLowerCase('tr').includes(ql);
  const lastAt = (key) => state.chats.get(key)?.last?.at || 0;
  const items = [];
  const seen = new Set();
  [...state.peers]
    .filter((p) => p.fingerprint && !isBlocked(p.fingerprint))
    .sort((a, b) => lastAt(b.fingerprint) - lastAt(a.fingerprint))
    .forEach((p) => {
      if (seen.has(p.fingerprint) || !match(p.name)) return;
      seen.add(p.fingerprint);
      items.push({ kind: 'peer', key: p.fingerprint, label: p.name, sub: p.status ? `${statusOf(p.status).icon} ${statusOf(p.status).label}` : 'çevrimiçi', peer: p });
    });
  [...state.chats.values()]
    .filter((c) => c.group && !c.left && !c.group.deleted && match(c.name))
    .sort((a, b) => lastAt(b.fingerprint) - lastAt(a.fingerprint))
    .forEach((c) => items.push({ kind: 'group', key: c.fingerprint, label: c.name, sub: `${c.group.members.length} kişilik grup` }));
  const actions = [
    { kind: 'action', id: 'shot', label: '📸 Ekran görüntüsü al, çiz, gönder', sub: 'Ctrl+Shift+PrtSc' },
    { kind: 'action', id: 'phone', label: '📱 Telefon bağla (QR)', sub: 'Telefondan bilgisayara, bilgisayardan telefona' },
    { kind: 'action', id: 'office', label: '🏢 Ofisi aç', sub: '3B görünüm' },
  ].filter((a) => match(a.label));
  return q ? [...items, ...actions] : [...actions, ...items];
}

function openPalette() {
  if (lockVisible() || modalKind === 'incoming' || modalKind === 'welcome') return;
  let sel = 0;
  let items = [];
  const m = openModal('palette', `
    <div class="palette">
      <input class="input pal-input" id="pal-q" placeholder="Kime gönderilsin? İsim yazın…" autocomplete="off" spellcheck="false" autofocus />
      <div class="pal-list" id="pal-list"></div>
      <div class="pal-foot"><b>Enter</b> panodakini gönder · <b>Shift+Enter</b> sohbeti aç · <b>Esc</b> kapat</div>
    </div>`);
  m.classList.add('palette-modal');
  const render = () => {
    items = paletteItems(m.querySelector('#pal-q').value.trim());
    sel = Math.min(sel, Math.max(0, items.length - 1));
    m.querySelector('#pal-list').innerHTML = items.length
      ? items.map((it, i) => `<button class="pal-item${i === sel ? ' on' : ''}" data-i="${i}">
          ${it.kind === 'action' ? '' : `<span class="avatar xs" style="background:${grad(it.key)}">${it.kind === 'group' ? ic.users : face(it.key, it.label)}</span>`}
          <span class="pal-main"><b>${esc(it.label)}</b><small>${esc(it.sub || '')}</small></span>
          ${it.kind === 'peer' ? '<kbd>Enter ↵</kbd>' : ''}</button>`).join('')
      : '<div class="pal-empty">Kimse bulunamadı</div>';
    m.querySelector('.pal-item.on')?.scrollIntoView({ block: 'nearest' });
  };
  const run = (it, openOnly) => {
    if (!it) return;
    closeModal();
    if (it.kind === 'action') {
      if (it.id === 'shot') return startShot(null);
      if (it.id === 'phone') return openPhone();
      if (it.id === 'office') return openOffice();
    }
    if (it.kind === 'group' || openOnly) {
      if (officeOpen()) closeOffice();
      return openChat(it.key, it.label);
    }
    sendClipboard([it.peer.id]);
  };
  m.querySelector('#pal-q').addEventListener('input', () => { sel = 0; render(); });
  m.querySelector('#pal-q').addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      sel = items.length ? (sel + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length : 0;
      render();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      run(items[sel], e.shiftKey);
    }
  });
  m.querySelector('#pal-list').addEventListener('click', (e) => {
    const b = e.target.closest('[data-i]');
    if (b) run(items[Number(b.dataset.i)], e.shiftKey);
  });
  render();
}

// ───────────────────────────── Telefon ─────────────────────────────

state.phoneRecv = [];
async function openPhone() {
  if (lockVisible()) return;
  let info;
  try {
    info = await call('phone_start');
  } catch { return; }
  const m = openModal('phone', `
    <h3>📱 Telefonla dosya alışverişi</h3>
    <div class="phone-grid">
      <div class="qr"><img alt="QR kod" draggable="false" src="data:image/svg+xml;charset=utf-8,${encodeURIComponent(info.svg)}"></div>
      <div class="phone-side">
        <ol class="phone-steps">
          <li>Telefon bu bilgisayarla <b>aynı Wi-Fi</b>'a bağlı olsun.</li>
          <li>Telefonun kamerasıyla QR kodu okutun.</li>
          <li>Açılan sayfadan fotoğraf, video ya da dosya seçin. Uygulama kurmaya gerek yok.</li>
        </ol>
        <div class="phone-url"><code>${esc(info.url)}</code><button class="icon-btn sm" data-copyurl title="Adresi kopyala">${ic.copy}</button></div>
        <div class="phone-state" id="phone-state">Telefon bekleniyor…</div>
      </div>
    </div>
    <div class="phone-sec">
      <div class="ps-head"><b>Telefondan gelenler</b></div>
      <div class="phone-list" id="phone-recv"></div>
    </div>
    <div class="phone-sec">
      <div class="ps-head"><b>Telefona gönder</b><button class="btn soft small" data-offer>${ic.file}Dosya seç</button></div>
      <div class="phone-list" id="phone-offered"></div>
    </div>
    <div class="foot"><span class="muted small">Bağlantı yerel ağda kalır, internete çıkmaz. Pencere kapanınca adres geçersiz olur.</span><span class="spacer"></span><button class="btn" data-close>Kapat</button></div>`, {
    onClose: () => api.invoke('phone_stop').catch(() => {}),
  });
  m.classList.add('phone-modal');
  renderPhoneLists(info.offered || []);
  m.addEventListener('click', async (e) => {
    if (e.target.closest('[data-close]')) return closeModal();
    if (e.target.closest('[data-copyurl]')) {
      try { await navigator.clipboard.writeText(info.url); toast('Adres kopyalandı', { type: 'success', timeout: 2000 }); } catch { /* yok say */ }
      return;
    }
    if (e.target.closest('[data-offer]')) {
      const picked = await api.pick({ multiple: true, directory: false, title: 'Telefona gönderilecek dosyalar' }).catch(() => null);
      const paths = Array.isArray(picked) ? picked : picked ? [picked] : [];
      if (!paths.length) return;
      try {
        const r = await call('phone_offer', { paths });
        renderPhoneLists(r.offered);
        toast('Dosyalar telefonda "Bilgisayardan al" bölümünde görünüyor', { type: 'success' });
      } catch { /* toast gösterildi */ }
      return;
    }
    const un = e.target.closest('[data-unoffer]');
    if (un) renderPhoneLists((await call('phone_unoffer', { index: Number(un.dataset.unoffer) })).offered);
    const rv = e.target.closest('[data-reveal]');
    if (rv) call('reveal_path', { path: rv.dataset.reveal }).catch(() => {});
  });
}
function renderPhoneLists(offered) {
  const recv = $('#phone-recv');
  if (!recv) return;
  recv.innerHTML = state.phoneRecv.length
    ? state.phoneRecv.map((f) => `<div class="pl-row"><span class="pl-ic">${ic.file}</span><span class="pl-name">${esc(f.name)}</span><span class="pl-size">${fmtBytes(f.size)}</span>
        <button class="icon-btn sm" data-reveal="${esc(f.path)}" title="Klasörde göster">${ic.reveal}</button></div>`).join('')
    : '<div class="pl-empty">Henüz bir şey gelmedi. Dosyalar İndirilenler\\FastFiles\\Telefon klasörüne kaydedilir.</div>';
  if (offered) {
    $('#phone-offered').innerHTML = offered.length
      ? offered.map((f) => `<div class="pl-row"><span class="pl-ic">${ic.file}</span><span class="pl-name">${esc(f.name)}</span><span class="pl-size">${fmtBytes(f.size)}</span>
          <button class="icon-btn sm" data-unoffer="${f.i}" title="Listeden çıkar">${ic.x}</button></div>`).join('')
      : '<div class="pl-empty">Telefona göndermek istediğiniz dosyaları seçin; telefonda "İndir" ile alınır.</div>';
  }
}
function onPhone(p) {
  if (p.kind === 'received') {
    state.phoneRecv.unshift({ name: p.name, size: p.size, path: p.path });
    state.phoneRecv = state.phoneRecv.slice(0, 50);
    renderPhoneLists();
    toast(`📱 Telefondan geldi: ${p.name}`, { type: 'success', action: 'Göster', timeout: 7000, onAction: () => call('reveal_path', { path: p.path }) });
  } else if (p.kind === 'connected') {
    const st = $('#phone-state');
    if (st) { st.textContent = '✓ Telefon bağlandı'; st.classList.add('ok'); }
  } else if (p.kind === 'downloaded') {
    toast(`📱 Telefona indirildi: ${p.name}`, { type: 'success', timeout: 3000 });
  } else if (p.kind === 'closed' && modalKind === 'phone') {
    const st = $('#phone-state');
    if (st) { st.textContent = 'Bağlantı süresi doldu, pencereyi kapatıp yeniden açın'; st.classList.remove('ok'); }
  }
}
$('#btn-phone').onclick = openPhone;

// ───────────────────────────── Ekran paylaşımı ─────────────────────────────

const screenState = { offers: new Map(), watching: null, url: null, actual: false };

$('#chat-screen').onclick = async () => {
  const key = state.openChat;
  if (!key) return;
  try {
    await call('screen_share_start', { key });
    toast('Ekran paylaşımı başladı; davet edilenler "İzle" deyince ekranınızı görür', { type: 'success', timeout: 6000 });
  } catch { /* toast gösterildi */ }
};
$('#share-bar-stop').onclick = () => api.invoke('screen_share_stop').catch(() => {});

function onScreenSharing(p) {
  document.body.classList.toggle('sharing', !!p.active);
  $('#share-bar').hidden = !p.active;
  if (!p.active) return;
  $('#share-bar-text').textContent = p.viewers?.length
    ? `Ekranınız paylaşılıyor · İzleyen: ${p.viewers.join(', ')}`
    : `Ekranınız paylaşılıyor · ${p.invited} kişi davet edildi, izlemeleri bekleniyor`;
}

function onScreenOffer({ session, fingerprint, name }) {
  screenState.offers.set(session, { fingerprint, name });
  if (screenState.watching) return;
  toast(`🖥️ ${name} ekranını paylaşıyor`, { action: 'İzle', timeout: 30000, onAction: () => watchScreen(session) });
}
function onScreenEnd({ session }) {
  const offer = screenState.offers.get(session);
  screenState.offers.delete(session);
  if (screenState.watching?.session === session) {
    closeScreenViewer(false);
    toast(`${offer?.name || 'Paylaşan'} ekran paylaşımını bitirdi`);
  }
}

function showFrame(buf) {
  const url = URL.createObjectURL(new Blob([buf], { type: 'image/jpeg' }));
  const prev = screenState.url;
  screenState.url = url;
  const img = $('#sv-img');
  img.src = url;
  $('#sv-wait').hidden = true;
  if (prev) setTimeout(() => URL.revokeObjectURL(prev), 500);
}

async function watchScreen(session) {
  const offer = screenState.offers.get(session);
  if (!offer) return toast('Bu paylaşım artık yok', { type: 'error' });
  if (screenState.watching) closeScreenViewer();
  if (modalKind && modalKind !== 'incoming') closeModal();
  screenState.watching = { session, name: offer.name };
  $('#sv-title').textContent = `${offer.name} ekranı`;
  $('#sv-info').textContent = '';
  $('#sv-wait').hidden = false;
  $('#sv-wait').textContent = 'Bağlanılıyor…';
  $('#sv-img').removeAttribute('src');
  $('#screen-viewer').hidden = false;
  document.body.classList.add('screen-open');
  if (!T) return previewScreen(session);
  const ch = new T.core.Channel();
  ch.onmessage = (msg) => {
    if (screenState.watching?.session !== session) return;
    if (msg instanceof ArrayBuffer) showFrame(msg);
    else if (msg && msg.width) $('#sv-info').textContent = `${msg.width} × ${msg.height}`;
  };
  try {
    await api.invoke('screen_watch', { session, onFrame: ch });
    if (screenState.watching?.session === session) {
      closeScreenViewer(false);
      toast(`${offer.name} ekran paylaşımını bitirdi`);
    }
  } catch (e) {
    if (screenState.watching?.session === session) {
      closeScreenViewer(false);
      toast(String(e), { type: 'error' });
    }
  }
}

function closeScreenViewer(stop = true) {
  const w = screenState.watching;
  screenState.watching = null;
  if (w && stop) api.invoke('screen_watch_stop', { session: w.session }).catch(() => {});
  if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  $('#screen-viewer').hidden = true;
  document.body.classList.remove('screen-open');
  if (screenState.url) URL.revokeObjectURL(screenState.url);
  screenState.url = null;
  $('#sv-img').removeAttribute('src');
}
$('#sv-close').onclick = () => closeScreenViewer();
$('#sv-full').onclick = () => (document.fullscreenElement ? document.exitFullscreen() : $('#screen-viewer').requestFullscreen()).catch?.(() => {});
$('#sv-fit').onclick = () => {
  screenState.actual = !screenState.actual;
  $('#sv-stage').classList.toggle('actual', screenState.actual);
  $('#sv-fit').textContent = screenState.actual ? 'Sığdır' : 'Gerçek boyut';
};
$('#sv-img').addEventListener('dblclick', () => $('#sv-full').click());

// ───────────────────────────── Kısayollar ─────────────────────────────

document.addEventListener('keydown', (e) => {
  if (lockVisible()) return;
  const k = e.key.toLowerCase();
  if ((e.ctrlKey || e.metaKey) && k === 'k' && !modalKind) { e.preventDefault(); openPalette(); }
  if ((e.ctrlKey || e.metaKey) && k === 'f' && chatOpen() && !modalKind) { e.preventDefault(); openSearch(); }
  if (k === 'f' && !e.ctrlKey && !$('#screen-viewer').hidden && !e.target.closest('input, textarea')) $('#sv-full').click();
});

// Sohbet başlığındaki "Diğer" menüsü
$('#chat-more').onclick = (e) => {
  e.stopPropagation();
  $('#chat-more-menu').hidden = !$('#chat-more-menu').hidden;
};
$('#chat-more-menu').addEventListener('click', () => { $('#chat-more-menu').hidden = true; });
document.addEventListener('mousedown', (e) => {
  if (!e.target.closest('.chat-more')) $('#chat-more-menu').hidden = true;
});

// ───────────────────────────── Sanal ofis ─────────────────────────────
// Masanın 3B görünümü (office.js). Three.js yalnızca ekran ilk açıldığında yüklenir.
// Herkes Ofis ekranından kendi koltuğunu seçer; seçim keşif duyurusuyla diğer cihazlara gider.

const OFFICE_KNOWN_KEY = 'ff.office.known';
const officeState = {
  mod: null,
  loading: null,
  failed: false,
  seen: new Set(),     // ofiste uçurulan mesajlar (aynı mesajın durum güncellemeleri tekrar uçmasın)
  feed: [],            // son olaylar: { icon, html, at }
  known: (() => { try { return JSON.parse(localStorage.getItem(OFFICE_KNOWN_KEY) || '{}') || {}; } catch { return {}; } })(),
  askedSeat: false,
};
const officeOpen = () => !$('#office-screen').hidden;

function saveOfficeKnown() {
  try { localStorage.setItem(OFFICE_KNOWN_KEY, JSON.stringify(officeState.known)); } catch { /* önemli değil */ }
}

function officeName(fp) {
  if (fp === myFp()) return 'Siz';
  const p = peerByFp(fp);
  if (p) return p.name;
  if (state.chats.get(fp)?.name) return state.chats.get(fp).name;
  if (officeState.known[fp]) return officeState.known[fp].name;
  for (const c of state.chats.values()) {
    const m = c.group?.members?.find((x) => x.fingerprint === fp);
    if (m) return m.name;
  }
  return 'Biri';
}

/** Masadakiler: ben, çevrimiçi cihazlar ve koltuğu bilinen çevrimdışı kişiler. */
function officePeople() {
  const me = myFp();
  const list = [];
  if (me && state.settings) list.push({ fp: me, name: state.settings.deviceName, seat: state.settings.seat || 0, online: true, me: true, status: myStatus() });
  const online = new Set([me]);
  const now = Date.now();
  let dirty = false;
  for (const p of state.peers) {
    if (!p.fingerprint || online.has(p.fingerprint)) continue;
    online.add(p.fingerprint);
    const seat = p.seat || 0;
    list.push({ fp: p.fingerprint, name: p.name, seat, online: true, me: false, status: p.status || '' });
    // Çevrimdışı olunca da yeri boş görünsün diye koltuğu hatırlanır.
    const k = officeState.known[p.fingerprint];
    if (seat && (!k || k.seat !== seat || k.name !== p.name || now - k.at > 3600e3)) {
      officeState.known[p.fingerprint] = { name: p.name, seat, at: now };
      dirty = true;
    } else if (!seat && k) {
      delete officeState.known[p.fingerprint];
      dirty = true;
    }
  }
  for (const [fp, k] of Object.entries(officeState.known)) {
    if (online.has(fp)) continue;
    if (now - k.at > 30 * 86400e3) { delete officeState.known[fp]; dirty = true; continue; }
    list.push({ fp, name: k.name, seat: k.seat, online: false, me: false });
  }
  if (dirty) saveOfficeKnown();
  return list.map((p) => ({ ...p, faceHtml: face(p.fp, p.name), faceBg: grad(p.fp) }));
}

function officeSyncPeople() {
  const list = officePeople();
  if (!officeState.mod) return;
  const { overflow } = officeState.mod.setPeople(list);
  const online = list.filter((p) => p.online).length;
  $('#office-sub').textContent = `${online} kişi çevrimiçi${overflow.length ? ` · masada yer yok: ${overflow.map((p) => p.name).join(', ')}` : ''}`;
  const seat = state.settings?.seat;
  $('#office-seat').lastChild.textContent = seat ? `Koltuk ${seat}` : 'Koltuğumu seç';
}

function officeTransfers() {
  if (!officeState.mod) return;
  const me = myFp();
  const list = [];
  for (const t of state.transfers.values()) {
    const fp = state.peers.find((p) => p.id === t.peerId)?.fingerprint;
    if (!fp) continue;
    const send = t.direction === 'send';
    list.push({ id: t.id, from: send ? me : fp, to: send ? fp : me, status: t.status, done: t.done, total: t.total, speed: t.speed, title: t.title });
  }
  officeState.mod.syncTransfers(list);
}

function officeOnTransfer(prev, t) {
  officeTransfers();
  if (prev?.status === t.status) return;
  const fp = state.peers.find((p) => p.id === t.peerId)?.fingerprint;
  const who = (f) => `<b>${esc(f ? officeName(f) : t.peerName)}</b>`;
  const send = t.direction === 'send';
  const pair = send ? `${who(myFp())} → ${who(fp)}` : `${who(fp)} → ${who(myFp())}`;
  const title = esc(t.title.length > 28 ? `${t.title.slice(0, 27)}…` : t.title);
  if (!prev && ['waiting', 'active'].includes(t.status)) officeFeed('📎', `${pair} · ${title}`);
  else if (t.status === 'done') officeFeed('✅', `${pair} · ${title} ${send ? 'gönderildi' : 'alındı'}`);
  else if (t.status === 'declined') officeFeed('✖️', `${pair} · ${title} reddedildi`);
  else if (t.status === 'failed') officeFeed('⚠️', `${pair} · ${title} başarısız`);
}

function officeGames() {
  if (!officeState.mod) return;
  officeState.mod.setGames(state.games
    .filter((g) => g.status === 'active' && g.game === 'chess' && g.players.length === 2)
    .map((g) => ({ id: g.id, a: g.players[0].fingerprint, b: g.players[1].fingerprint })));
}

function officeOnChat(key, incoming, m) {
  if (!m || m.system || m.file) return;
  const id = `${key}|${m.id}`;
  if (officeState.seen.has(id)) return;
  if (officeState.seen.size > 3000) officeState.seen.clear();
  officeState.seen.add(id);
  // Uzun süre sonra toplu gelen eski mesajlar (çevrimdışı kuyruğu) uçurulmaz.
  if (Math.abs(Date.now() - (m.at || 0)) > 120e3) return;
  const me = myFp();
  const group = groupOf(key);
  const members = group ? group.members.map((x) => x.fingerprint) : [];
  const from = incoming ? (group ? m.sender : key) : me;
  if (!from) return;
  const to = group ? members.filter((f) => f !== from) : [incoming ? me : key];
  if (!to.length) return;
  const gname = state.chats.get(key)?.name || group?.name || 'grup';
  officeFeed('💬', group
    ? `<b>${esc(officeName(from))}</b> → ${esc(gname)}`
    : `<b>${esc(officeName(from))}</b> → <b>${esc(officeName(to[0]))}</b>`);
  officeState.mod?.message(from, to, group ? gname : 'Mesaj');
}

function officeFeed(icon, html) {
  officeState.feed.push({ icon, html, at: Date.now() });
  if (officeState.feed.length > 5) officeState.feed.shift();
  if (officeOpen()) renderOfficeFeed();
}

function renderOfficeFeed() {
  const now = Date.now();
  $('#office-feed').innerHTML = officeState.feed
    .filter((f) => now - f.at < 10 * 60e3)
    .map((f) => `<div class="fe${now - f.at > 20e3 ? ' old' : ''}"><span class="fe-ic">${f.icon}</span><span>${f.html}</span><time>${fmtTime(f.at)}</time></div>`)
    .join('');
}

function setOfficePick(on) {
  officeState.mod?.setPickMode(on);
  $('#office-pick').hidden = !on;
  $('#office-unseat').hidden = !state.settings?.seat;
}

async function officePickSeat(n) {
  const s = await call('update_settings', { seat: n }).catch(() => null);
  if (!s) return;
  state.settings = s;
  setOfficePick(false);
  officeSyncPeople();
  toast(n ? `Koltuk ${n} artık sizin. Diğer bilgisayarlarda da burada görüneceksiniz.` : 'Koltuktan kalktınız', { type: 'success' });
}

async function openOffice() {
  if (gamesOpen()) closeGames();
  closeEmoji();
  closeViewer();
  $('#office-screen').hidden = false;
  document.body.classList.add('office-open');
  renderOfficeFeed();
  if (!officeState.mod && !officeState.failed) {
    try {
      officeState.loading ||= import('./office.js');
      const { createOffice } = await officeState.loading;
      officeState.mod ||= createOffice($('#office-stage'), {
        onPerson: (fp) => { closeOffice(); openChat(fp, officeName(fp)); },
        onSeat: (n) => officePickSeat(n),
      });
    } catch (e) {
      officeState.failed = true;
      const err = $('#office-error');
      err.hidden = false;
      err.innerHTML = `<h3>3B görünüm açılamadı</h3><p>Bu bilgisayarın ekran kartı sürücüsü WebGL'i desteklemiyor olabilir.</p><p class="small">${esc(e?.message || e)}</p>`;
    }
  }
  if (!officeState.mod || !officeOpen()) return;
  officeSyncPeople();
  officeTransfers();
  officeGames();
  officeState.mod.start();
  if (!state.settings?.seat && !officeState.askedSeat) {
    officeState.askedSeat = true;
    toast('Masada nerede oturduğunuzu seçin, herkes sizi orada görsün', { action: 'Koltuk seç', timeout: 9000, onAction: () => setOfficePick(true) });
  }
}

function closeOffice() {
  setOfficePick(false);
  $('#office-screen').hidden = true;
  document.body.classList.remove('office-open');
  officeState.mod?.stop();
}

$('#btn-office').onclick = () => (officeOpen() ? closeOffice() : openOffice());
$('#office-close').onclick = closeOffice;
$('#office-home').onclick = () => officeState.mod?.resetCamera();
$('#office-seat').onclick = () => setOfficePick(!officeState.mod?.pickMode);
$('#office-pick-done').onclick = () => setOfficePick(false);
$('#office-unseat').onclick = () => officePickSeat(0);
setInterval(() => { if (officeOpen()) renderOfficeFeed(); }, 10e3);

// ───────────────────────────── Sürükle-bırak ─────────────────────────────

function peerIdAt(pos) {
  if (!pos) return null;
  const r = window.devicePixelRatio || 1;
  if (officeOpen() && officeState.mod) {
    const fp = officeState.mod.fpAt(pos.x / r, pos.y / r);
    return (fp && peerByFp(fp)?.id) || null;
  }
  const el = document.elementFromPoint(pos.x / r, pos.y / r);
  return el?.closest('.peer')?.dataset.id || null;
}

function updateDragTarget(pos) {
  const id = peerIdAt(pos);
  officeState.mod?.setDropTarget(state.peers.find((p) => p.id === id)?.fingerprint);
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
  if (!pos || !chatOpen() || $('#chat-attach').disabled) return false;
  const r = window.devicePixelRatio || 1;
  const el = document.elementFromPoint(pos.x / r, pos.y / r);
  return !!el?.closest('#chat-view');
}

function endDrag() {
  officeState.mod?.setDropTarget(null);
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
  state.games = snap.games || [];
  state.gameCatalog = snap.gameCatalog || [];
  renderGames();
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
  api.listen('games', (e) => onGames(e.payload));
  api.listen('game', (e) => onGame(e.payload));
  api.listen('game-error', (e) => toast(e.payload.message, { type: 'error', timeout: 6000 }));
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
  api.listen('my-status', (e) => { state.myStatus = e.payload.status; renderMyStatus(); officeSyncPeople(); });
  api.listen('nudge', (e) => onNudge(e.payload));
  api.listen('chat-remove', (e) => onChatRemove(e.payload));
  api.listen('quick-send', () => openPalette());
  api.listen('quick-shot', () => startShot(null));
  api.listen('phone', (e) => onPhone(e.payload));
  api.listen('screen-offer', (e) => onScreenOffer(e.payload));
  api.listen('screen-end', (e) => onScreenEnd(e.payload));
  api.listen('screen-sharing', (e) => onScreenSharing(e.payload));
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
      { id: 't9', direction: 'receive', peerId: 'p2', peerName: 'Elif (Tasarım)', title: 'afis-taslak.png', fileCount: 1, total: 2.4e6, done: 2.4e6, speed: 0, status: 'done', createdAt: now - 26e3, finishedAt: now - 25e3, savePath: 'C:\a.png' },
      { id: 't10', direction: 'send', peerId: 'p2', peerName: 'Elif (Tasarım)', title: 'ofis-panorama.jpg', fileCount: 1, total: 5.1e6, done: 2.2e6, speed: 41e6, status: 'active', createdAt: now - 21e3, sources: ['C:\b.jpg'] },
      { id: 't11', direction: 'receive', peerId: 'p2', peerName: 'Elif (Tasarım)', title: 'ekran.png', fileCount: 1, total: 900e3, done: 300e3, speed: 12e6, status: 'active', createdAt: now - 16e3 },
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
      { id: 'f-t9', outgoing: false, text: '📎 afis-taslak.png', at: now - 25e3, status: 'received', file: { title: 'afis-taslak.png', count: 1, total: 2.4e6, transfers: [{ to: 'me', id: 't9' }] } },
      { id: 'f-t10', outgoing: true, text: '📎 ofis-panorama.jpg', at: now - 20e3, status: 'sent', file: { title: 'ofis-panorama.jpg', count: 1, total: 5.1e6, transfers: [{ to: 'x', id: 't10' }] } },
      { id: 'f-t11', outgoing: false, text: '📎 ekran.png', at: now - 15e3, status: 'received', file: { title: 'ekran.png', count: 1, total: 900e3, transfers: [{ to: 'me', id: 't11' }] } },
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
  // Örnek oyunlar: Ahmet'le süren satranç (siyahız, sıra bizde), Elif'ten davet, kazanılmış bir oyun.
  snap.gameCatalog = [{ id: 'chess', name: 'Satranç', description: 'İki kişilik klasik satranç. Rok, geçerken alma, terfi, şah mat, pat ve beraberlik kuralları dahil.', minPlayers: 2, maxPlayers: 2, seats: ['Beyaz', 'Siyah'] }];
  const START_FEN = 'rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1';
  const startLegal = [...'abcdefgh'].flatMap((f) => [`${f}2${f}3`, `${f}2${f}4`]).concat(['b1a3', 'b1c3', 'g1f3', 'g1h3']);
  const gameMock = (o) => ({
    title: 'Satranç', game: 'chess', result: null, drawOffer: null, rev: 1, pending: false, legal: [], turn: null, myTurn: false, needsMe: false,
    moves: [], info: { fen: START_FEN, check: false, san: [], lastMove: null }, ...o,
  });
  snap.games = [
    gameMock({ id: 'gm1', host: fp1, hostName: 'Ahmet', isHost: false, players: [{ fingerprint: fp1, name: 'Ahmet' }, { fingerprint: me, name: 'Nur' }], joined: [fp1, me],
      status: 'active', mySeat: 1, turn: 1, myTurn: true, needsMe: true, createdAt: now - 30 * 60e3, updatedAt: now - 40e3,
      moves: 'e2e4 e7e5 g1f3 b8c6 f1c4 f8c5 c2c3 g8f6 d2d4 e5d4 c3d4 c5b4 b1c3 f6e4 e1g1 e4c3 b2c3'.split(' '),
      legal: 'b4c5 b4d6 b4e7 b4f8 b4c3 b4a5 b4a3 c6e7 c6e5 c6d4 c6a5 c6b8 a7a6 a7a5 b7b6 b7b5 d7d6 d7d5 f7f6 f7f5 g7g6 g7g5 h7h6 h7h5 a8b8 d8e7 d8f6 d8g5 d8h4 e8f8 e8e7 e8g8 h8g8 h8f8'.split(' '),
      info: { fen: 'r1bqk2r/pppp1ppp/2n5/8/1bBP4/2P2N2/P4PPP/R1BQ1RK1 b kq - 0 9', check: false, lastMove: 'b2c3',
        san: ['e4', 'e5', 'Nf3', 'Nc6', 'Bc4', 'Bc5', 'c3', 'Nf6', 'd4', 'exd4', 'cxd4', 'Bb4+', 'Nc3', 'Nxe4', 'O-O', 'Nxc3', 'bxc3'] } }),
    gameMock({ id: 'gm2', host: fp2, hostName: 'Elif (Tasarım)', isHost: false, players: [{ fingerprint: me, name: 'Nur' }, { fingerprint: fp2, name: 'Elif (Tasarım)' }], joined: [fp2],
      status: 'invited', mySeat: 0, needsMe: true, createdAt: now - 2 * 60e3, updatedAt: now - 2 * 60e3 }),
    gameMock({ id: 'gm0', host: me, hostName: 'Nur', isHost: true, players: [{ fingerprint: me, name: 'Nur' }, { fingerprint: fpM, name: 'Mehmet (İzinde)' }], joined: [me, fpM],
      status: 'finished', mySeat: 0, moves: 'e2e4 e7e5 d1h5 b8c6 f1c4 g8f6 h5f7'.split(' '), result: { winner: me, reason: 'checkmate', by: null },
      createdAt: now - 2 * 86400e3, updatedAt: now - 2 * 86400e3 + 600e3,
      info: { fen: 'r1bqkb1r/pppp1Qpp/2n2n2/4p3/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 0 4', check: true, lastMove: 'h5f7', san: ['e4', 'e5', 'Qh5', 'Nc6', 'Bc4', 'Nf6', 'Qxf7#'] } }),
  ];
  // Örnek mini golf (gerçek arka uç çıktısı): Ahmet deliği 2 vuruşta bitirdi, sıra bizde.
  snap.gameCatalog.push({ id: 'golf', name: 'Mini golf', description: '2–6 kişilik, 6 delikli mini golf. Sırayla vurulur; duvar, kum, su ve engeller var. En az vuruşla bitiren kazanır.', minPlayers: 2, maxPlayers: 6, seats: [] });
  const golfHoles = JSON.parse('[{"blocks":[],"bounds":[[80,150],[720,150],[720,350],[80,350]],"cup":[650,250],"name":"Isınma","par":2,"posts":[],"sand":[],"tee":[150,250],"water":[]},{"blocks":[[[385,80],[415,80],[415,320],[385,320]]],"bounds":[[60,80],[740,80],[740,420],[60,420]],"cup":[640,160],"name":"Duvar","par":3,"posts":[],"sand":[],"tee":[160,160],"water":[]},{"blocks":[],"bounds":[[60,60],[360,60],[360,290],[740,290],[740,440],[60,440]],"cup":[660,365],"name":"Dönemeç","par":3,"posts":[],"sand":[[430,380,120,60]],"tee":[200,130],"water":[]},{"blocks":[],"bounds":[[60,90],[740,90],[740,410],[60,410]],"cup":[670,190],"name":"Göl","par":3,"posts":[],"sand":[[330,350,140,60]],"tee":[130,250],"water":[[330,90,140,200]]},{"blocks":[],"bounds":[[60,70],[740,70],[740,430],[60,430]],"cup":[690,250],"name":"Direkler","par":3,"posts":[[300,180,22],[300,320,22],[450,250,28],[590,165,20],[590,335,20]],"sand":[],"tee":[120,250],"water":[]},{"blocks":[[[230,60],[260,60],[260,330],[230,330]],[[480,170],[510,170],[510,440],[480,440]]],"bounds":[[60,60],[740,60],[740,440],[60,440]],"cup":[650,380],"name":"Zikzak","par":4,"posts":[],"sand":[[560,60,180,60]],"tee":[140,120],"water":[]}]');
  const golfPlayers = [{ fingerprint: fp1, name: 'Ahmet' }, { fingerprint: me, name: 'Nur' }, { fingerprint: fp2, name: 'Elif (Tasarım)' }];
  snap.games.unshift(gameMock({ id: 'gm3', game: 'golf', title: 'Mini golf', host: me, hostName: 'Nur', isHost: true, players: golfPlayers,
    joined: golfPlayers.map((p) => p.fingerprint), status: 'active', mySeat: 1, turn: 1, myTurn: true, needsMe: true,
    moves: ['a0p55', 'a3550p50', 'a50p45', 'a0p20'], createdAt: now - 10 * 60e3, updatedAt: now - 20e3,
    info: { width: 800, height: 500, ballR: 7, cupR: 11, maxStrokes: 8, holes: golfHoles, hole: 0, quit: [false, false, false],
      balls: [{ x: 650, y: 250, strokes: 2, done: true }, { x: 534.6, y: 216.4, strokes: 1, done: false }, { x: 475.8, y: 278.5, strokes: 1, done: false }],
      scores: [[], [], []], totals: [0, 0, 0],
      lastShot: { seat: 0, hole: 0, result: 'in', strokes: 2, end: [650, 250], path: [[597.7, 250], [610, 250], [622, 250], [632.5, 250], [650, 250]] } } }));
  // Önizleme: vuruş düz bir çizgide, güce göre yol alır (gerçek fizik arka uçtadır).
  const golfMockShot = (g, seat, mv) => {
    const [, a, p] = mv.match(/^a(\d+)p(\d+)$/).map(Number);
    const b = g.info.balls[seat];
    const ang = (a / 10) * Math.PI / 180;
    const dist = p * 6;
    const steps = Math.max(2, Math.round(p / 3));
    const path = Array.from({ length: steps }, (_, i) => {
      const t = 1 - (1 - i / (steps - 1)) ** 2;
      return [Math.max(90, Math.min(710, b.x + Math.cos(ang) * dist * t)), Math.max(160, Math.min(340, b.y + Math.sin(ang) * dist * t))];
    });
    const [x, y] = path.at(-1);
    const holed = Math.hypot(x - 650, y - 250) < 14;
    Object.assign(b, { x: holed ? 650 : x, y: holed ? 250 : y, strokes: b.strokes + 1, done: holed });
    g.moves = [...g.moves, mv];
    g.info.lastShot = { seat, hole: 0, result: holed ? 'in' : 'stop', strokes: b.strokes, end: [b.x, b.y], path };
    const next = [1, 2, 0].map((k) => (seat + k) % 3).find((s) => !g.info.balls[s].done);
    Object.assign(g, { turn: next ?? null, myTurn: next === g.mySeat, needsMe: next === g.mySeat, updatedAt: Date.now() });
  };
  const fenPlacement = (b) => [7, 6, 5, 4, 3, 2, 1, 0].map((r) => b.slice(r * 8, r * 8 + 8)
    .reduce((s, p) => (p ? s + p : s.replace(/(\d?)$/, (d) => String(Number(d || 0) + 1))), '')).join('/');
  const gameAct = (g, a) => {
    const opp = g.players.find((p) => p.fingerprint !== me).fingerprint;
    const end = (result) => Object.assign(g, { status: 'finished', result, drawOffer: null, myTurn: false, needsMe: false, legal: [], turn: null });
    if (a.kind === 'move' && g.game === 'golf') {
      golfMockShot(g, g.mySeat, a.mv);
      // Diğer oyuncular da sırayla vurur.
      const others = () => {
        if (g.turn == null || g.turn === g.mySeat) return;
        golfMockShot(g, g.turn, `a${[3400, 100, 0, 3550][Math.floor(Math.random() * 4)]}p${30 + Math.floor(Math.random() * 30)}`);
        emit('games', structuredClone(snap.games));
        setTimeout(others, 1800);
      };
      setTimeout(others, 1800);
      return;
    }
    if (a.kind === 'move') {
      if (!g.legal.includes(a.mv)) throw 'Kurallara aykırı hamle';
      Object.assign(g, { pending: true, myTurn: false, needsMe: false, legal: [] });
      setTimeout(() => {
        const parts = g.info.fen.split(' ');
        parts[0] = fenPlacement(applyLocalMove(parseFen(g.info.fen).board, a.mv));
        parts[1] = parts[1] === 'w' ? 'b' : 'w';
        parts[3] = '-';
        Object.assign(g, { pending: false, moves: [...g.moves, a.mv], turn: 1 - g.mySeat, rev: g.rev + 1, updatedAt: Date.now(),
          info: { ...g.info, fen: parts.join(' '), lastMove: a.mv, check: false, san: [...g.info.san, a.mv] } });
        emit('games', structuredClone(snap.games));
      }, 500);
    } else if (a.kind === 'join') {
      Object.assign(g, { joined: [...g.joined, me], status: 'active', turn: 0, myTurn: g.mySeat === 0, needsMe: g.mySeat === 0, legal: g.mySeat === 0 ? startLegal : [] });
    } else if (a.kind === 'decline') Object.assign(g, { status: g.isHost ? 'cancelled' : 'declined', needsMe: false });
    else if (a.kind === 'resign') end({ winner: opp, reason: 'resign', by: me });
    else if (a.kind === 'offerDraw') g.drawOffer = me;
    else if (a.kind === 'acceptDraw') end({ winner: null, reason: 'agreement', by: me });
    else if (a.kind === 'declineDraw') g.drawOffer = null;
    g.updatedAt = Date.now();
  };
  const tallyMock = (p) => {
    p.counts = p.options.map((_, i) => p.ballots.filter((b) => b.option === i).length);
    p.voted = p.ballots.map((b) => b.fingerprint);
  };

  snap.update = location.hash.includes('update')
    ? { current: '1.2.0', ready: '1.2.1', serving: '1.2.1' }
    : { current: '1.2.0', ready: null, serving: null };
  snap.peers[0].version = '1.2.0';
  snap.peers[1].version = '1.1.0';
  snap.peers[0].status = 'focus';
  snap.peers[2].status = 'busy';
  // Sanal ofis önizlemesi: çizimdeki gibi alt sağ köşe bizim; Mehmet çevrimdışı ama koltuğu biliniyor.
  snap.settings.seat = 7;
  snap.peers[0].seat = 2;
  snap.peers[1].seat = 5;
  try {
    localStorage.setItem('ff.office.known', JSON.stringify({ [fpM]: { name: 'Mehmet (İzinde)', seat: 3, at: Date.now() } }));
  } catch { /* önizleme */ }
  const emit = (ev, payload) => (listeners[ev] || []).forEach((cb) => cb({ payload }));
  // Önizlemede olay tetiklemek için: __preview.emit('peers', [...])
  window.__preview = { emit, snap };
  // Sanal ofis gösterimi: mesajlar, yazıyor, dosya aktarımı ve oyun sırayla oynatılır.
  window.officeDemo = function officeDemo() {
    const p = snap.peers;
    const at = (ms, f) => setTimeout(f, ms);
    const msg = (from, text, key = from.fingerprint) => emit('chat', { fingerprint: key, name: from.name, unread: 1, incoming: true,
      message: { id: `d${Math.random()}`, outgoing: false, text, at: Date.now(), status: 'received', sender: key.startsWith('g:') ? from.fingerprint : undefined, senderName: key.startsWith('g:') ? from.name : undefined } });
    at(300, () => emit('typing', { key: p[1].fingerprint, fingerprint: p[1].fingerprint, name: p[1].name }));
    at(2200, () => msg(p[1], 'Logo hazır mı?'));
    at(3500, () => emit('chat', { fingerprint: p[0].fingerprint, name: p[0].name, unread: 0, incoming: false, message: { id: `o${Date.now()}`, outgoing: true, text: 'Geliyorum', at: Date.now(), status: 'sent' } }));
    const tr = { id: 'demo-t', direction: 'send', peerId: p[1].id, peerName: p[1].name, title: 'Sunum_Final.pptx', fileCount: 1, total: 240e6, done: 0, speed: 0, status: 'waiting', createdAt: Date.now() };
    at(4500, () => emit('transfer', { ...tr }));
    let done = 0;
    at(6000, () => {
      const iv = setInterval(() => {
        done = Math.min(tr.total, done + 9e6);
        emit('transfer', { ...tr, status: done >= tr.total ? 'done' : 'active', done, speed: 90e6, finishedAt: done >= tr.total ? Date.now() : undefined });
        if (done >= tr.total) clearInterval(iv);
      }, 250);
    });
    at(7000, () => emit('chat', { fingerprint: 'g:grp-1', name: 'Proje Ekibi', unread: 1, incoming: true, message: { id: `dg${Date.now()}`, outgoing: false, text: 'Toplantı 15:00', at: Date.now(), status: 'received', sender: p[0].fingerprint, senderName: p[0].name } }));
    const tr2 = { id: 'demo-r', direction: 'receive', peerId: p[2].id, peerName: p[2].name, title: 'render_v2.mp4', fileCount: 1, total: 1.2e9, done: 0.3e9, speed: 110e6, status: 'active', createdAt: Date.now() };
    at(8000, () => emit('transfer', tr2));
    at(9000, () => { snap.games[0].status = 'active'; emit('games', structuredClone(snap.games)); });
  };
  setTimeout(() => {
    if (location.hash.includes('chat')) document.querySelector('.peer[data-id="p2"] [data-act="chat"]')?.click();
    if (location.hash.includes('group')) openChat(gkey);
    if (location.hash.includes('share')) { setTab('shares'); openShare('sh1'); }
    if (location.hash.includes('locked')) { setTab('shares'); openShare('sh2'); }
    if (location.hash.includes('poll')) openPoll(location.hash.includes('poll-closed') ? 'pl0' : 'pl2');
    if (location.hash.includes('games')) openGames();
    if (location.hash.includes('game-play')) openRoom('gm1');
    if (location.hash.includes('game-invite')) openRoom('gm2');
    if (location.hash.includes('game-over')) openRoom('gm0');
    if (location.hash.includes('game-golf')) openRoom('gm3');
    if (location.hash.includes('game-new')) showGameCreate();
    if (location.hash.includes('palette')) openPalette();
    if (location.hash.includes('phone')) openPhone();
    if (location.hash.includes('shot')) startShot(null);
    if (location.hash.includes('screen')) { emit('screen-offer', { session: 'v1', fingerprint: fp2, name: 'Elif (Tasarım)' }); }
    if (location.hash.includes('office')) openOffice().then(() => { if (location.hash.includes('office-demo')) officeDemo(); });
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
      if (cmd === 'chat_image') {
        // Örnek resim: mesaja göre farklı boyut/renkte çizilir.
        const spec = { 'f-t9': [1080, 1350, 280], 'f-t10': [3200, 1200, 170], 'f-t11': [1600, 1000, 20] }[args.id];
        if (!spec) throw 'Dosya artık yok';
        const [w, h, hueA] = spec;
        const c = document.createElement('canvas');
        c.width = w; c.height = h;
        const x = c.getContext('2d');
        const g = x.createLinearGradient(0, 0, w, h);
        g.addColorStop(0, `hsl(${hueA} 70% 55%)`);
        g.addColorStop(1, `hsl(${hueA + 60} 70% 30%)`);
        x.fillStyle = g; x.fillRect(0, 0, w, h);
        x.fillStyle = 'rgba(255,255,255,.85)';
        x.font = `bold ${Math.round(h / 9)}px sans-serif`;
        x.textAlign = 'center';
        x.fillText(`${w} × ${h}`, w / 2, h / 2);
        for (let i = 0; i < 40; i++) {
          x.fillStyle = `rgba(255,255,255,${0.05 + (i % 5) * 0.03})`;
          x.beginPath(); x.arc((i * 997) % w, (i * 577) % h, 20 + (i % 7) * 18, 0, Math.PI * 2); x.fill();
        }
        const blob = await new Promise((r) => c.toBlob(r, 'image/png'));
        return blob.arrayBuffer();
      }
      if (cmd === 'chat_image_open') return null;
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
      if (cmd === 'game_act') {
        gameAct(snap.games.find((x) => x.id === args.id), args.action);
        setTimeout(() => emit('games', structuredClone(snap.games)), 40);
        return null;
      }
      if (cmd === 'game_invite') {
        const others = args.players.map((f) => ({ fingerprint: f, name: snap.peers.find((p) => p.fingerprint === f)?.name || 'Kişi' }));
        const seat = args.seat ?? Math.round(Math.random());
        const players = seat === 0 ? [{ fingerprint: me, name: 'Nur' }, ...others] : [...others, { fingerprint: me, name: 'Nur' }];
        const golf = args.game === 'golf';
        const tee = golfHoles[0].tee;
        const g = gameMock({ id: `gm${Date.now()}`, host: me, hostName: 'Nur', isHost: true, players, joined: [me], status: 'invited', mySeat: seat, createdAt: Date.now(), updatedAt: Date.now(),
          ...(golf ? { game: 'golf', title: 'Mini golf', info: { width: 800, height: 500, ballR: 7, cupR: 11, maxStrokes: 8, holes: golfHoles, hole: 0, quit: players.map(() => false),
            balls: players.map(() => ({ x: tee[0], y: tee[1], strokes: 0, done: false })), scores: players.map(() => []), totals: players.map(() => 0), lastShot: null } } : {}) });
        snap.games.unshift(g);
        setTimeout(() => emit('games', structuredClone(snap.games)), 40);
        return structuredClone(g);
      }
      if (cmd === 'game_remove') {
        snap.games = snap.games.filter((x) => x.id !== args.id);
        setTimeout(() => emit('games', structuredClone(snap.games)), 40);
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
      if (cmd === 'set_status') {
        snap.settings.status = args.status;
        snap.device.status = args.status;
        setTimeout(() => emit('my-status', { status: args.status, auto: false }), 20);
        return structuredClone(snap.settings);
      }
      if (['chat_react', 'chat_edit', 'chat_delete'].includes(cmd)) {
        const m = (history[args.key] || []).find((x) => x.id === args.id);
        if (!m) throw 'Mesaj bulunamadı';
        if (cmd === 'chat_react') {
          m.reactions = (m.reactions || []).filter((r) => r.fp !== me);
          if (args.emoji) m.reactions.push({ fp: me, emoji: args.emoji });
        }
        if (cmd === 'chat_edit') Object.assign(m, { text: args.text, edited: true });
        if (cmd === 'chat_delete') Object.assign(m, { text: '', deleted: true, reactions: [] });
        const conv = snap.conversations.find((c) => c.fingerprint === args.key);
        setTimeout(() => emit('chat', { fingerprint: args.key, name: conv?.name || '', unread: 0, incoming: false, message: structuredClone(m) }), 20);
        return structuredClone(m);
      }
      if (cmd === 'chat_nudge') return null;
      if (cmd === 'chat_queue_files') {
        const name = args.paths[0].split(/[\\/]/).pop();
        const msg = { id: `q${Date.now()}`, outgoing: true, text: `📎 ${name}`, at: Date.now(), status: 'sending', pending: [args.fingerprint], recipients: 1,
          file: { title: name, count: 1, total: 1.2e6, transfers: [], sources: args.paths } };
        (history[args.fingerprint] ||= []).push(msg);
        setTimeout(() => emit('chat', { fingerprint: args.fingerprint, name: args.name, unread: 0, incoming: false, message: msg }), 20);
        return null;
      }
      if (cmd === 'phone_start') {
        const cells = Array.from({ length: 21 * 21 }, (_, i) => ((i * 7919) % 11 < 5 ? `<rect x="${i % 21}" y="${Math.floor(i / 21)}" width="1" height="1"/>` : '')).join('');
        return { url: 'http://192.168.1.107:51234/p/3f9c2a7be1d04c6a9b0e5d7f12ab34cd', offered: [],
          svg: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="-2 -2 25 25" width="240" height="240"><rect x="-2" y="-2" width="25" height="25" fill="#fff"/><g fill="#000">${cells}</g></svg>` };
      }
      if (cmd === 'phone_offer') return { offered: args.paths.map((p, i) => ({ i, name: p.split(/[\\/]/).pop(), size: 2.4e6 })), skipped: 0 };
      if (cmd === 'phone_unoffer') return { offered: [] };
      if (cmd === 'screen_share_start') {
        setTimeout(() => emit('screen-sharing', { active: true, session: 's1', viewers: [], invited: 1 }), 50);
        setTimeout(() => emit('screen-sharing', { active: true, session: 's1', viewers: ['Elif (Tasarım)'], invited: 1 }), 1500);
        return 's1';
      }
      if (cmd === 'screen_share_stop') { setTimeout(() => emit('screen-sharing', { active: false }), 20); return null; }
      if (cmd === 'screen_clip') {
        const c = document.createElement('canvas');
        c.width = 1200; c.height = 700;
        const x = c.getContext('2d');
        x.fillStyle = '#f4f5fa'; x.fillRect(0, 0, 1200, 700);
        x.fillStyle = '#5b55f6'; x.fillRect(0, 0, 1200, 64);
        x.fillStyle = '#fff'; x.font = 'bold 28px sans-serif'; x.fillText('Örnek ekran görüntüsü', 24, 42);
        x.fillStyle = '#171a2b'; x.font = '22px sans-serif';
        ['Satış raporu — Kasım', 'Toplam: 1.284.500 ₺', 'Hedef: %92'].forEach((t, i) => x.fillText(t, 40, 140 + i * 48));
        const blob = await new Promise((r) => c.toBlob(r, 'image/png'));
        return blob.arrayBuffer();
      }
      if (['screen_clip_cancel', 'screen_watch_stop', 'screen_dismiss', 'phone_stop', 'show_window'].includes(cmd)) return null;
      if (cmd === 'update_settings') {
        if ('seat' in args) snap.settings.seat = args.seat;
        return structuredClone(snap.settings);
      }
      return snap.settings;
    },
    listen: async (ev, cb) => { (listeners[ev] ||= []).push(cb); },
    invokeRaw: async (cmd, body, headers) => { console.log('[önizleme]', cmd, body.length, 'bayt', headers); return null; },
    onDragDrop: () => {},
    pick: async () => null,
  };
}

// Önizleme: sahte ekran kareleri.
function previewScreen(session) {
  const c = document.createElement('canvas');
  c.width = 1280; c.height = 720;
  const x = c.getContext('2d');
  $('#sv-info').textContent = '1280 × 720';
  const tick = async () => {
    if (screenState.watching?.session !== session) return;
    const t = Date.now() / 1000;
    x.fillStyle = '#1e1f2e'; x.fillRect(0, 0, 1280, 720);
    x.fillStyle = '#2b2d42'; x.fillRect(0, 0, 1280, 44);
    x.fillStyle = '#fff'; x.font = '20px sans-serif'; x.fillText('Figma — Ana sayfa tasarımı', 20, 29);
    for (let i = 0; i < 6; i++) { x.fillStyle = `hsl(${(i * 50 + t * 20) % 360} 60% 55%)`; x.fillRect(80 + i * 190, 160 + Math.sin(t + i) * 30, 160, 260); }
    x.fillStyle = '#fff'; x.beginPath(); x.arc(640 + Math.cos(t) * 300, 560, 10, 0, 7); x.fill();
    const blob = await new Promise((r) => c.toBlob(r, 'image/jpeg', 0.7));
    showFrame(await blob.arrayBuffer());
    setTimeout(tick, 150);
  };
  setTimeout(tick, 400);
}
