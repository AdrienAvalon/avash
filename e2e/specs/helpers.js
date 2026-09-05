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

// Double-clic sur un élément. Sous le serveur WebDriver embarqué (Windows,
// macOS, ou E2E_EMBARQUE=1), l'action « doubleClick » du protocole n'arrive
// jamais au DOM : les quatre passages Windows de la suite complète montraient
// des sessions SSH « jamais live » sans qu'un seul appel n'atteigne le cœur,
// alors que la connexion directe, ouverte par un clic simple, passait. On émet
// donc l'événement `dblclick` nous-mêmes, comme la souris l'aurait fait ;
// ailleurs, la vraie action garde toute sa valeur (elle traverse le pilote).
import { EMBARQUE } from "../wdio.conf.js";
export async function doubleCliquer(el) {
  if (!EMBARQUE) {
    await el.doubleClick();
    return;
  }
  await browser.execute((e) => {
    e.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true, view: window, detail: 2, button: 0 }));
  }, el);
}

/** Ouvre un hôte de la barre latérale par double-clic sur sa ligne.
 *
 *  La liste se reconstruit à chaque changement d'état d'une session (le voyant
 *  de la ligne) : une référence prise pendant la reconstruction est caduque, et
 *  un double-clic dessus tombe dans le vide sans erreur sous WebKitWebDriver.
 *  Vu sur le miroir GitLab, dans vue-partagee : le second double-clic suivait
 *  immédiatement le premier « live », et « session 2 jamais live », sans onglet
 *  mort ni sortie. On attend donc que la même ligne soit rendue deux fois de
 *  suite avant de cliquer : un état, pas une durée. */
export async function doubleCliquerHote(alias) {
  let ligne = await findHostRow(alias);
  await browser.waitUntil(async () => {
    const encore = await findHostRow(alias);
    const stable = encore.elementId === ligne.elementId;
    ligne = encore;
    return stable;
  }, { timeout: 5000, interval: 200, timeoutMsg: `la ligne « ${alias} » ne cesse d'être reconstruite` });
  await doubleCliquer(ligne);
}
export async function findFolderRow(name) {
  const r = await trouverLigne("#host-list .folder-row", ".fname", name);
  if (!r) throw new Error(`dossier « ${name} » introuvable`);
  return r;
}
export async function folderExists(name) {
  try { await findFolderRow(name); return true; } catch { return false; }
}
// Les noms des dossiers affichés, pour qu'un échec dise ce qu'il y avait.
async function listeDossiers() {
  try {
    return await Promise.all((await $$("#host-list .folder-row .fname")).map((e) => e.getProperty("textContent")));
  } catch {
    return ["(liste en reconstruction)"];
  }
}
// Attend qu'un dossier apparaisse après sa création par la modale.
//
// Vu une fois sur l'exécuteur Windows (2026-09-03) : « clients » absent après
// 8 s, sans qu'on sache si la création traînait ou si le nom saisi différait.
// On attend plus longtemps, et l'échec nomme les dossiers présents.
export async function attendreDossier(name, timeout = 15000) {
  try {
    await browser.waitUntil(async () => folderExists(name), { timeout });
  } catch (e) {
    throw new Error(
      `dossier « ${name} » absent après ${timeout} ms ; présents : ${JSON.stringify(await listeDossiers())}`,
      { cause: e },
    );
  }
}
// Le clic droit de WebdriverIO ne génère pas d'event `contextmenu` sous WebKitGTK :
// on le dispatche directement sur la ligne.
export async function openCtx(row) {
  await browser.execute((el) => {
    el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 120, clientY: 120 }));
  }, row);
}

/** Écoute ce que les sessions de terminal écrivent (`pty-output`), pour que
 *  l'échec d'une connexion dise ce que l'onglet affichait — la seule trace de
 *  la raison (clé introuvable, refus, hôte injoignable) est dans le terminal,
 *  et son rendu WebGL n'a pas de texte à lire dans le DOM. À poser AVANT le
 *  geste qui connecte. */
export async function ecouterSortiePty() {
  await browser.execute(() => {
    if (window.__sortiePty !== undefined) return;
    window.__sortiePty = "";
    const i = window.__TAURI_INTERNALS__;
    return i.invoke("plugin:event|listen", {
      event: "pty-output",
      target: { kind: "Any" },
      handler: i.transformCallback((ev) => { window.__sortiePty += ev.payload?.data ?? ""; }),
    });
  });
}

/** Ce que les terminaux ont écrit depuis `ecouterSortiePty`, sans les séquences d'échappement. */
export async function sortiePty() {
  const brut = await browser.execute(() => window.__sortiePty ?? "");
  // eslint-disable-next-line no-control-regex
  return String(brut).replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "").replace(/\r/g, "").trim();
}

/** Attend qu'une session de terminal soit live, et dit sinon ce que l'onglet affichait. */
export async function attendreSessionLive(quoi = "session SSH", minimum = 1) {
  try {
    await browser.waitUntil(async () => (await $$(".state.live")).length >= minimum,
      { timeout: 20000 });
  } catch (e) {
    // L'onglet mort porte la raison en infobulle (markClosed) : c'est le
    // message que le front a écrit, celui que pty-output ne transporte pas.
    const morts = await browser.execute(() => [...document.querySelectorAll(".tab.dead")].map((t) => t.title).join(" | "));
    throw new Error(`${quoi} jamais live ; onglet : ${morts || "(vivant, sans raison)"} ; le terminal disait :\n${await sortiePty()}`, { cause: e });
  }
}

// Démarre un serveur RDP de test dédié (identifiants test/test) sur `port`.
// Chaque spec RDP a le sien : aucun couplage entre specs.
import { spawn } from "node:child_process";
import { resolve } from "node:path";
// Le binaire d'un serveur de test, avec son suffixe sous Windows : libuv ne
// l'ajoute pas toujours quand le chemin porte un répertoire.
const EXE = process.platform === "win32" ? ".exe" : "";
// `surLigne`, s'il est donné, reçoit la sortie standard du serveur : c'est par
// elle que le scénario du lecteur partagé lit ce que le serveur a vu du dossier.
export function startRdpServer(port, surLigne) {
  const p = spawn(`./target/release/test-rdp-server${EXE}`,
    ["--bind-addr", `127.0.0.1:${port}`, "--cert", "cert.pem", "--key", "key.pem",
     "--user", "test", "--pass", "test", "--sec", "hybrid"],
    { cwd: resolve("../test-rdp-server"), stdio: surLigne ? ["ignore", "pipe", "ignore"] : "ignore" });
  if (surLigne) {
    p.stdout.setEncoding("utf8");
    p.stdout.on("data", (d) => surLigne(String(d)));
  }
  return p;
}

// Démarre le serveur VNC de test (mot de passe « test ») sur `port`. Sa sortie
// standard, une ligne par entrée reçue, va à `surLigne` : c'est par elle que
// le scénario lit ce que le serveur a compris d'une frappe.
export function startVncServer(port, surLigne, options = {}) {
  // VeNCrypt : un second port, TLS, relié au premier (test-vnc-server/src/vencrypt.rs).
  const tls = options.tlsPort
    ? ["--tls-port", String(options.tlsPort), "--cert", options.cert, "--key", options.key]
    : [];
  const p = spawn(`./target/release/test-vnc-server${EXE}`,
    ["--port", String(port), "--pass", "test", ...tls],
    { cwd: resolve("../test-vnc-server"), stdio: ["ignore", "pipe", "ignore"] });
  p.stdout.setEncoding("utf8");
  p.stdout.on("data", (d) => surLigne?.(String(d)));
  return p;
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
// C'est bien un état qu'on attend, pas une durée. Le délai plafond, lui, a été
// vu dépassé une fois en suite complète (34 fichiers en parallèle, 05/09/2026 :
// « port 33898 pas prêt à temps » dans le before all de rdp-reconnect, qui
// passait seul) : il couvre le démarrage d'un serveur sur une machine chargée.
import { connect } from "node:net";
export function waitForPort(port, timeout = 15000) {
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
      // L'application n'expose pas l'API Tauri globale (`window.__TAURI__`
      // est un objet vide) : ce diagnostic répondait toujours « interface non
      // instrumentée ». Les internes de la webview portent `invoke`, comme
      // pour `@tauri-apps/api` lui-même (vu en écrivant la sonde de mesure du
      // front, 2026-09-04).
      const invoke = window.__TAURI_INTERNALS__?.invoke;
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
