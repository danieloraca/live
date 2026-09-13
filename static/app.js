"use strict";

const $ = (id) => document.getElementById(id);
const text = (id, value) => { $(id).textContent = value; };
const known = (value) => typeof value === "number" && Number.isFinite(value);
const percent = (value) => known(value) ? value.toFixed(1) + "%" : "—";

function bytes(value, decimals = 1) {
  if (!known(value)) return "—";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let index = 0;
  while (value >= 1024 && index < units.length - 1) { value /= 1024; index++; }
  return value.toFixed(index === 0 ? 0 : decimals).replace(/\.0$/, "") + " " + units[index];
}

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
  text("network-chart-value", known(m.rx) ? "↓ " + bytes(m.rx) + "/s  ↑ " + bytes(m.tx) + "/s" : "—");
  text("network-interface", m.interfaces.length ? m.interfaces.join(" + ") : "No physical interface");
  text("updated", "Sampled " + new Date(m.timestamp * 1000).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" }));
  text("live-text", stale ? "delayed" : "live");
  $("live-indicator").classList.toggle("warning", stale);
  health(data, stale);
  const notice = $("notice");
  notice.hidden = true;
  if (stale) {
    notice.textContent = "The server is reachable, but its readings are delayed. Showing the last collected sample.";
    notice.hidden = false;
  } else if (m.uptime === null && m.memory === null) {
    notice.textContent = "This preview is running on " + system.os + ". CPU, memory, uptime, and Pi sensors will appear when this app runs on the Raspberry Pi.";
    notice.hidden = false;
  }
  renderServices(data.services);
  renderCharts();
}

function drawChart(id, keys, ceiling, end) {
  const svg = $(id);
  const start = end - minutes * 60;
  const points = history.filter((p) => p.timestamp >= start && p.timestamp <= end);
  svg.replaceChildren();
  const element = (tag, attrs) => {
    const e = document.createElementNS("http://www.w3.org/2000/svg", tag);
    Object.entries(attrs).forEach(([key, value]) => e.setAttribute(key, value));
    svg.append(e);
    return e;
  };
  [20, 60, 100].forEach((y) => element("line", { x1: 0, y1: y, x2: 500, y2: y, class: "chart-grid" }));
  let count = 0;
  keys.forEach((key) => {
    let segment = [];
    const flush = () => {
      if (!segment.length) return;
      if (segment.length === 1) {
        const [x, y] = segment[0];
        element("circle", { cx: x, cy: y, r: 1.8, fill: key === "tx" ? "#229953" : "#2864e8" });
      } else {
        const path = segment.map(([x, y], i) => (i ? "L" : "M") + x.toFixed(2) + "," + y.toFixed(2)).join(" ");
        if (key === "cpu") element("path", { d: path + " L" + segment.at(-1)[0].toFixed(2) + ",100 L" + segment[0][0].toFixed(2) + ",100 Z", class: "chart-area" });
        element("path", { d: path, class: "chart-line " + key });
      }
      segment = [];
    };
    let previousTime = null;
    points.forEach((p) => {
      if (!known(p[key]) || (previousTime !== null && p.timestamp - previousTime > 15)) flush();
      if (known(p[key])) {
        count++;
        segment.push([(p.timestamp - start) / (minutes * 60) * 500, 100 - Math.min(p[key] / ceiling, 1) * 90]);
      }
      previousTime = p.timestamp;
    });
    flush();
  });
  return count;
}

function renderCharts() {
  if (!latest) return;
  const end = latest.metrics.timestamp;
  const recent = history.filter((p) => p.timestamp >= end - minutes * 60);
  const maxNetwork = Math.max(1024, ...recent.flatMap((p) => [known(p.rx) ? p.rx : 0, known(p.tx) ? p.tx : 0])) * 1.15;
  $("cpu-empty").hidden = drawChart("cpu-chart", ["cpu"], 100, end) > 0;
  $("network-empty").hidden = drawChart("network-chart", ["rx", "tx"], maxNetwork, end) > 0;
  text("cpu-empty", latest.metrics.cores ? "Collecting the first CPU reading…" : "CPU history unavailable on this host");
  text("network-empty", latest.metrics.interfaces.length ? "Collecting the first network reading…" : "Network history unavailable on this host");
  document.querySelectorAll(".range-label").forEach((e) => { e.textContent = minutes + " minutes ago"; });
  const duration = history.length > 1 ? Math.max(0, end - history[0].timestamp) : 0;
  text("history-caption", duration < minutes * 60 - 10 ? "Building history · " + (duration < 60 ? "less than a minute" : Math.floor(duration / 60) + " min collected") : "Recent activity · last " + minutes + " minutes");
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
    if (needsHistory) {
      try {
        const collected = await request("/api/history");
        if (Array.isArray(collected)) {
          history = collected.filter((p) => p && known(p.timestamp));
          needsHistory = false;
        }
      } catch { /* Current readings still work; retry the history on the next poll. */ }
    }
    const m = data.metrics;
    const point = { timestamp: m.timestamp, cpu: m.cpu, rx: m.rx, tx: m.tx };
    history = history.filter((p) => p.timestamp !== point.timestamp && p.timestamp >= point.timestamp - 3600 && p.timestamp <= point.timestamp);
    history.push(point);
    history.sort((a, b) => a.timestamp - b.timestamp);
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
  }
}

document.querySelectorAll("[data-minutes]").forEach((button) => {
  button.addEventListener("click", () => {
    minutes = Number(button.dataset.minutes);
    document.querySelectorAll("[data-minutes]").forEach((b) => b.setAttribute("aria-pressed", String(b === button)));
    renderCharts();
  });
});
$("refresh").addEventListener("click", refresh);
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
