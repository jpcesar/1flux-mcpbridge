#!/usr/bin/env node
/**
 * @1flux/bridge — ponte stdio ↔ WebSocket local.
 *
 * Lançado por um cliente MCP (Claude Code) via stdio. Abre um servidor
 * WebSocket em 127.0.0.1 e espera que o 1fluxo aberto no browser se ligue a
 * ele com um token de emparelhamento. A partir daí é um cano: cada linha
 * JSON-RPC do stdin segue para o tab; cada mensagem do tab sai no stdout.
 *
 * O que este processo NÃO faz, de propósito:
 *  - não interpreta tools nem lê o conteúdo das mensagens — só `id` e
 *    `method`, para saber a quem responder quando o tab desaparece;
 *  - não fala com a rede: a única ligação que aceita é do loopback;
 *  - não persiste nada nem escreve conteúdo em logs (o stderr só tem eventos
 *    de ligação, nunca pedidos ou respostas).
 *
 * Zero-knowledge é face ao 1fluxo: a decifragem acontece no tab, com a sessão
 * e a chave que já lá estão. Este processo vê o plaintext a passar, mas corre
 * na máquina do utilizador, ao lado do cliente MCP que o vai receber de
 * qualquer maneira.
 *
 * Duas excepções ao "cano burro", ambas só quando NÃO há tab ligado:
 *  - `initialize` é respondido localmente, senão o cliente dava o servidor
 *    como morto ao arrancar (o Claude Code lança o processo e pergunta logo).
 *    A resposta traz a URL de emparelhamento em `instructions`, para o modelo
 *    saber dizer ao utilizador o que fazer.
 *  - `ping` é respondido localmente pelo mesmo motivo.
 * Tudo o resto espera até NO_TAB_WAIT_MS por um tab e depois falha com um
 * erro JSON-RPC legível — nunca fica pendurado.
 */

import { createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { createInterface } from "node:readline";
import { WebSocketServer, type WebSocket } from "ws";

const VERSION = "0.1.0";
const DEFAULT_PORT = 17337;
const DEFAULT_APP_URL = "http://localhost:3000";
/** Origens do web app que podem emparelhar. `--origin` acrescenta outras; a
 *  origem de `--app-url` entra sempre. */
const DEFAULT_ORIGINS = [
  "https://1fluxo.com",
  "https://www.1fluxo.com",
];
/** Tempo que o tab tem para mandar o token depois de abrir o socket. */
const PAIR_TIMEOUT_MS = 5_000;
/** Tempo que um pedido espera por um tab antes de falhar com erro legível. */
const NO_TAB_WAIT_MS = 10_000;
/** Versões MCP conhecidas — a mais recente primeiro. A resposta local ao
 *  `initialize` devolve a pedida se estiver aqui, senão a mais recente.
 *  Espelha `src/lib/mcp/protocol.ts` na web app. */
const PROTOCOL_VERSIONS = ["2025-06-18", "2025-03-26", "2024-11-05"];

/** Código JSON-RPC (gama "server error") para "sem tab ligado". */
const ERR_NO_TAB = -32000;

type JsonRpcId = string | number;
type JsonRpcMessage = {
  jsonrpc?: string;
  id?: JsonRpcId | null;
  method?: string;
  params?: unknown;
  result?: unknown;
  error?: unknown;
};

/* ── argumentos ─────────────────────────────────────────────────────────── */

type Args = { port: number; appUrl: string; origins: string[] };

function usage(): string {
  return [
    "Uso: 1flux-bridge [--port N] [--app-url URL] [--origin URL]...",
    "",
    `  --port N        porta do WebSocket local (defeito ${DEFAULT_PORT}; se ocupada escolhe outra)`,
    `  --app-url URL   onde o 1fluxo está a correr (defeito ${DEFAULT_APP_URL})`,
    "  --origin URL    origem adicional autorizada a emparelhar (repetível)",
    "",
    "Lançado por um cliente MCP via stdio, p. ex.:",
    "  claude mcp add 1flux -- node <caminho>/packages/bridge/dist/index.js",
  ].join("\n");
}

function parseArgs(argv: string[]): Args {
  const args: Args = { port: DEFAULT_PORT, appUrl: DEFAULT_APP_URL, origins: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => {
      const v = argv[++i];
      if (v === undefined) throw new Error(`${a} precisa de um valor`);
      return v;
    };
    if (a === "--help" || a === "-h") {
      process.stderr.write(usage() + "\n");
      process.exit(0);
    } else if (a === "--port") {
      const n = Number(next());
      if (!Number.isInteger(n) || n < 0 || n > 65535) throw new Error("--port inválida");
      args.port = n;
    } else if (a === "--app-url") {
      args.appUrl = new URL(next()).toString().replace(/\/$/, "");
    } else if (a === "--origin") {
      args.origins.push(new URL(next()).origin);
    } else {
      throw new Error(`argumento desconhecido: ${a}\n\n${usage()}`);
    }
  }
  return args;
}

/* ── estado ─────────────────────────────────────────────────────────────── */

const args = parseArgs(process.argv.slice(2));
const allowedOrigins = new Set<string>([
  ...DEFAULT_ORIGINS,
  new URL(args.appUrl).origin,
  ...args.origins,
]);
let token = randomBytes(32).toString("base64url");
/** O token serve um emparelhamento; depois disso só um token novo entra
 *. Renova-se quando o tab se desliga. */
let tokenUsed = false;
let pairPort = 0;
/** Ligações ainda por emparelhar; sem tecto qualquer processo local enchia
 *  a fila. */
let pendingConnections = 0;
const MAX_PENDING = 3;

let tab: WebSocket | null = null;
/** O cliente já completou o handshake MCP — só depois disso se pode mandar
 *  `notifications/tools/list_changed`. */
let clientInitialized = false;
let pairUrl = "";

/** Pedidos já enviados ao tab e ainda sem resposta. */
const pending = new Map<string, { id: JsonRpcId; method: string }>();
/** Pedidos à espera de um tab. */
type Queued = { line: string; msg: JsonRpcMessage; timer: NodeJS.Timeout };
const queue: Queued[] = [];

/* ── utilitários ────────────────────────────────────────────────────────── */

function log(message: string): void {
  process.stderr.write(`[1flux-bridge] ${message}\n`);
}

function writeOut(msg: object): void {
  // Uma mensagem por linha; JSON.stringify nunca emite newlines cruas.
  process.stdout.write(JSON.stringify(msg) + "\n");
}

function hasId(msg: JsonRpcMessage): msg is JsonRpcMessage & { id: JsonRpcId } {
  return typeof msg.id === "string" || typeof msg.id === "number";
}

function idKey(id: JsonRpcId): string {
  return typeof id === "number" ? `n:${id}` : `s:${id}`;
}

function replyError(id: JsonRpcId, code: number, message: string): void {
  writeOut({ jsonrpc: "2.0", id, error: { code, message } });
}

function noTabMessage(): string {
  return `O 1fluxo não está aberto no browser. Abre ${pairUrl} para ligar.`;
}

function rotateToken(): void {
  token = randomBytes(32).toString("base64url");
  tokenUsed = false;
  pairUrl = `${args.appUrl}/connect?port=${pairPort}&token=${token}`;
}

/** Prova de que quem responde "paired" conhece o token: HMAC do nonce que o
 *  tab escolheu. O tab verifica antes de confiar. */
function pairingProof(nonce: string): string {
  return createHmac("sha256", token).update(nonce).digest("base64url");
}

function tokenMatches(candidate: unknown): boolean {
  if (tokenUsed) return false;
  if (typeof candidate !== "string") return false;
  const a = Buffer.from(candidate, "utf8");
  const b = Buffer.from(token, "utf8");
  return a.length === b.length && timingSafeEqual(a, b);
}

/* ── lado do cliente MCP (stdin/stdout) ─────────────────────────────────── */

function localInitialize(msg: JsonRpcMessage & { id: JsonRpcId }): object {
  const params = (msg.params ?? {}) as { protocolVersion?: unknown };
  const requested =
    typeof params.protocolVersion === "string" ? params.protocolVersion : "";
  const protocolVersion = PROTOCOL_VERSIONS.includes(requested)
    ? requested
    : PROTOCOL_VERSIONS[0];
  return {
    jsonrpc: "2.0",
    id: msg.id,
    result: {
      protocolVersion,
      capabilities: { tools: { listChanged: true } },
      serverInfo: { name: "1fluxo", version: VERSION },
      instructions:
        "As tools do 1fluxo só ficam disponíveis depois de o utilizador abrir o " +
        `1fluxo no browser e autorizar a ligação em ${pairUrl} — se uma tool falhar ` +
        "com esse aviso, pede-lhe que abra essa página.",
    },
  };
}

function forwardToTab(msg: JsonRpcMessage, line: string): void {
  if (!tab) return;
  if (hasId(msg) && typeof msg.method === "string") {
    pending.set(idKey(msg.id), { id: msg.id, method: msg.method });
  }
  tab.send(line);
}

function handleClientMessage(msg: JsonRpcMessage, line: string): void {
  if (msg.method === "notifications/initialized") {
    clientInitialized = true;
    if (tab) tab.send(line);
    return;
  }

  if (tab) {
    forwardToTab(msg, line);
    return;
  }

  // Sem tab: o handshake responde-se aqui para o cliente não dar o servidor
  // como morto; o resto espera um bocado e depois falha com erro legível.
  if (hasId(msg) && msg.method === "initialize") {
    writeOut(localInitialize(msg));
    return;
  }
  if (hasId(msg) && msg.method === "ping") {
    writeOut({ jsonrpc: "2.0", id: msg.id, result: {} });
    return;
  }
  // Notificações sem destino não têm a quem responder — descartam-se.
  if (!hasId(msg)) return;

  const entry: Queued = {
    line,
    msg,
    timer: setTimeout(() => {
      const i = queue.indexOf(entry);
      if (i >= 0) queue.splice(i, 1);
      replyError(msg.id, ERR_NO_TAB, noTabMessage());
    }, NO_TAB_WAIT_MS),
  };
  queue.push(entry);
}

/* ── lado do tab (WebSocket) ────────────────────────────────────────────── */

function onTabMessage(raw: string): void {
  let msg: JsonRpcMessage;
  try {
    msg = JSON.parse(raw) as JsonRpcMessage;
  } catch {
    return; // não é JSON — nunca chega ao cliente
  }
  if (!msg || typeof msg !== "object") return;
  // Mensagens de controlo do próprio emparelhamento (sem `jsonrpc`) não são
  // para o cliente.
  if (msg.jsonrpc !== "2.0") return;
  if (hasId(msg) && msg.method === undefined) pending.delete(idKey(msg.id));
  writeOut(msg);
}

function attachTab(ws: WebSocket, nonce: string): void {
  tab = ws;
  tokenUsed = true;
  log("Tab do 1fluxo ligado.");
  ws.send(JSON.stringify({ type: "paired", bridge: VERSION, proof: pairingProof(nonce) }));
  ws.on("message", (data) => onTabMessage(data.toString()));
  ws.on("close", () => detachTab(ws));
  ws.on("error", () => detachTab(ws));

  for (const q of queue.splice(0)) {
    clearTimeout(q.timer);
    forwardToTab(q.msg, q.line);
  }
  // O cliente pode ter pedido a lista de tools antes de haver tab — avisa-o
  // de que vale a pena voltar a pedir.
  if (clientInitialized) {
    writeOut({ jsonrpc: "2.0", method: "notifications/tools/list_changed" });
  }
}

function detachTab(ws: WebSocket): void {
  if (tab !== ws) return;
  tab = null;
  log("Tab do 1fluxo desligado.");
  // Quem ficou à espera de resposta não pode ficar pendurado.
  for (const { id } of pending.values()) {
    replyError(
      id,
      ERR_NO_TAB,
      `O 1fluxo fechou a ligação no browser antes de responder. Abre ${pairUrl} para voltar a ligar.`
    );
  }
  pending.clear();
  // O token que emparelhou este tab morre com ele; a URL nova vai para o log.
  rotateToken();
  log("Token de emparelhamento renovado. Para voltar a ligar:");
  log(`  ${pairUrl}`);
}

function onConnection(ws: WebSocket, origin: string | undefined): void {
  if (!origin || !allowedOrigins.has(origin)) {
    log(`Ligação recusada: origem não autorizada (${origin ?? "sem Origin"}).`);
    ws.close(4003, "origin not allowed");
    return;
  }
  if (tab) {
    log("Ligação recusada: já há um tab ligado.");
    ws.close(4009, "already paired");
    return;
  }
  if (pendingConnections >= MAX_PENDING) {
    log("Ligação recusada: demasiados emparelhamentos pendentes.");
    ws.close(4029, "too many pending");
    return;
  }
  pendingConnections += 1;
  let settled = false;
  const settle = () => {
    if (settled) return;
    settled = true;
    pendingConnections -= 1;
  };
  const timer = setTimeout(() => {
    log("Emparelhamento expirou sem token.");
    settle();
    ws.close(4008, "pairing timeout");
    // Sem esperar pelo handshake de fecho: um socket mudo ficava a ocupar vaga.
    setTimeout(() => ws.terminate(), 1_000).unref();
  }, PAIR_TIMEOUT_MS);
  ws.once("close", () => {
    clearTimeout(timer);
    settle();
  });
  ws.once("message", (data) => {
    clearTimeout(timer);
    settle();
    let hello: { type?: unknown; token?: unknown; nonce?: unknown } | null = null;
    try {
      hello = JSON.parse(data.toString()) as { type?: unknown; token?: unknown; nonce?: unknown };
    } catch {
      hello = null;
    }
    const nonce = typeof hello?.nonce === "string" && hello.nonce.length >= 16 && hello.nonce.length <= 128 ? hello.nonce : null;
    if (!hello || hello.type !== "pair" || !nonce || !tokenMatches(hello.token)) {
      log("Ligação recusada: token de emparelhamento inválido.");
      ws.close(4001, "bad token");
      return;
    }
    if (tab) {
      // Dois tabs a emparelhar ao mesmo tempo — fica o primeiro.
      ws.close(4009, "already paired");
      return;
    }
    attachTab(ws, nonce);
  });
}

/* ── arranque ───────────────────────────────────────────────────────────── */

function listen(port: number): Promise<WebSocketServer> {
  return new Promise((resolve, reject) => {
    const wss = new WebSocketServer({ host: "127.0.0.1", port });
    wss.once("listening", () => resolve(wss));
    wss.once("error", (err) => reject(err));
  });
}

async function main(): Promise<void> {
  let wss: WebSocketServer;
  try {
    wss = await listen(args.port);
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code !== "EADDRINUSE") throw err;
    log(`Porta ${args.port} ocupada; a escolher outra.`);
    wss = await listen(0);
  }
  const addr = wss.address();
  const port = typeof addr === "object" && addr ? addr.port : args.port;
  pairPort = port;
  pairUrl = `${args.appUrl}/connect?port=${port}&token=${token}`;

  wss.on("connection", (ws, req) => onConnection(ws, req.headers.origin));
  wss.on("error", (err) => log(`Erro no servidor WebSocket: ${err.message}`));

  log(`v${VERSION} à escuta em ws://127.0.0.1:${port}`);
  log("Abre no browser, com a sessão do 1fluxo desbloqueada:");
  log(`  ${pairUrl}`);

  const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
  rl.on("line", (line) => {
    const trimmed = line.trim();
    if (!trimmed) return;
    let msg: JsonRpcMessage;
    try {
      msg = JSON.parse(trimmed) as JsonRpcMessage;
    } catch {
      return; // sem JSON válido não há `id` a quem responder
    }
    if (!msg || typeof msg !== "object") return;
    handleClientMessage(msg, trimmed);
  });

  const shutdown = () => {
    for (const q of queue) clearTimeout(q.timer);
    tab?.close(1001, "bridge shutting down");
    wss.close();
    process.exit(0);
  };
  rl.on("close", shutdown);
  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
  // O cliente morreu a meio de uma escrita — não há mais nada a fazer.
  process.stdout.on("error", shutdown);
}

main().catch((err) => {
  log(`Falha ao arrancar: ${err instanceof Error ? err.message : String(err)}`);
  process.exit(1);
});
