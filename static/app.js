import { HistoryChart, known, percent, bytes, rate, summarize } from "./charts.mjs";

const $ = (id) => document.getElementById(id);
const text = (id, value) => { $(id).textContent = value; };

function uptime(seconds) {
  if (!known(seconds)) return "—";
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor(seconds / 3600) % 24;
  const minutes = Math.floor(seconds / 60) % 60;
  return days ? days + "d " + hours + "h" : hours ? hours + "h " + minutes + "m" : minutes + "m";
}

let history = [];
let minutes = 60;
let latest = null;
let refreshing = false;
let needsHistory = true;
let historyResolution = 5;
let historySummary = null;
const cpuChart = new HistoryChart("cpu", ["cpu"], "CPU usage");
const networkChart = new HistoryChart("network", ["rx", "tx"], "network activity");
let historyMinutes = null;
let historyFailed = false;
let lastHistoryLoad = 0;
let rangeChanged = false;
let connectionFailed = false;
let lastServiceSignature = "";
let lastSuccess = 0;

function setHealth(label, level = "") {
  text("health-text", label);
  $("health").className = "health-pill" + (level ? " " + level : "");
}

function health(data, stale) {
  if (stale) { setHealth("Live data is delayed", "warning"); return; }
  const services = data.services;
  const active = services.filter((s) => s.state === "active").length;
  if (services.some((s) => s.state === "failed")) setHealth("A service needs attention", "warning");
  else if (!services.length || services.every((s) => s.state === "unknown")) setHealth("Service status unavailable", "warning");
  else if (active !== services.length) setHealth("Some services aren’t running", "warning");
  else if (known(data.metrics.throttled) && (data.metrics.throttled & 15)) setHealth("Hardware needs attention", "warning");
  else setHealth("All services running");
}

function storage(prefix, data) {
  const p = data && known(data.percent) ? data.percent : null;
  text(prefix + "-percent", known(p) ? p.toFixed(1) + "% used" : "Unavailable");
  $(prefix + "-bar").value = known(p) ? Math.max(0, Math.min(p, 100)) : 0;
  $(prefix + "-bar").setAttribute("aria-valuetext", known(p) ? p.toFixed(1) + "% used" : "Unavailable");
  $(prefix + "-bar").classList.toggle("warning", known(p) && p >= 90);
  text(prefix + "-free", data ? bytes(data.available) + " available of " + bytes(data.total) : "No reading available");
}

function renderServices(services) {
  const signature = JSON.stringify(services);
  if (signature === lastServiceSignature) return;
  lastServiceSignature = signature;
  const initials = ["IP", "ID", "TE", "SO", "TR", "SN", "JP", "PI"];
  const rows = services.map((service, index) => {
    const link = document.createElement("a");
    link.className = "service-row";
    const url = new URL("/", window.location.href);
    url.port = String(service.port);
    link.href = url.href;
    link.target = "_blank";
    link.rel = "noopener noreferrer";
    link.setAttribute("aria-label", service.name + ", " + service.state + ", port " + service.port + ", opens in a new tab");
    const label = document.createElement("span");
    label.className = "service-label";
    const icon = document.createElement("span");
    icon.className = "app-icon";
    icon.textContent = initials[index] || "AP";
    icon.setAttribute("aria-hidden", "true");
    const name = document.createElement("span");
    const strong = document.createElement("strong");
    strong.textContent = service.name;
    const small = document.createElement("small");
    small.textContent = service.unit;
    name.append(strong, small);
    label.append(icon, name);
    const port = document.createElement("span");
    port.className = "service-port";
    port.textContent = service.port;
    const state = document.createElement("span");
    const stateClass = ["active", "failed", "inactive", "activating", "deactivating"].includes(service.state) ? service.state : "unknown";
    state.className = "service-state " + stateClass;
    const dot = document.createElement("span");
    dot.className = "dot";
    state.append(dot, document.createTextNode(service.state.charAt(0).toUpperCase() + service.state.slice(1)));
    const arrow = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    arrow.setAttribute("class", "icon service-open");
    arrow.setAttribute("aria-hidden", "true");
    const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
    use.setAttribute("href", "#i-arrow");
    arrow.append(use);
    link.append(label, port, state, arrow);
    return link;
  });
  $("service-list").replaceChildren(...rows);
}

function render(data) {
  const m = data.metrics;
  const stale = Date.now() / 1000 - m.timestamp > 20;
  const active = data.services.filter((s) => s.state === "active").length;
  const unknown = data.services.every((s) => s.state === "unknown");
  const attention = data.services.length - active;
  text("active-count", unknown ? "—" : active);
  text("total-count", " / " + data.services.length);
  text("service-caption", unknown ? "Status unavailable" : active === data.services.length ? "All at your service" : attention + (attention === 1 ? " needs attention" : " need attention"));
  const system = data.system;
  text("machine-detail", [system.model, system.os, system.kernel, system.architecture].filter(Boolean).join(" · "));
  text("uptime", uptime(m.uptime));
  text("uptime-detail", known(m.uptime) ? "Booted " + new Date((m.timestamp - m.uptime) * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short" }) : "Available on Linux");
  text("cpu", percent(m.cpu));
  text("cpu-chart-value", percent(m.cpu));
  text("cpu-context", m.cores ? "Across " + m.cores + " cores" : "Total CPU capacity");
  text("cpu-detail", m.cores ? m.cores + " cores" + (m.cpu === null ? " · first reading pending" : "") : "Available on Linux");
  text("memory", m.memory ? bytes(m.memory.used) : "—");
  text("memory-detail", m.memory ? "of " + bytes(m.memory.total) : "Available on Linux");
  text("temperature", known(m.temperature) ? m.temperature.toFixed(1) + "°C" : "—");
  let temperatureDetail = "Sensor unavailable";
  if (known(m.temperature)) {
    if (known(m.throttled)) {
      const flags = m.throttled;
      temperatureDetail = flags & 1 ? "Undervoltage detected" : flags & 4 ? "Currently throttled" : flags & 8 ? "Temperature limit active" : flags & 2 ? "CPU frequency capped" : flags & 0xf0000 ? "No current alerts · earlier warning" : "No throttling or undervoltage";
    } else temperatureDetail = "SoC temperature";
  }
  text("temperature-detail", temperatureDetail);
  $("temperature-detail").classList.toggle("warning", known(m.throttled) && (m.throttled & 15) !== 0);
  storage("memory", m.memory);
  storage("disk", m.disk);
  text("load", m.load ? m.load.map((n) => n.toFixed(2)).join(" / ") : "—");
  text("frequency", known(m.frequency) ? (m.frequency >= 1000 ? (m.frequency / 1000).toFixed(2).replace(/0$/, "") + " GHz" : Math.round(m.frequency) + " MHz") : "—");
  text("transferred", known(m.received) ? "↓ " + bytes(m.received, 0) + "  ↑ " + bytes(m.sent, 0) : "—");
  text("swap", m.memory ? bytes(m.memory.swap_used, 0) : "—");
  text("swap-detail", m.memory ? (m.memory.swap_total ? "of " + bytes(m.memory.swap_total) : "No swap configured") : "Available on Linux");
  text("processes", known(m.processes) ? m.processes.toLocaleString() : "—");
  text("rx-now", rate(m.rx));
  text("tx-now", rate(m.tx));
  text("network-interface", m.interfaces.length ? m.interfaces.join(" + ") : "No physical interface");
  text("updated", "Sampled " + new Date(m.timestamp * 1000).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" }));
  text("live-text", stale ? "delayed" : "live");
  $("live-indicator").classList.toggle("warning", stale);
  health(data, stale);
  const saved = data.history;
  text("history-storage", saved?.state === "error" ? "History isn’t being saved" : known(saved?.persisted_through) ? "History saved on disk · kept forever" : "Saving the first history sample…");
  $("history-storage").classList.toggle("warning", saved?.state === "error");
  const notice = $("notice");
  const notices = [];
  if (saved?.state === "error") notices.push("History could not be saved to disk. Live readings still refresh; saving will retry automatically.");
  if (saved?.dropped_samples > 0) notices.push("Some unsaved samples were lost while storage was unavailable.");
  if (historyFailed) notices.push("Saved history could not be loaded. Retrying automatically.");
  if (stale) {
    notices.push("The server is reachable, but its readings are delayed. Showing the last collected sample.");
  } else if (m.uptime === null && m.memory === null) {
    notices.push("This preview is running on " + system.os + ". CPU, memory, uptime, and Pi sensors will appear when this app runs on the Raspberry Pi.");
  }
  notice.textContent = notices.join(" ");
  notice.hidden = notices.length === 0;
  renderServices(data.services);
  renderCharts();
}

function renderCharts() {
  if (!latest) return;
  const label = minutes < 60 ? minutes + " minutes" : minutes === 60 ? "1 hour" : minutes === 1440 ? "24 hours" : minutes / 1440 + " days";
  const end = latest.metrics.timestamp;
  const start = end - minutes * 60;
  const recent = history.filter((p) => p.timestamp >= start && p.timestamp <= end);
  const loading = historyMinutes !== minutes;
  cpuChart.render(recent, start, end, historyResolution, loading);
  networkChart.render(recent, start, end, historyResolution, loading);
  for (const key of ["cpu", "rx", "tx"]) {
    const summary = loading ? null : historyResolution === 5 ? summarize(recent, key) : historySummary?.[key];
    const format = key === "cpu" ? percent : rate;
    text(key + "-average", format(summary?.average));
    text(key + "-peak", format(summary?.peak));
  }
  const detail = historyResolution === 5 ? "5-second samples" : historyResolution / 60 + "-minute averages";
  text("history-caption", historyFailed ? "Saved history unavailable · retrying" : loading ? "Loading saved history…" : "Last " + label + " · " + detail);
}

async function request(path) {
  const response = await fetch(path, { cache: "no-store", signal: AbortSignal.timeout(7000) });
  if (!response.ok) throw new Error("HTTP " + response.status);
  return response.json();
}

async function refresh() {
  if (refreshing) return;
  refreshing = true;
  $("refresh").disabled = true;
  try {
    const data = await request("/api/status");
    if (!data.metrics || !Array.isArray(data.services) || !known(data.metrics.timestamp)) throw new Error("Invalid status");
    const requestedMinutes = minutes;
    if (needsHistory || Date.now() - lastHistoryLoad >= 30000) {
      try {
        const collected = await request("/api/history?minutes=" + requestedMinutes);
        if (!Array.isArray(collected.points) || !known(collected.resolution_seconds)) throw new Error("Invalid history");
        if (requestedMinutes === minutes) {
          history = collected.points.filter((p) => p && known(p.timestamp));
          historyResolution = collected.resolution_seconds;
          historySummary = collected.summary ?? null;
          historyMinutes = requestedMinutes;
          historyFailed = false;
          lastHistoryLoad = Date.now();
          needsHistory = false;
        }
      } catch {
        if (requestedMinutes === minutes) { historyFailed = true; needsHistory = true; }
      }
    }
    const m = data.metrics;
    if (historyMinutes === minutes && historyResolution === 5) {
      const point = { timestamp: m.timestamp, cpu: m.cpu, rx: m.rx, tx: m.tx };
      history = history.filter((p) => p.timestamp !== point.timestamp && p.timestamp >= point.timestamp - minutes * 60);
      history.push(point);
      history.sort((a, b) => a.timestamp - b.timestamp);
    }
    latest = data;
    connectionFailed = false;
    lastSuccess = Date.now();
    render(data);
  } catch {
    connectionFailed = true;
    needsHistory = true;
    setHealth("Connection lost", "offline");
    text("live-text", "offline");
    $("live-indicator").classList.add("warning");
    $("notice").hidden = false;
    $("notice").textContent = latest ? "Can’t reach the Pi. These are the last known readings; retrying automatically every 5 seconds." : "Waiting for the Pi’s first reading. Retrying automatically every 5 seconds.";
    if (!latest) text("machine-detail", "Waiting for the server");
  } finally {
    refreshing = false;
    $("refresh").disabled = false;
    if (rangeChanged) { rangeChanged = false; refresh(); }
  }
}

document.querySelectorAll("[data-minutes]").forEach((button) => {
  button.addEventListener("click", () => {
    if (minutes === Number(button.dataset.minutes)) return;
    minutes = Number(button.dataset.minutes);
    history = [];
    historyMinutes = null;
    historySummary = null;
    cpuChart.reset();
    networkChart.reset();
    historyFailed = false;
    needsHistory = true;
    document.querySelectorAll("[data-minutes]").forEach((b) => b.setAttribute("aria-pressed", String(b === button)));
    renderCharts();
    if (refreshing) rangeChanged = true;
    else refresh();
  });
});
$("refresh").addEventListener("click", () => { needsHistory = true; refresh(); });
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) { needsHistory = true; refresh(); }
});
setInterval(() => {
  if (!document.hidden) refresh();
  if (latest && !connectionFailed && Date.now() - lastSuccess > 20000) {
    text("live-text", "delayed");
    $("live-indicator").classList.add("warning");
    setHealth("Live data is delayed", "warning");
  }
}, 5000);
refresh();
