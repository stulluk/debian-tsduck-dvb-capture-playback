// TS capture page: single TP, whole satellite, manual TP, terrestrial, cable.
import { api, on } from "./api.js";
import { $, $$, LNB_TYPES, buildFields, esc, intVal, signalMetrics, tabs, toast } from "./ui.js";
import { loadCatalogues, state, stopTuner, tuners } from "./main.js";

let delivery = "satellite";
let satMode = "tp";
let editingTp = -1;

const POLS = [["vertical", "Vertical"], ["horizontal", "Horizontal"], ["left", "Left (circular)"], ["right", "Right (circular)"]];
const SYS = ["DVB-S2", "DVB-S"];
const MODS = ["8-PSK", "QPSK", "16-APSK", "32-APSK"];
const FECS = ["auto", "1/2", "2/3", "3/4", "3/5", "4/5", "5/6", "7/8", "8/9", "9/10"];

function tpLabel(t) {
  return `${t.frequency_mhz} ${t.polarity[0].toUpperCase()} ${t.symbol_rate_ksps} ${t.delivery_system} ${t.modulation} ${t.fec}${t.name ? " — " + t.name : ""}`;
}

function selectedDevice() {
  return tuners().find((d) => d.address === $("#cap-device").value);
}

function fillDevices() {
  const cur = $("#cap-device").value;
  const list = tuners();
  $("#cap-device").innerHTML =
    list
      .map(
        (d) => `<option value="${esc(d.address)}">${esc(d.label)} @ ${esc(d.address)} — ${esc((d.delivery_systems || []).join(", "))}</option>`,
      )
      .join("") || '<option value="">— no tuner —</option>';
  if (cur && list.some((d) => d.address === cur)) $("#cap-device").value = cur;
  buildDeliveryTabs();
}

function buildDeliveryTabs() {
  const dev = selectedDevice();
  const cats = dev ? dev.categories || [] : [];
  const items = [
    { key: "satellite", label: "Satellite" },
    { key: "terrestrial", label: "Terrestrial" },
    { key: "cable", label: "Cable" },
  ].filter((t) => cats.includes(t.key));
  if (!items.length) {
    $("#cap-tabs").innerHTML = '<span class="muted">Select a capture device.</span>';
    showDelivery(null);
    return;
  }
  if (!items.some((t) => t.key === delivery)) delivery = items[0].key;
  tabs($("#cap-tabs"), items, delivery, showDelivery);
}

function showDelivery(key) {
  delivery = key;
  $("#cap-sat").classList.toggle("hidden", key !== "satellite");
  $("#cap-ter").classList.toggle("hidden", key !== "terrestrial");
  $("#cap-cab").classList.toggle("hidden", key !== "cable");
  $("#cap-start").disabled = !key;
}

function showSatMode(mode) {
  satMode = mode;
  $$("#cap-sat-mode button").forEach((b) => b.classList.toggle("active", b.dataset.mode === mode));
  $("#cap-sat-tp").classList.toggle("hidden", mode !== "tp");
  $("#cap-sat-sat").classList.toggle("hidden", mode !== "sat");
  $("#cap-sat-manual").classList.toggle("hidden", mode !== "manual");
}

// ---- single TP -------------------------------------------------------------

function fillTpSources() {
  const cur = $("#cap-tp-source").value;
  $("#cap-tp-source").innerHTML =
    `<option value="__ref">Reference transponders (${state.refTps.length})</option>` +
    state.satellites.map((s) => `<option value="${esc(s.name)}">${esc(s.name)} (${s.transponders.length} TP)</option>`).join("");
  if (cur) $("#cap-tp-source").value = cur;
  fillTpList();
}

function tpSourceList() {
  const src = $("#cap-tp-source").value;
  if (src === "__ref") return state.refTps;
  const sat = state.satellites.find((s) => s.name === src);
  return sat ? sat.transponders : [];
}

function fillTpList() {
  $("#cap-tp").innerHTML = tpSourceList()
    .map((t, i) => `<option value="${i}">${esc(tpLabel(t))}</option>`)
    .join("");
}

/** Satellite that lists this TP (gives the file prefix, LNB and DiSEqC). */
function satelliteFor(tp) {
  return state.satellites.find((s) =>
    s.transponders.some((t) => Math.abs(t.frequency_mhz - tp.frequency_mhz) <= 1 && t.polarity === tp.polarity),
  );
}

// ---- whole satellite ---------------------------------------------------------

function fillSatSelect() {
  const cur = $("#sat-select").value;
  $("#sat-select").innerHTML = state.satellites
    .map((s) => `<option value="${esc(s.name)}">${esc(s.name)} — ${s.transponders.length} TP · LNB ${esc(s.lnb)} · DiSEqC ${s.diseqc}</option>`)
    .join("");
  if (cur) $("#sat-select").value = cur;
  renderSatTps();
}

function currentSat() {
  return state.satellites.find((s) => s.name === $("#sat-select").value);
}

function renderSatTps() {
  const sat = currentSat();
  $("#sat-tp-count").textContent = sat ? `(${sat.transponders.length})` : "";
  $("#sat-tps").innerHTML = sat
    ? sat.transponders
        .map(
          (t, i) => `<tr><td class="mono">${t.frequency_mhz}</td><td>${t.polarity[0].toUpperCase()}</td><td>${t.symbol_rate_ksps}</td>
          <td>${esc(t.delivery_system)}</td><td>${esc(t.modulation)}</td><td>${esc(t.fec)}</td>
          <td><button class="secondary small" data-edit="${i}">Edit</button> <button class="danger small" data-del="${i}">✕</button></td></tr>`,
        )
        .join("")
    : "";
  $$("#sat-tps [data-edit]").forEach((b) => (b.onclick = () => editTp(+b.dataset.edit)));
  $$("#sat-tps [data-del]").forEach((b) => (b.onclick = () => delTp(+b.dataset.del)));
}

async function saveSat(sat) {
  await api("POST", "/api/satellites", sat);
  await loadCatalogues();
  fillSatSelect();
  fillTpSources();
  $("#sat-select").value = sat.name;
  renderSatTps();
}

function tpForm(prefix, tp) {
  buildFields($(`#${prefix === "te" ? "te-form" : "manual-form"}`), [
    { id: `${prefix}-freq`, label: "Frequency (MHz)", type: "number", value: tp.frequency_mhz },
    { id: `${prefix}-pol`, label: "Polarity", type: "select", value: tp.polarity, options: POLS },
    { id: `${prefix}-sr`, label: "Symbol rate (kS/s)", type: "number", value: tp.symbol_rate_ksps },
    { id: `${prefix}-sys`, label: "Delivery system", type: "select", value: tp.delivery_system, options: SYS },
    { id: `${prefix}-mod`, label: "Modulation", type: "select", value: tp.modulation, options: MODS },
    { id: `${prefix}-fec`, label: "FEC", type: "select", value: tp.fec, options: FECS },
    ...(prefix === "m"
      ? [
          { id: "m-lnb", label: "LNB", type: "select", value: "universal", options: LNB_TYPES },
          { id: "m-diseqc", label: "DiSEqC port (0-63)", type: "number", value: 0 },
          { id: "m-name", label: "Satellite name (file prefix)", value: "manual" },
        ]
      : [{ id: "te-name", label: "Note", value: tp.name || "" }]),
  ]);
}

function readTp(prefix) {
  return {
    frequency_mhz: intVal(`#${prefix}-freq`) || 0,
    polarity: $(`#${prefix}-pol`).value,
    symbol_rate_ksps: intVal(`#${prefix}-sr`) || 0,
    delivery_system: $(`#${prefix}-sys`).value,
    modulation: $(`#${prefix}-mod`).value,
    fec: $(`#${prefix}-fec`).value,
    name: prefix === "te" ? $("#te-name").value : "",
  };
}

const BLANK_TP = { frequency_mhz: 11054, polarity: "vertical", symbol_rate_ksps: 27500, delivery_system: "DVB-S2", modulation: "8-PSK", fec: "3/4", name: "" };

function editTp(i) {
  const sat = currentSat();
  if (!sat) return;
  editingTp = i;
  tpForm("te", sat.transponders[i]);
  $("#te-form").closest("details").open = true;
}

async function delTp(i) {
  const sat = currentSat();
  if (!sat || !confirm(`Remove ${tpLabel(sat.transponders[i])}?`)) return;
  sat.transponders.splice(i, 1);
  await saveSat(sat).catch((e) => toast(e.message, "err"));
}

function openSatEditor(sat) {
  $("#sat-editor").classList.remove("hidden");
  $("#se-name").value = sat ? sat.name : "";
  $("#se-name").dataset.orig = sat ? sat.name : "";
  $("#se-lon").value = sat ? sat.longitude : "";
  $("#se-lnb").innerHTML = LNB_TYPES.map(([v, l]) => `<option value="${esc(v)}">${esc(l)}</option>`).join("");
  $("#se-lnb").value = sat ? sat.lnb : "universal";
  $("#se-diseqc").value = sat ? sat.diseqc : 0;
}

// ---- start / progress ----------------------------------------------------------

function captureBody() {
  const body = { delivery, device_name: $("#cap-device").value, seconds: intVal("#cap-seconds") };
  if (delivery === "satellite") {
    if (satMode === "tp") {
      const tp = tpSourceList()[intVal("#cap-tp")];
      if (!tp) throw new Error("select a transponder");
      const sat = $("#cap-tp-source").value === "__ref" ? satelliteFor(tp) : currentSourceSat();
      body.satellite = sat ? sat.name : "reference";
      body.tps = [tp];
    } else if (satMode === "sat") {
      const sat = currentSat();
      if (!sat) throw new Error("select a satellite");
      body.satellite = sat.name;
      if (!confirm(`Capture all ${sat.transponders.length} transponders of ${sat.name}, ${body.seconds}s each?`)) return null;
    } else {
      const tp = readTp("m");
      if (!tp.frequency_mhz || !tp.symbol_rate_ksps) throw new Error("frequency and symbol rate are required");
      body.satellite = $("#m-name").value.trim() || "manual";
      body.tps = [tp];
      body.lnb = $("#m-lnb").value;
      body.diseqc = intVal("#m-diseqc") ?? 0;
    }
  } else if (delivery === "terrestrial") {
    Object.assign(body, {
      frequency_mhz: intVal("#ter-freq"),
      delivery_system: $("#ter-sys").value,
      bandwidth_mhz: intVal("#ter-bw"),
      plp: $("#ter-sys").value === "DVB-T2" ? intVal("#ter-plp") : null,
    });
  } else {
    Object.assign(body, {
      frequency_mhz: intVal("#cab-freq"),
      symbol_rate_ksps: intVal("#cab-sr"),
      modulation: $("#cab-mod").value,
    });
  }
  return body;
}

function currentSourceSat() {
  return state.satellites.find((s) => s.name === $("#cap-tp-source").value);
}

function renderProgress() {
  const st = state.status;
  const running = !!(st.capturing && st.capture);
  $("#cap-stop").disabled = !running;
  $("#cap-start").disabled = running || !delivery;
  if (!st.capture) return;
  $("#cap-progress").classList.remove("hidden");
  const c = st.capture;
  const pct = c.total ? (100 * Math.max(0, (c.index || 1) - 1 + (running ? 0.5 : 1))) / c.total : 0;
  $("#cap-bar").style.width = `${Math.min(100, pct)}%`;
  $("#cap-now").innerHTML = running
    ? `${esc(c.label)} · target ${c.index || 1}/${c.total}: <strong>${esc(c.current)}</strong>${c.remaining_s != null ? ` · ${c.remaining_s} s left` : " · tuning…"}`
    : `${esc(c.label)} · finished`;
  $("#cap-files").innerHTML = (c.files || []).map((f) => esc(f)).join("<br>");
}

export function init() {
  $$("#cap-sat-mode button").forEach((b) => (b.onclick = () => showSatMode(b.dataset.mode)));
  $("#cap-device").onchange = buildDeliveryTabs;
  $("#cap-tp-source").onchange = fillTpList;
  $("#sat-select").onchange = renderSatTps;
  tpForm("m", BLANK_TP);
  tpForm("te", BLANK_TP);
  $("#ter-uhf").innerHTML =
    '<option value="">—</option>' +
    Array.from({ length: 49 }, (_, i) => i + 21)
      .map((c) => `<option value="${474 + (c - 21) * 8}">CH ${c} — ${474 + (c - 21) * 8} MHz</option>`)
      .join("");
  $("#ter-uhf").onchange = () => {
    if ($("#ter-uhf").value) $("#ter-freq").value = $("#ter-uhf").value;
  };

  $("#sat-add").onclick = () => openSatEditor(null);
  $("#sat-edit").onclick = () => currentSat() && openSatEditor(currentSat());
  $("#se-cancel").onclick = () => $("#sat-editor").classList.add("hidden");
  $("#se-save").onclick = async () => {
    const orig = $("#se-name").dataset.orig;
    const prev = state.satellites.find((s) => s.name === orig);
    const sat = {
      name: $("#se-name").value.trim(),
      longitude: $("#se-lon").value.trim(),
      lnb: $("#se-lnb").value,
      diseqc: intVal("#se-diseqc") ?? 0,
      transponders: prev ? prev.transponders : [],
    };
    if (!sat.name) return toast("name is required", "err");
    try {
      if (orig && orig !== sat.name) await api("DELETE", `/api/satellites/${encodeURIComponent(orig)}`);
      await saveSat(sat);
      $("#sat-editor").classList.add("hidden");
      toast(`Saved ${sat.name}`, "ok");
    } catch (e) {
      toast(e.message, "err");
    }
  };
  $("#sat-del").onclick = async () => {
    const sat = currentSat();
    if (!sat || !confirm(`Remove satellite ${sat.name}?`)) return;
    try {
      await api("DELETE", `/api/satellites/${encodeURIComponent(sat.name)}`);
      await loadCatalogues();
      fillSatSelect();
      fillTpSources();
    } catch (e) {
      toast(e.message, "err");
    }
  };
  $("#kos-import").onclick = async () => {
    const pos = $("#kos-pos").value.trim();
    if (!pos) return toast("enter a position, e.g. 13E", "err");
    try {
      const sat = currentSat();
      const body = { pos };
      if (sat && confirm(`Refresh "${sat.name}" from KingOfSat ${pos}? (Cancel = import as a new satellite)`)) body.name = sat.name;
      const r = await api("POST", "/api/satellites/import", body);
      toast(`Imported ${r.name}: ${r.transponders} TPs`, "ok");
      await loadCatalogues();
      fillSatSelect();
      fillTpSources();
      $("#sat-select").value = r.name;
      renderSatTps();
    } catch (e) {
      toast(e.message, "err");
    }
  };
  $("#te-clear").onclick = () => {
    editingTp = -1;
    tpForm("te", BLANK_TP);
  };
  $("#te-save").onclick = async () => {
    const sat = currentSat();
    if (!sat) return;
    const tp = readTp("te");
    if (!tp.frequency_mhz || !tp.symbol_rate_ksps) return toast("frequency and symbol rate are required", "err");
    if (editingTp >= 0) sat.transponders[editingTp] = tp;
    else sat.transponders.push(tp);
    editingTp = -1;
    await saveSat(sat).catch((e) => toast(e.message, "err"));
  };
  $("#save-ref").onclick = async () => {
    try {
      const tp = readTp("m");
      tp.name = $("#m-name").value.trim();
      await api("POST", "/api/reference-transponders", tp);
      await loadCatalogues();
      fillTpSources();
      toast("Saved as reference transponder", "ok");
    } catch (e) {
      toast(e.message, "err");
    }
  };

  $("#cap-start").onclick = async () => {
    try {
      const body = captureBody();
      if (!body) return;
      $("#cap-msg").textContent = "starting…";
      $("#cap-files").innerHTML = "";
      await api("POST", "/api/session/capture", body);
      $("#cap-msg").textContent = "";
    } catch (e) {
      $("#cap-msg").innerHTML = `<span class="err">${esc(e.message)}</span>`;
    }
  };
  $("#cap-stop").onclick = () => stopTuner().catch((e) => toast(e.message, "err"));

  on("devices", () => fillDevices());
  on("status", renderProgress);
  on("signal", (m) => {
    if (state.status.capturing) $("#cap-metrics").innerHTML = signalMetrics(m.signal);
  });
  on("capture_done", (m) => {
    const msg = `${m.cancelled ? "Capture stopped" : "Capture finished"}: ${m.ok} ok, ${m.failed} failed`;
    $("#cap-msg").textContent = msg;
    toast(msg, m.failed ? "err" : "ok");
  });
}

export async function show() {
  fillDevices();
  fillTpSources();
  fillSatSelect();
  showSatMode(satMode);
  try {
    const c = await api("GET", "/api/config");
    if (!$("#cap-seconds").dataset.touched) $("#cap-seconds").value = c.capture_seconds;
  } catch (e) {
    /* keep default */
  }
  $("#cap-seconds").oninput = () => ($("#cap-seconds").dataset.touched = "1");
  renderProgress();
}
