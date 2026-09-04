// Panneau SFTP sur une session SSH réelle (sshd local) : liste le répertoire
// distant, télécharge un dossier entier par la file des transferts, et copie
// un fichier vers un autre onglet SSH sans l'écrire sur le poste. Valide
// UI → commandes → russh/SFTP contre un vrai sshd.
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import { randomBytes } from "node:crypto";
import { findHostRow } from "./helpers.js";

/** Le dossier de réception de l'application : celui des téléchargements du
 *  bac à sable s'il existe, sinon le bac à sable lui-même (repli du cœur). */
function dossierDeReception() {
  const sandbox = process.env.AVASH_E2E_SANDBOX;
  for (const d of ["Downloads", "Téléchargements"]) {
    if (existsSync(join(sandbox, d))) return join(sandbox, d);
  }
  return sandbox;
}

async function ouvrirSessionEtPanneau() {
  await (await findHostRow("test-ssh")).doubleClick();
  await browser.waitUntil(async () => (await $$(".state.live")).length > 0,
    { timeout: 20000, timeoutMsg: "session SSH jamais live" });
  await browser.waitUntil(async () => $("#sftp-toggle").isEnabled(),
    { timeout: 5000, timeoutMsg: "bouton SFTP jamais activé" });
  // Le panneau est toujours dans la page : c'est sa classe « open » qui dit
  // s'il est déployé, pas isDisplayed.
  if (!(await $("#sftp-panel").getAttribute("class")).includes("open")) await $("#sftp-toggle").click();
  await browser.waitUntil(async () => (await $("#sftp-panel").getAttribute("class")).includes("open"), { timeout: 5000, timeoutMsg: "panneau SFTP jamais ouvert" });
  try {
    await browser.waitUntil(async () => (await $$("#sftp-list .sftp-entry")).length > 0, { timeout: 15000 });
  } catch (e) {
    const etat = await browser.execute(() => `${document.querySelector("#sftp-status")?.textContent} | chemin=${document.querySelector("#sftp-path")?.value} | liste=${document.querySelector("#sftp-list")?.textContent?.slice(0, 200)}`);
    throw new Error(`aucune entrée SFTP listée ; ${etat}`, { cause: e });
  }
}

/** Va dans un dossier distant par la barre de chemin. */
async function allerDans(chemin) {
  const barre = $("#sftp-path");
  await barre.click();
  await browser.keys(["Control", "a"]);
  await browser.keys(chemin);
  await browser.keys("Enter");
  await browser.waitUntil(async () => (await barre.getValue()) === chemin && (await $$("#sftp-list .sftp-entry")).length > 0,
    { timeout: 10000, timeoutMsg: `le panneau n'est pas allé dans ${chemin}` });
}

/** La ligne d'une entrée du panneau, par son nom. */
async function ligne(nom) {
  const noms = [];
  for (const el of await $$("#sftp-list .sftp-entry")) {
    // textContent, pas getText : WebKitWebDriver rend vide le texte d'une
    // ligne rognée par le panneau étroit.
    const n = await el.$(".nm").getProperty("textContent");
    if (n === nom) return el;
    noms.push(n);
  }
  const chemin = await $("#sftp-path").getValue();
  throw new Error(`entrée « ${nom} » absente du panneau (chemin ${chemin}) ; présentes : ${noms.join(", ")}`);
}

async function menuSur(el, action) {
  await browser.execute((e) => {
    e.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 200, clientY: 200 }));
  }, el);
  await $("#sftp-context").waitForDisplayed({ timeout: 3000 });
  await $(`#sftp-context [data-act="${action}"]`).click();
}

async function attendreTransfertFini(nom) {
  let etat = "";
  try {
    await browser.waitUntil(async () => {
      for (const el of await $$("#sftp-transferts .sftp-transfert")) {
        const texte = await el.getProperty("textContent");
        if (!texte.includes(nom)) continue;
        const classes = await el.getAttribute("class");
        etat = `${classes} : ${texte}`;
        if (classes.includes("fini")) return true;
        if (classes.includes("erreur") || classes.includes("annule")) throw new Error(etat);
      }
      return false;
    }, { timeout: 30000 });
  } catch (e) {
    throw new Error(`transfert « ${nom} » pas terminé ; dernier état : ${etat}`, { cause: e });
  }
}

describe("SFTP — panneau, dossier entier, copie vers un autre hôte", () => {
  const racine = mkdtempSync(join(tmpdir(), "avash-e2e-sftp-"));
  const arbre = join(racine, "arbre");
  const gros = randomBytes(300 * 1024 + 9);
  mkdirSync(join(arbre, "sous"), { recursive: true });
  writeFileSync(join(arbre, "a.txt"), "alpha");
  writeFileSync(join(arbre, "sous", "b.bin"), gros);
  const cible = join(racine, "cible");
  mkdirSync(cible);

  it("ouvre le panneau et affiche des entrées", async () => {
    await ouvrirSessionEtPanneau();
  });

  it("télécharge un dossier entier, sous-dossiers compris, par la file des transferts", async () => {
    await allerDans(racine);
    await menuSur(await ligne("arbre"), "download");
    await attendreTransfertFini("arbre");
    const recu = join(dossierDeReception(), "arbre");
    expect(readFileSync(join(recu, "a.txt"), "utf8")).toBe("alpha");
    expect(readFileSync(join(recu, "sous", "b.bin")).equals(gros)).toBe(true);
  });

  it("copie un fichier vers un autre onglet SSH sans l'écrire sur le poste", async () => {
    // Un second onglet vers le même sshd : c'est « l'autre hôte ».
    await (await findHostRow("test-ssh")).doubleClick();
    await browser.waitUntil(async () => (await $$(".state.live")).length >= 2,
      { timeout: 20000, timeoutMsg: "second onglet jamais live" });
    // Retour au premier onglet, dont le panneau est ouvert sur la racine.
    await $$(".tab")[0].click();
    await browser.waitUntil(async () => $("#sftp-panel").isDisplayed(), { timeout: 5000 });
    await allerDans(join(arbre, "sous"));
    await menuSur(await ligne("b.bin"), "copy-to");
    await $("#sftp-copier-modal").waitForDisplayed({ timeout: 3000 });
    const options = await $$("#sc-cible option");
    expect(options.length).toBe(1);
    await $("#sc-dossier").setValue(cible);
    await $("#sc-submit").click();
    await attendreTransfertFini("b.bin");
    expect(readFileSync(join(cible, "b.bin")).equals(gros)).toBe(true);
    // Rien n'est passé par le dossier de réception du poste.
    expect(existsSync(join(dossierDeReception(), "b.bin"))).toBe(false);
    expect(basename(cible)).toBe("cible");
  });
});
