# 1fluxo Bridge

Liga um assistente de IA (Claude, Cursor, VS Code…) ao [1fluxo](https://1fluxo.com)
**sem que os teus dados saiam do teu computador**.

O 1fluxo guarda as tuas finanças cifradas e nem o servidor os consegue ler. Para
um assistente poder responder a «quanto gastei este mês?», alguém tem de os
decifrar. Aqui, quem decifra és tu, no teu browser, com a sessão que já tens
aberta; este programa só faz a ponte entre o assistente e esse separador.

```
Claude Code, Cursor, VS Code ──HTTP + token──┐
Claude Desktop ── 1fluxo-bridge --stdio ─────┤→ Bridge ──WebSocket local──→ separador do 1fluxo
                                             │  (no teu computador)         (decifra e responde)
```

* Só fala com o `127.0.0.1`. Não abre ligações para a rede.
* Não interpreta pedidos nem guarda conteúdo: só encaminha mensagens.
* O que guarda em disco é um segredo de emparelhamento (`~/.1fluxo/`) e um token
  local; nenhum deles abre dados.
* Exige o 1fluxo aberto e desbloqueado no browser. Sem separador, não há dados.

## Instalar

Há duas formas, escolhe uma.

### Opção 1 — Claude, via extensão

Para o Claude Desktop. Descarrega a extensão `.mcpb` em Definições → Ligação a IA
no 1fluxo e abre o ficheiro com o Claude Desktop.

### Opção 2 — Executável local (bridge MCP)

Funciona com qualquer cliente MCP.

1. Descarrega o ficheiro da tua plataforma em [Releases](../../releases/latest):
   `1fluxo-bridge-macos.zip` ou `1fluxo-bridge-windows.zip`. Não há instalador:
   descomprime e abre.
2. Aparece um ícone no tray (Windows) ou na barra de menus (macOS). Dali: ligar e
   desligar (ON/OFF), ver o estado, abrir o 1fluxo e sair.
3. No menu, carrega em **Ligar ao Claude Desktop** e/ou **Ligar ao Claude Code**
   (um clique cada; depois reinicia o Claude Desktop, ou abre uma sessão nova do
   Claude Code). Para outros clientes MCP (Cursor, VS Code…), usa **Copiar endereço MCP**
   e o token que o acompanha, por HTTP.
   Move primeiro a app para a pasta Aplicações: o caminho fica guardado na configuração.
4. Faz uma pergunta. Na primeira vez abre-se o browser para carregares em
   **Permitir**; a ligação mantém-se durante 30 dias.

**Primeira abertura.** Os executáveis não são assinados (não há conta Apple Developer
nem certificado Windows). O macOS bloqueia-os: botão direito → Abrir, ou
Definições do Sistema → Privacidade e segurança → «Abrir mesmo assim». O Windows
avisa «editor desconhecido»: Mais informações → Executar mesmo assim. Por isso o
código é público e os builds saem do GitHub Actions; confirma o ficheiro com o
`SHA256SUMS` da Release:

```bash
shasum -a 256 -c SHA256SUMS --ignore-missing      # macOS / Linux
```

## Segurança

O endpoint só aceita pedidos de `127.0.0.1`, exige um token, recusa qualquer pedido
com `Origin` (um site a tentar a sorte) e valida o `Host` (DNS rebinding). Vários
clientes podem usar o mesmo bridge: os ids dos pedidos são remapeados para as
respostas não se trocarem. O botão **OFF** corta o acesso por completo.

Encontraste uma vulnerabilidade? Vê o [SECURITY.md](SECURITY.md).

## Compilar

Precisas de [Rust](https://rustup.rs). A app de tray vive em `tray/`:

```bash
cd tray
cargo test                                   # emparelhamento, endpoint, remapeamento de ids
cargo run -p fluxo-bridge-app -- --app-url http://localhost:3000
scripts/pack-macos.sh                        # dist/1fluxo-bridge-macos.zip
```

Mais em [`tray/README.md`](tray/README.md). O bridge original em TypeScript, que a
extensão `.mcpb` ainda usa, está na raiz (`src/`, `npm run build`, `npm test`) e
descrito em [`docs/bridge-typescript.md`](docs/bridge-typescript.md).

## Licença

[FSL-1.1-MIT](LICENSE) (Functional Source License). Podes ler, usar e modificar o
código para uso próprio, interno ou não comercial, mas não para lançar um produto
ou serviço concorrente. Cada versão passa a MIT dois anos depois de publicada.
Não é «open source» no sentido estrito da OSI, e o nome e o logótipo 1fluxo não
estão incluídos na licença.
