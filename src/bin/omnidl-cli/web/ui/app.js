// omnidl web interface. No build step: plain JavaScript against the JSON API.
// The page loads /api/state once, then follows /api/events. Every event carries
// a sequence number, so events that raced with the state request are applied
// only if they are newer than the state.
'use strict';

const $ = (id) => document.getElementById(id);

const state = {
  jobs: new Map(),
  seq: 0,
  tools: null,
  settings: null,
  dirLocked: false,
  options: null,
  version: '',
  auth: false,
  loaded: false,
  connected: false,
  offlineSince: 0,
  // Events that arrive while /api/state is loading.
  pending: null,
  // Groups folded in this browser.
  folded: new Set(),
  hideWarning: false,
};

// ---------------------------------------------------------------- server

async function api(method, path, body) {
  const init = { method, headers: { 'X-Omnidl': '1' }, credentials: 'same-origin' };
  if (body !== undefined) {
    init.headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(body);
  }
  let res;
  try {
    res = await fetch(path, init);
  } catch {
    throw new Error('Server nicht erreichbar.');
  }
  if (res.status === 401) {
    location.href = '/login';
    throw new Error('Bitte anmelden.');
  }
  if (!res.ok) {
    let msg = `Fehler ${res.status}`;
    try { msg = (await res.json()).error || msg; } catch { /* not JSON */ }
    throw new Error(msg);
  }
  return res.status === 204 ? null : res.json();
}

let loading = false;
let loadAgain = false;

async function loadState() {
  if (loading) { loadAgain = true; return; }
  loading = true;
  state.pending = [];
  try {
    const s = await api('GET', '/api/state');
    state.jobs = new Map(s.jobs.map((j) => [j.id, j]));
    state.seq = s.seq;
    state.tools = s.tools;
    state.settings = s.settings;
    state.dirLocked = s.dir_locked;
    state.options = s.options;
    state.version = s.version;
    state.auth = s.auth;
    for (const [kind, data] of state.pending) apply(kind, data);
    state.loaded = true;
    document.body.classList.add('ready');
    settingsChanged();
    render();
  } catch (e) {
    toast(e.message);
  } finally {
    state.pending = null;
    loading = false;
    if (loadAgain) { loadAgain = false; loadState(); }
  }
}

let source = null;

function connect() {
  source = new EventSource('/api/events');
  source.addEventListener('open', () => {
    state.connected = true;
    loadState();
  });
  source.addEventListener('error', () => {
    if (state.connected) state.offlineSince = Date.now();
    state.connected = false;
    render();
    // The browser retries by itself unless the server answered with an error
    // (logged out, or gone for good); then look again in a moment.
    if (source.readyState === EventSource.CLOSED) {
      source = null;
      setTimeout(() => { loadState(); connect(); }, 3000);
    }
  });
  for (const kind of ['job', 'tools', 'settings', 'removed', 'reset']) {
    source.addEventListener(kind, (e) => onEvent(kind, JSON.parse(e.data)));
  }
}

function onEvent(kind, data) {
  if (kind === 'reset') { loadState(); return; }
  if (state.pending) { state.pending.push([kind, data]); return; }
  apply(kind, data);
  render();
}

function apply(kind, data) {
  if (data.seq <= state.seq) return;
  state.seq = data.seq;
  switch (kind) {
    case 'job': state.jobs.set(data.job.id, data.job); break;
    case 'tools': state.tools = data.tools; break;
    case 'settings':
      state.settings = data.settings;
      state.dirLocked = data.dir_locked;
      settingsChanged();
      break;
    case 'removed': for (const id of data.ids) state.jobs.delete(id); break;
  }
}

// ---------------------------------------------------------------- links

// Mirrors detect.rs: the label shown in the badge.
function sourceLabel(url) {
  const u = url.trim();
  const sp = /(?:open\.spotify\.com\/(?:intl-[a-z-]+\/)?(?:embed\/)?|spotify:)(track|album|playlist|artist)[/:]([A-Za-z0-9]{22})/i.exec(u);
  if (sp) {
    return { track: 'Spotify-Track', album: 'Spotify-Album', playlist: 'Spotify-Playlist', artist: 'Spotify-Künstler' }[sp[1].toLowerCase()];
  }
  const rest = u.includes('://') ? u.slice(u.indexOf('://') + 3) : u;
  const host = rest.split(/[/?#]/)[0].split('@').pop().split(':')[0].toLowerCase().replace(/^(www\.|m\.)/, '');
  const path = rest.includes('/') ? rest.slice(rest.indexOf('/') + 1) : '';
  if (host === 'spotify.link') return 'Spotify';
  if (host === 'youtu.be' || host.endsWith('youtube.com')) {
    const list = /^(playlist|@|channel\/|c\/|user\/)/.test(path) || path.includes('list=');
    return list ? 'YouTube-Playlist' : 'YouTube';
  }
  if (host.endsWith('tiktok.com')) return path.includes('/photo/') ? 'TikTok-Fotos' : 'TikTok';
  if (host.endsWith('instagram.com')) return 'Instagram';
  if (['x.com', 'twitter.com', 'mobile.twitter.com'].includes(host)) return 'X / Twitter';
  if (host.endsWith('soundcloud.com')) return path.includes('/sets/') ? 'SoundCloud-Set' : 'SoundCloud';
  return 'Web';
}

// Mirrors detect::split_urls and launch::is_web_link.
function splitUrls(text) {
  return text.split(/\s+/)
    .map((s) => s.replace(/^[<>"']+|[<>"']+$/g, ''))
    .filter((s) => s.includes('://') || s.startsWith('spotify:') || s.includes('.'))
    .map((s) => (s.includes('://') || s.startsWith('spotify:') ? s : `https://${s}`))
    .filter((s) => /^(https?:\/\/|spotify:)/i.test(s) && s.length <= 4096);
}

function links() {
  return [...new Set(splitUrls($('link').value))];
}

function linkChanged() {
  const urls = links();
  const badge = $('badge');
  badge.hidden = urls.length === 0;
  badge.textContent = urls.length === 1 ? sourceLabel(urls[0]) : `${urls.length} Links`;
  $('clear').hidden = $('link').value === '';
  $('start').disabled = urls.length === 0;
  drawLater();
}

async function startDownload(startAt) {
  const urls = links();
  if (!urls.length || !state.settings) return;
  const s = state.settings;
  const body = { urls, mode: isAudio(s.format) ? 'audio' : 'video', format: s.format, playlist: s.playlist };
  if (isAudio(s.format)) body.audio_quality = s.audio_quality; else body.video_quality = s.video_quality;
  if (startAt) body.start_at = startAt;
  $('start').disabled = true;
  try {
    await api('POST', '/api/add', body);
    $('link').value = '';
    linkChanged();
    if (startAt) toast(`Geplant: startet ${describe(startAt)}.`);
  } catch (e) {
    toast(e.message);
    linkChanged();
  }
}

// ---------------------------------------------------------------- time

const pad = (n) => String(n).padStart(2, '0');

// "heute um 2:00", "morgen um 23:30", "am 24.09. um 8:15", "am 01.01.2030 um 1:00"
function describe(unix) {
  const at = new Date(unix * 1000);
  const now = new Date();
  const time = `${at.getHours()}:${pad(at.getMinutes())}`;
  const day = (d) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const tomorrow = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1).getTime();
  if (day(at) === day(now)) return `heute um ${time}`;
  if (day(at) === tomorrow) return `morgen um ${time}`;
  const date = `${pad(at.getDate())}.${pad(at.getMonth() + 1)}.`;
  return at.getFullYear() === now.getFullYear() ? `am ${date} um ${time}` : `am ${date}${at.getFullYear()} um ${time}`;
}

// The next time the clock shows h:m, today or tomorrow (Unix seconds).
function nextAt(h, m) {
  const now = new Date();
  const t = new Date(now.getFullYear(), now.getMonth(), now.getDate(), h, m, 0);
  if (t <= now) t.setDate(t.getDate() + 1);
  return Math.floor(t.getTime() / 1000);
}

function presetStart(p) {
  const now = Math.floor(Date.now() / 1000);
  if (p === '30') return now + 30 * 60;
  if (p === '60') return now + 60 * 60;
  return nextAt(2, 0);
}

function customTime() {
  const [h, m] = ($('later-time').value || '').split(':').map(Number);
  return Number.isInteger(h) && Number.isInteger(m) ? nextAt(h, m) : null;
}

// ---------------------------------------------------------------- settings

const isAudio = (format) => !['Mp4', 'Mkv'].includes(format);

function option(value, label) {
  const o = document.createElement('option');
  o.value = value;
  o.textContent = label;
  return o;
}

function fill(select, items, selected) {
  const key = items.map((i) => i.id).join();
  if (select.dataset.key !== key) {
    select.replaceChildren(...items.map((i) => option(i.id, i.label)));
    select.dataset.key = key;
  }
  select.value = selected;
}

function settingsChanged() {
  const s = state.settings;
  const o = state.options;
  if (!s || !o) return;
  const audio = isAudio(s.format);
  $('kind').dataset.value = audio ? 'audio' : 'video';
  for (const b of $('kind').querySelectorAll('button')) {
    b.setAttribute('aria-checked', String((b.dataset.mode === 'audio') === audio));
  }
  fill($('format'), o.formats.filter((f) => f.audio === audio), s.format);

  const quality = $('quality');
  const format = o.formats.find((f) => f.id === s.format);
  if (!audio) {
    quality.disabled = false;
    quality.title = '';
    fill(quality, o.video_qualities, s.video_quality);
  } else if (format && format.bitrate) {
    quality.disabled = false;
    quality.title = 'Die Quelle hat meist 128–160 kbps. Höhere Werte machen die Datei größer, nicht besser.';
    fill(quality, o.audio_qualities, s.audio_quality);
  } else {
    quality.disabled = true;
    quality.title = 'Die Quelle ist verlustbehaftet (~128–160 kbps). FLAC und WAV machen die Datei größer, nicht besser.';
    fill(quality, [{ id: 'lossless', label: 'Verlustfrei' }], 'lossless');
  }
  $('playlist').checked = s.playlist;

  const dirInput = $('dir-input');
  if (document.activeElement !== dirInput) dirInput.value = s.download_dir;
  dirInput.hidden = state.dirLocked;
  $('dir-fixed').hidden = !state.dirLocked;
  $('dir-fixed').textContent = s.download_dir;
  if (!$('dir-note').classList.contains('error')) {
    $('dir-note').textContent = state.dirLocked
      ? 'Beim Start mit --dir festgelegt.'
      : 'Ordner auf dem Server. Fehlt er, wird er angelegt.';
  }
  $('parallel').textContent = s.parallel;
  for (const b of document.querySelectorAll('.stepper button')) {
    const next = s.parallel + Number(b.dataset.step);
    b.disabled = next < 1 || next > 16;
  }
  fill($('cookies'), o.cookies, s.cookies);
  render();
}

// Changes one or more settings for every browser; shows the result right away.
async function change(patch) {
  const s = state.settings;
  if (patch.mode) {
    const audio = patch.mode === 'audio';
    if (audio !== isAudio(s.format)) s.format = audio ? s.last_audio : s.last_video;
  }
  if (patch.format) {
    s.format = patch.format;
    if (isAudio(patch.format)) s.last_audio = patch.format; else s.last_video = patch.format;
  }
  for (const k of ['video_quality', 'audio_quality', 'playlist', 'cookies', 'parallel']) {
    if (k in patch) s[k] = patch[k];
  }
  settingsChanged();
  try {
    const v = await api('PUT', '/api/settings', patch);
    state.settings = v.settings;
    state.dirLocked = v.dir_locked;
    settingsChanged();
    return true;
  } catch (e) {
    toast(e.message);
    loadState();
    return false;
  }
}

async function changeDir() {
  const input = $('dir-input');
  const value = input.value.trim();
  if (state.dirLocked || !state.settings || value === state.settings.download_dir) return;
  const note = $('dir-note');
  try {
    const v = await api('PUT', '/api/settings', { download_dir: value });
    note.classList.remove('error');
    state.settings = v.settings;
    settingsChanged();
    note.textContent = 'Gespeichert.';
  } catch (e) {
    note.classList.add('error');
    note.textContent = e.message;
  }
}

// ---------------------------------------------------------------- drawing

let frame = 0;

function render() {
  if (!frame) frame = requestAnimationFrame(() => { frame = 0; draw(); });
}

function draw() {
  if (!state.loaded) return;
  drawBanner();
  drawList();
  drawSettings();
  drawFooter();
}

const finished = (job) => ['done', 'nomatch', 'failed', 'cancelled'].includes(job.state.kind);
const scheduled = (job) => job.state.kind === 'scheduled';

const ICON = {
  done: '<svg viewBox="0 0 24 24"><circle class="fill" cx="12" cy="12" r="11"/><path class="glyph" d="M7.4 12.2l3.2 3.3 6-6.6"/></svg>',
  failed: '<svg viewBox="0 0 24 24"><circle class="fill" cx="12" cy="12" r="11"/><path class="glyph" d="M12 6.8v6.2"/><circle cx="12" cy="16.8" r="1.35" fill="#fff"/></svg>',
  cancelled: '<svg viewBox="0 0 24 24"><circle class="fill" cx="12" cy="12" r="11"/><path class="glyph" d="M7.4 12h9.2"/></svg>',
  clock: '<svg viewBox="0 0 24 24"><circle class="line" cx="12" cy="12" r="10"/><path class="line" d="M12 6.4V12l4.2 2.4"/></svg>',
  ring: '<svg class="ring" viewBox="0 0 24 24"><circle class="track" cx="12" cy="12" r="9.5"/><circle class="arc" cx="12" cy="12" r="9.5" transform="rotate(-90 12 12)"/></svg>',
  info: '<svg viewBox="0 0 24 24"><circle class="line" cx="12" cy="12" r="9.5"/><path class="line" d="M12 7.5v8M8.4 12.4L12 16l3.6-3.6"/></svg>',
  warning: '<svg viewBox="0 0 24 24"><circle class="fill" cx="12" cy="12" r="10"/><path class="glyph" d="M12 7v5.6"/><circle cx="12" cy="16.4" r="1.3" fill="#fff"/></svg>',
};

const ACT = {
  save: '<svg viewBox="0 0 24 24"><path class="line" d="M12 3.5v12M6.8 10.6L12 15.8l5.2-5.2M4.5 20h15"/></svg>',
  retry: '<svg viewBox="0 0 24 24"><path class="line" d="M17.66 8.39A7 7 0 1 1 12 5.5"/><path class="fill" d="M15.6 5.5l-4.6-3v6z"/></svg>',
  now: '<svg viewBox="0 0 24 24"><path class="fill" d="M8 5.6v12.8a.8.8 0 0 0 1.2.7l10-6.4a.8.8 0 0 0 0-1.4l-10-6.4A.8.8 0 0 0 8 5.6z"/></svg>',
  stop: '<svg viewBox="0 0 24 24"><circle class="fill" cx="12" cy="12" r="9.5"/><path class="cut" d="M9 9l6 6M15 9l-6 6"/></svg>',
  chevron: '<svg viewBox="0 0 12 12"><path d="M4.2 1.8L8.4 6l-4.2 4.2" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>',
};

function iconKind(job) {
  switch (job.state.kind) {
    case 'done': return 'done';
    case 'failed': return 'failed';
    case 'nomatch': return 'nomatch';
    case 'cancelled': return 'cancelled';
    case 'scheduled': return 'scheduled';
    case 'queued': return 'queued';
    case 'downloading':
    case 'group': return job.frac == null ? 'spin' : 'ring';
    default: return 'spin';
  }
}

const ICON_SVG = {
  done: ICON.done, failed: ICON.failed, nomatch: ICON.failed, cancelled: ICON.cancelled,
  scheduled: ICON.clock, queued: ICON.clock, ring: ICON.ring, spin: ICON.ring,
};

// The list in display order: newest first, children under their group.
function rows() {
  const kids = new Map();
  const top = [];
  for (const j of state.jobs.values()) {
    if (j.parent != null && state.jobs.has(j.parent)) {
      if (!kids.has(j.parent)) kids.set(j.parent, []);
      kids.get(j.parent).push(j);
    } else {
      top.push(j);
    }
  }
  top.sort((a, b) => b.id - a.id);
  const out = [];
  for (const job of top) {
    const children = (kids.get(job.id) || []).sort((a, b) => a.id - b.id);
    const open = !state.folded.has(job.id);
    out.push({ job, child: false, group: children.length > 0 || job.state.kind === 'group', open });
    if (open) for (const c of children) out.push({ job: c, child: true, group: false, open: false });
  }
  return out;
}

function statusLine(job, child) {
  if (scheduled(job)) {
    const when = `Startet ${describe(job.state.detail)}`;
    return child ? when : `${job.source} · ${when}`;
  }
  // Finished files say what they are and how big, before they go onto a phone.
  if (job.download === 'file' && job.state.kind === 'done') {
    const ext = job.file && job.file.includes('.') ? job.file.split('.').pop().toUpperCase() : '';
    return [job.status, ext, job.size].filter(Boolean).join(' · ');
  }
  return job.status;
}

function makeRow() {
  const el = document.createElement('div');
  el.className = 'row';
  const icon = document.createElement('div');
  icon.className = 'status-icon';
  const text = document.createElement('div');
  text.className = 'text';
  const title = document.createElement('div');
  title.className = 'title';
  const line = document.createElement('div');
  line.className = 'line';
  text.append(title, line);
  const actions = document.createElement('div');
  actions.className = 'row-actions';
  el.append(icon, text, actions);
  el.parts = { icon, title, line, actions };
  el.sig = {};
  return el;
}

function actionButton(act, label) {
  const b = document.createElement('button');
  b.type = 'button';
  b.className = `act ${act}`;
  b.dataset.act = act;
  b.title = label;
  b.setAttribute('aria-label', label);
  b.innerHTML = ACT[act];
  return b;
}

function saveLink(job) {
  const a = document.createElement('a');
  a.className = 'save';
  a.href = `/api/jobs/${Number(job.id)}/file`;
  a.setAttribute('download', '');
  const zip = job.download !== 'file';
  a.title = zip ? 'Als ZIP auf dieses Gerät laden' : 'Auf dieses Gerät laden';
  a.setAttribute('aria-label', a.title);
  a.innerHTML = ACT.save;
  const label = document.createElement('span');
  label.textContent = zip ? 'ZIP laden' : 'Herunterladen';
  a.append(label);
  return a;
}

function updateRow(el, { job, child, group, open }) {
  const p = el.parts;
  el.dataset.id = job.id;
  el.classList.toggle('child', child);
  el.classList.toggle('group', group);
  el.classList.toggle('open', group && open);

  const kind = iconKind(job);
  if (el.sig.icon !== kind) {
    p.icon.innerHTML = ICON_SVG[kind];
    p.icon.className = `status-icon ${kind}`;
    el.sig.icon = kind;
    p.ring = p.icon.querySelector('.ring');
    if (p.ring) p.ring.classList.toggle('spin', kind === 'spin');
  }
  if (kind === 'ring') {
    const frac = Math.max(0, Math.min(1, job.frac));
    p.ring.querySelector('.arc').setAttribute('stroke-dashoffset', (59.69 * (1 - frac)).toFixed(2));
  }

  if (p.title.textContent !== job.title) p.title.textContent = job.title;
  const line = statusLine(job, child);
  if (p.line.textContent !== line) p.line.textContent = line;
  p.line.className = `line ${job.tone}`;
  const detail = job.state.kind === 'failed' ? `${job.title}\n\n${job.state.detail}` : '';
  if (el.title !== detail) el.title = detail;

  const acts = [];
  if (job.download) acts.push('save');
  if (job.can_retry) acts.push('retry');
  if (scheduled(job)) acts.push('now');
  if (!finished(job)) acts.push('stop');
  if (group) acts.push('chevron');
  const sig = acts.join() + job.download;
  if (el.sig.actions !== sig) {
    const nodes = acts.map((a) => {
      if (a === 'save') return saveLink(job);
      if (a === 'retry') return actionButton('retry', 'Erneut versuchen');
      if (a === 'now') return actionButton('now', 'Jetzt starten');
      if (a === 'stop') return actionButton('stop', 'Stoppen');
      const c = document.createElement('span');
      c.className = 'chevron';
      c.innerHTML = ACT.chevron;
      return c;
    });
    p.actions.replaceChildren(...nodes);
    el.sig.actions = sig;
  }
}

const rowEls = new Map();

function drawList() {
  const list = $('list');
  const all = rows();
  $('empty').hidden = state.jobs.size > 0;
  list.hidden = state.jobs.size === 0;

  const keep = new Set();
  let at = list.firstChild;
  for (const r of all) {
    let el = rowEls.get(r.job.id);
    if (!el) { el = makeRow(); rowEls.set(r.job.id, el); }
    keep.add(r.job.id);
    updateRow(el, r);
    if (el !== at) list.insertBefore(el, at); else at = at.nextSibling;
  }
  for (const [id, el] of rowEls) {
    if (!keep.has(id)) { el.remove(); rowEls.delete(id); }
  }

  const top = all.filter((r) => !r.child).map((r) => r.job);
  const planned = top.filter(scheduled).length;
  const active = top.filter((j) => !finished(j) && !scheduled(j)).length;
  const counts = [[active, 'aktiv'], [planned, 'geplant']].filter(([n]) => n > 0).map(([n, w]) => `${n} ${w}`);
  $('counts').textContent = counts.join(' · ');
  $('cancel-all').hidden = !top.some((j) => !finished(j));
  $('clear-list').hidden = !top.some(finished);
  document.title = active ? `omnidl – ${active} aktiv` : 'omnidl';
}

function banner() {
  if (!state.connected && state.offlineSince && Date.now() - state.offlineSince > 1500) {
    return { tone: 'warning', text: 'Keine Verbindung zum Server – verbinde neu …' };
  }
  const d = state.tools;
  if (!d) return null;
  const update = ['Aktualisieren', updateTools];
  if (d.busy) {
    // The routine check at startup takes a moment; not worth a banner.
    if (!d.checked && d.busy.startsWith('Prüfe')) return null;
    return { tone: 'busy', text: d.busy };
  }
  if (!d.checked) return null;
  if (d.error) return { tone: 'error', text: `Werkzeuge konnten nicht eingerichtet werden: ${d.error}`, action: ['Erneut versuchen', updateTools] };
  if (!d.ytdlp) return { tone: 'error', text: 'yt-dlp fehlt oder startet nicht.', action: ['Neu laden', updateTools] };
  if (!d.ffmpeg) return { tone: 'error', text: 'ffmpeg fehlt – ohne geht keine Umwandlung.', action: ['Laden', updateTools] };
  if (state.hideWarning) return null;
  if (!d.js) return { tone: 'warning', text: 'Keine JavaScript-Laufzeit gefunden – YouTube braucht eine.', action: ['Laden', updateTools], dismiss: true };
  if (d.ytdlp_age_days > 14) {
    return {
      tone: 'warning',
      text: `yt-dlp wurde seit ${d.ytdlp_age_days} Tagen nicht aktualisiert. Scheitern YouTube-Downloads, hilft meist ein Update.`,
      action: update,
      dismiss: true,
    };
  }
  return null;
}

let bannerAction = null;

function drawBanner() {
  const b = banner();
  const el = $('banner');
  el.hidden = !b;
  if (!b) return;
  el.dataset.tone = b.tone;
  const icon = b.tone === 'busy' ? 'busy' : b.tone === 'info' ? 'info' : 'warning';
  if (el.dataset.icon !== icon) {
    $('banner-icon').innerHTML = icon === 'busy' ? ICON.ring : ICON[icon];
    const ring = $('banner-icon').querySelector('.ring');
    if (ring) ring.classList.add('spin');
    el.dataset.icon = icon;
  }
  $('banner-text').textContent = b.text;
  $('banner-action').hidden = !b.action;
  if (b.action) $('banner-action').textContent = b.action[0];
  bannerAction = b.action ? b.action[1] : null;
  $('banner-dismiss').hidden = !b.dismiss;
}

function drawSettings() {
  const d = state.tools || {};
  const rowsEl = $('tools');
  const stale = d.ytdlp && d.ytdlp_age_days > 14;
  const tools = [
    ['yt-dlp', d.ytdlp ? (stale ? `${d.ytdlp} · veraltet` : d.ytdlp) : (d.checked ? 'fehlt' : '…'), stale ? 'warning' : d.ytdlp || !d.checked ? '' : 'error', !d.busy],
    ['ffmpeg', d.ffmpeg ? 'bereit' : d.checked ? 'fehlt' : '…', d.ffmpeg || !d.checked ? '' : 'error'],
    ['JavaScript-Laufzeit', d.js === 'deno' ? 'Deno' : d.js ? 'Node.js' : d.checked ? 'fehlt' : '…', d.js || !d.checked ? '' : 'warning'],
    ['gallery-dl', d.gallery ? 'bereit' : d.checked ? 'fehlt' : '…', d.gallery || !d.checked ? '' : 'warning'],
  ];
  const sig = JSON.stringify(tools);
  if (rowsEl.dataset.sig !== sig) {
    rowsEl.replaceChildren(...tools.map(([name, value, tone, button]) => {
      const row = document.createElement('div');
      row.className = 'form-row';
      const n = document.createElement('span');
      n.textContent = name;
      const v = document.createElement('span');
      v.className = `value ${tone}`;
      v.textContent = value;
      row.append(n, v);
      if (button) {
        const b = document.createElement('button');
        b.type = 'button';
        b.className = 'text-button';
        b.textContent = 'Aktualisieren';
        b.addEventListener('click', updateTools);
        row.append(b);
      }
      return row;
    }));
    rowsEl.dataset.sig = sig;
  }
  const note = $('tools-note');
  note.className = `section-note ${d.busy ? 'busy' : d.error ? 'error' : ''}`;
  note.textContent = d.busy || d.error || 'Scheitern YouTube-Downloads, zuerst yt-dlp aktualisieren.';
  $('version').textContent = state.version;
  $('logout-row').hidden = !state.auth;
}

// Long paths keep their end: "…/Downloads/omnidl".
function truncateStart(s, max) {
  return s.length <= max ? s : `…${s.slice(s.length - max + 1)}`;
}

function drawFooter() {
  const dir = state.settings ? state.settings.download_dir : '';
  const max = window.innerWidth < 620 ? 34 : 64;
  $('dir').textContent = truncateStart(dir, max);
  $('folder').title = dir;
  $('ytdlp').textContent = state.tools && state.tools.ytdlp ? `yt-dlp ${state.tools.ytdlp}` : '';
}

// ---------------------------------------------------------------- menu

function drawLater() {
  const can = links().length > 0;
  for (const b of $('later-menu').querySelectorAll('.menu-item')) b.disabled = !can;
  $('later-plan').disabled = !can || customTime() == null;
  $('later-need').hidden = can;
  const at = customTime();
  $('later-note').textContent = at ? `Startet ${describe(at)}.` : '';
}

function openLater(open) {
  $('later-menu').hidden = !open;
  $('later').setAttribute('aria-expanded', String(open));
  if (open) {
    if (!$('later-time').value) $('later-time').value = `${pad((new Date().getHours() + 1) % 24)}:00`;
    drawLater();
  }
}

// ---------------------------------------------------------------- actions

async function updateTools() {
  try { await api('POST', '/api/tools/update'); } catch (e) { toast(e.message); }
}

async function act(action, id) {
  const path = { stop: 'cancel', now: 'start-now', retry: 'retry' }[action];
  if (!path) return;
  try { await api('POST', `/api/jobs/${id}/${path}`); } catch (e) { toast(e.message); }
}

let toastTimer = 0;

function toast(text) {
  const el = $('toast');
  el.textContent = text;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { el.hidden = true; }, 3800);
}

function remember(key, value) {
  try { sessionStorage.setItem(key, value); } catch { /* private mode */ }
}

function recall(key) {
  try { return sessionStorage.getItem(key); } catch { return null; }
}

// ---------------------------------------------------------------- wiring

function wire() {
  const link = $('link');
  link.addEventListener('input', linkChanged);
  link.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.isComposing) { e.preventDefault(); startDownload(null); }
  });
  // Text fields drop line breaks on paste; keep pasted lists apart.
  link.addEventListener('paste', (e) => {
    const text = e.clipboardData && e.clipboardData.getData('text');
    if (!text || !/\s/.test(text.trim())) return;
    e.preventDefault();
    link.setRangeText(text.trim().split(/\s+/).join(' '), link.selectionStart, link.selectionEnd, 'end');
    linkChanged();
  });
  $('field').addEventListener('click', (e) => { if (e.target === $('field')) link.focus(); });
  $('clear').addEventListener('click', () => { link.value = ''; linkChanged(); link.focus(); });
  $('start').addEventListener('click', () => startDownload(null));

  $('later').addEventListener('click', (e) => { e.stopPropagation(); openLater($('later-menu').hidden); });
  $('later-menu').addEventListener('click', (e) => {
    e.stopPropagation();
    const item = e.target.closest('[data-preset]');
    if (item && !item.disabled) { openLater(false); startDownload(presetStart(item.dataset.preset)); }
  });
  $('later-time').addEventListener('input', drawLater);
  $('later-plan').addEventListener('click', () => {
    const at = customTime();
    if (at) { openLater(false); startDownload(at); }
  });
  document.addEventListener('click', () => openLater(false));
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape') openLater(false); });

  $('kind').addEventListener('click', (e) => {
    const b = e.target.closest('[data-mode]');
    if (b && state.settings) change({ mode: b.dataset.mode });
  });
  $('format').addEventListener('change', (e) => change({ format: e.target.value }));
  $('quality').addEventListener('change', (e) => {
    const key = isAudio(state.settings.format) ? 'audio_quality' : 'video_quality';
    change({ [key]: e.target.value });
  });
  $('playlist').addEventListener('change', (e) => change({ playlist: e.target.checked }));

  $('list').addEventListener('click', (e) => {
    const row = e.target.closest('.row');
    if (!row) return;
    if (e.target.closest('a')) return;
    const button = e.target.closest('[data-act]');
    const id = Number(row.dataset.id);
    if (button) { act(button.dataset.act, id); return; }
    if (row.classList.contains('group')) {
      if (state.folded.has(id)) state.folded.delete(id); else state.folded.add(id);
      render();
    }
  });

  $('cancel-all').addEventListener('click', () => api('POST', '/api/cancel-all').catch((e) => toast(e.message)));
  $('clear-list').addEventListener('click', () => api('POST', '/api/clear').catch((e) => toast(e.message)));

  $('banner-action').addEventListener('click', () => { if (bannerAction) bannerAction(); });
  $('banner-dismiss').addEventListener('click', () => {
    state.hideWarning = true;
    remember('omnidl-hide-warning', '1');
    render();
  });

  const dialog = $('settings');
  const openSettings = () => { $('dir-note').classList.remove('error'); settingsChanged(); dialog.showModal(); };
  $('open-settings').addEventListener('click', openSettings);
  $('folder').addEventListener('click', openSettings);
  dialog.addEventListener('click', (e) => { if (e.target === dialog) dialog.close(); });
  $('dir-input').addEventListener('change', changeDir);
  $('dir-input').addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); changeDir(); } });
  for (const b of document.querySelectorAll('.stepper button')) {
    b.addEventListener('click', () => {
      const n = state.settings.parallel + Number(b.dataset.step);
      if (n >= 1 && n <= 16) change({ parallel: n });
    });
  }
  $('cookies').addEventListener('change', (e) => change({ cookies: e.target.value }));
  $('logout').addEventListener('click', async () => {
    try { await api('POST', '/api/logout'); } catch { /* going anyway */ }
    location.href = '/login';
  });

  window.addEventListener('resize', render);
  // Start times read "heute"/"morgen"; keep them true past midnight.
  setInterval(render, 30000);
  // Show the offline banner once the grace period is over.
  setInterval(() => { if (!state.connected) render(); }, 1000);
}

function init() {
  state.hideWarning = recall('omnidl-hide-warning') === '1';
  wire();
  // Shared from another app (installed web app) or a bookmarklet: prefill only.
  const q = new URLSearchParams(location.search);
  const shared = [q.get('url'), q.get('text')].filter(Boolean).join(' ');
  if (shared) {
    $('link').value = splitUrls(shared).join(' ');
    history.replaceState(null, '', '/');
  }
  linkChanged();
  if (matchMedia('(pointer: fine)').matches) $('link').focus();
  loadState();
  connect();
}

init();
