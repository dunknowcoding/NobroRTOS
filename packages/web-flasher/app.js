"use strict";

const state = {
  file: null,
  bytes: null,
};

const els = {
  fileInput: document.getElementById("fileInput"),
  dropZone: document.getElementById("dropZone"),
  fileName: document.getElementById("fileName"),
  fileSize: document.getElementById("fileSize"),
  fileCrc: document.getElementById("fileCrc"),
  apiStatus: document.getElementById("apiStatus"),
  serialTouchBtn: document.getElementById("serialTouchBtn"),
  serialCommandBtn: document.getElementById("serialCommandBtn"),
  webUsbBtn: document.getElementById("webUsbBtn"),
  saveBtn: document.getElementById("saveBtn"),
  clearLogBtn: document.getElementById("clearLogBtn"),
  log: document.getElementById("log"),
};

const crcTable = new Uint32Array(256);
for (let i = 0; i < 256; i += 1) {
  let c = i;
  for (let k = 0; k < 8; k += 1) {
    c = (c & 1) ? (0xedb88320 ^ (c >>> 1)) : (c >>> 1);
  }
  crcTable[i] = c >>> 0;
}

function log(kind, title, detail = "") {
  const li = document.createElement("li");
  li.innerHTML = `<strong class="${kind}">${title}</strong>${detail ? ` ${detail}` : ""}`;
  els.log.prepend(li);
}

function formatBytes(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / (1024 * 1024)).toFixed(2)} MiB`;
}

function crc32(bytes) {
  let c = 0xffffffff;
  for (const b of bytes) {
    c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

function updateApiStatus() {
  const serial = "serial" in navigator;
  const usb = "usb" in navigator;
  els.apiStatus.textContent = `${serial ? "Serial" : "No Serial"} / ${usb ? "USB" : "No USB"}`;
  els.serialTouchBtn.disabled = !serial;
  els.serialCommandBtn.disabled = !serial;
  els.webUsbBtn.disabled = !usb || !state.bytes;
  els.saveBtn.disabled = !state.bytes;
}

async function loadFile(file) {
  const buf = await file.arrayBuffer();
  const bytes = new Uint8Array(buf);
  state.file = file;
  state.bytes = bytes;
  els.fileName.textContent = file.name;
  els.fileSize.textContent = formatBytes(bytes.length);
  els.fileCrc.textContent = crc32(bytes).toString(16).toUpperCase().padStart(8, "0");
  updateApiStatus();
  log("ok", "Loaded", `${file.name}, ${formatBytes(bytes.length)}.`);
}

async function serialTouch1200() {
  try {
    const port = await navigator.serial.requestPort();
    await port.open({ baudRate: 1200 });
    await port.close();
    log("ok", "Boot entry", "1200-baud touch completed.");
  } catch (err) {
    log("bad", "Serial failed", err.message || String(err));
  }
}

async function serialCommand() {
  try {
    const port = await navigator.serial.requestPort();
    await port.open({ baudRate: 115200 });
    const writer = port.writable.getWriter();
    await writer.write(new TextEncoder().encode("DFU\n"));
    writer.releaseLock();
    await port.close();
    log("ok", "Boot command", "DFU command sent.");
  } catch (err) {
    log("bad", "Serial failed", err.message || String(err));
  }
}

function firstBulkOutEndpoint(device) {
  const config = device.configuration;
  if (!config) return null;
  for (const iface of config.interfaces) {
    for (const alt of iface.alternates) {
      const endpoint = alt.endpoints.find((ep) => ep.direction === "out" && ep.type === "bulk");
      if (endpoint) return { interfaceNumber: iface.interfaceNumber, alternate: alt.alternateSetting, endpointNumber: endpoint.endpointNumber };
    }
  }
  return null;
}

async function webUsbSend() {
  if (!state.bytes) {
    log("warn", "No firmware", "Drop a firmware image first.");
    return;
  }
  try {
    const device = await navigator.usb.requestDevice({ filters: [] });
    await device.open();
    if (!device.configuration) {
      await device.selectConfiguration(1);
    }
    const target = firstBulkOutEndpoint(device);
    if (!target) {
      await device.close();
      log("warn", "WebUSB paired", "No bulk OUT endpoint was found.");
      return;
    }
    await device.claimInterface(target.interfaceNumber);
    if (target.alternate !== 0) {
      await device.selectAlternateInterface(target.interfaceNumber, target.alternate);
    }
    const chunk = 4096;
    for (let off = 0; off < state.bytes.length; off += chunk) {
      await device.transferOut(target.endpointNumber, state.bytes.slice(off, off + chunk));
    }
    await device.releaseInterface(target.interfaceNumber);
    await device.close();
    log("ok", "WebUSB sent", `${formatBytes(state.bytes.length)} transferred.`);
  } catch (err) {
    log("bad", "WebUSB failed", err.message || String(err));
  }
}

function saveCopy() {
  if (!state.file || !state.bytes) return;
  const blob = new Blob([state.bytes], { type: "application/octet-stream" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = state.file.name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
  log("ok", "Copy ready", "Use the browser download target for UF2 mass-storage bootloaders.");
}

els.fileInput.addEventListener("change", (event) => {
  const file = event.target.files && event.target.files[0];
  if (file) loadFile(file);
});

for (const eventName of ["dragenter", "dragover"]) {
  els.dropZone.addEventListener(eventName, (event) => {
    event.preventDefault();
    els.dropZone.classList.add("drag");
  });
}

for (const eventName of ["dragleave", "drop"]) {
  els.dropZone.addEventListener(eventName, (event) => {
    event.preventDefault();
    els.dropZone.classList.remove("drag");
  });
}

els.dropZone.addEventListener("drop", (event) => {
  const file = event.dataTransfer.files && event.dataTransfer.files[0];
  if (file) loadFile(file);
});

els.serialTouchBtn.addEventListener("click", serialTouch1200);
els.serialCommandBtn.addEventListener("click", serialCommand);
els.webUsbBtn.addEventListener("click", webUsbSend);
els.saveBtn.addEventListener("click", saveCopy);
els.clearLogBtn.addEventListener("click", () => {
  els.log.innerHTML = "";
});

updateApiStatus();
log("warn", "Ready", "Firmware bytes stay inside this browser session.");
