#!/usr/bin/env node
/**
 * Empacota o bridge como extensão do Claude Desktop (`.mcpb`).
 *
 * Um `.mcpb` é um zip com `manifest.json` na raiz, o código e o
 * `node_modules` de produção. Aqui: compila (`tsc`), monta uma pasta de
 * estágio só com o que vai no pacote, instala lá as dependências de runtime
 * (`ws`) sem as de desenvolvimento, e faz o zip com o `zip` do sistema — sem
 * dependências novas. O resultado vai para `dist-mcpb/` e é copiado para
 * `public/downloads/` da web app, de onde Definições → Ligação a IA o liga.
 *
 *   npm run pack:mcpb
 */

import { execSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
const manifest = JSON.parse(readFileSync(join(root, "manifest.json"), "utf8"));
if (manifest.version !== pkg.version) {
  throw new Error(`manifest.json (${manifest.version}) e package.json (${pkg.version}) com versões diferentes`);
}

const run = (cmd, cwd) => execSync(cmd, { cwd, stdio: "inherit" });

run("npm run build", root);

const out = join(root, "dist-mcpb");
const stage = join(out, "stage");
rmSync(out, { recursive: true, force: true });
mkdirSync(stage, { recursive: true });

cpSync(join(root, "manifest.json"), join(stage, "manifest.json"));
cpSync(join(root, "dist"), join(stage, "dist"), { recursive: true });
// O ícone da app serve de ícone da extensão.
const icon = join(root, "..", "..", "public", "icon-192.png");
if (existsSync(icon)) cpSync(icon, join(stage, "icon.png"));
// package.json de produção: só o que o runtime precisa.
writeFileSync(
  join(stage, "package.json"),
  JSON.stringify(
    { name: pkg.name, version: pkg.version, private: true, type: pkg.type, dependencies: pkg.dependencies },
    null,
    2
  ) + "\n"
);
run("npm install --omit=dev --ignore-scripts --no-audit --no-fund --no-package-lock", stage);

const fileName = `1fluxo-bridge-${pkg.version}.mcpb`;
const file = join(out, fileName);
const entries = ["manifest.json", "dist", "package.json", "node_modules", ...(existsSync(join(stage, "icon.png")) ? ["icon.png"] : [])];
run(`zip -r -q -X ${JSON.stringify(file)} ${entries.join(" ")}`, stage);

const downloads = join(root, "..", "..", "public", "downloads");
mkdirSync(downloads, { recursive: true });
cpSync(file, join(downloads, fileName));

console.log(`ok: ${file}`);
console.log(`copiado para public/downloads/${fileName}`);
