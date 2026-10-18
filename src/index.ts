#!/usr/bin/env node
/**
 * @1fluxo/bridge — ponte stdio ↔ WebSocket local.
 *
 * Lançado por um cliente MCP (Claude Code, Claude Desktop) via stdio. Abre um
 * servidor WebSocket em 127.0.0.1 e espera que o 1fluxo aberto no browser se
 * ligue a ele. A partir daí é um cano: cada linha JSON-RPC do stdin segue
 * para o tab; cada mensagem do tab sai no stdout.
 *
 * O que este processo NÃO faz, de propósito:
 *  - não interpreta tools nem lê o conteúdo das mensagens — só `id` e
 *    `method`, para saber a quem responder quando o tab desaparece;
 *  - não fala com a rede: a única ligação que aceita é do loopback;
 *  - não persiste dados nem escreve conteúdo em logs (o stderr só tem eventos
 *    de ligação, nunca pedidos ou respostas).
 *
 * Zero-knowledge é face ao 1fluxo: a decifragem acontece no tab, com a sessão
 * e a chave que já lá estão. Este processo vê o plaintext a passar, mas corre
 * na máquina do utilizador, ao lado do cliente MCP que o vai receber de
 * qualquer maneira.
 *
 * Emparelhamento: a primeira ligação faz-se com o token de uso
 * único impresso na URL de emparelhamento (o utilizador abre-a e carrega em
 * «Permitir»). Nessa altura o bridge gera um segredo de emparelhamento,
 * guarda-o em `<state-dir>/bridge-pairing.json` (só legível pelo utilizador)
 * e devolve-o ao tab, que o guarda no browser. Durante 30 dias qualquer tab
 * do 1fluxo retoma a ligação provando conhecer o segredo (HMAC de um nonce),
 * e o bridge prova o mesmo ao tab — o segredo nunca volta a passar no fio. O
 * ficheiro só tem esse segredo: não abre dados nenhuns.
 *
 * Duas excepções ao "cano burro", ambas só quando NÃO há tab ligado:
 *  - `initialize` é respondido localmente, senão o cliente dava o servidor
 *    como morto ao arrancar (o Claude Code lança o processo e pergunta logo).
 *    A resposta diz ao modelo o que o utilizador tem de fazer (abrir o
 *    1fluxo, ou emparelhar pela URL).
 *  - `ping` é respondido localmente pelo mesmo motivo.
 * Tudo o resto espera até NO_TAB_WAIT_MS por um tab e depois falha com um
 * erro JSON-RPC legível — nunca fica pendurado.
 */

import { spawn } from "node:child_process";
import { createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { WebSocketServer, type WebSocket } from "ws";

const VERSION = "1.0.0";
const DEFAULT_PORT = 17337;
/** Com a porta ocupada tenta-se as seguintes, por esta ordem, para o tab do
 *  1fluxo as poder varrer (espelha `BRIDGE_PORT_RANGE` na web app). */
const PORT_FALLBACKS = 10;
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
/** Onde fica o emparelhamento persistente. */
const DEFAULT_STATE_DIR = join(homedir(), ".1fluxo");
const PAIRING_FILE = "bridge-pairing.json";
/** Validade de um emparelhamento: ao fim disto o tab volta a pedir «Permitir». */
const PAIRING_TTL_MS = 30 * 24 * 60 * 60 * 1000;
/** Entre aberturas automáticas do browser: um pedido sem tab abre-o, os
 *  seguintes no mesmo minuto não. */
const OPEN_COOLDOWN_MS = 60_000;
/** Uma tool call sem tab abre o browser e espera por este tempo pelo
 *  «Permitir» — para a primeira pergunta ser mesmo respondida. */
const TOOL_CALL_WAIT_MS = 60_000;

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

type Args = {
  port: number;
  appUrl: string;
  origins: string[];
  stateDir: string;
  forget: boolean;
  /** Abrir o browser sozinho quando um pedido chega sem tab ligado. */
  open: boolean;
};

function usage(): string {
  return [
    "Uso: 1flux-bridge [--port N] [--app-url URL] [--origin URL]... [--state-dir DIR] [--forget] [--no-open]",
    "",
    `  --port N        porta do WebSocket local (defeito ${DEFAULT_PORT}; se ocupada escolhe outra)`,
    `  --app-url URL   onde o 1fluxo está a correr (defeito ${DEFAULT_APP_URL})`,
    "  --origin URL    origem adicional autorizada a emparelhar (repetível)",
    `  --state-dir DIR onde fica o emparelhamento (defeito ${DEFAULT_STATE_DIR})`,
    "  --forget        apaga o emparelhamento guardado e termina",
    "  --no-open       não abrir o browser sozinho quando um pedido chega sem tab ligado",
    "",
    "Lançado por um cliente MCP via stdio, p. ex.:",
    "  claude mcp add 1flux -- node <caminho>/dist/index.js",
  ].join("\n");
}

function parseArgs(argv: string[]): Args {
  const args: Args = {
    port: DEFAULT_PORT,
    appUrl: DEFAULT_APP_URL,
    origins: [],
    stateDir: DEFAULT_STATE_DIR,
    forget: false,
    open: true,
  };
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
    } else if (a === "--state-dir") {
      // Vazio (ex.: `${HOME}` por resolver num manifesto) → defeito.
      const dir = next().trim();
      args.stateDir = dir && !dir.includes("${") ? dir : DEFAULT_STATE_DIR;
    } else if (a === "--forget") {
      args.forget = true;
    } else if (a === "--no-open") {
      args.open = false;
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
 *  (um token reutilizável deixava o emparelhamento ser repetido). Renova-se quando o tab se desliga. */
let tokenUsed = false;
let pairPort = 0;
/** Ligações ainda por emparelhar; sem tecto qualquer processo local enchia
 *  a fila. */
let pendingConnections = 0;
const MAX_PENDING = 3;

/**
 * Cópia das tools da web app (`tools.json`, copiada de
 * `src/lib/mcp/bridge-tools.json` do repo 1flux-app, onde é verificada por
 * `tools-sync.test.ts`). Serve o `tools/list`
 * quando não há tab: senão o cliente arranca sem tools e nunca chega a fazer
 * a chamada que abre o browser. Com tab, é o tab que responde.
 */
function loadToolsJson(): unknown[] {
  try {
    const raw: unknown = JSON.parse(readFileSync(new URL("../tools.json", import.meta.url), "utf8"));
    return Array.isArray(raw) ? raw : [];
  } catch {
    return [];
  }
}
const TOOLS = loadToolsJson();

let tab: WebSocket | null = null;
/** Quem é o cliente MCP (`clientInfo` do `initialize`): vai ao tab no `paired`,
 *  para a app dizer «Claude Code ligado» em vez de «assistente ligado». */
let clientInfo: { name: string; version?: string } | null = null;
/** O cliente já completou o handshake MCP — só depois disso se pode mandar
 *  `notifications/tools/list_changed`. */
let clientInitialized = false;
let pairUrl = "";

/** Pedidos já enviados ao tab e ainda sem resposta. */
const pending = new Map<string, { id: JsonRpcId; method: string }>();
/** Pedidos à espera de um tab. */
type Queued = { line: string; msg: JsonRpcMessage; timer: NodeJS.Timeout };
const queue: Queued[] = [];

/* ── emparelhamento persistente ─────────────────────────────── */

type Pairing = {
  id: string;
  secret: string;
  origin: string;
  createdAt: string;
  expiresAt: string;
};

let pairing: Pairing | null = null;

function pairingPath(): string {
  return join(args.stateDir, PAIRING_FILE);
}

function isPairing(v: unknown): v is Pairing {
  if (!v || typeof v !== "object") return false;
  const p = v as Record<string, unknown>;
  return (
    typeof p.id === "string" &&
    typeof p.secret === "string" &&
    p.secret.length >= 32 &&
    typeof p.origin === "string" &&
    typeof p.createdAt === "string" &&
    typeof p.expiresAt === "string" &&
    !Number.isNaN(Date.parse(p.expiresAt))
  );
}

function loadPairing(): Pairing | null {
  try {
    const raw: unknown = JSON.parse(readFileSync(pairingPath(), "utf8"));
    return isPairing(raw) ? raw : null;
  } catch {
    return null; // sem ficheiro, ou ilegível — como se não houvesse
  }
}

function savePairing(p: Pairing): void {
  // Só o utilizador lê a pasta e o ficheiro (no Windows o modo é ignorado).
  mkdirSync(args.stateDir, { recursive: true, mode: 0o700 });
  writeFileSync(pairingPath(), JSON.stringify(p, null, 2) + "\n", { mode: 0o600 });
}

function clearPairing(): void {
  try {
    rmSync(pairingPath(), { force: true });
  } catch {
    // já não existe — é o que se queria
  }
  pairing = null;
}

function pairingValid(p: Pairing | null): p is Pairing {
  return p !== null && Date.parse(p.expiresAt) > Date.now();
}

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

function hmac(key: string, data: string): string {
  return createHmac("sha256", key).update(data).digest("base64url");
}

/** Comparação em tempo constante; `false` para tudo o que não for string. */
function safeEqual(candidate: unknown, expected: string): boolean {
  if (typeof candidate !== "string") return false;
  const a = Buffer.from(candidate, "utf8");
  const b = Buffer.from(expected, "utf8");
  return a.length === b.length && timingSafeEqual(a, b);
}

let browserOpenedAt = 0;

/**
 * Abre o browser do sistema na página de ligação. Chamado quando
 * um pedido chega sem tab ligado — a pessoa perguntou algo ao assistente, é
 * agora que quer o browser à frente. Sem shell pelo meio, para o `&` da URL
 * chegar inteiro. Falhar aqui só tira a comodidade: o erro JSON-RPC continua
 * a dizer que URL abrir à mão.
 */
function openBrowserForTab(): void {
  if (!args.open) return;
  const now = Date.now();
  if (now - browserOpenedAt < OPEN_COOLDOWN_MS) return;
  browserOpenedAt = now;
  // Sempre o /connect: a página tenta retomar com o emparelhamento que o
  // utilizador actual tiver (e segue para o dashboard) e, se não houver,
  // mostra o «Permitir» com este token. Serve o utilizador de sempre e um
  // utilizador novo no mesmo browser, sem o bridge ter de adivinhar.
  const url = pairUrl;
  const [cmd, cmdArgs] =
    process.platform === "darwin"
      ? ["open", [url]]
      : process.platform === "win32"
        ? ["rundll32", ["url.dll,FileProtocolHandler", url]]
        : ["xdg-open", [url]];
  try {
    const child = spawn(cmd, cmdArgs, { stdio: "ignore", detached: true });
    child.on("error", () => log(`Não consegui abrir o browser; abre à mão: ${url}`));
    child.unref();
    log(`A abrir o browser: ${url}`);
  } catch {
    log(`Não consegui abrir o browser; abre à mão: ${url}`);
  }
}

function noTabMessage(): string {
  return pairingValid(pairing)
    ? `O 1fluxo não está ligado neste momento. Abre o 1fluxo no browser (${args.appUrl}) com a sessão desbloqueada — a ligação faz-se sozinha. Se não ligar, abre ${pairUrl} para emparelhar de novo.`
    : `O 1fluxo não está aberto no browser. Abre ${pairUrl} para ligar.`;
}

function rotateToken(): void {
  token = randomBytes(32).toString("base64url");
  tokenUsed = false;
  pairUrl = `${args.appUrl}/connect?port=${pairPort}&token=${token}`;
}

/** Prova de que quem responde "paired" conhece o token: HMAC do nonce que o
 *  tab escolheu (sem isto qualquer processo local podia fingir ser o bridge). O tab verifica antes de confiar. */
function pairingProof(nonce: string): string {
  return hmac(token, nonce);
}

function tokenMatches(candidate: unknown): boolean {
  if (tokenUsed) return false;
  return safeEqual(candidate, token);
}

/* ── lado do cliente MCP (stdin/stdout) ─────────────────────────────────── */

function instructions(): string {
  return pairingValid(pairing)
    ? "As tools do 1fluxo ficam disponíveis quando o utilizador tiver o 1fluxo aberto no " +
        `browser (${args.appUrl}) com a sessão desbloqueada — a ligação faz-se sozinha. ` +
        "Se uma tool falhar com esse aviso, pede-lhe que abra o 1fluxo; se continuar a " +
        `falhar, que emparelhe de novo em ${pairUrl}.`
    : "As tools do 1fluxo só ficam disponíveis depois de o utilizador abrir o " +
        `1fluxo no browser e autorizar a ligação em ${pairUrl} — se uma tool falhar ` +
        "com esse aviso, pede-lhe que abra essa página.";
}

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
      instructions: instructions(),
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
  if (msg.method === "initialize") {
    const params = (msg.params ?? {}) as { clientInfo?: { name?: unknown; version?: unknown } };
    const name = typeof params.clientInfo?.name === "string" ? params.clientInfo.name.trim().slice(0, 80) : "";
    if (name) {
      clientInfo = {
        name,
        version:
          typeof params.clientInfo?.version === "string" ? params.clientInfo.version.slice(0, 40) : undefined,
      };
    }
  }
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
  if (hasId(msg) && msg.method === "tools/list" && TOOLS.length > 0) {
    writeOut({ jsonrpc: "2.0", id: msg.id, result: { tools: TOOLS } });
    return;
  }
  // Notificações sem destino não têm a quem responder — descartam-se.
  if (!hasId(msg)) return;

  // Alguém perguntou algo e não há tab: é a altura de pôr o 1fluxo à frente,
  // e de esperar que a pessoa carregue em «Permitir». Só nas tool calls — o
  // `tools/list` do arranque não é a pessoa a pedir nada.
  const isCall = msg.method === "tools/call";
  if (isCall) openBrowserForTab();

  const entry: Queued = {
    line,
    msg,
    timer: setTimeout(
      () => {
        const i = queue.indexOf(entry);
        if (i >= 0) queue.splice(i, 1);
        replyError(msg.id, ERR_NO_TAB, noTabMessage());
      },
      isCall ? TOOL_CALL_WAIT_MS : NO_TAB_WAIT_MS
    ),
  };
  queue.push(entry);
}

/* ── lado do tab (WebSocket) ────────────────────────────────────────────── */

function onTabMessage(raw: string): void {
  let msg: JsonRpcMessage & { type?: unknown };
  try {
    msg = JSON.parse(raw) as JsonRpcMessage & { type?: unknown };
  } catch {
    return; // não é JSON — nunca chega ao cliente
  }
  if (!msg || typeof msg !== "object") return;
  // Mensagens de controlo do próprio emparelhamento (sem `jsonrpc`) não são
  // para o cliente. «forget» é o utilizador a esquecer este bridge em
  // Definições: o ficheiro vai-se e a próxima ligação volta a pedir token.
  if (msg.type === "forget") {
    clearPairing();
    log("Emparelhamento esquecido a pedido do 1fluxo.");
    return;
  }
  if (msg.jsonrpc !== "2.0") return;
  if (hasId(msg) && msg.method === undefined) pending.delete(idKey(msg.id));
  writeOut(msg);
}

type AttachMode = { kind: "pair"; origin: string } | { kind: "resume"; pairing: Pairing };

function attachTab(ws: WebSocket, nonce: string, mode: AttachMode): void {
  tab = ws;
  let hello: object;
  if (mode.kind === "pair") {
    tokenUsed = true;
    const now = Date.now();
    pairing = {
      id: randomBytes(12).toString("base64url"),
      secret: randomBytes(32).toString("base64url"),
      origin: mode.origin,
      createdAt: new Date(now).toISOString(),
      expiresAt: new Date(now + PAIRING_TTL_MS).toISOString(),
    };
    savePairing(pairing);
    log(`Tab do 1fluxo emparelhado (válido até ${pairing.expiresAt.slice(0, 10)}).`);
    // O segredo só passa no fio aqui, uma vez, para o tab o guardar.
    hello = {
      type: "paired",
      bridge: VERSION,
      client: clientInfo,
      proof: pairingProof(nonce),
      pairing: { id: pairing.id, secret: pairing.secret, expiresAt: pairing.expiresAt },
    };
  } else {
    log("Tab do 1fluxo ligado (emparelhamento retomado).");
    hello = {
      type: "paired",
      bridge: VERSION,
      client: clientInfo,
      proof: hmac(mode.pairing.secret, `bridge:${nonce}`),
      pairing: { id: mode.pairing.id, expiresAt: mode.pairing.expiresAt },
    };
  }
  ws.send(JSON.stringify(hello));
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
      `O 1fluxo fechou a ligação no browser antes de responder. ${noTabMessage()}`
    );
  }
  pending.clear();
  // O token que emparelhou este tab morre com ele; a URL nova vai para o log.
  rotateToken();
  if (pairingValid(pairing)) {
    log("Volta a ligar sozinho quando o 1fluxo estiver aberto no browser.");
  } else {
    log("Token de emparelhamento renovado. Para voltar a ligar:");
    log(`  ${pairUrl}`);
  }
}

type Hello = {
  type?: unknown;
  token?: unknown;
  nonce?: unknown;
  pairingId?: unknown;
  proof?: unknown;
};

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
    let hello: Hello | null = null;
    try {
      hello = JSON.parse(data.toString()) as Hello;
    } catch {
      hello = null;
    }
    const nonce =
      typeof hello?.nonce === "string" && hello.nonce.length >= 16 && hello.nonce.length <= 128
        ? hello.nonce
        : null;
    if (!hello || !nonce) {
      log("Ligação recusada: pedido de emparelhamento inválido.");
      ws.close(4001, "bad token");
      return;
    }

    if (hello.type === "resume") {
      // Retoma de um emparelhamento guardado: o tab prova conhecer o segredo.
      if (!pairing) {
        log("Retoma recusada: não há emparelhamento guardado.");
        ws.close(4011, "unknown pairing");
        return;
      }
      if (!pairingValid(pairing)) {
        log("Retoma recusada: o emparelhamento expirou.");
        clearPairing();
        ws.close(4010, "pairing expired");
        return;
      }
      if (
        !safeEqual(hello.pairingId, pairing.id) ||
        !safeEqual(hello.proof, hmac(pairing.secret, `tab:${nonce}`))
      ) {
        log("Retoma recusada: prova de emparelhamento inválida.");
        ws.close(4011, "bad pairing proof");
        return;
      }
      if (tab) {
        ws.close(4009, "already paired");
        return;
      }
      attachTab(ws, nonce, { kind: "resume", pairing });
      return;
    }

    if (hello.type !== "pair" || !tokenMatches(hello.token)) {
      log("Ligação recusada: token de emparelhamento inválido.");
      ws.close(4001, "bad token");
      return;
    }
    if (tab) {
      // Dois tabs a emparelhar ao mesmo tempo — fica o primeiro.
      ws.close(4009, "already paired");
      return;
    }
    attachTab(ws, nonce, { kind: "pair", origin });
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
  if (args.forget) {
    clearPairing();
    log("Emparelhamento apagado.");
    process.exit(0);
  }

  const stored = loadPairing();
  if (stored && !pairingValid(stored)) {
    clearPairing();
    log("O emparelhamento guardado expirou; apagado.");
  } else {
    pairing = stored;
  }

  let wss: WebSocketServer | null = null;
  // Porta pedida, depois as seguintes (o tab varre a mesma gama); só no fim
  // uma qualquer. `--port 0` é «qualquer», sem gama.
  const candidates = args.port === 0 ? [0] : Array.from({ length: PORT_FALLBACKS }, (_, i) => args.port + i);
  for (const candidate of candidates) {
    try {
      wss = await listen(candidate);
      break;
    } catch (err) {
      if ((err as NodeJS.ErrnoException).code !== "EADDRINUSE") throw err;
      log(`Porta ${candidate} ocupada; a tentar a seguinte.`);
    }
  }
  if (!wss) wss = await listen(0);
  const addr = wss.address();
  const port = typeof addr === "object" && addr ? addr.port : args.port;
  pairPort = port;
  pairUrl = `${args.appUrl}/connect?port=${port}&token=${token}`;

  wss.on("connection", (ws, req) => onConnection(ws, req.headers.origin));
  wss.on("error", (err) => log(`Erro no servidor WebSocket: ${err.message}`));

  log(`v${VERSION} à escuta em ws://127.0.0.1:${port}`);
  if (pairingValid(pairing)) {
    log(`Emparelhamento válido até ${pairing.expiresAt.slice(0, 10)}.`);
    log(`Abre o 1fluxo no browser (${args.appUrl}) com a sessão desbloqueada — a ligação faz-se sozinha.`);
    log("Para emparelhar de novo, se for preciso:");
  } else {
    log("Abre no browser, com a sessão do 1fluxo desbloqueada:");
  }
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
