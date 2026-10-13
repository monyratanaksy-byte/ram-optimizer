// RAM Optimizer website: scroll reveals, theme, live version, and a working
// copy of the app window in the hero.

const REPO = "monyratanaksy-byte/ram-optimizer";
let latestVersion = "0.4.0";
const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/* Theme -------------------------------------------------------------------- */

const root = document.documentElement;
document.getElementById("theme-toggle").addEventListener("click", () => {
  const next = root.dataset.theme === "dark" ? "light" : "dark";
  root.dataset.theme = next;
  try { localStorage.setItem("theme", next); } catch (_) {}
});

/* Logo scrolls back to the top --------------------------------------------- */

// The header is sticky, so linking to it doesn't scroll anywhere; scroll the
// page itself instead.
document.querySelector(".nav .brand").addEventListener("click", (event) => {
  event.preventDefault();
  window.scrollTo({ top: 0, behavior: reducedMotion ? "auto" : "smooth" });
  window.history.replaceState(null, "", location.pathname + location.search);
});

/* Navigation background once scrolled -------------------------------------- */

const nav = document.querySelector(".nav");
const onScroll = () => nav.classList.toggle("scrolled", window.scrollY > 10);
window.addEventListener("scroll", onScroll, { passive: true });
onScroll();

/* Scroll reveal, staggered within each group ------------------------------- */

const reveals = document.querySelectorAll(".reveal");
const groups = new Map();
reveals.forEach((el) => {
  const siblings = groups.get(el.parentElement) || [];
  el.style.setProperty("--delay", `${Math.min(siblings.length, 8) * 80}ms`);
  siblings.push(el);
  groups.set(el.parentElement, siblings);
});

const revealObserver = new IntersectionObserver((entries) => {
  for (const entry of entries) {
    if (!entry.isIntersecting) continue;
    entry.target.classList.add("in");
    revealObserver.unobserve(entry.target);
  }
}, { threshold: 0.15, rootMargin: "0px 0px -40px 0px" });
reveals.forEach((el) => revealObserver.observe(el));

// The game mode chart plays when it comes into view, and replays every few seconds.
const gameViz = document.querySelector(".game-viz");
let gameLoop;
new IntersectionObserver(([entry]) => {
  clearInterval(gameLoop);
  if (!entry.isIntersecting) return;
  setTimeout(() => gameViz.classList.add("play"), 500);
  if (reducedMotion) return;
  gameLoop = setInterval(() => {
    gameViz.classList.remove("play");
    setTimeout(() => gameViz.classList.add("play"), 900);
  }, 6000);
}, { threshold: 0.4 }).observe(gameViz);

/* Latest version from GitHub ----------------------------------------------- */

fetch(`https://api.github.com/repos/${REPO}/releases/latest`)
  .then((response) => (response.ok ? response.json() : Promise.reject()))
  .then((release) => {
    const tag = release.tag_name;
    latestVersion = tag.replace(/^v/, "");
    document.querySelectorAll(".version-label").forEach((el) => {
      el.textContent = el.closest(".title") ? tag : `Version ${tag.replace(/^v/, "")}`;
    });
  })
  .catch(() => {});

/* Scale the app preview down to fit narrow screens -------------------------- */

const APP_WIDTH = 380;
/** Larger than the real window on big screens, so it's the first thing you see. */
const MAX_SCALE = 1.22;
function fitDemo() {
  const hero = document.querySelector(".hero");
  const columns = getComputedStyle(hero).gridTemplateColumns.split(" ").map(parseFloat);
  // Two columns: fit the demo's column. One column: fit the screen.
  const room = columns.length > 1 ? columns[1] : document.documentElement.clientWidth - 32;
  const scale = Math.min(MAX_SCALE, room / APP_WIDTH);
  document.getElementById("app").style.zoom = scale === 1 ? "" : scale.toFixed(3);
}
fitDemo();
window.addEventListener("resize", fitDemo);

/* 3D tilt on the app preview ----------------------------------------------- */

const demo = document.getElementById("demo-wrap");
const appEl = document.getElementById("app");
if (!reducedMotion && window.matchMedia("(hover: hover)").matches) {
  demo.addEventListener("pointermove", (event) => {
    const box = demo.getBoundingClientRect();
    const x = (event.clientX - box.left) / box.width - 0.5;
    const y = (event.clientY - box.top) / box.height - 0.5;
    appEl.style.transform = `rotateY(${x * 8}deg) rotateX(${-y * 8}deg)`;
  });
  demo.addEventListener("pointerleave", () => { appEl.style.transform = ""; });
}

/* The interactive app ------------------------------------------------------- */

const GB = 1024;          // sizes below are in MB
const TOTAL = 16 * GB;
const SYSTEM = 4.1 * GB;  // Windows itself
const COMPRESSED = 420;

/** An app can't be trimmed below what it's actively using. */
const ACTIVE_SHARE = 0.55;
/** Trimmed memory comes back as the app runs: this share of the gap per second. */
const REGROW = 0.1;
/** Auto clear gaps, in seconds, like the app's "Every" setting. */
const INTERVALS = [5, 10, 15, 30, 60, 300];

const apps = [
  { name: "Chrome", natural: 1950, idle: null },
  { name: "Discord", natural: 640, idle: "idle 42m" },
  { name: "Steam", natural: 520, idle: "idle 17m" },
  { name: "Visual Studio Code", natural: 480, idle: null },
  { name: "Spotify", natural: 310, idle: "idle 65m" },
].map((app) => ({ ...app, size: app.natural, cap: null }));

/** Used-RAM ratio, one sample per second (not `history`, which is the browser's). */
const samples = [];
let status = null;
let statusTimer;
let gameMode = false;
let autoOn = true;
let threshold = 80;
let interval = 300;
let lastAuto = { at: 0, freed: null };
let openPanel = null;
/** App whose row shows the RAM limit slider, and the slider's value (tenths of a GB). */
let editing = null;
let draft = 15;
const settings = { start: true, alerts: true, admin: false };

const $ = (id) => document.getElementById(id);
const fmt = (mb) => (mb >= GB ? `${(mb / GB).toFixed(1)} GB` : `${Math.round(mb)} MB`);
/** "2 GB" or "2.5 GB". */
const gbShort = (mb) => `${+(mb / GB).toFixed(1)} GB`;
const every = (seconds) => (seconds < 60 ? `${seconds} sec` : `${seconds / 60} min`);
/** Bytes to tenths of a GB, rounded to the nearest half GB (at least 0.5). */
const halfSteps = (mb) => Math.max(1, Math.round((mb / GB) * 2)) * 5;

function memory() {
  const appsTotal = apps.reduce((sum, app) => sum + app.size, 0);
  const used = SYSTEM + COMPRESSED + appsTotal;
  const cached = (TOTAL - used) * 0.55;
  return { used, cached, free: TOTAL - used - cached, ratio: used / TOTAL };
}

function setStatus(message) {
  status = message;
  clearTimeout(statusTimer);
  statusTimer = setTimeout(() => { status = null; renderSummary(); }, 4000);
  renderSummary();
}

function renderSummary() {
  const m = memory();
  $("used-gb").textContent = fmt(m.used);
  $("avail-gb").textContent = fmt(TOTAL - m.used);
  const caption = $("caption");
  caption.textContent = status || `of 16 GB in use · ${Math.round(m.ratio * 100)}%`;
  caption.classList.toggle("status", Boolean(status));
  const pressure = $("pressure");
  pressure.textContent = m.ratio >= 0.85 ? "High" : m.ratio >= 0.7 ? "Busy" : "Healthy";
  pressure.classList.toggle("warn", m.ratio >= 0.85);

  const pct = (mb) => `${(mb / TOTAL) * 100}%`;
  $("seg-use").style.width = pct(m.used - COMPRESSED);
  $("seg-comp").style.width = pct(COMPRESSED);
  $("seg-cache").style.width = pct(m.cached);
  $("lg-use").textContent = fmt(m.used - COMPRESSED);
  $("lg-comp").textContent = fmt(COMPRESSED);
  $("lg-cache").textContent = fmt(m.cached);
  $("lg-free").textContent = fmt(m.free);

  $("ring-num").textContent = autoOn ? threshold : "off";
  const ring = $("ring-fill");
  ring.style.strokeDashoffset = `${81.68 * (1 - threshold / 100)}`;
  ring.style.stroke = autoOn ? "" : "var(--ink-faint)";
}

/* Panels: open, switch and close with a height animation. */

const panel = $("panel");
const inner = $("panel-inner");

function fitPanel() {
  panel.style.height = openPanel ? `${inner.offsetHeight}px` : "0px";
}

function togglePanel(name) {
  openPanel = openPanel === name ? null : name;
  editing = null;
  document.querySelectorAll("[data-panel]").forEach((button) => {
    const on = button.dataset.panel === openPanel;
    button.classList.toggle("on", on);
    button.setAttribute("aria-expanded", String(on));
  });
  if (openPanel) renderPanel();
  fitPanel();
}

function renderPanel() {
  if (openPanel === "apps") renderApps();
  if (openPanel === "history") renderHistory();
  if (openPanel === "auto") renderAuto();
  if (openPanel === "settings") renderSettings();
}

/* Top apps, with close and RAM limit buttons on every row. */

const ICON_CAP = `<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 5h14M12 20V9.5M8 13.5l4-4 4 4"/></svg>`;
const ICON_CLOSE = `<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M7 7l10 10M17 7 7 17"/></svg>`;

/** Slider end: twice what the app uses, at least 4 GB. */
const limitMax = (app) => Math.max(40, halfSteps(Math.max(app.natural, app.cap || 0) * 2));

function rowHTML(app) {
  if (editing === app.name) {
    return `
      <span class="avatar">${app.name[0]}</span>
      <div class="row-main">
        <div class="limit-head"><span>Max <b class="limit-val">${draft / 10} GB</b></span><small>lower is slower</small></div>
        <input class="limit-range" type="range" min="5" max="${limitMax(app)}" step="5" value="${draft}" aria-label="Most RAM ${app.name} can use">
      </div>
      <button class="pill" data-act="${app.cap ? "off" : "cancel"}" type="button">${app.cap ? "Off" : "Cancel"}</button>
      <button class="pill" data-act="save" type="button">Save</button>`;
  }
  return `
    <span class="avatar">${app.name[0]}</span>
    <div class="row-main">
      <div class="row-name"><span class="row-title">${app.name}</span><span class="chip"></span></div>
      <div class="row-bar"><i></i></div>
    </div>
    <span class="row-size"></span>
    <button class="nbtn xbtn cap-btn" data-act="cap" type="button" aria-label="Limit the RAM ${app.name} can use">${ICON_CAP}</button>
    <button class="nbtn xbtn" data-act="close" type="button" aria-label="Close ${app.name}">${ICON_CLOSE}</button>`;
}

function onRowClick(event) {
  const button = event.target.closest("button[data-act]");
  const row = event.target.closest(".row");
  if (!button || !row) return;
  const app = apps.find((a) => a.name === row.dataset.app);
  if (!app) return;
  switch (button.dataset.act) {
    case "cap":
      editing = app.name;
      draft = app.cap ? halfSteps(app.cap) : halfSteps(app.size * 0.75);
      break;
    case "cancel":
      editing = null;
      break;
    case "off":
      app.cap = null;
      editing = null;
      setStatus(`${app.name} has no limit now`);
      break;
    case "save":
      app.cap = (draft / 10) * GB;
      editing = null;
      enforceLimit(app);
      setStatus(`${app.name} now stays under ${gbShort(app.cap)}`);
      break;
    case "close":
      editing = null;
      closeApp(app.name, row);
      return;
  }
  renderApps();
  fitPanel();
}

function renderApps() {
  const largest = Math.max(...apps.map((app) => app.size), 1);
  if (!inner.querySelector(".rows")) {
    inner.innerHTML = `<div class="p-head">Top apps <span id="app-count"></span></div><div class="rows"></div>`;
    const rows = inner.querySelector(".rows");
    rows.addEventListener("click", onRowClick);
    rows.addEventListener("input", (event) => {
      if (!event.target.matches(".limit-range")) return;
      draft = Number(event.target.value);
      event.target.closest(".row").querySelector(".limit-val").textContent = `${draft / 10} GB`;
    });
  }
  const rows = inner.querySelector(".rows");
  const sorted = [...apps].sort((a, b) => b.size - a.size);
  for (const app of sorted) {
    let row = rows.querySelector(`[data-app="${app.name}"]`);
    if (!row) {
      row = document.createElement("div");
      row.className = "row";
      row.dataset.app = app.name;
      rows.appendChild(row);
    }
    const mode = editing === app.name ? "edit" : "normal";
    if (row.dataset.mode !== mode) {
      row.innerHTML = rowHTML(app);
      row.dataset.mode = mode;
    }
    if (mode === "edit") continue;
    row.querySelector(".row-bar i").style.width = `${(app.size / largest) * 100}%`;
    row.querySelector(".row-size").textContent = fmt(app.size);
    const chip = app.cap ? `≤ ${gbShort(app.cap)}` : app.idle || "";
    const chipEl = row.querySelector(".chip");
    chipEl.textContent = chip;
    chipEl.hidden = !chip;
    row.querySelector(".cap-btn").classList.toggle("on", Boolean(app.cap));
  }
  // Keep rows in size order, moving them only when the order changes (so a
  // click is never lost to a row being re-inserted). Not while a slider is
  // in use, so it doesn't jump away from the pointer.
  const current = [...rows.querySelectorAll(".row:not(.leaving)")].map((row) => row.dataset.app);
  if (!editing && current.join("|") !== sorted.map((app) => app.name).join("|")) {
    sorted.forEach((app) => {
      const row = rows.querySelector(`[data-app="${app.name}"]`);
      if (row && !row.classList.contains("leaving")) rows.appendChild(row);
    });
  }
  $("app-count").textContent = `${apps.length} running`;
}

/** Like the real limit, the app settles just under its cap. */
function enforceLimit(app) {
  if (app.cap && app.size > app.cap * 0.97) app.size = app.cap * 0.97;
}

function closeApp(name, row) {
  const index = apps.findIndex((app) => app.name === name);
  if (index < 0) return;
  const [app] = apps.splice(index, 1);
  row.classList.add("leaving");
  setStatus(`Closed ${name}: ${fmt(app.size)} freed`);
  setTimeout(() => {
    row.remove();
    renderApps();
    fitPanel();
  }, 360);
  // Bring it back after a while so the demo never runs out of apps.
  setTimeout(() => {
    apps.push({ ...app, size: app.natural * 0.6 });
    if (openPanel === "apps") { renderApps(); fitPanel(); }
  }, 12000);
}

function renderHistory() {
  if (!inner.querySelector(".spark")) {
    inner.innerHTML = `
      <div class="spark inset">
        <div class="spark-cap"><span>Last 60 seconds</span><span id="peak"></span></div>
        <svg viewBox="0 0 320 48" preserveAspectRatio="none" aria-hidden="true">
          <path class="area" id="spark-area"/><path class="line" id="spark-line"/>
        </svg>
      </div>`;
  }
  if (samples.length < 2) return;
  const lo = Math.min(...samples), hi = Math.max(...samples);
  const mid = (lo + hi) / 2, half = Math.max((hi - lo) / 2, 0.05);
  const step = 320 / 59, offset = 60 - samples.length;
  const points = samples.map((v, i) => {
    const x = (offset + i) * step;
    const y = 4 + (1 - (v - (mid - half)) / (2 * half)) * 40;
    return `${x.toFixed(1)} ${y.toFixed(1)}`;
  });
  const line = `M ${points.join(" L ")}`;
  $("spark-line").setAttribute("d", line);
  $("spark-area").setAttribute("d", `${line} L 320 48 L ${(offset * step).toFixed(1)} 48 Z`);
  $("peak").textContent = `peak ${Math.round(hi * 100)}%`;
}

function renderAuto() {
  if (!inner.querySelector(".range")) {
    const labels = INTERVALS.map((s) => (s < 60 ? `${s}s` : `${s / 60}m`));
    inner.innerHTML = `
      <div class="auto-row">
        <div>Auto clear<small id="auto-detail"></small></div>
        <button class="switch" id="auto-switch" type="button" role="switch" aria-label="Auto clear"></button>
      </div>
      <input class="range" id="auto-range" type="range" min="30" max="90" step="5" aria-label="Auto clear limit">
      <div class="range-marks"><span>30</span><span>40</span><span>50</span><span>60</span><span>70</span><span>80</span><span>90</span></div>
      <div class="auto-row every-row">
        <div>Every</div>
        <div class="seg-ctl" id="auto-every">${INTERVALS.map((s, i) => `<button type="button" data-every="${s}">${labels[i]}</button>`).join("")}</div>
      </div>
      <p class="auto-note" id="auto-note"></p>`;
    $("auto-switch").addEventListener("click", () => { autoOn = !autoOn; renderAuto(); renderSummary(); });
    $("auto-range").addEventListener("input", (event) => {
      threshold = Number(event.target.value);
      renderAuto();
      renderSummary();
    });
    $("auto-every").addEventListener("click", (event) => {
      const button = event.target.closest("[data-every]");
      if (!button) return;
      interval = Number(button.dataset.every);
      renderAuto();
    });
  }
  $("auto-switch").setAttribute("aria-checked", String(autoOn));
  $("auto-range").value = threshold;
  $("auto-detail").textContent = `Frees idle apps when RAM passes ${threshold}%`;
  document.querySelectorAll("#auto-every [data-every]").forEach((button) => {
    button.classList.toggle("on", Number(button.dataset.every) === interval);
  });
  const percent = Math.round(memory().ratio * 100);
  const short = interval < 30;
  let note;
  if (!autoOn) {
    note = "Off. Turn it on to free idle apps automatically.";
  } else if (percent >= threshold) {
    note = `RAM is at ${percent}%, so this runs every ${every(interval)}. ${short ? "A longer gap is easier on your PC." : "Raise the limit if apps feel slow."}`;
  } else {
    note = `Waiting. RAM is at ${percent}%.${short ? " Short gaps can make idle apps slow to wake." : ""}`;
  }
  $("auto-note").textContent = note;
}

/** Trims idle apps when RAM is over the limit, at most once per interval. */
function runAutoClear() {
  const percent = memory().ratio * 100;
  const now = Date.now();
  if (!autoOn || gameMode || percent < threshold || now - lastAuto.at < interval * 1000) return;
  let freed = 0;
  for (const app of apps.filter((a) => a.idle)) {
    const trimmed = Math.max(0, app.size - app.natural * ACTIVE_SHARE);
    app.size -= trimmed;
    freed += trimmed;
  }
  lastAuto = { at: now, freed };
  // Short intervals run constantly; only mention runs that did something.
  if (freed >= 25) setStatus(`Auto cleared ${fmt(freed)} at ${Math.round(percent)}%`);
}

/* Settings: the General tab. */

function settingRow(key, title, detail) {
  return `
    <div class="auto-row set-row">
      <div>${title}<small>${detail}</small></div>
      <button class="switch" type="button" role="switch" data-setting="${key}" aria-label="${title}"></button>
    </div>`;
}

function renderSettings() {
  if (!inner.querySelector(".set-list")) {
    inner.innerHTML = `
      <div class="set-list">
        ${settingRow("start", "Start with Windows", "Runs quietly in the tray")}
        ${settingRow("alerts", "Alerts", "When memory runs low or an app keeps growing")}
        ${settingRow("admin", "Admin mode", "Lets game mode clear the cache too")}
        <div class="auto-row set-row">
          <div>Theme</div>
          <div class="seg-ctl" id="theme-seg"><button type="button" data-pick="light">Light</button><button type="button" data-pick="dark">Dark</button></div>
        </div>
      </div>
      <p class="set-version" id="set-version"></p>`;
    inner.querySelector(".set-list").addEventListener("click", (event) => {
      const toggle = event.target.closest("[data-setting]");
      if (toggle) {
        const key = toggle.dataset.setting;
        settings[key] = !settings[key];
        const on = settings[key];
        setStatus({
          start: on ? "Starts with Windows" : "Won't start with Windows",
          alerts: on ? "Alerts on" : "Alerts off",
          admin: on ? "Admin mode on" : "Admin mode off",
        }[key]);
      }
      const pick = event.target.closest("[data-pick]");
      if (pick) {
        root.dataset.theme = pick.dataset.pick;
        try { localStorage.setItem("theme", pick.dataset.pick); } catch (_) {}
      }
      renderSettings();
    });
  }
  inner.querySelectorAll("[data-setting]").forEach((toggle) => {
    toggle.setAttribute("aria-checked", String(settings[toggle.dataset.setting]));
  });
  const theme = root.dataset.theme === "dark" ? "dark" : "light";
  inner.querySelectorAll("[data-pick]").forEach((button) => {
    button.classList.toggle("on", button.dataset.pick === theme);
  });
  $("set-version").textContent = `Version ${latestVersion} · updates automatically`;
}

/* Buttons. */

document.querySelectorAll("[data-panel]").forEach((button) => {
  button.addEventListener("click", () => togglePanel(button.dataset.panel));
});

$("btn-optimize").addEventListener("click", () => {
  let freed = 0;
  for (const app of apps) {
    const trimmed = Math.max(0, app.size - Math.max(app.size * 0.65, app.natural * ACTIVE_SHARE));
    app.size -= trimmed;
    freed += trimmed;
  }
  // Like the real app: pressing it again right away frees next to nothing.
  setStatus(`Freed ${fmt(freed)} from ${apps.length} background apps`);
  renderPanel();
});

$("btn-game").addEventListener("click", (event) => {
  gameMode = !gameMode;
  event.currentTarget.classList.toggle("on", gameMode);
  event.currentTarget.setAttribute("aria-pressed", String(gameMode));
  if (gameMode) {
    let freed = 0;
    for (const app of apps.filter((a) => a.idle)) {
      const trimmed = Math.max(0, app.size - app.natural * ACTIVE_SHARE);
      app.size -= trimmed;
      freed += trimmed;
    }
    const cache = settings.admin ? " + cache" : "";
    setStatus(`Game mode: ${apps.length} lowered, ${fmt(freed)}${cache} freed`);
  } else {
    setStatus(`Game mode off: ${apps.length} apps back to normal`);
  }
  renderPanel();
});

/* "Try it" hints: a hand taps one button after another, and the buttons
   glow, until the visitor touches the demo for the first time. */

const hand = $("tap-hand");
const hintTargets = [
  "[data-panel='apps']", "#btn-optimize", "[data-panel='auto']", "#btn-game", "[data-panel='settings']",
].map((selector) => appEl.querySelector(selector));
let hintIndex = 0;
let hintTimer = null;

function moveHand() {
  const target = hintTargets[hintIndex % hintTargets.length];
  hintIndex += 1;
  hintTargets.forEach((el) => el.classList.toggle("hint", el === target));
  // Offsets inside the app, so the demo's zoom doesn't matter.
  let x = target.offsetWidth / 2, y = target.offsetHeight / 2;
  for (let el = target; el && el !== appEl; el = el.offsetParent) {
    x += el.offsetLeft;
    y += el.offsetTop;
  }
  hand.style.left = `${x}px`;
  hand.style.top = `${y}px`;
  hand.classList.remove("tap");
  void hand.offsetWidth; // restart the tap animation
  hand.classList.add("tap");
}

function stopHints() {
  clearInterval(hintTimer);
  hintTimer = null;
  demo.classList.add("touched");
  hintTargets.forEach((el) => el.classList.remove("hint"));
}

if (!reducedMotion) {
  demo.classList.add("hinting");
  new IntersectionObserver(([entry], observer) => {
    if (!entry.isIntersecting || demo.classList.contains("touched")) return;
    observer.disconnect();
    setTimeout(() => {
      if (demo.classList.contains("touched")) return;
      moveHand();
      hintTimer = setInterval(moveHand, 2200);
    }, 1200);
  }, { threshold: 0.5 }).observe(demo);
}
appEl.addEventListener("pointerdown", stopHints, { once: true });
appEl.addEventListener("keydown", stopHints, { once: true });

/* Live updates: apps drift and slowly page back in after a trim. */

function tick() {
  for (const app of apps) {
    app.size += (app.natural - app.size) * REGROW;
    app.size *= 1 + (Math.random() - 0.5) * 0.01;
    enforceLimit(app);
  }
  runAutoClear();
  samples.push(memory().ratio);
  if (samples.length > 60) samples.shift();
  renderSummary();
  if (openPanel) {
    renderPanel();
    fitPanel();
  }
}

for (let i = 0; i < 20; i++) samples.push(memory().ratio + (Math.random() - 0.5) * 0.01);
renderSummary();
setInterval(tick, 1000);
window.addEventListener("resize", fitPanel);
