# Segurança

Este programa lê dados financeiros em claro, no computador de quem o usa. Levamos
a sério qualquer falha que o possa expor.

## Reportar uma vulnerabilidade

**Não abras uma issue pública.** Usa o reporte privado do GitHub:
separador **Security → Report a vulnerability** neste repositório.

Diz-nos o que encontraste, como reproduzir e o impacto que vês. Respondemos assim
que possível e dizemos-te se confirmamos o problema e quando haverá correção.

## O que está no âmbito

* Acesso ao endpoint local sem o token, ou com um token que não devia servir.
* Pedidos de um site no browser (`Origin`) ou de outro anfitrião (`Host`) que o
  bridge aceite.
* Emparelhamento forjado ou reutilizado: ligar um separador sem a prova do segredo.
* Respostas entregues ao cliente errado quando há vários clientes.
* Segredos ou conteúdo de pedidos em logs ou em ficheiros legíveis por outros utilizadores.

## Fora do âmbito

* Quem já controla o teu computador ou o teu utilizador do sistema (consegue ler o
  `~/.1fluxo/` e o separador do browser por qualquer via).
* O aviso do macOS ou do Windows na primeira abertura: os executáveis não são
  assinados, e isso está documentado no README.
* Falhas do próprio 1fluxo (a web app), que têm outro canal: [1fluxo.com](https://1fluxo.com).

## Versões

Só a última Release recebe correções.
