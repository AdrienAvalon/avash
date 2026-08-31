// Localise une ligne d'hôte par son alias (SSH) ou une ligne de dossier par son nom.
// La liste est peuplée de façon asynchrone au démarrage : on patiente au lieu de
// conclure à l'absence sur la toute première interrogation.
async function chercheHote(alias) {
  for (const r of await $$("#host-list .host")) {
    const a = await r.$(".alias");
    if ((await a.getProperty("textContent")) === alias) return r;
  }
  return null;
}
export async function findHostRow(alias, timeout = 8000) {
  let trouve = null;
  await browser.waitUntil(
    async () => {
      trouve = await chercheHote(alias);
      return trouve !== null;
    },
    { timeout, timeoutMsg: `hôte « ${alias} » introuvable` },
  );
  return trouve;
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

// Attend que `port` soit prêt à recevoir l'app.
//
// Une seule connexion réussie ne suffit pas : elle prouve que le socket écoute,
// pas que le serveur est *revenu* l'écouter. Le serveur de test traite ses
// clients l'un après l'autre — notre sonde en est un — et l'app, elle, ne
// réessaie pas si son handshake tombe dans l'intervalle. D'où des échecs
// intermittents, uniquement en suite complète, où la machine est chargée.
//
// On exige donc DEUX connexions successives : la seconde n'est tentée qu'après
// fermeture de la première, ce qui vérifie que la boucle d'acceptation a bouclé.
// C'est bien un état qu'on attend, pas une durée.
import { connect } from "node:net";
export function waitForPort(port, timeout = 8000) {
  const deadline = Date.now() + timeout;
  const uneConnexion = () =>
    new Promise((ok, ko) => {
      const sock = connect(port, "127.0.0.1");
      sock.once("connect", () => { sock.end(); ok(); });
      sock.once("error", (e) => { sock.destroy(); ko(e); });
    });
  return new Promise((resolve, reject) => {
    const essayer = async () => {
      try {
        await uneConnexion();
        await uneConnexion(); // le serveur est revenu accepter
        resolve();
      } catch {
        if (Date.now() > deadline) reject(new Error(`port ${port} pas prêt à temps`));
        else setTimeout(() => void essayer(), 120);
      }
    };
    void essayer();
  });
}
