import test from "node:test";
import assert from "node:assert/strict";
import { summarize, segments, missingSpans, nearestPoint, networkScale, networkTick, rate } from "../static/charts.mjs";

test("summaries include zero and exclude missing samples", () => {
  assert.deepEqual(summarize([{ cpu: 0 }, { cpu: 9 }, { cpu: null }, { cpu: NaN }], "cpu"), { average: 4.5, peak: 9, samples: 2 });
  assert.deepEqual(summarize([{ cpu: null }], "cpu"), { average: null, peak: null, samples: 0 });
  assert.equal(rate(null), "—");
  assert.equal(rate(0), "0 B/s");
});

test("lines break at null readings and outages, including averaged views", () => {
  const raw = [{ timestamp: 0, cpu: 0 }, { timestamp: 5, cpu: null }, { timestamp: 10, cpu: 2 }, { timestamp: 15, cpu: 4 }, { timestamp: 60, cpu: 3 }];
  assert.deepEqual(segments(raw, "cpu", 5).map((s) => s.map((p) => p.timestamp)), [[0], [10, 15], [60]]);
  const averaged = [0, 60, 120, 300].map((timestamp) => ({ timestamp, rx: 1 }));
  assert.deepEqual(segments(averaged, "rx", 60).map((s) => s.length), [3, 1]);
});

test("uncollected history and internal gaps are explicit; zero traffic is data", () => {
  assert.deepEqual(missingSpans([], ["cpu"], 0, 600, 5), [{ start: 0, end: 600, label: "No data yet" }]);
  const points = [{ timestamp: 100, rx: 0, tx: null }, { timestamp: 105, rx: 0, tx: 0 }, { timestamp: 200, rx: 0, tx: 0 }];
  const spans = missingSpans(points, ["rx", "tx"], 0, 205, 5);
  assert.equal(spans[0].end, 100);
  assert.equal(spans[1].label, "No data");
  assert.equal(spans.length, 2);
  assert.ok(missingSpans([{ timestamp: 0, cpu: 1 }, { timestamp: 100, cpu: 1 }], ["cpu"], 0, 100, 60).every((span) => span.end > span.start));
});

test("inspection does not jump across empty history or invent a missing value", () => {
  const points = [{ timestamp: 100, cpu: 5 }, { timestamp: 105, cpu: null }, { timestamp: 500, cpu: 0 }];
  assert.equal(nearestPoint(points, 102, 5), points[0]);
  assert.equal(nearestPoint(points, 105, 5), points[1]);
  assert.equal(nearestPoint(points, 300, 5), null);
  assert.equal(nearestPoint(points, 0, 5), null);
  assert.equal(nearestPoint([], 0, 5), null);
  assert.equal(nearestPoint([{ timestamp: 1000, cpu: 1 }], 1020, 60)?.cpu, 1);
});

test("network scale contains both directions and uses consistent axis units", () => {
  assert.equal(networkScale([]), 1024);
  const ceiling = networkScale([{ rx: 100, tx: 3000 }, { rx: null, tx: 0 }]);
  assert.equal(ceiling, 4096);
  assert.deepEqual([1, .75, .5, .25, 0].map((fraction) => networkTick(ceiling * fraction, ceiling)), ["4 KiB/s", "3 KiB/s", "2 KiB/s", "1 KiB/s", "0 KiB/s"]);
});
