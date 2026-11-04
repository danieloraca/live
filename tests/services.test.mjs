import test from "node:test";
import assert from "node:assert/strict";
import { serviceCheck } from "../static/services.mjs";

test("open TCP availability leaves authenticated MCP health unverified", () => {
  const check = serviceCheck({ check: "tcp", state: "active", tcp_connected: true, http_status: null });
  assert.equal(check.label, "TCP open");
  assert.equal(check.needsAttention, false);
  assert.equal(check.unverified, true);
  assert.equal(check.level, "warning");
});

test("a missing or refused listener needs attention regardless of an HTTP auth reply", () => {
  for (const tcp_connected of [false, null, undefined]) {
    const check = serviceCheck({ check: "tcp", state: "active", tcp_connected, http_status: 401 });
    assert.equal(check.label, "TCP unavailable");
    assert.equal(check.needsAttention, true);
    assert.equal(check.unverified, true);
    assert.equal(check.level, "error");
  }
  assert.equal(serviceCheck({ check: "tcp", state: "inactive", tcp_connected: null }).label, "Not checked");
});

test("HTTP app replies and failures preserve existing behavior, including older API data", () => {
  for (const [status, needsAttention, level] of [[200, false, "ok"], [404, false, "warning"], [503, true, "error"]]) {
    const check = serviceCheck({ state: "active", http_status: status, latency_ms: 1.6 });
    assert.equal(check.label, `HTTP ${status} · 2 ms`);
    assert.equal(check.needsAttention, needsAttention);
    assert.equal(check.level, level);
    assert.equal(check.unverified, false);
  }
  assert.equal(serviceCheck({ state: "active", http_status: null }).needsAttention, true);
});
