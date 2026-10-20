// Watch TV page: channel list + HLS player, and the channel scanner.
import { api, on } from "./api.js";
import { $, $$, esc, numberList, signalMetrics, tabs, toast } from "./ui.js";
import { loadChannels, state, stopTuner, tuners } from "./main.js";

let listDelivery = "satellite";
let scanDelivery = "satellite";
let hls = null;
let pending = null; // { serviceId, t0 } while waiting for the first frame
let activeChannel = null;

const LABEL = { satellite: "DVB-S", terrestrial: "DVB-T", cable: "DVB-C" };

// ---- channel list ---------------------------------------------------------------

function renderShell() {
  const has = state.channels.length > 0;
  const scanning = !!state.status.scanning;
  $("#watch-empty").classList.toggle("hidden", has || scanning || !$("#watch-main").classList.contains("hidden"));
  if (has || scanning) $("#watch-main").classList.remove("hidden");
  if (!has && $("#watch-main").classList.contains("hidden")) $("#watch-empty").classList.remove("hidden");
}

function renderChannelTabs() {
  const counts = { satellite: 0, terrestrial: 0, cable: 0 };
  state.channels.forEach((c) => (counts[c.delivery] += 1));
  const items = Object.keys(counts)
    .filter((k) => counts[k] > 0)
    .map((k) => ({ key: k, label: `${LABEL[k]} (${counts[k]})` }));
  if (!items.length) {
    $("#ch-tabs").innerHTML = "";
    $("#ch-list").innerHTML = '<p class="muted" style="padding:10px">No channels.</p>';
    return;
  }
  if (!items.some((t) => t.key === listDelivery)) listDelivery = items[0].key;
  tabs($("#ch-tabs"), items, listDelivery, (k) => {
    listDelivery = k;
    renderChannelList();
  });
}

function groupKey(c) {
  if (c.delivery === "satellite") return c.satellite || "Satellite (unknown — old scan)";
  return c.delivery === "terrestrial" ? "Terrestrial" : "Cable";
}

function renderChannelList() {
  const q = $("#ch-filter").value.trim().toLowerCase();
  const list = state.channels
    .filter((c) => c.delivery === listDelivery)
    .filter((c) => !q || c.name.toLowerCase().includes(q) || (c.provider || "").toLowerCase().includes(q))
    .sort((a, b) => a.scan_index - b.scan_index);
  let lastGroup = null;
  const html = [];
  for (const c of list) {
    const g = groupKey(c);
    if (g !== lastGroup) {
      html.push(`<div class="grp">${esc(g)}</div>`);
      lastGroup = g;
    }
    const tp =
      c.delivery === "satellite"
        ? `${c.frequency_mhz} ${(c.polarity || "?")[0].toUpperCase()} ${c.symbol_rate_ksps || ""}`
        : `${c.frequency_mhz} MHz ${c.delivery_system || ""}`;
    html.push(`<button class="ch${activeChannel === c.id ? " active" : ""}" data-id="${c.id}">
      <div class="n">${esc(c.name)}</div><div class="m">${esc(c.provider || "—")} · ${esc(tp)} · sid ${c.service_id}</div></button>`);
  }
  $("#ch-list").innerHTML = html.join("") || '<p class="muted" style="padding:10px">No match.</p>';
  $("#ch-count").textContent = `${list.length} channel(s)`;
  $$("#ch-list .ch").forEach((b) => (b.onclick = () => play(+b.dataset.id)));
}

// ---- playback ----------------------------------------------------------------------

function resetPlayer() {
  if (hls) {
    hls.destroy();
    hls = null;
  }
  const v = $("#player");
  v.pause();
  v.removeAttribute("src");
  v.load();
}

async function play(id) {
  const ch = state.channels.find((c) => c.id === id);
  if (!ch) return;
  activeChannel = id;
  renderChannelList();
  resetPlayer();
  pending = { serviceId: ch.service_id, t0: performance.now(), name: ch.name };
  $("#play-msg").textContent = `Tuning ${ch.name}…`;
  try {
    await api("POST", "/api/session/play", { channel_id: id });
  } catch (e) {
    pending = null;
    $("#play-msg").innerHTML = `<span class="err">${esc(e.message)}</span>`;
  }
}

function startHls(url) {
  const v = $("#player");
  resetPlayer();
  if (window.Hls && window.Hls.isSupported()) {
    hls = new window.Hls({
      liveSyncDurationCount: 2,
      maxBufferLength: 10,
      backBufferLength: 10,
      manifestLoadingMaxRetry: 10,
      levelLoadingMaxRetry: 10,
    });
    hls.on(window.Hls.Events.ERROR, (_, d) => {
      if (!d.fatal) return;
      if (d.type === window.Hls.ErrorTypes.NETWORK_ERROR) hls.startLoad();
      else if (d.type === window.Hls.ErrorTypes.MEDIA_ERROR) hls.recoverMediaError();
      else $("#play-msg").innerHTML = `<span class="err">player error: ${esc(d.details)}</span>`;
    });
    hls.loadSource(url);
    hls.attachMedia(v);
  } else {
    v.src = url; // Safari: native HLS
  }
  v.play().catch(() => {
    v.muted = true; // autoplay policy: retry muted
    v.play().catch(() => {});
  });
}

// ---- scan ----------------------------------------------------------------------------

function availableScanDeliveries() {
  const cats = new Set(tuners().flatMap((d) => d.categories || []));
  return ["satellite", "terrestrial", "cable"].filter((c) => cats.has(c));
}

function renderScanTabs() {
  const items = availableScanDeliveries().map((k) => ({ key: k, label: LABEL[k] }));
  if (!items.length) {
    $("#scan-tabs").innerHTML = '<span class="muted">No tuner connected.</span>';
    return;
  }
  if (!items.some((t) => t.key === scanDelivery)) scanDelivery = items[0].key;
  tabs($("#scan-tabs"), items, scanDelivery, (k) => {
    scanDelivery = k;
    $("#scan-sat").classList.toggle("hidden", k !== "satellite");
    $("#scan-ter").classList.toggle("hidden", k !== "terrestrial");
    $("#scan-cab").classList.toggle("hidden", k !== "cable");
    const devs = tuners().filter((d) => (d.categories || []).includes(k));
    $("#scan-device").innerHTML = devs
      .map((d) => `<option value="${esc(d.address)}">${esc(d.label)} @ ${esc(d.address)}</option>`)
      .join("");
  });
  $("#scan-sat-name").innerHTML = state.satellites
    .map((s) => `<option value="${esc(s.name)}">${esc(s.name)} (${s.transponders.length} TP)</option>`)
    .join("");
}

function scanBody() {
  const body = {
    delivery: scanDelivery,
    device_name: $("#scan-device").value || null,
    include_radio: $("#scan-radio").checked,
    include_scrambled: $("#scan-scrambled").checked,
    replace: $("#scan-replace").checked,
    save: true,
  };
  if (scanDelivery === "satellite") {
    body.satellite = $("#scan-sat-name").value;
    body.tps = $("#scan-tps").value.split(/[\s,;]+/).filter(Boolean);
  } else if (scanDelivery === "terrestrial") {
    body.frequencies_mhz = numberList($("#scan-ter-freqs").value);
    body.bandwidth_mhz = parseInt($("#scan-ter-bw").value, 10);
  } else {
    body.frequencies_mhz = numberList($("#scan-cab-freqs").value);
    body.symbol_rate_ksps = parseInt($("#scan-cab-sr").value, 10);
    body.modulation = $("#scan-cab-mod").value;
  }
  return body;
}

function renderScanStatus() {
  const st = state.status;
  $("#scan-stop").disabled = !st.scanning;
  $("#scan-start").disabled = !!st.scanning;
  if (st.scanning && st.scan) {
    const s = st.scan;
    $("#scan-bar").style.width = `${s.total ? (100 * Math.max(0, s.index - 1)) / s.total : 0}%`;
    $("#scan-msg").textContent = `${s.index}/${s.total} ${s.current} · ${s.locked} locked · ${s.found} channel(s)`;
  }
}

function showTab(key) {
  $$("#watch-tabs .tab").forEach((t) => t.classList.toggle("active", t.dataset.wt === key));
  $("#watch-pane-watch").classList.toggle("hidden", key !== "watch");
  $("#watch-pane-scan").classList.toggle("hidden", key !== "scan");
  if (key === "scan") renderScanTabs();
}

export function init() {
  $$("#watch-tabs .tab").forEach((t) => (t.onclick = () => showTab(t.dataset.wt)));
  $("#watch-start-scan").onclick = () => {
    $("#watch-empty").classList.add("hidden");
    $("#watch-main").classList.remove("hidden");
    showTab("scan");
  };
  $("#ch-filter").oninput = renderChannelList;
  $("#play-stop").onclick = () => {
    pending = null;
    activeChannel = null;
    resetPlayer();
    renderChannelList();
    stopTuner().catch((e) => toast(e.message, "err"));
  };
  $("#play-quality").onchange = async () => {
    const quality = $("#play-quality").value;
    try {
      await api("POST", "/api/config", { web_quality: quality });
      toast(`Browser quality: ${quality}`, "ok");
      // Restart the running channel so the new quality takes effect now.
      if (state.status.playing && activeChannel != null) play(activeChannel);
    } catch (e) {
      toast(e.message, "err");
    }
  };
  $("#scan-start").onclick = async () => {
    $("#scan-rows").innerHTML = "";
    $("#scan-bar").style.width = "0";
    try {
      await api("POST", "/api/session/scan", scanBody());
    } catch (e) {
      $("#scan-msg").innerHTML = `<span class="err">${esc(e.message)}</span>`;
    }
  };
  $("#scan-stop").onclick = () => stopTuner().catch((e) => toast(e.message, "err"));
  $("#scan-clear").onclick = async () => {
    const sat = scanDelivery === "satellite" ? $("#scan-sat-name").value : null;
    const what = sat ? `${LABEL[scanDelivery]} channels of ${sat}` : `all ${LABEL[scanDelivery]} channels`;
    if (!confirm(`Delete ${what}?`)) return;
    const q = `delivery=${scanDelivery}${sat ? `&satellite=${encodeURIComponent(sat)}` : ""}`;
    try {
      const r = await api("DELETE", `/api/channels?${q}`);
      toast(`Deleted ${r.removed} channel(s)`, "ok");
      await loadChannels();
      renderChannelTabs();
    } catch (e) {
      toast(e.message, "err");
    }
  };

  const v = $("#player");
  v.addEventListener("playing", () => {
    if (!pending) return;
    const secs = ((performance.now() - pending.t0) / 1000).toFixed(1);
    $("#play-msg").textContent = `Playing ${pending.name} — first frame after ${secs} s`;
    pending = null;
  });

  on("status", (m) => {
    const st = m.status || {};
    $("#play-stop").disabled = !st.playing;
    $("#play-metrics").innerHTML = st.playing ? signalMetrics(st.signal) : "";
    if (pending && st.play && !st.play.ready) $("#play-msg").textContent = `Tuning ${pending.name}… (waiting for the first video segment)`;
    renderScanStatus();
  });
  on("signal", (m) => {
    if (state.status.playing) $("#play-metrics").innerHTML = signalMetrics(m.signal);
  });
  on("play_ready", (m) => {
    if (pending && m.service_id === pending.serviceId) startHls(m.hls_url);
  });
  on("scan_target", (m) => {
    const r = m.result;
    const kept = m.kept;
    $("#scan-rows").insertAdjacentHTML(
      "afterbegin",
      `<tr><td>${r.index + 1}/${m.total}</td><td class="mono">${esc(r.label)}</td>
        <td>${r.locked ? `<span class="pill ok">${esc(r.delivery_system || "lock")}</span>` : '<span class="pill">no lock</span>'}</td>
        <td>${r.snr_db != null ? r.snr_db.toFixed(1) + " dB" : "—"}</td>
        <td>${(r.services || []).length}</td><td>${r.locked ? kept : ""}</td></tr>`,
    );
  });
  on("scan_done", async (m) => {
    $("#scan-bar").style.width = "100%";
    const msg = `Scan ${m.cancelled ? "stopped" : "finished"}: ${m.locked}/${m.total} locked, ${m.found} channel(s), ${m.added} new`;
    $("#scan-msg").textContent = msg;
    toast(msg, "ok");
    await loadChannels().catch(() => {});
    renderChannelTabs();
    renderShell();
  });
}

export async function show() {
  api("GET", "/api/config")
    .then((c) => ($("#play-quality").value = c.web_quality || "720p"))
    .catch(() => {});
  try {
    await loadChannels();
  } catch (e) {
    toast(e.message, "err");
  }
  renderShell();
  renderChannelTabs();
  renderScanStatus();
}
