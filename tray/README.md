# 1fluxo Bridge (tray)

Utilitário de tray / barra de menus para macOS e Windows. Sem
instalador, sem notarização, sem certificados: descarrega-se e corre-se.

É o mesmo bridge do resto do repo (`src/index.ts`), reescrito em Rust e
transformado num serviço local permanente. Em vez de cada cliente MCP lançar o
seu processo por stdio, a app corre o bridge uma vez e expõe um **endereço MCP
local** a que qualquer cliente se liga. O WebSocket para o tab do 1fluxo e o
emparelhamento persistente são os mesmos, com o mesmo ficheiro de
estado (`~/.1fluxo/bridge-pairing.json`).

```
Claude Code, Cursor, VS Code ──HTTP+token──┐
Claude Desktop ── 1fluxo-bridge --stdio ───┤→ app de tray ──WebSocket──→ tab do 1fluxo
                                           │   (endpoint MCP + bridge)
```

## O que faz o tray

* **ON / OFF**: ON arranca o bridge e o endpoint; OFF fecha tudo e o assistente
  deixa de ter acesso aos dados. O estado persiste entre reinícios.
* **Estado**: desligado · ligado, à espera do 1fluxo · 1fluxo ligado (e o nome do cliente).
* **Abrir o 1fluxo**: abre o `/connect` no browser (retoma o emparelhamento ou pede «Permitir»).
* **Copiar endereço MCP** (com o token no caminho), para clientes HTTP.
* **Ligar ao Claude Desktop** e **Ligar ao Claude Code**: um clique configura cada um (abaixo).
* **Arrancar com o sistema** (ligado por defeito nos builds de release), **Esquecer emparelhamento**, **Sair**.

Uma tool call sem tab ligado abre o browser sozinha (no máximo uma vez por
minuto) e espera até 60 s pelo tab, como o bridge em TypeScript.

## Ligar os clientes

* **Claude Code** (CLI e separador Code do Claude Desktop, que partilham o `~/.claude.json`):
  **Ligar ao Claude Code** corre `claude mcp add --scope user 1fluxo -- <esta app> --stdio`
  (primeiro vê se já está registado e, se sim, diz «já estava ligado» e não muda nada; senão faz um `claude mcp remove` da mesma entrada, se existir). Não edita o `~/.claude.json`
  à mão. Procura o `claude` nos sítios habituais e no `PATH` de um shell de login; se não o
  encontrar, ou ele recusar, copia o comando para colar no terminal. Só vale para sessões novas.
  Uma entrada `1fluxo` registada antes num projecto (âmbito local) continua a ter prioridade nesse projecto.
* **Cursor, VS Code e outros clientes HTTP**: **Copiar endereço MCP**. O token vai
  no header `Authorization: Bearer …` (preferido) ou no caminho (`/mcp/<token>`), para os
  clientes que não deixam configurar headers — nesse caso fica em ficheiros de configuração e logs.
* **Claude Desktop** (separador Chat): só lança processos por stdio e não aceita servidores
  HTTP locais. No menu, **Ligar ao Claude Desktop** acrescenta a entrada `1fluxo` ao
  `claude_desktop_config.json` (macOS: `~/Library/Application Support/Claude/`; Windows:
  `%APPDATA%\Claude\`, ou a pasta virtualizada da versão da Store). A entrada lança este
  mesmo executável com `--stdio` e não leva token: o shim lê a porta e o token do estado local.
  Só mexe em `mcpServers.1fluxo` (o resto fica como estava), recusa JSON ilegível, guarda uma
  cópia (`claude_desktop_config.json.1fluxo.bak`) antes da primeira alteração e escreve de
  forma atómica. Recusa-se a correr de uma app «translocada» (aberta a partir das
  Transferências): move-a primeiro para Aplicações. Depois, fechar o Claude Desktop por
  completo e abri-lo outra vez. A app tem de estar aberta; com ela em baixo o shim responde
  localmente ao `initialize`, `ping` e `tools/list` e falha as tools com um erro legível.

## Segurança do endpoint

Só 127.0.0.1; token obrigatório (guardado em `~/.1fluxo/bridge-endpoint-token`, a 0600);
`Host` tem de ser local e qualquer pedido com `Origin` é recusado (DNS rebinding).
Os ids JSON-RPC de cada cliente são remapeados antes de irem ao tab, para vários
clientes poderem usar o mesmo bridge sem as respostas se trocarem.

## Desenvolver

```bash
cd tray
cargo test                       # 22 testes: emparelhamento, endpoint, remapeamento de ids, config do Claude Desktop e do Claude Code
cargo run -p bridge-core --bin 1fluxo-bridge-headless -- --app-url http://localhost:3000
cargo run -p fluxo-bridge-app -- --app-url http://localhost:3000     # com tray
scripts/pack-macos.sh            # dist/1fluxo-bridge-macos.zip (.app com assinatura ad-hoc)
```

Argumentos (comuns): `--port`, `--http-port` (defeito 17350, tem de ser estável),
`--app-url` (defeito `https://1fluxo.com`), `--origin`, `--state-dir`, `--no-open`,
`--stdio`, `--forget`. `FLUXO_BRIDGE_NO_AUTOSTART=1` impede o registo do arranque com o
sistema no primeiro arranque de um build de release.

Os ícones derivam do ícone da app pós-redesign (`1flux-app/src/app/icon.png`);
`icons/render.py` regenera os PNG do tray (modelos monocromáticos para o macOS, cores
da marca para o Windows).

## Primeira abertura (sem assinatura)

* **macOS**: o Gatekeeper bloqueia a app descarregada; abrir com botão direito → Abrir, ou
  Definições do Sistema → Privacidade e segurança → «Abrir mesmo assim».
* **Windows**: o SmartScreen avisa «editor desconhecido» → Mais informações → Executar mesmo assim.

## Estado

* Núcleo (`crates/bridge-core`): testado, incluindo um tab de mentira e dois clientes HTTP.
* App de tray: compila e arranca em macOS; o ícone no tray **não foi verificado visualmente**.
* Windows: **compila** (cross-compilação a partir do macOS, alvo `x86_64-pc-windows-gnu`, e os
  testes também compilam), mas **nunca foi executado**. Pontos a validar: o `--stdio` com
  `windows_subsystem = "windows"`, o `clip` para copiar, o arranque com o sistema, o ícone no tray.
  No alvo GNU o executável precisa de `WebView2Loader.dll` ao lado (por isso o zip de teste leva
  dois ficheiros) e do runtime WebView2 do sistema; o build MSVC do CI (`.github/workflows/tray.yml`)
  liga-o estaticamente e dá um só `.exe`. Cross-compilar: `brew install mingw-w64`,
  `rustup target add x86_64-pc-windows-gnu` e
  `cargo build --release --target x86_64-pc-windows-gnu -p fluxo-bridge-app`.
* Fora do âmbito: auto-update, instalador, notificações `tools/list_changed` por SSE.
