// HTTP + WebSocket access to the dvbhub server.

/**
 * Calls a JSON API endpoint; throws Error(message) on HTTP or API errors.
 * @param {string} method
 * @param {string} path
 * @param {object} [body]
 */
export async function api(method, path, body) {
  const opt = { method, headers: {} };
  if (body !== undefined) {
    opt.headers["Content-Type"] = "application/json";
    opt.body = JSON.stringify(body);
  }
  const res = await fetch(path, opt);
  const data = await res.json().catch(() => ({}));
  if (!res.ok || data.ok === false) {
    throw new Error(data.error || `HTTP ${res.status}`);
  }
  return data;
}

const listeners = new Map();

/**
 * Subscribes to a WebSocket event type ("status", "signal", "devices", …, or "*").
 * @param {string} type
 * @param {(msg: object) => void} fn
 */
export function on(type, fn) {
  if (!listeners.has(type)) listeners.set(type, []);
  listeners.get(type).push(fn);
}

function emit(msg) {
  for (const key of [msg.type, "*"]) {
    for (const fn of listeners.get(key) || []) {
      try {
        fn(msg);
      } catch (e) {
        console.error("listener failed", key, e);
      }
    }
  }
}

/**
 * Opens the event WebSocket and reconnects forever.
 * @param {(connected: boolean) => void} onState
 */
export function connect(onState) {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/ws`);
  ws.onopen = () => onState(true);
  ws.onclose = () => {
    onState(false);
    setTimeout(() => connect(onState), 2000);
  };
  ws.onmessage = (ev) => {
    try {
      emit(JSON.parse(ev.data));
    } catch (e) {
      console.error("bad ws message", e);
    }
  };
}
