// DVB devices page: one card per physical device, frontends listed inside.
import { on } from "./api.js";
import { $, esc } from "./ui.js";
import { state } from "./main.js";

function render() {
  const groups = new Map();
  for (const d of state.devices) {
    const g = groups.get(d.label) || { ...d, frontends: [] };
    g.frontends.push(d);
    groups.set(d.label, g);
  }
  const cards = [...groups.values()].map((g) => {
    const fes = g.frontends
      .map(
        (f) => `<div class="fe"><span class="mono">${esc(f.address)}</span> · ${esc(f.model)}<br>
          <span class="muted">${esc((f.delivery_systems || []).join(", ") || "—")}</span></div>`,
      )
      .join("");
    const role = g.kind === "modulator" ? "Modulator (playout)" : "Tuner (capture / watch)";
    return `<div class="device ${g.kind}">
      <h4>${esc(g.label)}</h4>
      <div><span class="pill">${role}</span> <span class="pill">${esc(g.backend)}</span>
        ${g.vid_pid ? `<span class="pill mono">USB ${esc(g.vid_pid)}</span>` : ""}
        ${(g.categories || []).map((c) => `<span class="pill ok">${esc(c)}</span>`).join(" ")}</div>
      ${fes}
    </div>`;
  });
  $("#device-cards").innerHTML = cards.join("") || '<p class="muted">No devices detected.</p>';
  $("#device-warnings").innerHTML = state.warnings
    .map((w) => `<p class="err small">⚠ ${esc(w)}</p>`)
    .join("");
}

export function init() {
  on("devices", render);
}

export function show() {
  render();
}
