/**
 * Emparelhamento persistente do bridge, de ponta a ponta sobre o
 * processo real: arranca `dist/index.js` com um `--state-dir` temporário e
 * fala-lhe por WebSocket como o tab do 1fluxo faria. Corre com `npm test`
 * depois de `npm run build`.
 */

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHmac, randomBytes } from "node:crypto";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import WebSocket from "ws";

const BIN = fileURLToPath(new URL("../dist/index.js", import.meta.url));
const ORIGIN = "http://localhost:3000";
const procs = new Set();

after(() => {
  for (const p of procs) p.kill("SIGTERM");
});

function hmac(key, data) {
  return createHmac("sha256", key).update(data).digest("base64url");
}

function nonce() {
  return randomBytes(24).toString("base64url");
}

/** Arranca o bridge e espera pela URL de emparelhamento no stderr. */
function startBridge(stateDir, extra = []) {
  return new Promise((resolve, reject) => {
    const proc = spawn(process.execPath, [BIN, "--port", "0", "--app-url", ORIGIN, "--state-dir", stateDir, "--no-open", ...extra], {
      stdio: ["pipe", "pipe", "pipe"],
    });
    procs.add(proc);
    let stderr = "";
    const stdoutLines = [];
    proc.stdout.on("data", (chunk) => {
      for (const line of chunk.toString().split("\n")) if (line.trim()) stdoutLines.push(line);
    });
    const timer = setTimeout(() => reject(new Error(`bridge não arrancou:\n${stderr}`)), 8_000);
    proc.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
      const m = stderr.match(/connect\?port=(\d+)&token=([\w-]+)/);
      if (m) {
        clearTimeout(timer);
        resolve({ proc, port: Number(m[1]), token: m[2], stderr: () => stderr, stdoutLines });
      }
    });
    proc.on("exit", (code) => {
      clearTimeout(timer);
      procs.delete(proc);
      resolve({ proc, exitCode: code, stderr: () => stderr });
    });
  });
}

/** Manda um pedido JSON-RPC ao bridge pelo stdin e espera a resposta com o mesmo id no stdout. */
function rpc(bridge, msg, timeoutMs = 3_000) {
  bridge.proc.stdin.write(JSON.stringify(msg) + "\n");
  return new Promise((resolve, reject) => {
    const started = Date.now();
    const tick = () => {
      for (const line of bridge.stdoutLines) {
        try {
          const parsed = JSON.parse(line);
          if (parsed.id === msg.id) return resolve(parsed);
        } catch {
          // linha que não é JSON — ignora
        }
      }
      if (Date.now() - started > timeoutMs) return reject(new Error(`sem resposta ao pedido ${msg.id}`));
      setTimeout(tick, 25);
    };
    tick();
  });
}

/** Abre um socket, manda o `hello` e devolve a primeira mensagem ou o fecho. */
function handshake(port, hello) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`ws://127.0.0.1:${port}`, { headers: { Origin: ORIGIN } });
    const timer = setTimeout(() => reject(new Error("handshake sem resposta")), 5_000);
    ws.once("open", () => ws.send(JSON.stringify(hello)));
    ws.once("message", (data) => {
      clearTimeout(timer);
      resolve({ ws, message: JSON.parse(data.toString()) });
    });
    ws.once("close", (code) => {
      clearTimeout(timer);
      resolve({ ws, closeCode: code });
    });
    ws.once("error", reject);
  });
}

function closed(ws) {
  return new Promise((resolve) => {
    if (ws.readyState === WebSocket.CLOSED) return resolve();
    ws.once("close", () => resolve());
  });
}

function pairingFile(stateDir) {
  return join(stateDir, "bridge-pairing.json");
}

test("primeiro emparelhamento devolve o segredo e grava-o em disco", async () => {
  const stateDir = mkdtempSync(join(tmpdir(), "1fluxo-bridge-"));
  const bridge = await startBridge(stateDir);
  // O cliente MCP apresenta-se primeiro; o nome tem de chegar ao tab.
  bridge.proc.stdin.write(
    JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "cliente-de-teste", version: "9" } },
    }) + "\n"
  );
  await new Promise((r) => setTimeout(r, 200));
  const n = nonce();
  const { ws, message } = await handshake(bridge.port, { type: "pair", token: bridge.token, nonce: n });
  assert.equal(message.type, "paired");
  assert.deepEqual(message.client, { name: "cliente-de-teste", version: "9" });
  assert.equal(message.proof, hmac(bridge.token, n), "prova do token");
  assert.ok(message.pairing?.id && message.pairing?.secret && message.pairing?.expiresAt);
  assert.ok(existsSync(pairingFile(stateDir)), "ficheiro de emparelhamento");
  const onDisk = JSON.parse(readFileSync(pairingFile(stateDir), "utf8"));
  assert.equal(onDisk.id, message.pairing.id);
  assert.equal(onDisk.secret, message.pairing.secret);
  assert.equal(onDisk.origin, ORIGIN);
  assert.ok(Date.parse(onDisk.expiresAt) - Date.now() > 29 * 24 * 3600 * 1000, "30 dias");
  ws.close(1000);
  await closed(ws);
  bridge.proc.kill("SIGTERM");
});

test("retoma com a prova certa; recusa prova errada com 4011", async () => {
  const stateDir = mkdtempSync(join(tmpdir(), "1fluxo-bridge-"));
  const bridge = await startBridge(stateDir);
  const first = await handshake(bridge.port, { type: "pair", token: bridge.token, nonce: nonce() });
  const { id, secret } = first.message.pairing;
  first.ws.close(1000);
  await closed(first.ws);

  // Prova errada: nem o tab passa, nem recebe nada.
  const bad = await handshake(bridge.port, { type: "resume", pairingId: id, nonce: nonce(), proof: "nope" });
  assert.equal(bad.closeCode, 4011);

  // Prova certa: o bridge prova o mesmo de volta, sem repetir o segredo.
  const n = nonce();
  const good = await handshake(bridge.port, {
    type: "resume",
    pairingId: id,
    nonce: n,
    proof: hmac(secret, `tab:${n}`),
  });
  assert.equal(good.message?.type, "paired");
  assert.equal(good.message.proof, hmac(secret, `bridge:${n}`));
  assert.equal(good.message.pairing.id, id);
  assert.equal(good.message.pairing.secret, undefined, "o segredo não volta a passar no fio");
  good.ws.close(1000);
  await closed(good.ws);
  bridge.proc.kill("SIGTERM");
});

test("retoma depois de reiniciar o bridge, com o ficheiro em disco", async () => {
  const stateDir = mkdtempSync(join(tmpdir(), "1fluxo-bridge-"));
  const first = await startBridge(stateDir);
  const paired = await handshake(first.port, { type: "pair", token: first.token, nonce: nonce() });
  const { id, secret } = paired.message.pairing;
  paired.ws.close(1000);
  await closed(paired.ws);
  first.proc.kill("SIGTERM");

  const second = await startBridge(stateDir);
  assert.match(second.stderr(), /Emparelhamento válido até/);
  const n = nonce();
  const good = await handshake(second.port, { type: "resume", pairingId: id, nonce: n, proof: hmac(secret, `tab:${n}`) });
  assert.equal(good.message?.type, "paired");
  good.ws.close(1000);
  await closed(good.ws);
  second.proc.kill("SIGTERM");
});

test("emparelhamento expirado é recusado com 4010 e apagado", async () => {
  const stateDir = mkdtempSync(join(tmpdir(), "1fluxo-bridge-"));
  const bridge = await startBridge(stateDir);
  const paired = await handshake(bridge.port, { type: "pair", token: bridge.token, nonce: nonce() });
  const { id, secret } = paired.message.pairing;
  paired.ws.close(1000);
  await closed(paired.ws);
  bridge.proc.kill("SIGTERM");

  // Simula a passagem do tempo no ficheiro e arranca de novo: o arranque
  // apaga o que expirou, e a retoma só pode falhar como «desconhecido».
  const onDisk = JSON.parse(readFileSync(pairingFile(stateDir), "utf8"));
  onDisk.expiresAt = new Date(Date.now() - 60_000).toISOString();
  writeFileSync(pairingFile(stateDir), JSON.stringify(onDisk));
  const second = await startBridge(stateDir);
  assert.match(second.stderr(), /expirou; apagado/);
  assert.ok(!existsSync(pairingFile(stateDir)));
  const n = nonce();
  const res = await handshake(second.port, { type: "resume", pairingId: id, nonce: n, proof: hmac(secret, `tab:${n}`) });
  assert.equal(res.closeCode, 4011);
  second.proc.kill("SIGTERM");
});

test("«forget» do tab apaga o emparelhamento; --forget idem", async () => {
  const stateDir = mkdtempSync(join(tmpdir(), "1fluxo-bridge-"));
  const bridge = await startBridge(stateDir);
  const paired = await handshake(bridge.port, { type: "pair", token: bridge.token, nonce: nonce() });
  assert.ok(existsSync(pairingFile(stateDir)));
  paired.ws.send(JSON.stringify({ type: "forget" }));
  await new Promise((r) => setTimeout(r, 300));
  assert.ok(!existsSync(pairingFile(stateDir)), "ficheiro apagado pelo forget");
  paired.ws.close(1000);
  await closed(paired.ws);
  bridge.proc.kill("SIGTERM");

  // --forget sobre um ficheiro novo.
  writeFileSync(pairingFile(stateDir), JSON.stringify({ id: "x", secret: "y".repeat(40), origin: ORIGIN, createdAt: new Date().toISOString(), expiresAt: new Date(Date.now() + 1000).toISOString() }));
  const forget = await startBridge(stateDir, ["--forget"]);
  assert.equal(forget.exitCode, 0);
  assert.ok(!existsSync(pairingFile(stateDir)));
});

test("sem tab, initialize e tools/list respondem localmente com as tools embebidas", async () => {
  const stateDir = mkdtempSync(join(tmpdir(), "1fluxo-bridge-"));
  const bridge = await startBridge(stateDir);
  const init = await rpc(bridge, {
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "teste", version: "1" } },
  });
  assert.equal(init.result?.serverInfo?.name, "1fluxo");
  assert.match(init.result.instructions, /connect\?port=/);
  const list = await rpc(bridge, { jsonrpc: "2.0", id: 2, method: "tools/list" });
  const names = list.result.tools.map((t) => t.name);
  assert.ok(names.includes("list_months") && names.includes("search_movements"), names.join(","));
  assert.equal(names.length, 7);
  bridge.proc.kill("SIGTERM");
});
