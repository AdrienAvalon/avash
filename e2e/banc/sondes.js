// Sondes du banc, posées dans la page ou autour de l'application pilotée.
//
// Une règle pour toutes les mesures de frappe : le t0 se prend DANS la page,
// juste avant d'y injecter le caractère, et le caractère entre par un
// InputEvent sur le textarea de xterm.js. C'est le même chemin sous Linux
// (tauri-driver) et sous Windows (serveur WebDriver embarqué, dont
// `browser.keys` double les caractères) : les deux plateformes mesurent la
// même chose, du front jusqu'au shell distant et retour, sans le temps du
// pilote WebDriver.
import { readdirSync, readlinkSync, realpathSync } from "node:fs";
import { APP, EMBARQUE, pidAppEmbarquee } from "../wdio.conf.js";
import { doubleCliquerHote } from "../specs/helpers.js";
import { arbreDeProcessus, releveProcessus } from "./commun.js";

/** PID de l'application pilotée. Embarqué : c'est le harnais qui l'a lancée.
 *  Linux (tauri-driver) : le processus dont l'exécutable est APP, le plus
 *  récent ; jamais par nom, une Avash ouverte sur le poste porterait le même. */
export function pidApplication() {
  if (EMBARQUE) return pidAppEmbarquee();
  const cible = realpathSync(APP);
  let trouve = null;
  for (const nom of readdirSync("/proc")) {
    if (!/^\d+$/.test(nom)) continue;
    try {
      if (readlinkSync(`/proc/${nom}/exe`) === cible && (trouve === null || Number(nom) > trouve)) trouve = Number(nom);
    } catch { /* processus d'un autre utilisateur, ou parti */ }
  }
  return trouve;
}

/** Mémoire et processeur de toute l'application : processus principal, page
 *  (WebKitWebProcess, msedgewebview2) et processus RDP. */
export async function releveApplication() {
  const pid = pidApplication();
  if (!pid) return { memoireMo: null, processeurS: null, vivants: 0 };
  return releveProcessus(await arbreDeProcessus(pid));
}

/** Processeur moyen de l'application sur une fenêtre, en % d'un cœur. */
export async function processeurSur(fenetreMs) {
  const a = await releveApplication();
  const t0 = Date.now();
  await browser.pause(fenetreMs);
  const b = await releveApplication();
  if (a.processeurS === null || b.processeurS === null) return null;
  return ((b.processeurS - a.processeurS) / ((Date.now() - t0) / 1000)) * 100;
}

/** Abonne la page aux sorties des terminaux. À poser une fois par
 *  application, avant la première mesure. */
export async function poserSonde() {
  await browser.execute(async () => {
    if (window.__banc) return;
    const m = {
      attente: null, frappes: [], pose: performance.now(), derniereSortie: performance.now(),
      sorties: 0, caracteres: 0, marque: null, images: null,
      // Sessions déjà vues : la première sortie d'une session inconnue est
      // l'arrivée de sa bannière ou de son invite (S7).
      ids: new Set(), connexion: null, connexions: [],
    };
    window.__banc = m;
    const i = window.__TAURI_INTERNALS__;
    await i.invoke("plugin:event|listen", {
      event: "pty-output",
      target: { kind: "Any" },
      handler: i.transformCallback((ev) => {
        const donnees = String(ev.payload.data);
        m.derniereSortie = performance.now();
        if (!m.ids.has(ev.payload.id)) {
          m.ids.add(ev.payload.id);
          if (m.connexion !== null) { m.connexions.push(m.derniereSortie - m.connexion); m.connexion = null; }
        }
        m.sorties++;
        m.caracteres += donnees.length;
        const a = m.attente;
        if (a && donnees.includes(a.touche)) {
          const t1 = performance.now();
          m.attente = null;
          requestAnimationFrame(() => m.frappes.push({ echo: t1 - a.t0, image: performance.now() - a.t0 }));
        }
      }),
    });
  });
}

/** Attend que les terminaux se taisent depuis `silenceMs` (invite du shell
 *  affichée, flot terminé). Un silence total depuis la pose vaut aussi : la
 *  sortie a pu arriver avant la sonde (vu le 22 novembre 2026). */
export async function attendreSilence(silenceMs = 700, delai = 20000) {
  await browser.waitUntil(
    () => browser.execute((s) => {
      const m = window.__banc;
      const t = performance.now();
      return t - m.derniereSortie > s && (m.sorties > 0 || t - m.pose > 2500);
    }, silenceMs),
    { timeout: delai, interval: 100, timeoutMsg: `les terminaux ne se sont jamais tus ${silenceMs} ms` },
  );
}

/** Une frappe mesurée dans le terminal visible : { echo, image } en ms.
 *  `touche` doit différer de la précédente (l'écho se reconnaît à sa
 *  présence dans la sortie). */
export async function frapperEtMesurer(touche) {
  const avant = await browser.execute((t) => {
    const m = window.__banc;
    const visibles = [...document.querySelectorAll("#terminal .xterm-container")].filter((c) => getComputedStyle(c).display !== "none");
    const zone = (visibles.at(-1) ?? document).querySelector(".xterm-helper-textarea");
    zone?.focus();
    const n = m.frappes.length;
    m.attente = { t0: performance.now(), touche: t };
    zone?.dispatchEvent(new InputEvent("input", { data: t, inputType: "insertText", bubbles: true }));
    return n;
  }, touche);
  await browser.waitUntil(
    () => browser.execute((n) => window.__banc.frappes.length > n, avant),
    { timeout: 5000, interval: 20, timeoutMsg: `écho de « ${touche} » jamais arrivé` },
  );
  return browser.execute(() => window.__banc.frappes.at(-1));
}

/** Tape un texte d'un seul tenant dans le terminal visible, par un
 *  InputEvent, sans mesure. Jamais par `browser.keys` : la frappe de
 *  tauri-driver avale une lettre doublée, et le nom aléatoire du bac à sable
 *  en contient parfois une (« cat: …/avash-e2e-pCJ89/flot.txt: Aucun fichier »,
 *  vu le 27 novembre 2026). */
export async function taperTexte(texte) {
  await browser.execute((t) => {
    const visibles = [...document.querySelectorAll("#terminal .xterm-container")].filter((c) => getComputedStyle(c).display !== "none");
    const zone = (visibles.at(-1) ?? document).querySelector(".xterm-helper-textarea");
    zone?.focus();
    zone?.dispatchEvent(new InputEvent("input", { data: t, inputType: "insertText", bubbles: true }));
  }, texte);
}

/** N frappes de lettres toutes différentes de la précédente. */
export async function serieDeFrappes(n) {
  const lettres = "abcdefghijklmnopqrstuvwxyz";
  const r = [];
  for (let i = 0; i < n; i++) r.push(await frapperEtMesurer(lettres[i % lettres.length]));
  return r;
}

/** Ouvre une session SSH sur l'hôte de test et attend son invite. Rend la
 *  durée (ms), mesurée dans la page, du geste à la première sortie de la
 *  nouvelle session (S7) : poignée de main, authentification, ouverture du
 *  canal et du PTY, premier octet du shell. */
export async function ouvrirSessionSsh(alias = "test-ssh") {
  const avant = await browser.execute(() => { const m = window.__banc; m.connexion = performance.now(); return m.connexions.length; });
  await doubleCliquerHote(alias);
  await browser.waitUntil(
    () => browser.execute((n) => window.__banc.connexions.length > n, avant),
    { timeout: 30000, interval: 50, timeoutMsg: `session ${alias} : aucune sortie en 30 s` },
  );
  await attendreSilence(700, 20000);
  return browser.execute(() => window.__banc.connexions.at(-1));
}

/** Surveille le rythme des images pendant un flot : la pire image (ms) dit si
 *  l'interface restait réactive. */
export async function surveillerImages() {
  await browser.execute(() => {
    const m = window.__banc;
    m.images = { derniere: performance.now(), pire: 0, lentes: 0, actif: true };
    const boucle = () => {
      const im = m.images;
      if (!im?.actif) return;
      const t = performance.now();
      const ecart = t - im.derniere;
      if (ecart > im.pire) im.pire = ecart;
      if (ecart > 50) im.lentes++;
      im.derniere = t;
      requestAnimationFrame(boucle);
    };
    requestAnimationFrame(boucle);
  });
}

export async function arreterSurveillance() {
  return browser.execute(() => {
    const im = window.__banc.images;
    im.actif = false;
    return { pireImageMs: im.pire, imagesLentes: im.lentes };
  });
}

/** Chronologie du démarrage de la page qui vient de s'ouvrir (S1). */
export async function chronologieDemarrage() {
  return browser.execute(() => {
    const n = performance.getEntriesByType("navigation")[0];
    return { dcl: n.domContentLoadedEventEnd, chargement: n.loadEventEnd };
  });
}

/** S1 : chaque fichier du banc démarre une application neuve. Sa chronologie
 *  et sa mémoire au repos (3 s après la liste) font un échantillon de plus. */
export async function mesurerDemarrage(verser) {
  const { dcl } = await chronologieDemarrage();
  verser("S1/dcl", { unite: "ms", sens: "bas" }, [dcl]);
  await browser.pause(3000);
  const r = await releveApplication();
  verser("S1/memoire-repos", { unite: "Mo", sens: "bas" }, [r.memoireMo]);
}

/** Connecte un bureau à distance par la connexion directe, en acceptant les
 *  questions de premier contact (empreinte du serveur, repli sans NLA d'un
 *  xrdp) comme le ferait l'utilisateur. Rend la durée (ms) du clic
 *  « Se connecter » à l'onglet vivant (S7). */
export async function connecterRdp({ hote, port, utilisateur, mdp }) {
  const vivants = await browser.execute(() => document.querySelectorAll(".tab .state.live").length);
  await $("#manual-btn").click();
  await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
  await browser.execute(() => {
    const r = document.querySelector('input[name="proto"][value="rdp"]');
    r.checked = true;
    r.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await $("#m-addr").setValue(hote);
  await $("#m-port").setValue(String(port));
  await $("#m-user").setValue(utilisateur);
  await $("#m-password").setValue(mdp);
  const t0 = Date.now();
  await $("#m-submit").click();
  await browser.waitUntil(async () => {
    if (await browser.execute(() => !!document.querySelector("#confirm-modal.open"))) {
      await $("#confirm-ok").click();
      return false;
    }
    return browser.execute((n) => document.querySelectorAll(".tab .state.live").length > n, vivants);
  }, { timeout: 60000, interval: 50, timeoutMsg: `bureau ${hote}:${port} jamais connecté` });
  return Date.now() - t0;
}

/** Lit le bandeau du bureau actif : { fps, moParS, ms } ou null. */
export async function lireBandeauRdp() {
  const hud = await browser.execute(() => {
    const visibles = [...document.querySelectorAll(".rdp-container")].filter((c) => getComputedStyle(c).display !== "none");
    return (visibles.at(-1) ?? document).querySelector(".rdp-hud")?.textContent ?? "";
  });
  const m = hud.match(/(\d+)\s*fps\s*·\s*([\d.]+)\s*(Ko|Mo)\/s\s*·\s*(\d+)\s*ms/);
  return m ? { fps: Number(m[1]), moParS: m[3] === "Mo" ? Number(m[2]) : Number(m[2]) / 1024, ms: Number(m[4]) } : null;
}

/** Active l'onglet de rang `rang` (0 = le premier ouvert). */
export async function activerOnglet(rang) {
  await browser.execute((r) => document.querySelectorAll(".tab")[r]?.dispatchEvent(new MouseEvent("click", { bubbles: true })), rang);
  await browser.pause(300);
}

/** Un chemin local tel que le serveur SFTP le nomme : sous Windows, OpenSSH
 *  Server présente `C:\Users\x` comme `/C:/Users/x` (comme sftp.spec.js). */
export const cheminDistant = (p) => (process.platform === "win32" ? `/${p.replace(/\\/g, "/")}` : p);

/** Lance, depuis le panneau SFTP de l'onglet actif, le téléchargement de
 *  `nom` qui se trouve dans le dossier distant `dossier`. N'attend pas sa
 *  fin : le transfert sert de charge pendant la mesure. */
export async function lancerTelechargementSftp(dossier, nom) {
  await browser.waitUntil(async () => $("#sftp-toggle").isEnabled(), { timeout: 10000, timeoutMsg: "bouton SFTP jamais activé" });
  if (!(await $("#sftp-panel").getAttribute("class")).includes("open")) await $("#sftp-toggle").click();
  await browser.execute((c) => {
    const el = document.getElementById("sftp-path");
    el.focus();
    el.value = c;
    el.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  }, dossier);
  await browser.waitUntil(async () => browser.execute((n) =>
    [...document.querySelectorAll("#sftp-list .sftp-entry .nm")].some((e) => e.textContent === n), nom),
  { timeout: 15000, timeoutMsg: `« ${nom} » absent du panneau SFTP` });
  await browser.execute((n) => {
    const ligne = [...document.querySelectorAll("#sftp-list .sftp-entry")].find((e) => e.querySelector(".nm")?.textContent === n);
    ligne.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 200, clientY: 200 }));
  }, nom);
  await $("#sftp-context").waitForDisplayed({ timeout: 3000 });
  await $('#sftp-context [data-act="download"]').click();
}

/** Attend la fin (ou l'échec) du transfert de `nom` ; rend son état. */
export async function attendreTransfert(nom, delai = 300000) {
  let etat = "";
  await browser.waitUntil(async () => {
    etat = await browser.execute((n) => {
      const el = [...document.querySelectorAll("#sftp-transferts .sftp-transfert")].find((e) => e.textContent.includes(n));
      return el ? el.className : "";
    }, nom);
    return /fini|erreur|annule/.test(etat);
  }, { timeout: delai, interval: 250, timeoutMsg: `transfert ${nom} jamais terminé` });
  return etat;
}
