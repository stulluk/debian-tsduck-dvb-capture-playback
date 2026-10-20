// Small DOM helpers shared by the pages.

/** @param {string} sel */
export const $ = (sel) => document.querySelector(sel);
/** @param {string} sel */
export const $$ = (sel) => Array.from(document.querySelectorAll(sel));

/** Escapes text for safe insertion into HTML. */
export function esc(value) {
  return String(value ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** Shows a short notification. kind: "info" | "ok" | "err". */
export function toast(text, kind = "info") {
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = text;
  $("#toast").appendChild(el);
  setTimeout(() => el.remove(), kind === "err" ? 8000 : 4000);
}

const LOG_MAX = 500;

/** Appends a line to the activity log drawer. */
export function logLine(text, cls = "") {
  const log = $("#log");
  const div = document.createElement("div");
  if (cls) div.className = cls;
  div.textContent = `${new Date().toLocaleTimeString()}  ${text}`;
  const stick = log.scrollHeight - log.scrollTop - log.clientHeight < 40;
  log.appendChild(div);
  while (log.childNodes.length > LOG_MAX) log.removeChild(log.firstChild);
  if (stick) log.scrollTop = log.scrollHeight;
  $("#log-last").textContent = text;
}

/**
 * Builds labelled form fields into a container.
 * spec: [{ id, label, type: "number"|"text"|"select", value, options: [v|[v,label]], step }]
 */
export function buildFields(container, spec) {
  container.innerHTML = spec
    .map((f) => {
      const input =
        f.type === "select"
          ? `<select id="${f.id}">${f.options
              .map((o) => {
                const [v, l] = Array.isArray(o) ? o : [o, o];
                return `<option value="${esc(v)}"${String(v) === String(f.value) ? " selected" : ""}>${esc(l)}</option>`;
              })
              .join("")}</select>`
          : `<input id="${f.id}" type="${f.type || "text"}" value="${esc(f.value ?? "")}"${f.placeholder ? ` placeholder="${esc(f.placeholder)}"` : ""} />`;
      return `<div class="field"><label>${esc(f.label)}</label>${input}</div>`;
    })
    .join("");
}

/** Value of an input as integer (or null when empty / invalid). */
export function intVal(sel) {
  const v = parseInt($(sel).value, 10);
  return Number.isFinite(v) ? v : null;
}

/**
 * Renders tab buttons and wires selection.
 * items: [{ key, label }]; onPick(key) is called on click and for the initial tab.
 */
export function tabs(container, items, active, onPick) {
  container.innerHTML = items
    .map((t) => `<button class="tab${t.key === active ? " active" : ""}" data-key="${t.key}">${esc(t.label)}</button>`)
    .join("");
  container.querySelectorAll(".tab").forEach((b) => {
    b.onclick = () => {
      container.querySelectorAll(".tab").forEach((x) => x.classList.toggle("active", x === b));
      onPick(b.dataset.key);
    };
  });
  if (active) onPick(active);
}

/** Splits "618 682, 690" into numbers. */
export function numberList(text) {
  return String(text || "")
    .split(/[\s,;]+/)
    .map((s) => parseInt(s, 10))
    .filter((n) => Number.isFinite(n) && n > 0);
}

/** Human file size. */
export function fmtSize(bytes) {
  if (bytes >= 1e9) return (bytes / 1e9).toFixed(2) + " GB";
  if (bytes >= 1e6) return (bytes / 1e6).toFixed(1) + " MB";
  return Math.round(bytes / 1e3) + " kB";
}

/** "Lock / SNR / Strength / BER" metric tiles from a signal status. */
export function signalMetrics(sig) {
  const s = sig || {};
  const tile = (lbl, val) => `<div class="metric"><div class="lbl">${lbl}</div><div class="val">${val}</div></div>`;
  return [
    tile("Lock", s.locked ? '<span class="pill ok">LOCKED</span>' : '<span class="pill">—</span>'),
    tile("SNR", s.snr_db != null ? s.snr_db.toFixed(1) + " dB" : "—"),
    tile("Strength", s.strength_db != null ? s.strength_db.toFixed(1) + " dB" : "—"),
    tile("BER", esc(s.ber || "—")),
  ].join("");
}

export const LNB_TYPES = [
  ["universal", "Universal Ku (9750/10600)"],
  ["Universal", "Universal Ku old (10.8–12.7 GHz)"],
  ["Astra", "Single LO 9750 (10.7–11.7 GHz)"],
  ["9750,10600,11700", "Universal Ku MHz (9750,10600,11700)"],
  ["0", "Direct L-band (no LNB)"],
  ["C-Band", "C-Band 5150"],
  ["C-Multi", "C-Band multipoint"],
  ["L10700", "LO 10700"],
  ["L10750", "LO 10750"],
  ["L11300", "LO 11300"],
  ["DBS", "DBS / Expressvu"],
  ["QPH031", "Invacom QPH-031"],
];
