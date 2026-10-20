// Entry point: shared state, routing, header (badges + job), settings, log.
import { api, connect, on } from "./api.js";
import { $, $$, esc, logLine, toast } from "./ui.js";
import * as devices from "./devices.js";
import * as capture from "./capture.js";
import * as playback from "./playback.js";
import * as watch from "./watch.js";

/** Shared client state; pages read from it and listen for updates. */
export const state = {
  devices: [],
  warnings: [],
  status: {},
  satellites: [],
  refTps: [],
  channels: [],
};

/** Tuner devices (one entry per frontend). */
export const tuners = () => state.devices.filter((d) => d.kind === "tuner");
/** Modulator devices. */
export const modulators = () => state.devices.filter((d) => d.kind === "modulator");

/** Reloads the satellite catalogue and reference TPs. */
export async function loadCatalogues() {
  const [sats, refs] = await Promise.all([api("GET", "/api/satellites"), api("GET", "/api/reference-transponders")]);
  state.satellites = sats.satellites || [];
  state.refTps = refs.transponders || [];
}

/** Reloads saved channels. */
export async function loadChannels() {
  const db = await api("GET", "/api/channels");
  state.channels = db.channels || [];
}

/** Stops the current tuner job (monitor / capture / scan / play). */
export async function stopTuner() {
  await api("DELETE", "/api/session/tuner");
}

// ---- routing ----------------------------------------------------------------

const PAGES = { home: null, devices, capture, playback, watch, settings: null };

function route() {
  let name = location.hash.replace(/^#\/?/, "") || "home";
  if (!(name in PAGES)) name = "home";
  $$(".page").forEach((p) => p.classList.toggle("active", p.id === `page-${name}`));
  const mod = PAGES[name];
  if (mod && mod.show) mod.show();
  if (name === "settings") loadSettings();
}

/** Navigates to a page. */
export function go(name) {
  location.hash = name === "home" ? "" : `#/${name}`;
}

// ---- header ------------------------------------------------------------------

function renderBadges() {
  // One badge per physical device: frontends of the same tuner share a label.
  const groups = new Map();
  for (const d of state.devices) {
    const g = groups.get(d.label) || { label: d.label, kinds: new Set() };
    g.kinds.add(d.kind);
    groups.set(d.label, g);
  }
  const html = [...groups.values()].map(
    (g) => `<span class="badge" title="${esc([...g.kinds].join(", "))}"><span class="dot on"></span>${esc(g.label)}</span>`,
  );
  for (const w of state.warnings) {
    const name = w.split(/[ (:]/)[0] || "device";
    html.push(`<span class="badge" title="${esc(w)}"><span class="dot off"></span>${esc(name)}</span>`);
  }
  $("#badges").innerHTML = html.length ? html.join("") : '<span class="badge none">no DVB devices available</span>';

  const hasTuner = tuners().length > 0;
  const hasMod = modulators().length > 0;
  const gate = { devices: state.devices.length > 0, capture: hasTuner, playback: hasMod, watch: hasTuner };
  for (const [name, enabled] of Object.entries(gate)) {
    $(`.tile[data-go="${name}"]`).classList.toggle("disabled", !enabled);
  }
  $("#landing-msg").textContent = state.devices.length
    ? ""
    : "No DVB devices detected — plug in a tuner or the DTU-315; this page updates automatically.";
}

function renderJob() {
  const st = state.status;
  const parts = [];
  if (st.capturing && st.capture) {
    const c = st.capture;
    const left = c.remaining_s != null ? ` · ${c.remaining_s}s left` : "";
    parts.push(`<span class="dot busy"></span> Recording ${c.index || 1}/${c.total} ${esc(c.current)}${left}`);
  } else if (st.scanning && st.scan) {
    parts.push(`<span class="dot busy"></span> Scanning ${st.scan.index}/${st.scan.total} · ${st.scan.found} found`);
  } else if (st.playing && st.play) {
    parts.push(`<span class="dot busy"></span> Watching ${esc(st.play.name)}`);
  } else if (st.monitoring) {
    parts.push('<span class="dot busy"></span> Monitoring');
  }
  if (parts.length) parts.push('<button class="secondary small" id="job-stop">Stop</button>');
  if (st.playout) {
    const f = (st.playout_file || "").split("/").pop();
    parts.push(`<span class="dot busy"></span> Playout ${esc(f)} <button class="secondary small" id="playout-stop">Stop</button>`);
  }
  $("#job").innerHTML = parts.join(" ");
  const js = $("#job-stop");
  if (js) js.onclick = () => stopTuner().catch((e) => toast(e.message, "err"));
  const ps = $("#playout-stop");
  if (ps) ps.onclick = () => api("DELETE", "/api/session/playout").catch((e) => toast(e.message, "err"));
}

// ---- settings ----------------------------------------------------------------

async function loadSettings() {
  try {
    const c = await api("GET", "/api/config");
    $("#cfg-dir").value = c.capture_dir;
    $("#cfg-info").innerHTML = [
      ["Config file", c.config_file],
      ["Log file", c.log_file],
      ["HLS working dir (tmpfs)", c.hls_dir],
      ["Default capture length", `${c.capture_seconds} s`],
    ]
      .map(([k, v]) => `<tr><td class="muted">${k}</td><td class="mono">${esc(v)}</td></tr>`)
      .join("");
  } catch (e) {
    toast(e.message, "err");
  }
}

$("#cfg-save").onclick = async () => {
  try {
    const r = await api("POST", "/api/config", { capture_dir: $("#cfg-dir").value });
    $("#cfg-dir").value = r.capture_dir;
    toast("Capture directory saved", "ok");
  } catch (e) {
    toast(e.message, "err");
  }
};

// ---- wiring ------------------------------------------------------------------

on("devices", (m) => {
  state.devices = m.devices || [];
  state.warnings = m.warnings || [];
  renderBadges();
});
on("status", (m) => {
  state.status = m.status || {};
  renderJob();
});
on("log", (m) => logLine(m.line, m.line.startsWith(">>>") ? "act" : ""));
on("error", (m) => {
  logLine(m.message, "err");
  toast(m.message, "err");
});

$("#log-head").onclick = () => $("#logbar").classList.toggle("open");
$("#home").onclick = () => go("home");
$$("[data-go]").forEach((el) => (el.onclick = () => go(el.dataset.go)));
window.addEventListener("hashchange", route);

async function health() {
  try {
    const h = await api("GET", "/api/health");
    return h.tsduck_version.replace(/^.*version\s*/i, "TSDuck ");
  } catch (e) {
    return "";
  }
}

connect(async (up) => {
  const ver = up ? await health() : "";
  $("#conn").innerHTML = up
    ? `<span class="dot on"></span> ${esc(ver || "connected")}`
    : '<span class="dot off"></span> reconnecting…';
});

for (const mod of [devices, capture, playback, watch]) mod.init();
loadCatalogues()
  .catch((e) => toast("catalogue: " + e.message, "err"))
  .finally(route);
