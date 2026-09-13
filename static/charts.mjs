export const known = (value) => typeof value === "number" && Number.isFinite(value);
export const percent = (value) => known(value) ? value.toFixed(1) + "%" : "—";
const units = ["B", "KiB", "MiB", "GiB", "TiB"];

export function bytes(value, decimals = 1) {
  if (!known(value)) return "—";
  let index = 0;
  while (value >= 1024 && index < units.length - 1) { value /= 1024; index++; }
  return value.toFixed(index === 0 ? 0 : decimals).replace(/\.0$/, "") + " " + units[index];
}

export const rate = (value) => known(value) ? bytes(value) + "/s" : "—";

export function summarize(points, key) {
  const values = points.map((p) => p[key]).filter(known);
  return { average: values.length ? values.reduce((a, b) => a + b, 0) / values.length : null,
    peak: values.length ? Math.max(...values) : null, samples: values.length };
}

export function networkScale(points) {
  const peak = Math.max(0, ...points.flatMap((p) => [p.rx, p.tx]).filter(known));
  return Math.max(1024, 2 ** Math.ceil(Math.log2(peak * 1.05 || 1)));
}

export function networkTick(value, ceiling) {
  const unit = Math.min(units.length - 1, Math.floor(Math.log(ceiling) / Math.log(1024)));
  const amount = value / 1024 ** unit;
  return Number(amount.toFixed(2)) + " " + units[unit] + "/s";
}

export function segments(points, key, resolution) {
  const result = [];
  let current = [];
  let previous = null;
  for (const p of points) {
    if (!known(p[key]) || (previous !== null && p.timestamp - previous > Math.max(15, resolution * 1.5))) {
      if (current.length) result.push(current);
      current = [];
    }
    if (known(p[key])) current.push(p);
    previous = p.timestamp;
  }
  if (current.length) result.push(current);
  return result;
}

export function missingSpans(points, keys, start, end, resolution) {
  const present = points.filter((p) => keys.some((key) => known(p[key])));
  if (!present.length) return [{ start, end, label: "No data yet" }];
  const gap = Math.max(15, resolution * 1.5);
  const spans = [];
  if (present[0].timestamp - start > gap) spans.push({ start, end: present[0].timestamp, label: "No data yet" });
  for (let i = 1; i < present.length; i++) {
    if (present[i].timestamp - present[i - 1].timestamp > gap) {
      spans.push({ start: present[i - 1].timestamp + resolution / 2, end: present[i].timestamp - resolution / 2, label: "No data" });
    }
  }
  if (end - present.at(-1).timestamp > gap) spans.push({ start: present.at(-1).timestamp + resolution, end, label: "No data" });
  return spans;
}

export function nearestPoint(points, timestamp, resolution) {
  let nearest = null;
  for (const p of points) {
    if (!nearest || Math.abs(p.timestamp - timestamp) < Math.abs(nearest.timestamp - timestamp)) nearest = p;
  }
  // Never snap a cursor across an outage or into history that has not been recorded.
  return nearest && Math.abs(nearest.timestamp - timestamp) <= Math.max(7.5, resolution * 0.75) ? nearest : null;
}

const clock = (timestamp, seconds = false) => new Date(timestamp * 1000).toLocaleTimeString(undefined, {
  hour: "2-digit", minute: "2-digit", ...(seconds ? { second: "2-digit" } : {}), hour12: false,
});
const date = (timestamp) => new Date(timestamp * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short" });
const svgElement = (tag, attrs) => {
  const element = document.createElementNS("http://www.w3.org/2000/svg", tag);
  Object.entries(attrs).forEach(([key, value]) => element.setAttribute(key, value));
  return element;
};

export class HistoryChart {
  constructor(id, keys, name) {
    this.id = id;
    this.keys = keys;
    this.name = name;
    this.points = [];
    this.selection = null;
    this.pinned = false;
    const host = document.getElementById(id + "-view");
    // The markup and identifiers here are fixed application strings.
    host.innerHTML = `<div class="chart-layout">
      <div class="chart-y-axis" aria-hidden="true"></div>
      <div class="chart-plot" role="slider" tabindex="0" aria-valuemin="0" aria-valuemax="1" aria-valuenow="0" aria-disabled="true" aria-label="Inspect ${name} history" aria-describedby="${id}-help">
        <svg class="chart" viewBox="0 0 500 160" preserveAspectRatio="none" aria-hidden="true"></svg>
        <div class="chart-missing-labels" aria-hidden="true"></div>
        <svg class="chart-cursor" viewBox="0 0 500 160" preserveAspectRatio="none" aria-hidden="true"></svg>
      </div>
      <div class="chart-time-axis" aria-hidden="true"></div>
    </div><span class="sr-only" id="${id}-help">Hover or tap for a reading. Use Left and Right arrow keys, Home, or End to explore. Escape clears the selection.</span>
    <output class="chart-readout" aria-live="polite" aria-atomic="true"></output>`;
    this.plot = host.querySelector(".chart-plot");
    this.svg = host.querySelector(".chart");
    this.cursor = host.querySelector(".chart-cursor");
    this.labels = host.querySelector(".chart-missing-labels");
    this.yAxis = host.querySelector(".chart-y-axis");
    this.timeAxis = host.querySelector(".chart-time-axis");
    this.readout = host.querySelector(".chart-readout");
    const pointAt = (event) => {
      const bounds = this.plot.getBoundingClientRect();
      return this.start + Math.max(0, Math.min(1, (event.clientX - bounds.left) / bounds.width)) * (this.end - this.start);
    };
    this.plot.addEventListener("pointermove", (event) => {
      if (this.x && event.pointerType === "mouse") { this.selection = pointAt(event); this.inspect(); }
    });
    this.plot.addEventListener("pointerleave", () => {
      if (!this.pinned && document.activeElement !== this.plot) { this.selection = null; this.inspect(); }
    });
    this.plot.addEventListener("click", (event) => {
      if (!this.x) return;
      this.plot.focus({ preventScroll: true });
      this.pinned = true;
      this.selection = pointAt(event);
      this.inspect();
    });
    this.plot.addEventListener("focus", () => {
      if (!this.x) return;
      if (this.selection === null) this.selection = this.points.at(-1)?.timestamp ?? this.end;
      this.inspect();
    });
    this.plot.addEventListener("blur", () => {
      this.pinned = false;
      this.selection = null;
      this.inspect();
    });
    this.plot.addEventListener("keydown", (event) => {
      if (!this.x) return;
      if (!["ArrowLeft", "ArrowRight", "Home", "End", "Escape"].includes(event.key)) return;
      event.preventDefault();
      if (event.key === "Escape") { this.selection = null; this.pinned = false; }
      else if (this.points.length) {
        const selected = this.selection ?? this.end;
        if (event.key === "Home") this.selection = this.points[0].timestamp;
        if (event.key === "End") this.selection = this.points.at(-1).timestamp;
        if (event.key === "ArrowLeft") this.selection = (this.points.findLast((p) => p.timestamp < selected) ?? this.points[0]).timestamp;
        if (event.key === "ArrowRight") this.selection = (this.points.find((p) => p.timestamp > selected) ?? this.points.at(-1)).timestamp;
      }
      this.inspect();
    });
  }

  reset() { this.selection = null; this.pinned = false; }

  render(points, start, end, resolution, loading) {
    this.points = points;
    this.start = start;
    this.end = end;
    this.resolution = resolution;
    this.loading = loading;
    this.ceiling = this.id === "cpu" ? 100 : networkScale(points);
    this.x = (timestamp) => (timestamp - start) / (end - start) * 500;
    this.y = (value) => 154 - Math.max(0, Math.min(value / this.ceiling, 1)) * 148;
    this.svg.replaceChildren();
    this.yAxis.replaceChildren();
    this.labels.replaceChildren();
    for (const fraction of [1, .75, .5, .25, 0]) {
      const label = document.createElement("span");
      label.textContent = this.id === "cpu" ? fraction * 100 + "%" : networkTick(this.ceiling * fraction, this.ceiling);
      this.yAxis.append(label);
      const y = this.y(this.ceiling * fraction);
      this.svg.append(svgElement("line", { x1: 0, y1: y, x2: 500, y2: y, class: "chart-grid" }));
    }
    for (const span of missingSpans(points, this.keys, start, end, resolution)) {
      const left = this.x(span.start);
      const width = this.x(span.end) - left;
      this.svg.append(svgElement("rect", { x: left, y: 6, width, height: 148, class: "chart-missing" }));
      if (width >= 120) {
        const label = document.createElement("span");
        label.className = "chart-missing-note";
        label.style.left = left / 5 + "%";
        label.style.width = width / 5 + "%";
        label.textContent = loading ? "Loading history…" : span.label;
        this.labels.append(label);
      }
    }
    for (const key of this.keys) {
      for (const segment of segments(points, key, resolution)) {
        if (segment.length === 1) {
          this.svg.append(svgElement("circle", { cx: this.x(segment[0].timestamp), cy: this.y(segment[0][key]), r: 2, class: "chart-point " + key }));
          continue;
        }
        const d = segment.map((p, i) => (i ? "L" : "M") + this.x(p.timestamp).toFixed(2) + "," + this.y(p[key]).toFixed(2)).join(" ");
        if (key === "cpu") this.svg.append(svgElement("path", { d: d + ` L${this.x(segment.at(-1).timestamp)},154 L${this.x(segment[0].timestamp)},154 Z`, class: "chart-area" }));
        this.svg.append(svgElement("path", { d, class: "chart-line " + key }));
      }
    }
    this.timeAxis.replaceChildren();
    for (const timestamp of [start, (start + end) / 2, end]) {
      const label = document.createElement("time");
      label.dateTime = new Date(timestamp * 1000).toISOString();
      const time = document.createElement("span");
      time.textContent = clock(timestamp);
      label.append(time);
      if (end - start >= 86400 || date(start) !== date(end)) {
        const day = document.createElement("span");
        day.textContent = date(timestamp);
        label.append(day);
      }
      this.timeAxis.append(label);
    }
    this.plot.setAttribute("aria-valuemin", Math.round(start));
    this.plot.setAttribute("aria-valuemax", Math.round(end));
    this.plot.setAttribute("aria-disabled", String(loading || !points.length));
    if (this.selection !== null && (this.selection < start || this.selection > end)) this.reset();
    this.inspect();
  }

  inspect() {
    if (!this.x) return;
    this.cursor.replaceChildren();
    this.readout.replaceChildren();
    const time = document.createElement("span");
    const values = document.createElement("strong");
    this.readout.append(time, values);
    this.plot.setAttribute("aria-valuenow", Math.round(this.selection ?? this.end));
    if (this.selection === null) {
      time.textContent = this.loading ? "Loading history…" : "Hover or tap to inspect";
      values.textContent = "← → keys also work";
      this.plot.setAttribute("aria-valuetext", this.loading ? "Loading history" : "Select a time to inspect " + this.name);
      return;
    }
    const point = nearestPoint(this.points, this.selection, this.resolution);
    const timestamp = point?.timestamp ?? this.selection;
    this.selection = timestamp;
    this.plot.setAttribute("aria-valuenow", Math.round(timestamp));
    const x = this.x(timestamp);
    this.cursor.append(svgElement("line", { x1: x, y1: 6, x2: x, y2: 154, class: "chart-crosshair" }));
    time.textContent = date(timestamp) + " · " + clock(timestamp, this.resolution === 5);
    if (point && this.resolution > 5) {
      const bucketStart = Math.floor(timestamp / this.resolution) * this.resolution;
      time.textContent = date(timestamp) + " · " + clock(bucketStart) + "–" + clock(bucketStart + this.resolution) + " average";
    }
    if (!point || !this.keys.some((key) => known(point[key]))) values.textContent = "No recorded data";
    else {
      values.textContent = this.id === "cpu" ? "CPU " + percent(point.cpu) : "↓ " + rate(point.rx) + "  ↑ " + rate(point.tx);
      for (const key of this.keys) {
        if (known(point[key])) this.cursor.append(svgElement("circle", { cx: x, cy: this.y(point[key]), r: 3.5, class: "chart-point " + key }));
      }
    }
    const spoken = point && this.id === "network" ? "Download " + rate(point.rx) + ", upload " + rate(point.tx) : values.textContent;
    this.plot.setAttribute("aria-valuetext", time.textContent + ": " + spoken);
  }
}
