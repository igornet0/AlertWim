const tauri = window.__TAURI__
const invoke = tauri?.core?.invoke ?? (async () => null)
const listen = tauri?.event?.listen ?? (async () => () => {})
const appWindow = tauri?.window?.getCurrentWindow?.()

const $ = (id) => document.getElementById(id)
const logEl = $('log')
const state = { alerts: [], level: 'all', q: '', pinned: false, follow: true, unseen: 0, boarddoSeen: null }

const pad = (n) => String(n).padStart(2, '0')
const fmtTime = (ts) => { const d = new Date(ts); return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}` }
const esc = (s) => String(s ?? '').replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c])
function highlight(text) {
  const safe = esc(text)
  if (!state.q) return safe
  const q = state.q.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  return safe.replace(new RegExp(esc(q), 'gi'), (m) => `<mark>${m}</mark>`)
}
function matches(a) {
  if (state.level !== 'all' && a.level !== state.level) return false
  if (!state.q) return true
  const hay = `${a.title ?? ''} ${a.message} ${a.source ?? ''} ${a.data ? JSON.stringify(a.data) : ''}`.toLowerCase()
  return hay.includes(state.q.toLowerCase())
}

function rowHtml(a) {
  const head = a.title || a.source
    ? `<div class="head">${a.title ? `<span class="title">${highlight(a.title)}</span>` : ''}${a.source ? `<span class="src">${highlight(a.source)}</span>` : ''}</div>`
    : ''
  const msg = a.message ? `<p class="msg">${highlight(a.message)}</p>` : ''
  const data = a.data ? `<button class="more" data-id="${a.id}">данные ▸</button>` : ''
  return `<span class="time" title="${new Date(a.ts).toLocaleString()}">${fmtTime(a.ts)}</span>
    <span class="badge lv-${a.level}">${a.level}</span>
    <div class="body">${head}${msg}${data}</div>`
}
function makeRow(a, fresh) {
  const el = document.createElement('div')
  el.className = `row lv-${a.level}${fresh ? ' fresh' : ''}`
  el.dataset.id = a.id
  el.innerHTML = rowHtml(a)
  return el
}

function renderCounts() {
  const counts = { all: state.alerts.length }
  for (const a of state.alerts) counts[a.level] = (counts[a.level] ?? 0) + 1
  for (const b of $('filters').querySelectorAll('button')) {
    b.querySelector('b').textContent = counts[b.dataset.level] ?? 0
    b.classList.toggle('on', b.dataset.level === state.level)
  }
  $('total').textContent = `${state.alerts.length} ${plural(state.alerts.length)}`
}
const plural = (n) => (n % 10 === 1 && n % 100 !== 11 ? 'запись' : [2, 3, 4].includes(n % 10) && ![12, 13, 14].includes(n % 100) ? 'записи' : 'записей')

function renderAll() {
  const frag = document.createDocumentFragment()
  const visible = state.alerts.filter(matches)
  for (const a of visible) frag.appendChild(makeRow(a, false))
  logEl.replaceChildren(frag)
  $('empty').hidden = visible.length > 0
  renderCounts()
  scrollToEnd(true)
}
function addAlert(a) {
  state.alerts.push(a)
  if (state.alerts.length > 2000) { state.alerts.shift(); logEl.firstElementChild?.remove() }
  renderCounts()
  if (!matches(a)) return
  logEl.appendChild(makeRow(a, true))
  $('empty').hidden = true
  if (state.follow) scrollToEnd()
  else { state.unseen += 1; $('jump').hidden = false; $('jump').textContent = `↓ Новые (${state.unseen})` }
}
function scrollToEnd(instant) {
  logEl.scrollTo({ top: logEl.scrollHeight, behavior: instant ? 'auto' : 'smooth' })
  state.unseen = 0
  $('jump').hidden = true
}

logEl.addEventListener('scroll', () => {
  state.follow = logEl.scrollHeight - logEl.scrollTop - logEl.clientHeight < 40
  if (state.follow) { state.unseen = 0; $('jump').hidden = true }
})
logEl.addEventListener('click', (e) => {
  const btn = e.target.closest('.more')
  if (!btn) return
  const a = state.alerts.find((x) => x.id === btn.dataset.id)
  const open = btn.nextElementSibling?.classList.contains('data')
  if (open) { btn.nextElementSibling.remove(); btn.textContent = 'данные ▸'; return }
  const pre = document.createElement('pre')
  pre.className = 'data'
  pre.textContent = JSON.stringify(a?.data, null, 2)
  btn.after(pre)
  btn.textContent = 'данные ▾'
})
$('jump').addEventListener('click', () => { state.follow = true; scrollToEnd() })
$('filters').addEventListener('click', (e) => {
  const b = e.target.closest('button'); if (!b) return
  state.level = b.dataset.level; renderAll()
})
let qTimer
$('q').addEventListener('input', (e) => { clearTimeout(qTimer); qTimer = setTimeout(() => { state.q = e.target.value.trim(); renderAll() }, 120) })
$('clear').addEventListener('click', async () => { await invoke('clear_alerts'); state.alerts = []; renderAll() })
$('pin').addEventListener('click', async () => {
  state.pinned = !state.pinned
  $('pin').classList.toggle('on', state.pinned)
  try { await appWindow?.setAlwaysOnTop(state.pinned) } catch {}
})
document.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); $('q').focus() }
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'l') { e.preventDefault(); $('clear').click() }
  if (e.key === 'Escape' && document.activeElement === $('q')) { $('q').value = ''; state.q = ''; renderAll(); $('q').blur() }
})

function renderLink() {
  const el = $('link'), text = $('linkText')
  const seen = state.boarddoSeen
  const live = seen && Date.now() - seen < 30 * 60 * 1000
  el.classList.toggle('live', Boolean(live))
  if (!seen) { text.textContent = 'Ожидаю BoardDo'; return }
  const s = Math.round((Date.now() - seen) / 1000)
  const ago = s < 10 ? 'сейчас' : s < 60 ? `${s} с назад` : s < 3600 ? `${Math.round(s / 60)} мин назад` : `${Math.round(s / 3600)} ч назад`
  text.textContent = `BoardDo · ${ago}`
}

async function init() {
  const info = await invoke('get_info')
  if (info) { $('url').textContent = `${info.url} · v${info.version}`; state.boarddoSeen = info.boarddo_seen }
  state.alerts = (await invoke('get_alerts')) ?? []
  renderAll(); renderLink()
  await listen('alert', (e) => addAlert(e.payload))
  await listen('cleared', () => { state.alerts = []; renderAll() })
  await listen('boarddo-seen', (e) => { state.boarddoSeen = e.payload; renderLink() })
  setInterval(renderLink, 10_000)
}
init()
