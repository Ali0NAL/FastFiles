// YouTube oynatıcısı (iframe) denetimi — dış betik yüklemeden.
//
// YouTube'un resmî IFrame API betiği (youtube.com/iframe_api) uygulamanın içerik güvenliği
// politikası gereği yüklenmez. Onun yerine gömülü oynatıcıyla aynı `postMessage` protokolü
// doğrudan konuşulur: oynatıcıya "listening" denir, oynatıcı da durumunu (`infoDelivery`) ve
// olaylarını (`onReady`, `onStateChange`, `onError`) gönderir; komutlar `{event:"command"}` ile gider.
// Yalnızca https://www.youtube.com kaynağından gelen ve bu iframe'e ait mesajlar dikkate alınır.

export const YT_ORIGIN = 'https://www.youtube.com';
export const YT = { UNSTARTED: -1, ENDED: 0, PLAYING: 1, PAUSED: 2, BUFFERING: 3, CUED: 5 };

const ID_RE = /^[A-Za-z0-9_-]{11}$/;

/** "90", "90s", "1m30s", "1h2m3s" → saniye. */
function parseStart(t) {
  if (!t) return 0;
  if (/^\d+(\.\d+)?$/.test(t)) return Math.floor(Number(t));
  const m = /^(?:(\d+)h)?(?:(\d+)m)?(?:(\d+)s)?$/i.exec(t);
  if (!m || !m[0]) return 0;
  return (Number(m[1] || 0) * 3600) + (Number(m[2] || 0) * 60) + Number(m[3] || 0);
}

/**
 * YouTube bağlantısını çözer: watch?v=, youtu.be/, shorts/, live/, embed/ ve çıplak 11 karakterlik
 * kimlik. Başlangıç saniyesi `t`/`start` parametresinden alınır. Tanınmazsa `null`.
 */
export function parseYouTube(input) {
  const s = String(input || '').trim();
  if (!s) return null;
  if (ID_RE.test(s)) return { video: s, start: 0 };
  let url;
  try {
    url = new URL(/^[a-z]+:\/\//i.test(s) ? s : `https://${s}`);
  } catch {
    return null;
  }
  if (!/^https?:$/.test(url.protocol)) return null;
  const host = url.hostname.toLowerCase().replace(/^(www|m|music)\./, '');
  const parts = url.pathname.split('/').filter(Boolean);
  let video = null;
  if (host === 'youtu.be') video = parts[0];
  else if (host === 'youtube.com' || host === 'youtube-nocookie.com') {
    if (parts[0] === 'watch') video = url.searchParams.get('v');
    else if (['shorts', 'live', 'embed', 'v'].includes(parts[0])) video = parts[1];
  }
  if (!video || !ID_RE.test(video)) return null;
  const hashT = /(?:^|&)t=([^&]+)/.exec(url.hash.slice(1))?.[1];
  const start = parseStart(url.searchParams.get('t') || url.searchParams.get('start') || hashT);
  return { video, start };
}

/** Metindeki ilk YouTube bağlantısı (sohbet mesajları için). */
export function findYouTube(text) {
  for (const m of String(text || '').matchAll(/(?:https?:\/\/|www\.)[^\s<>"']+|(?:^|\s)(?:youtu\.be|youtube\.com)\/[^\s<>"']+/gi)) {
    const hit = parseYouTube(m[0].trim().replace(/[.,!?;:)\]]+$/, ''));
    if (hit) return hit;
  }
  return null;
}

export const ytThumb = (id) => `https://i.ytimg.com/vi/${id}/mqdefault.jpg`;
export const ytWatchUrl = (id, t = 0) => `https://www.youtube.com/watch?v=${id}${t >= 1 ? `&t=${Math.floor(t)}s` : ''}`;

export function fmtClock(sec) {
  if (!Number.isFinite(sec) || sec < 0) sec = 0;
  sec = Math.floor(sec);
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  const s = String(sec % 60).padStart(2, '0');
  return h ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`;
}

let nextId = 1;

export class YTPlayer {
  /**
   * @param {HTMLIFrameElement} iframe
   * @param {{onReady?, onState?, onTime?, onError?, onTitle?}} handlers
   */
  constructor(iframe, handlers = {}) {
    this.iframe = iframe;
    this.h = handlers;
    this.id = nextId++;
    this.ready = false;
    this.video = null;
    this.state = YT.UNSTARTED;
    this.duration = 0;
    this.title = '';
    this._t = 0;
    this._tAt = performance.now();
    this._hello = null;
    this._onMsg = (e) => this._message(e);
    window.addEventListener('message', this._onMsg);
    iframe.addEventListener('load', () => this._handshake());
  }

  destroy() {
    window.removeEventListener('message', this._onMsg);
    clearInterval(this._hello);
    this.iframe.src = 'about:blank';
    this.ready = false;
    this.video = null;
  }

  /** Videoyu açar. İlk açılışta iframe kurulur; sonrakilerde oynatıcıya komut gider. */
  load(video, start = 0, play = true) {
    start = Math.max(0, start || 0);
    this.video = video;
    this.title = '';
    this.duration = 0;
    this._setTime(start);
    if (this.ready) {
      this.command(play ? 'loadVideoById' : 'cueVideoById', [{ videoId: video, startSeconds: start }]);
      return;
    }
    const origin = location.origin;
    const q = new URLSearchParams({
      enablejsapi: '1',
      origin,
      widget_referrer: origin,
      playsinline: '1',
      rel: '0',
      iv_load_policy: '3',
      autoplay: play ? '1' : '0',
      start: String(Math.floor(start)),
    });
    this.iframe.src = `${YT_ORIGIN}/embed/${video}?${q}`;
  }

  play() { this.command('playVideo'); }
  pause() { this.command('pauseVideo'); }
  seek(sec) {
    this.command('seekTo', [Math.max(0, sec), true]);
    this._setTime(sec);
  }
  mute(on) { this.command(on ? 'mute' : 'unMute'); }

  /** Tahmini anlık konum (oynatıcı ~4 kez/sn bildirir; arası tahmin edilir). */
  time() {
    if (this.state !== YT.PLAYING) return this._t;
    return this._t + (performance.now() - this._tAt) / 1000;
  }

  command(func, args = []) {
    const w = this.iframe.contentWindow;
    if (!w) return;
    w.postMessage(JSON.stringify({ event: 'command', func, args, id: this.id, channel: 'widget' }), YT_ORIGIN);
  }

  _setTime(t) {
    this._t = t;
    this._tAt = performance.now();
  }

  _handshake() {
    clearInterval(this._hello);
    if (!this.iframe.src.startsWith(YT_ORIGIN)) return;
    this.ready = false;
    let tries = 0;
    const hello = () => {
      if (this.ready || ++tries > 60) return clearInterval(this._hello);
      this.iframe.contentWindow?.postMessage(JSON.stringify({ event: 'listening', id: this.id, channel: 'widget' }), YT_ORIGIN);
    };
    hello();
    this._hello = setInterval(hello, 250);
  }

  _message(e) {
    if (e.origin !== YT_ORIGIN || e.source !== this.iframe.contentWindow) return;
    let d;
    try {
      d = typeof e.data === 'string' ? JSON.parse(e.data) : e.data;
    } catch {
      return;
    }
    if (!d || typeof d !== 'object') return;
    if (d.event === 'onReady' || d.event === 'initialDelivery') {
      if (!this.ready) {
        this.ready = true;
        clearInterval(this._hello);
        for (const ev of ['onStateChange', 'onError']) this.command('addEventListener', [ev]);
        this.h.onReady?.();
      }
    }
    if (d.event === 'onStateChange') this._state(Number(d.info));
    if (d.event === 'onError') this.h.onError?.(Number(d.info));
    const info = d.info;
    if ((d.event === 'infoDelivery' || d.event === 'initialDelivery') && info && typeof info === 'object') {
      if (typeof info.duration === 'number' && info.duration > 0) this.duration = info.duration;
      if (typeof info.currentTime === 'number') {
        // Oynatıcı ölçümün ne zaman yapıldığını da bildirir: aradaki süre eklenir.
        const lag = typeof info.currentTimeLastUpdated_ === 'number' && this.state === YT.PLAYING
          ? Math.min(1, Math.max(0, Date.now() / 1000 - info.currentTimeLastUpdated_)) : 0;
        this._setTime(info.currentTime + lag);
        this.h.onTime?.(this.time());
      }
      if (typeof info.playerState === 'number' && info.playerState !== this.state) this._state(info.playerState);
      const title = info.videoData?.title;
      const vid = info.videoData?.video_id;
      if (title && title !== this.title && (!vid || vid === this.video)) {
        this.title = title;
        this.h.onTitle?.(title, this.video);
      }
    }
  }

  _state(s) {
    if (!Number.isFinite(s) || s === this.state) return;
    // Konum tahmini, durum değişmeden önceki hızla dondurulur.
    this._setTime(this.time());
    this.state = s;
    this.h.onState?.(s);
  }
}
