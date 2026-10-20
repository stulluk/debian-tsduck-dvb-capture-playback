// TS playback page: DTU-315 playout for satellite, terrestrial and cable.
import { api, on } from "./api.js";
import { $, buildFields, esc, fmtSize, intVal, tabs, toast } from "./ui.js";
import { modulators, state } from "./main.js";

let delivery = "satellite";
let files = [];
let refs = [];

const FORMS = {
  satellite: (v) => [
    { id: "pb-freq", label: "L-band IF (MHz, 950–2150)", type: "number", value: v.frequency_mhz ?? 1200 },
    { id: "pb-mod", label: "Modulation", type: "select", value: v.modulation ?? "DVB-S2-8PSK", options: ["DVB-S2-8PSK", "DVB-S2-QPSK", "DVB-S2-16APSK", "DVB-S2-32APSK", "DVB-S-QPSK"] },
    { id: "pb-sr", label: "Symbol rate (kS/s)", type: "number", value: v.symbol_rate_ksps ?? 27500 },
    { id: "pb-cr", label: "Code rate", type: "select", value: v.code_rate ?? "3/4", options: ["1/2", "2/3", "3/4", "3/5", "4/5", "5/6", "7/8", "8/9", "9/10"] },
  ],
  terrestrial: (v) => [
    { id: "pb-freq", label: "RF frequency (MHz)", type: "number", value: v.frequency_mhz ?? 618 },
    { id: "pb-mod", label: "System", type: "select", value: v.modulation ?? "DVB-T2", options: ["DVB-T2", "DVB-T"] },
    { id: "pb-bw", label: "Bandwidth (MHz)", type: "select", value: v.bandwidth_mhz ?? 8, options: [8, 7, 6] },
    { id: "pb-gi", label: "Guard interval", type: "select", value: v.guard_interval ?? "19/256", options: ["19/256", "1/8", "1/16", "1/32", "1/4", "19/128", "1/128"] },
    { id: "pb-plpmod", label: "Constellation", type: "select", value: v.plp_modulation ?? "64-QAM", options: ["64-QAM", "256-QAM", "16-QAM", "QPSK"] },
    { id: "pb-cr", label: "Code rate", type: "select", value: v.code_rate || "2/3", options: ["2/3", "1/2", "3/4", "3/5", "4/5", "5/6", "7/8"] },
  ],
  cable: (v) => [
    { id: "pb-freq", label: "RF frequency (MHz)", type: "number", value: v.frequency_mhz ?? 722 },
    { id: "pb-mod", label: "Modulation", type: "select", value: v.modulation ?? "256-QAM", options: ["256-QAM", "128-QAM", "64-QAM", "32-QAM", "16-QAM"] },
    { id: "pb-sr", label: "Symbol rate (kS/s)", type: "number", value: v.symbol_rate_ksps ?? 6875 },
    { id: "pb-j83", label: "J.83 annex", type: "select", value: v.j83 ?? "A", options: [["A", "A (DVB-C)"], ["B", "B (US)"], ["C", "C (Japan)"]] },
  ],
};

function buildForm(values = {}) {
  buildFields($("#pb-form"), FORMS[delivery](values));
}

function deliveryTabs(active) {
  const mod = modulators()[0];
  const cats = mod ? mod.categories || [] : [];
  const items = [
    { key: "satellite", label: "Satellite" },
    { key: "terrestrial", label: "Terrestrial" },
    { key: "cable", label: "Cable" },
  ].filter((t) => cats.includes(t.key));
  if (!items.length) {
    $("#pb-tabs").innerHTML = '<span class="muted">No modulator connected.</span>';
    $("#pb-form").innerHTML = "";
    return;
  }
  delivery = items.some((t) => t.key === active) ? active : items[0].key;
  tabs($("#pb-tabs"), items, delivery, (key) => {
    delivery = key;
    buildForm(pendingValues || {});
    pendingValues = null;
  });
}
let pendingValues = null;

/** Current form as a playout request body. */
function formBody() {
  const file = $("#pb-path").value.trim() || $("#pb-file").value;
  const body = {
    file,
    delivery,
    frequency_mhz: intVal("#pb-freq"),
    modulation: $("#pb-mod").value,
    infinite: $("#pb-loop").checked,
    level: intVal("#pb-level"),
  };
  if (delivery === "satellite") Object.assign(body, { symbol_rate_ksps: intVal("#pb-sr"), code_rate: $("#pb-cr").value });
  if (delivery === "terrestrial")
    Object.assign(body, {
      bandwidth_mhz: intVal("#pb-bw"),
      guard_interval: $("#pb-gi").value,
      plp_modulation: $("#pb-plpmod").value,
      code_rate: $("#pb-cr").value,
    });
  if (delivery === "cable") Object.assign(body, { symbol_rate_ksps: intVal("#pb-sr"), j83: $("#pb-j83").value });
  return body;
}

/** Fills the whole form from a saved reference / sidecar suggestion. */
function applyValues(v) {
  pendingValues = v;
  deliveryTabs(v.delivery || "satellite");
  if (v.level != null) $("#pb-level").value = v.level;
  if (v.infinite != null) $("#pb-loop").checked = !!v.infinite;
}

async function loadFiles() {
  try {
    files = (await api("GET", "/api/captures")).files || [];
  } catch (e) {
    files = [];
    toast(e.message, "err");
  }
  $("#pb-file").innerHTML =
    '<option value="">— select a capture —</option>' +
    files
      .map((f) => `<option value="${esc(f.file)}">${esc(f.file)} · ${fmtSize(f.size)}${f.sidecar ? " · ✓ params" : ""}</option>`)
      .join("");
}

async function loadRefs() {
  try {
    refs = (await api("GET", "/api/reference-captures")).captures || [];
  } catch (e) {
    refs = [];
  }
  $("#ref-list").innerHTML =
    '<option value="">— none —</option>' +
    refs.map((r) => `<option value="${esc(r.name)}">${esc(r.name)} (${esc(r.delivery || "satellite")})</option>`).join("");
}

async function onFile() {
  const file = $("#pb-path").value.trim() || $("#pb-file").value;
  $("#pb-hint").textContent = "";
  if (!file) return;
  try {
    const r = await api("GET", `/api/playout/suggest?file=${encodeURIComponent(file)}`);
    applyValues(r.form);
    const t = r.tune;
    $("#pb-hint").textContent = `Parameters from the capture sidecar: ${t.delivery_system} ${t.frequency_mhz} MHz ${t.modulation} ${t.fec || ""}${t.satellite ? " · " + t.satellite : ""}`;
  } catch (e) {
    $("#pb-hint").textContent = "No _services.json next to this file — enter the modulation parameters.";
  }
}

function renderStatus() {
  const st = state.status;
  $("#pb-stop").disabled = !st.playout;
  $("#pb-msg").textContent = st.playout ? `playing out ${st.playout_file || ""}` : "";
}

export function init() {
  $("#pb-file").onchange = () => {
    $("#pb-path").value = "";
    onFile();
  };
  $("#pb-path").onchange = onFile;
  $("#pb-play").onclick = async () => {
    const body = formBody();
    if (!body.file) return toast("select a TS file", "err");
    try {
      await api("POST", "/api/session/playout", body);
      toast("Playout started", "ok");
    } catch (e) {
      toast(e.message, "err");
    }
  };
  $("#pb-stop").onclick = () => api("DELETE", "/api/session/playout").catch((e) => toast(e.message, "err"));
  $("#ref-list").onchange = () => {
    const r = refs.find((x) => x.name === $("#ref-list").value);
    if (!r) return;
    $("#pb-path").value = r.file;
    applyValues(r);
    $("#pb-hint").textContent = `Loaded reference capture "${r.name}"`;
  };
  $("#ref-save").onclick = async () => {
    const body = formBody();
    body.name = $("#ref-name").value.trim();
    if (!body.name || !body.file) return toast("name and file are required", "err");
    try {
      await api("POST", "/api/reference-captures", body);
      await loadRefs();
      toast(`Saved reference capture "${body.name}"`, "ok");
    } catch (e) {
      toast(e.message, "err");
    }
  };
  $("#ref-del").onclick = async () => {
    const name = $("#ref-list").value;
    if (!name || !confirm(`Remove reference capture "${name}"?`)) return;
    await api("DELETE", `/api/reference-captures/${encodeURIComponent(name)}`).catch((e) => toast(e.message, "err"));
    loadRefs();
  };
  on("status", renderStatus);
  on("devices", () => {
    if ($("#page-playback").classList.contains("active") && !$("#pb-form").innerHTML) deliveryTabs(delivery);
  });
}

export async function show() {
  deliveryTabs(delivery);
  await Promise.all([loadFiles(), loadRefs()]);
  renderStatus();
}
