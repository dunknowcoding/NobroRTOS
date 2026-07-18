"use strict";

const MAX_TASKS = 8;
const MAX_WIRES = 8;
const NAME = /^[a-z][a-z0-9_-]{0,47}$/;
const ROLES = new Set(["periodic", "control", "service"]);

const state = {
  tasks: [
    { name: "imu", role: "periodic", period_us: 10000 },
    { name: "control", role: "control", period_us: 20000 },
  ],
  wires: [{ from: "imu", to: "control", capacity: 8 }],
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
  const out = value.toLowerCase().replace(/[^a-z0-9_-]+/g, "_").replace(/^_+|_+$/g, "");
  return out || fallback;
}

function nextName(prefix) {
  let index = 1;
  const used = new Set(state.tasks.map((task) => task.name));
  while (used.has(`${prefix}${index}`)) index += 1;
  return `${prefix}${index}`;
}

function defaults(task) {
  const period = Number(task.period_us);
  return {
    name: task.name,
    role: task.role,
    period_us: period,
    phase_us: 0,
    deadline_us: period,
    budget_us: Math.max(1, Math.floor(period / 10)),
    blocking_us: 0,
    flash_bytes: 1024,
    ram_bytes: 256,
  };
}

function addTask(role) {
  if (state.tasks.length >= MAX_TASKS) return;
  state.tasks.push({
    name: nextName(role === "periodic" ? "task" : role),
    role,
    period_us: role === "service" ? 100000 : 20000,
  });
  render();
}

function addWire() {
  if (state.wires.length >= MAX_WIRES || state.tasks.length < 2) return;
  const from = window.prompt("Wire from task", state.tasks[0].name);
  if (!from) return;
  const to = window.prompt("Wire to task", state.tasks[1].name);
  if (!to) return;
  state.wires.push({ from: slug(from, from), to: slug(to, to), capacity: 1 });
  render();
}

function move(kind, index, delta) {
  const values = kind === "task" ? state.tasks : state.wires;
  const next = index + delta;
  if (next < 0 || next >= values.length) return;
  const [item] = values.splice(index, 1);
  values.splice(next, 0, item);
  render();
}

function remove(kind, index) {
  const values = kind === "task" ? state.tasks : state.wires;
  values.splice(index, 1);
  render();
}

function editTask(index) {
  const task = state.tasks[index];
  const name = window.prompt("Task name", task.name);
  if (!name) return;
  const period = Number(window.prompt("Period (microseconds)", String(task.period_us)));
  const role = window.prompt("Role: periodic, control, or service", task.role);
  state.tasks[index] = {
    name: slug(name, task.name),
    role: ROLES.has(role) ? role : task.role,
    period_us: Number.isInteger(period) && period > 0 ? period : task.period_us,
  };
  render();
}

function editWire(index) {
  const wire = state.wires[index];
  const from = window.prompt("Wire from task", wire.from);
  if (!from) return;
  const to = window.prompt("Wire to task", wire.to);
  if (!to) return;
  const capacity = Number(window.prompt("Capacity (1..64)", String(wire.capacity)));
  state.wires[index] = {
    from: slug(from, wire.from),
    to: slug(to, wire.to),
    capacity: Number.isInteger(capacity) ? capacity : wire.capacity,
  };
  render();
}

function row(kind, item, index) {
  const li = document.createElement("li");
  li.className = "block";
  const content = document.createElement("div");
  if (kind === "task") {
    content.innerHTML = `<strong>task ${item.name}</strong><small>${item.role}, every ${item.period_us} us</small>`;
    content.addEventListener("click", () => editTask(index));
  } else {
    content.innerHTML = `<strong>wire ${item.from} \u2192 ${item.to}</strong><small>capacity ${item.capacity}</small>`;
    content.addEventListener("click", () => editWire(index));
  }
  const up = document.createElement("button");
  up.type = "button";
  up.textContent = "\u2191";
  up.setAttribute("aria-label", `Move ${kind} up`);
  up.addEventListener("click", () => move(kind, index, -1));
  const del = document.createElement("button");
  del.type = "button";
  del.textContent = "\u00d7";
  del.setAttribute("aria-label", `Remove ${kind}`);
  del.addEventListener("click", () => remove(kind, index));
  li.append(content, up, del);
  return li;
}

function renderBlocks() {
  els.blockList.innerHTML = "";
  state.tasks.forEach((task, index) => els.blockList.appendChild(row("task", task, index)));
  state.wires.forEach((wire, index) => els.blockList.appendChild(row("wire", wire, index)));
}

function appJson() {
  return {
    schema: "nobro-app-v1",
    app: slug(els.projectName.value, "nobro_app"),
    board: els.boardSelect.value,
    tasks: state.tasks.map(defaults),
    wires: state.wires.map((wire) => ({
      from: wire.from,
      to: wire.to,
      capacity: Number(wire.capacity),
    })),
  };
}

function validate(app) {
  const errors = [];
  const names = new Set();
  if (app.tasks.length === 0) errors.push("at least one task is required");
  if (app.tasks.length > MAX_TASKS) errors.push(`task capacity exceeds ${MAX_TASKS}`);
  for (const task of app.tasks) {
    if (!NAME.test(task.name)) errors.push(`invalid task name: ${task.name}`);
    else if (names.has(task.name)) errors.push(`duplicate task: ${task.name}`);
    names.add(task.name);
    if (!ROLES.has(task.role)) errors.push(`unsupported role: ${task.role}`);
    if (!Number.isInteger(task.period_us) || task.period_us <= 0) {
      errors.push(`${task.name}: period_us must be positive`);
    }
  }
  if (app.wires.length > MAX_WIRES) errors.push(`wire count exceeds ${MAX_WIRES}`);
  const edges = new Set();
  for (const wire of app.wires) {
    if (!Number.isInteger(wire.capacity) || wire.capacity < 1 || wire.capacity > 64) {
      errors.push(`${wire.from}->${wire.to}: capacity must be 1..64`);
    } else if (wire.from === wire.to) {
      errors.push("a task cannot wire to itself");
    } else if (edges.has(`${wire.from}\u0000${wire.to}`)) {
      errors.push(`duplicate wire: ${wire.from}->${wire.to}`);
    } else if (!names.has(wire.from)) {
      errors.push(`wire source references unknown task: ${wire.from}`);
    } else if (!names.has(wire.to)) {
      errors.push(`wire destination references unknown task: ${wire.to}`);
    }
    edges.add(`${wire.from}\u0000${wire.to}`);
  }
  return errors;
}

function renderJson() {
  const app = appJson();
  const errors = validate(app);
  els.jsonOut.textContent = JSON.stringify(app, null, 2);
  els.status.textContent = errors.length ? errors[0] : "Valid task/wire app.json";
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
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = "app.json";
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}

document.querySelectorAll(".palette button[data-role]").forEach((button) => {
  button.addEventListener("click", () => addTask(button.dataset.role));
});
document.getElementById("addWire").addEventListener("click", addWire);
els.projectName.addEventListener("input", renderJson);
els.boardSelect.addEventListener("change", renderJson);
els.copyBtn.addEventListener("click", copyJson);
els.downloadBtn.addEventListener("click", downloadJson);

render();
