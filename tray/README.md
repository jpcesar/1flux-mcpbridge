# 1fluxo Bridge (tray)

Utilitário de tray / barra de menus para macOS (13 ou mais recente) e Windows. No macOS
descarrega-se o DMG e arrasta-se a app para Aplicações; no Windows descomprime-se o zip e corre-se.

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

* **Ligar / Desligar**: Ligar arranca o bridge e o endpoint; Desligar fecha tudo e o
  assistente deixa de ter acesso aos dados. O estado persiste entre reinícios.
* **Estado**: desligado · ligado, à espera do 1fluxo · 1fluxo ligado (e o nome do cliente).
* **Emparelhar / Esquecer emparelhamento**: sem emparelhamento, «Emparelhar» abre o
  `/connect` no browser para carregar em «Permitir» (precisa do bridge ligado); com
  emparelhamento, «Esquecer emparelhamento» apaga-o e obriga a autorizar de novo.
* **Copiar endereço MCP** (com o token no caminho), para clientes HTTP.
* **Ligar ao Claude Desktop** e **Ligar ao Claude Code**: um clique configura cada um (abaixo).
* **Arrancar com o sistema** (ligado por defeito nos builds de release; ver abaixo) e **Sair**.

Uma tool call sem tab ligado abre o browser sozinha (no máximo uma vez por
minuto) e espera até 60 s pelo tab, como o bridge em TypeScript.

## Instalar no macOS e arrancar com o sistema

* **DMG**: `1fluxo-bridge-macos.dmg` abre uma janela com a app à esquerda, o atalho de
  Aplicações à direita e uma seta; arrasta-se a app. O zip continua a existir como alternativa.
* **Mover para Aplicações**: se a app abrir fora de `/Applications` e `~/Applications` (de dentro
  do DMG, ou de Descarregamentos, onde o macOS a corre de uma pasta temporária «AppTranslocation»),
  pergunta se a queres mover. Se aceitares, copia-a para Aplicações (ou `~/Applications`, se não
  houver permissão), substitui a versão que lá estiver, abre-a de lá e fecha-se. Com «Agora não»
  a app corre, mas **não** se regista para arrancar com o sistema (apontaria para o sítio errado); a
  recusa fica guardada por caminho (`move-declined.json` na pasta de estado) e volta a perguntar
  se a app aparecer noutro sítio. Fora de um bundle (`cargo run`) não pergunta nada.
* **Item de início** (macOS 13+): usa o `SMAppService` e aparece em Definições do Sistema → Geral →
  Itens de início. O visto de «Arrancar com o sistema» segue o estado real (relido de 5 em 5
  segundos, por isso também apanha o que mudares nas Definições). Se o sistema pedir aprovação (por
  exemplo, se o desligaste lá), aparece **Aprovar nas Definições…**, que abre essa página; enquanto
  não aprovares o item fica sem visto.
* **Migração**: as versões até à 1.0.x registavam um LaunchAgent em
  `~/Library/LaunchAgents/1fluxo Bridge.plist`. Na primeira abertura da 1.1 apaga-se esse ficheiro
  (só se for nosso) e, se o arranque estava ligado, regista-se o item novo, para não ficares com dois.

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
cargo test --workspace           # emparelhamento, endpoint, remapeamento de ids, config do Claude Desktop e do Claude Code, item de início e mover para Aplicações
cargo run -p bridge-core --bin 1fluxo-bridge-headless -- --app-url http://localhost:3000
cargo run -p fluxo-bridge-app -- --app-url http://localhost:3000     # com tray
scripts/pack-macos.sh            # dist/1fluxo-bridge-macos.dmg e .zip (.app com assinatura ad-hoc, DMG sem assinar)
scripts/check-dmg.sh             # confirma que o DMG monta e tem a app, o atalho, o fundo e o layout
```

Argumentos (comuns): `--port`, `--http-port` (defeito 17350, tem de ser estável),
`--app-url` (defeito `https://1fluxo.com`), `--origin`, `--state-dir`, `--no-open`,
`--stdio`, `--forget`. `FLUXO_BRIDGE_NO_AUTOSTART=1` impede o registo do arranque com o
sistema no primeiro arranque de um build de release.

**Janela do DMG.** O fundo (com a seta) sai de `icons/render-dmg.py` (`dmg-background.png` e
`@2x`, juntos num TIFF pelo `pack-macos.sh`). O Finder precisa de um `.DS_Store` com o tamanho da
janela e a posição dos ícones: `icons/dmg.DS_Store` é um modelo gerado por `scripts/dmg-layout.sh`
(corre-se à mão, numa sessão gráfica, quando mudar o fundo ou as posições) e commitado, para o CI
não depender do Finder. O nome do volume («1fluxo Bridge») e o caminho `.background/background.tiff`
têm de coincidir nos dois scripts.

Os ícones derivam do ícone da app pós-redesign (`1flux-app/src/app/icon.png`);
`icons/render.py` regenera os PNG do tray (modelos monocromáticos para o macOS, cores
da marca para o Windows).

## Primeira abertura (sem assinatura)

Só vale para builds sem Developer ID (locais, ou uma release feita sem os segredos de assinatura).

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
