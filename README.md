# @1flux/bridge

Ponte local `stdio ↔ WebSocket` que liga um cliente MCP (Claude Desktop, Claude
Code) ao 1fluxo aberto no browser, com emparelhamento persistente.

O processo é burro de propósito: não interpreta tools, não fala com a rede
(só loopback) e não escreve conteúdo em logs. Toda a lógica — autenticação,
decifragem, tools — vive no tab do web app. Zero-knowledge é face ao 1fluxo:
nada em claro passa por servidores nossos.

## Como funciona

1. O cliente MCP lança o bridge via stdio.
2. O bridge abre `ws://127.0.0.1:<porta>` (17337 por defeito; se estiver
   ocupada escolhe outra).
3. **Primeira vez:** gera um token de emparelhamento e imprime no stderr a URL
   `…/connect?port=<porta>&token=<token>` (também vai nas `instructions` do
   `initialize`, para o assistente a mostrar ao utilizador). O utilizador
   abre-a com a sessão do 1fluxo desbloqueada e carrega em "Permitir". O tab
   liga-se, envia o token, e o bridge devolve-lhe um **segredo de
   emparelhamento**, que guarda em `<state-dir>/bridge-pairing.json`
   (0600). Válido 30 dias.
4. **Depois disso:** qualquer página do 1fluxo aberta nesse browser liga-se
   sozinha. O tab manda `{ type: "resume", pairingId, nonce, proof }` com
   `proof = HMAC(segredo, "tab:" + nonce)`; o bridge verifica e responde com
   `HMAC(segredo, "bridge:" + nonce)`, que o tab verifica antes de servir
   dados. O segredo nunca volta a passar no fio.
5. A partir daí cada linha JSON-RPC do stdin vai para o tab e cada resposta do
   tab sai no stdout.

Só é aceite **uma** ligação de cada vez, e só se o `Origin` estiver na lista
autorizada **e** o handshake for válido (token de uso único, ou prova do
segredo). Comparações em tempo constante.

Sem tab ligado, `initialize` e `ping` são respondidos localmente (senão o
cliente dava o servidor como morto ao arrancar); qualquer outro pedido espera
até 10 s e depois falha com um erro JSON-RPC legível que diz o que fazer
(abrir o 1fluxo, ou emparelhar pela URL). Quando o tab liga, o bridge envia
`notifications/tools/list_changed` para o cliente voltar a pedir as tools.

### Códigos de fecho

| Código | Significado |
| --- | --- |
| 4001 | token de emparelhamento inválido (ou já usado) |
| 4003 | `Origin` não autorizada |
| 4008 | o tab não mandou o handshake a tempo |
| 4009 | já há um tab ligado |
| 4010 | o emparelhamento guardado expirou (apagado) |
| 4011 | emparelhamento desconhecido ou prova inválida |
| 4029 | demasiadas ligações por emparelhar |

### Esquecer o emparelhamento

- Em Definições → Ligação a IA → "Esquecer este bridge": o tab manda
  `{ type: "forget" }` e o bridge apaga o ficheiro.
- À mão: `node dist/index.js --forget` (apaga e termina).

## Uso (dev, por caminho local)

```bash
cd packages/bridge && npm ci && npm run build
```

```bash
claude mcp add 1fluxo -- node /caminho/para/1flux-app/packages/bridge/dist/index.js --app-url http://localhost:3000
```

Depois, no Claude Code, a primeira pergunta sobre o 1fluxo devolve a URL de
emparelhamento (ou lê-se no stderr do servidor MCP). Abre-a, autoriza, e
pergunta de novo. Nas vezes seguintes basta ter o 1fluxo aberto no browser.

### Opções

| Flag | Defeito | Para quê |
| --- | --- | --- |
| `--port N` | `17337` | porta do WebSocket local; ocupada → escolhe outra |
| `--app-url URL` | `http://localhost:3000` | onde o 1fluxo está a correr (entra na URL de emparelhamento e na lista de origens) |
| `--origin URL` | — | origem adicional autorizada a emparelhar (repetível) |
| `--state-dir DIR` | `~/.1fluxo` | onde fica `bridge-pairing.json` |
| `--forget` | — | apaga o emparelhamento guardado e termina |

Origens autorizadas por defeito: as do 1fluxo em produção. A origem de `--app-url`
entra sempre, e `--origin` acrescenta outras.

## Testes

```bash
npm run build && npm test
```

`test/pairing.test.mjs` arranca o bridge real com um `--state-dir` temporário e
fala-lhe por WebSocket: primeiro emparelhamento, retoma com prova certa e
errada, retoma depois de reiniciar, expiração (4010) e `forget` / `--forget`.

## Fora deste pacote

Publicar no npm, abrir o browser automaticamente, multi-tab, tools de escrita:
por decidir.
