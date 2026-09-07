# @1flux/bridge

Ponte local `stdio ↔ WebSocket` que liga um cliente MCP (Claude Code) ao 1fluxo
aberto no browser.

O processo é burro de propósito: não interpreta tools, não fala com a rede
(só loopback) e não escreve conteúdo em logs. Toda a lógica — autenticação,
decifragem, tools — vive no tab do web app, na página `/connect`.

## Como funciona

1. O cliente MCP lança o bridge via stdio.
2. O bridge abre `ws://127.0.0.1:<porta>` (17337 por defeito; se estiver
   ocupada escolhe outra), gera um token de emparelhamento e imprime no
   stderr a URL `…/connect?port=<porta>&token=<token>`.
3. O utilizador abre essa URL com a sessão do 1fluxo desbloqueada e carrega em
   "Permitir". O tab liga-se ao bridge, envia o token, e passa a ser o servidor
   MCP.
4. A partir daí cada linha JSON-RPC do stdin vai para o tab e cada resposta do
   tab sai no stdout.

Só é aceite **uma** ligação, e só se o `Origin` estiver na lista autorizada
**e** a primeira mensagem trouxer o token certo (comparação em tempo
constante).

Sem tab ligado, `initialize` e `ping` são respondidos localmente (senão o
cliente dava o servidor como morto ao arrancar); qualquer outro pedido espera
até 10 s e depois falha com um erro JSON-RPC legível que inclui a URL de
emparelhamento. Quando o tab liga, o bridge envia
`notifications/tools/list_changed` para o cliente voltar a pedir as tools.

## Uso (dev, por caminho local)

```bash
cd packages/bridge && npm install && npm run build
```

```bash
claude mcp add 1flux -- node /caminho/para/1flux-app/packages/bridge/dist/index.js --app-url http://localhost:3000
```

Depois, no Claude Code, a primeira pergunta sobre o 1fluxo devolve a URL de
emparelhamento (ou lê-se no stderr do servidor MCP). Abre-a, autoriza, e
pergunta de novo.

### Opções

| Flag | Defeito | Para quê |
| --- | --- | --- |
| `--port N` | `17337` | porta do WebSocket local; ocupada → escolhe outra |
| `--app-url URL` | `http://localhost:3000` | onde o 1fluxo está a correr (entra na URL de emparelhamento e na lista de origens) |
| `--origin URL` | — | origem adicional autorizada a emparelhar (repetível) |

Origens autorizadas por defeito: as do 1fluxo em produção. A origem de `--app-url`
entra sempre, e `--origin` acrescenta outras.

## Fora da POC

Publicar no npm (`npx -y @1flux/bridge`), abrir o browser automaticamente,
reconexão robusta, multi-tab, tools de escrita.
