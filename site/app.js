// RAM Optimizer website: scroll reveals, theme, live version, and a working
// copy of the app window in the hero.

const REPO = "monyratanaksy-byte/ram-optimizer";
const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/* Theme -------------------------------------------------------------------- */

const root = document.documentElement;
document.getElementById("theme-toggle").addEventListener("click", () => {
  const systemDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  const current = root.dataset.theme || (systemDark ? "dark" : "light");
  const next = current === "dark" ? "light" : "dark";
  root.dataset.theme = next;
  try { localStorage.setItem("theme", next); } catch (_) {}
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
    document.querySelectorAll(".version-label").forEach((el) => {
      el.textContent = el.closest(".title") ? tag : `Version ${tag.replace(/^v/, "")}`;
    });
  })
  .catch(() => {});

/* Scale the app preview down to fit narrow screens -------------------------- */

const APP_WIDTH = 380;
function fitDemo() {
  const available = document.documentElement.clientWidth - 32;
  const scale = Math.min(1, available / APP_WIDTH);
  document.getElementById("app").style.zoom = scale < 1 ? scale.toFixed(3) : "";
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

const apps = [
  { name: "Chrome", natural: 1950, idle: null },
  { name: "Discord", natural: 640, idle: "idle 42m" },
  { name: "Steam", natural: 520, idle: "idle 17m" },
  { name: "Visual Studio Code", natural: 480, idle: null },
  { name: "Spotify", natural: 310, idle: "idle 65m" },
].map((app) => ({ ...app, size: app.natural }));

const history = [];
let status = null;
let statusTimer;
let gameMode = false;
let autoOn = true;
let threshold = 80;
let openPanel = null;

const $ = (id) => document.getElementById(id);
const fmt = (mb) => (mb >= GB ? `${(mb / GB).toFixed(1)} GB` : `${Math.round(mb)} MB`);

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
}

function renderApps() {
  const largest = Math.max(...apps.map((app) => app.size), 1);
  if (!inner.querySelector(".rows")) {
    inner.innerHTML = `<div class="p-head">Top apps <span id="app-count"></span></div><div class="rows"></div>`;
  }
  const rows = inner.querySelector(".rows");
  const sorted = [...apps].sort((a, b) => b.size - a.size);
  for (const app of sorted) {
    let row = rows.querySelector(`[data-app="${app.name}"]`);
    if (!row) {
      row = document.createElement("div");
      row.className = "row";
      row.dataset.app = app.name;
      row.innerHTML = `
        <span class="avatar">${app.name[0]}</span>
        <div class="row-main">
          <div class="row-name">${app.name}${app.idle ? `<span class="chip">${app.idle}</span>` : ""}</div>
          <div class="row-bar"><i></i></div>
        </div>
        <span class="row-size"></span>
        <button class="nbtn xbtn" type="button" aria-label="Close ${app.name}">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M7 7l10 10M17 7 7 17"/></svg>
        </button>`;
      row.querySelector("button").addEventListener("click", () => closeApp(app.name, row));
      rows.appendChild(row);
    }
    row.querySelector(".row-bar i").style.width = `${(app.size / largest) * 100}%`;
    row.querySelector(".row-size").textContent = fmt(app.size);
  }
  // Keep rows in size order, moving them only when the order changes (so a
  // click is never lost to a row being re-inserted).
  const current = [...rows.querySelectorAll(".row:not(.leaving)")].map((row) => row.dataset.app);
  if (current.join("|") !== sorted.map((app) => app.name).join("|")) {
    sorted.forEach((app) => {
      const row = rows.querySelector(`[data-app="${app.name}"]`);
      if (row && !row.classList.contains("leaving")) rows.appendChild(row);
    });
  }
  $("app-count").textContent = `${apps.length} running`;
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
  if (history.length < 2) return;
  const lo = Math.min(...history), hi = Math.max(...history);
  const mid = (lo + hi) / 2, half = Math.max((hi - lo) / 2, 0.05);
  const step = 320 / 59, offset = 60 - history.length;
  const points = history.map((v, i) => {
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
    inner.innerHTML = `
      <div class="auto-row">
        <div>Auto-clear<small id="auto-detail"></small></div>
        <button class="switch" id="auto-switch" type="button" role="switch" aria-label="Auto-clear"></button>
      </div>
      <input class="range" id="auto-range" type="range" min="30" max="90" step="5" aria-label="Auto-clear limit">
      <div class="range-marks"><span>30</span><span>40</span><span>50</span><span>60</span><span>70</span><span>80</span><span>90</span></div>
      <p class="auto-note" id="auto-note"></p>`;
    $("auto-switch").addEventListener("click", () => { autoOn = !autoOn; renderAuto(); renderSummary(); });
    $("auto-range").addEventListener("input", (event) => {
      threshold = Number(event.target.value);
      renderAuto();
      renderSummary();
    });
  }
  $("auto-switch").setAttribute("aria-checked", String(autoOn));
  $("auto-range").value = threshold;
  $("auto-detail").textContent = `Frees idle apps when RAM passes ${threshold}%`;
  const percent = Math.round(memory().ratio * 100);
  $("auto-note").textContent = !autoOn
    ? "Off. Turn it on to free idle apps automatically."
    : percent >= threshold
      ? `RAM is already at ${percent}%, so this runs every 5 min.`
      : `Waiting. RAM is at ${percent}%.`;
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
    setStatus(`Game mode: ${apps.length} lowered, ${fmt(freed)} + cache freed`);
  } else {
    setStatus(`Game mode off: ${apps.length} apps back to normal`);
  }
  renderPanel();
});

/* Live updates: apps drift and slowly page back in after a trim. */

function tick() {
  for (const app of apps) {
    app.size += (app.natural - app.size) * REGROW;
    app.size *= 1 + (Math.random() - 0.5) * 0.01;
  }
  history.push(memory().ratio);
  if (history.length > 60) history.shift();
  renderSummary();
  if (openPanel) {
    renderPanel();
    fitPanel();
  }
}

for (let i = 0; i < 20; i++) history.push(memory().ratio + (Math.random() - 0.5) * 0.01);
renderSummary();
setInterval(tick, 1000);
window.addEventListener("resize", fitPanel);
