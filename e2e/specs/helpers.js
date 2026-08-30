// Localise une ligne d'hôte par son alias (SSH) ou une ligne de dossier par son nom.
export async function findHostRow(alias) {
  for (const r of await $$("#host-list .host")) {
    const a = await r.$(".alias");
    if ((await a.getProperty("textContent")) === alias) return r;
  }
  throw new Error(`hôte « ${alias} » introuvable`);
}
export async function findFolderRow(name) {
  for (const r of await $$("#host-list .folder-row")) {
    const f = await r.$(".fname");
    if ((await f.getProperty("textContent")) === name) return r;
  }
  throw new Error(`dossier « ${name} » introuvable`);
}
export async function folderExists(name) {
  try { await findFolderRow(name); return true; } catch { return false; }
}
// Le clic droit de WebdriverIO ne génère pas d'event `contextmenu` sous WebKitGTK :
// on le dispatche directement sur la ligne.
export async function openCtx(row) {
  await browser.execute((el) => {
    el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 120, clientY: 120 }));
  }, row);
}

// Démarre un serveur RDP de test dédié (identifiants test/test) sur `port`.
// Chaque spec RDP a le sien : aucun couplage entre specs.
import { spawn } from "node:child_process";
import { resolve } from "node:path";
export function startRdpServer(port) {
  return spawn("./target/release/test-rdp-server",
    ["--bind-addr", `127.0.0.1:${port}`, "--cert", "cert.pem", "--key", "key.pem",
     "--user", "test", "--pass", "test", "--sec", "hybrid"],
    { cwd: resolve("../test-rdp-server"), stdio: "ignore" });
}
