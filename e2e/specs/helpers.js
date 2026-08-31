/** Cherche une ligne par son libellé, en tolérant un rendu concurrent.
 *
 *  `$$()` rend des références d'éléments qui deviennent CADUQUES dès que la
 *  liste est reconstruite — ce qui arrive tout seul : le tick des tunnels
 *  toutes les cinq secondes, l'arrivée d'un logo d'OS, un changement d'état de
 *  session. Interroger une référence caduque **lève** une erreur, et à
 *  l'intérieur d'un `waitUntil` cette erreur avorte l'attente au lieu de la
 *  faire réessayer.
 *
 *  C'est la cause d'une famille entière d'échecs qui ne se manifestaient que
 *  sur une machine occupée — là où la fenêtre entre les deux appels s'élargit
 *  assez pour qu'un rendu s'y glisse. On rend `null`, et l'appelant réessaie.
 */
export async function trouverLigne(selecteurLigne, selecteurNom, texte) {
  try {
    for (const r of await $$(selecteurLigne)) {
      if ((await r.$(selecteurNom).getProperty("textContent")) === texte) return r;
    }
  } catch {
    return null; // liste reconstruite pendant le parcours : on réessaiera
  }
  return null;
}

// Localise une ligne d'hôte par son alias (SSH) ou une ligne de dossier par son nom.
// La liste est peuplée de façon asynchrone au démarrage : on patiente au lieu de
// conclure à l'absence sur la toute première interrogation.
async function chercheHote(alias) {
  return trouverLigne("#host-list .host", ".alias", alias);
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
  const r = await trouverLigne("#host-list .folder-row", ".fname", name);
  if (!r) throw new Error(`dossier « ${name} » introuvable`);
  return r;
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

/** Attend qu'un bureau RDP soit connecté, et EXPLIQUE l'échec le cas échéant.
 *
 *  « jamais connecté » ne dit rien : ni pourquoi, ni où. Le processus RDP écrit
 *  pourtant ses raisons, et l'interface les garde (`rdp_diagnostic`). On les
 *  joint au message plutôt que de laisser un échec muet — c'est ce qui manquait
 *  pour comprendre les rares échecs intermittents de ces scénarios.
 */
export async function attendreBureauConnecte(quoi = "le bureau RDP") {
  try {
    await browser.waitUntil(async () => (await $$(".state.live")).length > 0, {
      timeout: 20000,
      timeoutMsg: `${quoi} ne s'est jamais connecté`,
    });
  } catch (e) {
    const diag = await browser.execute(async () => {
      const { invoke } = window.__TAURI__?.core ?? {};
      if (!invoke) return "(interface non instrumentée)";
      const ids = [...document.querySelectorAll(".tab")].map((_, i) => i + 1);
      const out = [];
      for (const id of ids) {
        try {
          const d = await invoke("rdp_diagnostic", { id });
          if (d) out.push(`onglet ${id} : ${d}`);
        } catch { /* pas une session RDP */ }
      }
      const err = document.querySelector(".rdp-closed-diag")?.textContent;
      if (err) out.push(`incrustation : ${err}`);
      return out.join("\n") || "(le processus RDP n'a rien écrit)";
    }).catch(() => "(diagnostic illisible)");
    throw new Error(`${e.message}\n--- diagnostic du processus RDP ---\n${diag}`);
  }
}
