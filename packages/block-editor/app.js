"use strict";

const catalog = {
  boards: {
    nrf52840: { pwm: 4 },
    rp2350: { pwm: 8 },
    esp32c3: { pwm: 4 },
  },
};

// Trained ML models offered as inference blocks. Seeded with the on-device motion NN
// (kept in lockstep with tools/train_motion_nn.py + the nn-motion-ai firmware adapter);
// refreshed at load from models.json, which train_motion_nn.py writes on every run.
// Each contract mirrors AiModelContract.to_dict() so an ML block emits app.json that the
// host contract tooling accepts unchanged.
const mlModels = {
  nn_motion: {
    label: "Motion NN (idle/active)",
    contract: {
      model_id: 0x4e4e4d31,
      backend: "on_device",
      input_bytes_max: 64,
      output_bytes_max: 4,
      arena_bytes: 256,
      timeout_us: 2000,
      stale_after_us: 100000,
    },
  },
};

async function loadModels() {
  try {
    const res = await fetch("models.json", { cache: "no-store" });
    if (!res.ok) return;
    const cards = await res.json();
    for (const [preset, card] of Object.entries(cards)) {
      if (card && card.contract) {
        mlModels[preset] = { label: card.label || preset, contract: card.contract };
      }
    }
    render();
  } catch (err) {
    // Opened from file:// or served without models.json: keep the built-in seed.
  }
}

const state = {
  blocks: [
    { kind: "actuator", name: "arm", brand: "sg90", channel: 0 },
    { kind: "sensor", name: "imu", brand: "mpu6050", bus: "i2c", address: "0x68" },
    { kind: "behavior", text: "sweep actuator when imu detects a tap" },
  ],
};

const els = {
  projectName: document.getElementById("projectName"),
  boardSelect: document.getElementById("boardSelect"),
  blockList: document.getElementById("blockList"),
  jsonOut: document.getElementById("jsonOut"),
  status: document.getElementById("status"),
  copyBtn: document.getElementById("copyBtn"),
  downloadBtn: document.getElementById("downloadBtn"),
};

function slug(value, fallback) {
  const out = value.toLowerCase().replace(/[^a-z0-9_]+/g, "_").replace(/^_+|_+$/g, "");
  return out || fallback;
}

function nextName(prefix) {
  let i = 1;
  const used = new Set(state.blocks.map((b) => b.name).filter(Boolean));
  while (used.has(`${prefix}${i}`)) i += 1;
  return `${prefix}${i}`;
}

function addBlock(data) {
  if (data.kind === "actuator") {
    state.blocks.push({
      kind: "actuator",
      name: nextName("actuator"),
      brand: data.brand,
      channel: nextChannel(),
    });
  } else if (data.kind === "sensor") {
    state.blocks.push({
      kind: "sensor",
      name: nextName("sensor"),
      brand: data.brand,
      bus: "i2c",
      address: data.brand === "ina3221" ? "0x40" : "0x68",
    });
  } else if (data.kind === "ml") {
    const preset = data.model && mlModels[data.model] ? data.model : "nn_motion";
    state.blocks.push({
      kind: "ml",
      name: nextName("model"),
      model: preset,
      ...mlModels[preset].contract,
    });
  } else {
    state.blocks.push({ kind: "behavior", text: data.text });
  }
  render();
}

function nextChannel() {
  const board = catalog.boards[els.boardSelect.value];
  const used = new Set(state.blocks.filter((b) => b.kind === "actuator").map((b) => b.channel));
  for (let i = 0; i < board.pwm; i += 1) {
    if (!used.has(i)) return i;
  }
  return 0;
}

function move(index, delta) {
  const next = index + delta;
  if (next < 0 || next >= state.blocks.length) return;
  const [block] = state.blocks.splice(index, 1);
  state.blocks.splice(next, 0, block);
  render();
}

function remove(index) {
  state.blocks.splice(index, 1);
  render();
}

function updateBlock(index, patch) {
  state.blocks[index] = { ...state.blocks[index], ...patch };
  render();
}

function blockTitle(block) {
  if (block.kind === "actuator") return `${block.brand} actuator`;
  if (block.kind === "sensor") return `${block.brand} sensor`;
  if (block.kind === "ml") return `${(mlModels[block.model] || {}).label || block.model} model`;
  return "behavior";
}

function blockDetail(block) {
  if (block.kind === "actuator") return `${block.name} on PWM ${block.channel}`;
  if (block.kind === "sensor") return `${block.name} on ${block.bus}${block.address ? ` @ ${block.address}` : ""}`;
  if (block.kind === "ml") {
    return `${block.name}: ${block.backend} ${block.input_bytes_max}\u2192${block.output_bytes_max}B, ${block.timeout_us}us`;
  }
  return block.text;
}

function renderBlocks() {
  els.blockList.innerHTML = "";
  state.blocks.forEach((block, index) => {
    const li = document.createElement("li");
    li.className = "block";
    const content = document.createElement("div");
    content.innerHTML = `<strong>${blockTitle(block)}</strong><small>${blockDetail(block)}</small>`;
    content.addEventListener("click", () => editBlock(index));
    const up = document.createElement("button");
    up.type = "button";
    up.textContent = "\u2191";
    up.setAttribute("aria-label", "Move block up");
    up.addEventListener("click", () => move(index, -1));
    const del = document.createElement("button");
    del.type = "button";
    del.textContent = "\u00d7";
    del.setAttribute("aria-label", "Remove block");
    del.addEventListener("click", () => remove(index));
    li.append(content, up, del);
    els.blockList.appendChild(li);
  });
}

function editBlock(index) {
  const block = state.blocks[index];
  if (block.kind === "behavior") {
    const text = window.prompt("Behavior", block.text);
    if (text) updateBlock(index, { text });
    return;
  }
  const name = window.prompt("Name", block.name);
  if (!name) return;
  if (block.kind === "ml") {
    const stale = Number(window.prompt("Stale-after (us)", String(block.stale_after_us)));
    updateBlock(index, {
      name: slug(name, block.name),
      stale_after_us: Number.isFinite(stale) && stale > 0 ? stale : block.stale_after_us,
    });
    return;
  }
  if (block.kind === "actuator") {
    const channel = Number(window.prompt("PWM channel", String(block.channel)));
    updateBlock(index, { name: slug(name, block.name), channel: Number.isFinite(channel) ? channel : block.channel });
  } else {
    const address = window.prompt("I2C address", block.address || "");
    updateBlock(index, { name: slug(name, block.name), address: address || block.address });
  }
}

function appJson() {
  const actuators = state.blocks
    .filter((b) => b.kind === "actuator")
    .map((b) => ({ name: b.name, brand: b.brand, channel: Number(b.channel) }));
  const sensors = state.blocks
    .filter((b) => b.kind === "sensor")
    .map((b) => ({ name: b.name, brand: b.brand, bus: b.bus || "i2c", address: b.address || "0x68" }));
  const behaviors = state.blocks.filter((b) => b.kind === "behavior").map((b) => b.text);
  // ai_models[] matches AiModelContract.to_dict(): the same shape the host contract bundle
  // and the AI_MODEL boot report consume. Only emitted when an ML block is present.
  const aiModels = state.blocks
    .filter((b) => b.kind === "ml")
    .map((b) => ({
      model_id: b.model_id,
      backend: b.backend,
      input_bytes_max: b.input_bytes_max,
      output_bytes_max: b.output_bytes_max,
      arena_bytes: b.arena_bytes,
      timeout_us: b.timeout_us,
      stale_after_us: b.stale_after_us,
    }));
  const app = {
    name: slug(els.projectName.value, "nobro_app"),
    board: els.boardSelect.value,
    actuators,
    sensors,
    behaviors,
  };
  if (aiModels.length) app.ai_models = aiModels;
  return app;
}

const ML_BACKENDS = new Set(["on_device", "remote_api", "edge_sidecar", "hybrid"]);

function validate(app) {
  const errors = [];
  const board = catalog.boards[app.board];
  const names = new Set();
  for (const item of [...app.actuators, ...app.sensors]) {
    if (names.has(item.name)) errors.push(`duplicate name: ${item.name}`);
    names.add(item.name);
  }
  for (const actuator of app.actuators) {
    if (actuator.channel < 0 || actuator.channel >= board.pwm) {
      errors.push(`${actuator.name}: PWM ${actuator.channel} outside 0..${board.pwm - 1}`);
    }
  }
  for (const model of app.ai_models || []) {
    if (!ML_BACKENDS.has(model.backend)) errors.push(`model ${model.model_id}: bad backend`);
    for (const key of ["model_id", "input_bytes_max", "output_bytes_max", "timeout_us", "stale_after_us"]) {
      if (!(model[key] > 0)) errors.push(`model ${model.model_id}: ${key} must be > 0`);
    }
  }
  return errors;
}

function renderJson() {
  const app = appJson();
  const errors = validate(app);
  els.jsonOut.textContent = JSON.stringify(app, null, 2);
  els.status.textContent = errors.length ? errors.join(" | ") : "Valid app.json";
  els.status.className = errors.length ? "status error" : "status";
}

function render() {
  renderBlocks();
  renderJson();
}

async function copyJson() {
  await navigator.clipboard.writeText(els.jsonOut.textContent);
  els.status.textContent = "Copied app.json";
}

function downloadJson() {
  const blob = new Blob([els.jsonOut.textContent + "\n"], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = "app.json";
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}

document.querySelectorAll(".palette button").forEach((button) => {
  button.addEventListener("click", () => addBlock(button.dataset));
});
els.projectName.addEventListener("input", renderJson);
els.boardSelect.addEventListener("change", renderJson);
els.copyBtn.addEventListener("click", copyJson);
els.downloadBtn.addEventListener("click", downloadJson);

render();
loadModels();
