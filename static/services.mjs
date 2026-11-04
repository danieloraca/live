// TCP availability is deliberately separate from authenticated MCP/database health.
export function serviceCheck(service) {
  const active = service.state === "active";
  if (service.check === "tcp") {
    const open = service.tcp_connected === true;
    return {
      label: !active ? "Not checked" : open ? "TCP open" : "TCP unavailable",
      level: !active ? "" : open ? "warning" : "error",
      needsAttention: active && !open,
      unverified: true,
    };
  }
  const status = service.http_status;
  const known = Number.isFinite(status);
  const latency = Number.isFinite(service.latency_ms) ? " · " + Math.round(service.latency_ms) + " ms" : "";
  return {
    label: known ? "HTTP " + status + latency : active ? "No HTTP response" : "Not checked",
    level: known ? status < 400 ? "ok" : status < 500 ? "warning" : "error" : active ? "error" : "",
    needsAttention: active && (!known || status >= 500),
    unverified: false,
  };
}
