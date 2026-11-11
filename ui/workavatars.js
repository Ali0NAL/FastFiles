// İş modu profil resimleri: 20 sade, kurumsal portre (SVG, dosya gerektirmez).
// Her biri bir numarayla (1..20) seçilir; numara keşif duyurusuyla diğer cihazlara gider
// (arka uçta `work_avatar`, `protocol::WORK_AVATARS` ile aynı sayı).

const SKIN = ['#f6d7c3', '#eac2a5', '#d9a47f', '#b9805a', '#8d5a3b'];
const HAIR = { black: '#23201f', dark: '#3b2a22', brown: '#6b4630', auburn: '#8a3f22', blonde: '#d2a85c', gray: '#9a9a9f' };
const BG = ['#dfe7f2', '#e3eee9', '#ece6f3', '#f1e9df', '#e2ecf1', '#ebeef0', '#f2e4e4', '#e6ebdf'];

// bg: arka plan, s: ten, h: saç modeli, hc: saç rengi, o: kıyafet, c: kıyafet rengi,
// t: kravat/iç renk, g: gözlük, b: sakal
const LIST = [
  { bg: 0, s: 0, h: 'short', hc: 'brown', o: 'suit', c: '#2b3a55', t: '#9b2c2c' },
  { bg: 1, s: 1, h: 'long', hc: 'dark', o: 'blazer', c: '#3d4f63', t: '#f4f1ea' },
  { bg: 2, s: 2, h: 'side', hc: 'black', o: 'suit', c: '#33363d', t: '#2f5d8a', g: 1 },
  { bg: 3, s: 3, h: 'bun', hc: 'black', o: 'blazer', c: '#6b3b3b', t: '#f5ece4' },
  { bg: 4, s: 0, h: 'buzz', hc: 'gray', o: 'turtle', c: '#2f3337', g: 1 },
  { bg: 5, s: 4, h: 'curly', hc: 'black', o: 'shirt', c: '#e9eef5', t: '#c9d3e0' },
  { bg: 6, s: 1, h: 'bob', hc: 'auburn', o: 'turtle', c: '#5a6b5d' },
  { bg: 7, s: 2, h: 'short', hc: 'dark', o: 'suit', c: '#4a4f57', t: '#c48a2a', b: 1 },
  { bg: 0, s: 0, h: 'long', hc: 'blonde', o: 'blazer', c: '#1f2d45', t: '#e9eef6', g: 1 },
  { bg: 1, s: 3, h: 'side', hc: 'black', o: 'shirt', c: '#dfe9f3', t: '#b8c7d9', g: 1 },
  { bg: 2, s: 1, h: 'bald', hc: 'brown', o: 'suit', c: '#2e2f33', t: '#41617f', b: 1 },
  { bg: 3, s: 4, h: 'bun', hc: 'black', o: 'turtle', c: '#8b6f4e' },
  { bg: 4, s: 2, h: 'bob', hc: 'dark', o: 'blazer', c: '#24504a', t: '#f3efe6', g: 1 },
  { bg: 5, s: 0, h: 'side', hc: 'blonde', o: 'suit', c: '#3a4a66', t: '#7a2e46' },
  { bg: 6, s: 3, h: 'curly', hc: 'dark', o: 'blazer', c: '#4b3f5e', t: '#f4f0f7' },
  { bg: 7, s: 1, h: 'short', hc: 'gray', o: 'shirt', c: '#f2f2ee', t: '#d8d8d0', g: 1, b: 1 },
  { bg: 0, s: 2, h: 'long', hc: 'black', o: 'turtle', c: '#2c3a4f' },
  { bg: 1, s: 4, h: 'buzz', hc: 'black', o: 'suit', c: '#383b43', t: '#2e6b5a' },
  { bg: 2, s: 0, h: 'bob', hc: 'brown', o: 'shirt', c: '#e8eef4', t: '#c5d0dc' },
  { bg: 3, s: 2, h: 'side', hc: 'brown', o: 'blazer', c: '#55606e', t: '#eef1f4', b: 1 },
];

export const WORK_AVATARS = LIST.length;

function hairBack(h, hc) {
  if (h === 'long') return `<path d="M17.5 30C17 15 24 9 32 9s15 6 14.5 21l1.5 18H16z" fill="${hc}"/>`;
  if (h === 'bob') return `<path d="M18.5 29C18 15 24.5 10 32 10s14 5 13.5 19l.5 9c-2 1.5-5 2-7.5 2h-12c-2.5 0-5.5-.5-7.5-2z" fill="${hc}"/>`;
  return '';
}

function hairFront(h, hc) {
  switch (h) {
    case 'short': return `<path d="M20.5 27c-.8-9 4-15 11.5-15s12.3 6 11.5 15c-1.2-4.6-3.8-7.6-7.5-8.6-3 1.6-8 2-12.4.4-1.5 2-2.6 4.6-3.1 8.2z" fill="${hc}"/>`;
    case 'side': return `<path d="M20.5 28c-1.4-10 4.2-16.5 12-16.5 7 0 12.4 5 11 16-.6-3.6-1.8-6.4-3.6-8.2-5.6 1.2-12.6-.4-16.2-3.6-1.6 2.6-2.8 6.6-3.2 12.3z" fill="${hc}"/>`;
    case 'buzz': return `<path d="M21 25c0-8.6 5-12.6 11-12.6S43 16.4 43 25c-2.6-4.4-6.4-6.4-11-6.4S23.6 20.6 21 25z" fill="${hc}" opacity=".85"/>`;
    case 'bald': return `<path d="M21 28c-.4-2.6 0-4.4.7-5.6l1.2 5.4zM43 28c.4-2.6 0-4.4-.7-5.6l-1.2 5.4z" fill="${hc}"/>`;
    case 'curly': return `<g fill="${hc}"><circle cx="23" cy="21" r="4.4"/><circle cx="27.5" cy="16" r="4.8"/><circle cx="33" cy="14.2" r="5"/><circle cx="38.5" cy="16.4" r="4.8"/><circle cx="42" cy="21.4" r="4.2"/><circle cx="21.6" cy="25.6" r="3"/><circle cx="42.6" cy="25.6" r="3"/></g>`;
    case 'bun': return `<circle cx="32" cy="9.5" r="5.2" fill="${hc}"/><path d="M20.8 28c-1-9.6 4.4-15.6 11.2-15.6S44.2 18.4 43.2 28c-1.4-5.4-5.4-9-11.2-9s-9.8 3.6-11.2 9z" fill="${hc}"/>`;
    case 'long': return `<path d="M20.6 29c-.6-9.8 4.6-16 11.4-16s12 6.2 11.4 16c-2-6-6-8.4-9.4-9.4-2.8 3.2-8.4 5.4-13.4 9.4z" fill="${hc}"/>`;
    case 'bob': return `<path d="M20.6 28c-.6-9.4 4.8-15 11.4-15s12 5.6 11.4 15c-4.2-1.2-8.4-4.2-10.6-8-2.4 3.8-7.4 6.8-12.2 8z" fill="${hc}"/>`;
    default: return '';
  }
}

function outfit(o, c, t, skin) {
  const shoulders = `<path d="M8 64c0-11 9-18.5 24-18.5S56 53 56 64z" fill="${c}"/>`;
  switch (o) {
    case 'suit': return `${shoulders}
      <path d="M26.2 45.8 32 55.5l5.8-9.7c-1.8-.4-3.8-.6-5.8-.6s-4 .2-5.8.6z" fill="#f7f7f5"/>
      <path d="M30.6 47.2h2.8l.9 2.4-1.3 9.4h-2l-1.3-9.4z" fill="${t}"/>
      <path d="M25.6 46 32 58l-4.4 6H24l-2.4-14.4zM38.4 46 32 58l4.4 6H40l2.4-14.4z" fill="rgba(0,0,0,.16)"/>`;
    case 'blazer': return `${shoulders}
      <path d="M25.8 46c1.6 4.6 3.6 8.4 6.2 11.6 2.6-3.2 4.6-7 6.2-11.6-1.9-.4-4-.6-6.2-.6s-4.3.2-6.2.6z" fill="${t}"/>
      <path d="M25.2 46.2 32 60l-3.6 4H24l-2.6-14.6zM38.8 46.2 32 60l3.6 4H40l2.6-14.6z" fill="rgba(0,0,0,.14)"/>`;
    case 'turtle': return `${shoulders}<rect x="26" y="40.5" width="12" height="7.5" rx="3" fill="${c}"/><path d="M26.4 43.6h11.2" stroke="rgba(0,0,0,.14)" stroke-width="1"/>`;
    case 'shirt': return `${shoulders}
      <path d="M26.8 45.4 32 50.6l-3.2 3.4-3.6-6.4zM37.2 45.4 32 50.6l3.2 3.4 3.6-6.4z" fill="${t}"/>
      <path d="M32 50.6V64" stroke="${t}" stroke-width="1.1"/><circle cx="32" cy="55" r=".7" fill="${t}"/><circle cx="32" cy="59.5" r=".7" fill="${t}"/>`;
    default: return shoulders;
  }
}

/** n: 1..20 → SVG (avatar kutusunu doldurur). */
export function workAvatarSvg(n) {
  const a = LIST[(Math.max(1, Math.min(WORK_AVATARS, n | 0)) - 1)];
  const skin = SKIN[a.s];
  const hc = HAIR[a.hc];
  const shade = 'rgba(0,0,0,.12)';
  return `<svg class="wa" viewBox="0 0 64 64" aria-hidden="true">
    <rect width="64" height="64" fill="${BG[a.bg]}"/>
    ${hairBack(a.h, hc)}
    <path d="M27.4 36h9.2v10.4c-1.4 1.4-3 2-4.6 2s-3.2-.6-4.6-2z" fill="${skin}"/>
    <path d="M27.4 40.5c1.4 1 3 1.5 4.6 1.5s3.2-.5 4.6-1.5V43c-1.4 1-3 1.5-4.6 1.5s-3.2-.5-4.6-1.5z" fill="${shade}"/>
    ${outfit(a.o, a.c, a.t, skin)}
    <ellipse cx="20.8" cy="29" rx="2.2" ry="3" fill="${skin}"/><ellipse cx="43.2" cy="29" rx="2.2" ry="3" fill="${skin}"/>
    <ellipse cx="32" cy="27.5" rx="11.2" ry="13" fill="${skin}"/>
    ${a.b ? `<path d="M21.2 30c.6 7 5 11 10.8 11s10.2-4 10.8-11c-1.6 3-3.6 4.6-5.6 4.8-1.4-1.2-3.2-1.6-5.2-1.6s-3.8.4-5.2 1.6c-2-.2-4-1.8-5.6-4.8z" fill="${hc}" opacity=".9"/>` : ''}
    ${hairFront(a.h, hc)}
    <circle cx="27.6" cy="28.4" r="1.25" fill="#2a2522"/><circle cx="36.4" cy="28.4" r="1.25" fill="#2a2522"/>
    <path d="M25.6 25c1.1-.7 2.6-.8 3.8-.4M38.4 25c-1.1-.7-2.6-.8-3.8-.4" stroke="${a.hc === 'blonde' || a.hc === 'gray' ? '#8a7a66' : hc}" stroke-width="1.1" stroke-linecap="round" fill="none"/>
    <path d="M29 34.2c1.8 1.3 4.2 1.3 6 0" stroke="#9a5a4a" stroke-width="1.1" stroke-linecap="round" fill="none"/>
    ${a.g ? `<g fill="none" stroke="#2f3338" stroke-width="1.1"><rect x="23.6" y="25.6" width="7.4" height="5.6" rx="2"/><rect x="33" y="25.6" width="7.4" height="5.6" rx="2"/><path d="M31 27.6h2M23.6 27.4l-2.4-.8M40.4 27.4l2.4-.8"/></g>` : ''}
  </svg>`;
}
