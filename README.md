# 1fluxo Bridge

Liga um assistente de IA (Claude, Cursor, VS Code…) ao [1fluxo](https://1fluxo.com)
**sem que os teus dados saiam do teu computador**.

O 1fluxo guarda as tuas finanças cifradas e nem o servidor os consegue ler. Para
um assistente poder responder a «quanto gastei este mês?», alguém tem de os
decifrar. Aqui, quem decifra és tu, no teu browser, com a sessão que já tens
aberta; este programa só faz a ponte entre o assistente e esse separador.

```
Claude Desktop (Chat) ── 1fluxo-bridge --stdio ──┐
Claude Code (CLI e Code) ─ 1fluxo-bridge --stdio ─┤→ app de tray ──WebSocket local──→ separador do 1fluxo
Cursor, VS Code ─────────── HTTP + token ────────┘  (no teu computador)               (decifra e responde)
```

* Só fala com o `127.0.0.1`. Não abre ligações para a rede.
* Não interpreta pedidos nem guarda conteúdo: só encaminha mensagens.
* O que guarda em disco é um segredo de emparelhamento (`~/.1fluxo/`) e um token
  local; nenhum deles abre dados.
* Exige o 1fluxo aberto e desbloqueado no browser. Sem separador, não há dados.

## Instalar

Há duas formas, escolhe uma. Não uses as duas ao mesmo tempo: cada uma tem o seu
bridge e os dois disputariam o separador do 1fluxo.

### Opção 1 — Só o Claude Desktop, via extensão

Não instala mais nada: descarrega a extensão `.mcpb` em Definições → Ligação a IA
no 1fluxo e abre o ficheiro com o Claude Desktop. Funciona sozinha, sem a app da Opção 2.

### Opção 2 — App de tray (Claude Desktop, Claude Code e outros clientes)

Uma app pequena que fica a correr e serve todos os clientes MCP do computador.

1. Descarrega o ficheiro da tua plataforma em [Releases](../../releases/latest):
   `1fluxo-bridge-macos.zip` ou `1fluxo-bridge-windows.zip`. Não há instalador:
   descomprime e abre. No macOS, move primeiro a app para a pasta Aplicações (o
   caminho dela fica guardado na configuração dos clientes).
2. Aparece um ícone no tray (Windows) ou na barra de menus (macOS). Dali: ligar e
   desligar, ver o estado, emparelhar (ou esquecer o emparelhamento) e sair.
3. Liga o teu cliente com um clique no menu:

   | Cliente | Item do menu | Depois |
   | --- | --- | --- |
   | **Claude Desktop** (separador Chat) | **Ligar ao Claude Desktop** | Fecha o Claude Desktop por completo (⌘Q no macOS) e abre-o outra vez |
   | **Claude Code** (CLI e separador Code do Claude Desktop) | **Ligar ao Claude Code** | Abre uma sessão nova |
   | Cursor, VS Code e outros por HTTP | **Copiar endereço MCP** | Cola o endereço (já leva o token) no cliente |

   Cada item diz-te se a ligação já estava feita, por isso podes repeti-los sem
   risco. No Claude Desktop só se acrescenta a entrada `1fluxo` ao
   `claude_desktop_config.json` (o resto fica como estava e fica uma cópia ao lado);
   no Claude Code corre-se o `claude mcp add` por ti, ou, se o `claude` não for
   encontrado, copia-se o comando para colares no terminal. A app tem de estar
   aberta para o assistente ter acesso aos dados.
4. Faz uma pergunta. Na primeira vez abre-se o browser para carregares em
   **Permitir**; a ligação mantém-se durante 30 dias.

**Primeira abertura.** Enquanto os executáveis não saem assinados (ver
[Assinatura](#assinatura)), o macOS bloqueia-os: Definições do Sistema →
Privacidade e segurança → «Abrir mesmo assim» (no macOS 15 ou mais recente o
botão direito → Abrir já não chega). O Windows
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
respostas não se trocarem. O item **Desligar** corta o acesso por completo.

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

## Assinatura

A release (`.github/workflows/release.yml`) assina e notariza a app de macOS
quando estes segredos existem no repo (Settings → Secrets and variables →
Actions). Sem eles a release sai com assinatura ad-hoc e um aviso no workflow.

| Segredo | O que é |
| --- | --- |
| `MACOS_CERTIFICATE_P12` | Certificado **Developer ID Application** exportado do Keychain em `.p12`, em base64 (`base64 -i cert.p12 \| pbcopy`) |
| `MACOS_CERTIFICATE_PASSWORD` | A palavra-passe com que o `.p12` foi exportado |
| `MACOS_SIGN_IDENTITY` | O nome do certificado, p. ex. `Developer ID Application: Nome (TEAMID)` |
| `APPLE_API_KEY_P8` | Chave da App Store Connect API (`.p8`, papel *Developer*), em base64 |
| `APPLE_API_KEY_ID` | O *Key ID* dessa chave |
| `APPLE_API_ISSUER_ID` | O *Issuer ID* da equipa |

O `tray/scripts/pack-macos.sh` assina com hardened runtime e timestamp, envia o
zip ao `notarytool`, agrafa o ticket à app e volta a fazer o zip. Para confirmar
uma app descarregada:

```bash
spctl --assess --type execute --verbose=2 "1fluxo Bridge.app"   # accepted, source=Notarized Developer ID
```

A assinatura do executável de Windows ainda não está feita.

## Licença

[FSL-1.1-MIT](LICENSE) (Functional Source License). Podes ler, usar e modificar o
código para uso próprio, interno ou não comercial, mas não para lançar um produto
ou serviço concorrente. Cada versão passa a MIT dois anos depois de publicada.
Não é «open source» no sentido estrito da OSI, e o nome e o logótipo 1fluxo não
estão incluídos na licença.
