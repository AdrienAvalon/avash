// Journal des commandes passées en SSH (demandé par le mainteneur le
// 24 novembre 2026), sur la session SSH réelle : on l'allume à la palette, on
// passe une commande, puis une commande qui lit un secret sans l'afficher
// (comme l'invite de `sudo`), et une commande précédée d'une espace. Le journal
// doit garder la première et la deuxième, jamais le secret tapé ni la commande
// « sans trace », ni dans la fenêtre ni sur le disque.
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { attendreSessionLive, doubleCliquerHote, ecouterSortiePty, sortiePty, taperDansLeTerminal } from "./helpers.js";

// `AVASH_HOME` pose le dossier de configuration sur `<bac à sable>/.config`,
// sur les trois systèmes (comme pour vnc-tls.spec.js et rdp-lecteur.spec.js).
const DOSSIER = join(process.env.AVASH_E2E_SANDBOX, ".config", "avash", "journal-commandes");
const surDisque = () => (existsSync(DOSSIER)
  ? readdirSync(DOSSIER).filter((n) => n.endsWith(".jsonl")).map((n) => readFileSync(join(DOSSIER, n), "utf8")).join("")
  : "");
// Un marqueur unique, sans deux caractères identiques consécutifs : la frappe
// synthétique en perd parfois un (voir enregistrement.spec.js).
const marqueur = (base) => `${base}-${String(Date.now()).replace(/(.)(?=\1)/g, "$1x")}`;
// Les lignes telles qu'un terminal les montre : sans séquences (OSC compris,
// que `sortiePty` laisse : fish entoure chaque commande d'un titre et des
// marqueurs OSC 133) et, après un retour chariot, seulement ce qui suit.
async function lignesAffichees() {
  const brut = String(await browser.execute(() => window.__sortiePty ?? ""));
  /* eslint-disable no-control-regex */
  const nu = brut
    .replace(/\x1b\][\s\S]*?(\x07|\x1b\\)/g, "")
    .replace(/\x1b\[[0-9;?<>=]*[ -/]*[@-~]/g, "")
    .replace(/\x1b[()][\s\S]|\x1b[=>78]/g, "");
  /* eslint-enable no-control-regex */
  return nu.split("\n").map((l) => l.split("\r").filter((m) => m.trim() !== "").at(-1)?.trim() ?? "");
}
const ligneVue = async (texte) => (await lignesAffichees()).includes(texte);

// Toutes les notifications affichées, gardées même après leur disparition : un
// journal qui ne peut pas écrire, ou un capteur qui ne se pose pas, le dit par
// une notification. De quoi trancher sur un échec : vu le 24 novembre 2026
// sous Windows (PR #113), commandes exécutées et journal absent du disque.
async function retenirLesNotifications() {
  await browser.execute(() => {
    if (window.__toastsVus) return;
    window.__toastsVus = [];
    const zone = document.getElementById("toasts");
    new MutationObserver(() => {
      for (const t of zone.querySelectorAll(".toast")) {
        if (!window.__toastsVus.includes(t.textContent)) window.__toastsVus.push(t.textContent);
      }
    }).observe(zone, { childList: true, subtree: true, characterData: true });
  });
}
async function constat() {
  const toasts = await browser.execute(() => window.__toastsVus ?? []);
  const capteurs = await browser.execute(() => window.__journalDiagnostic?.() ?? "absent");
  return `notifications vues : ${JSON.stringify(toasts)} ; capteurs : ${JSON.stringify(capteurs)} ; `
    + `journal sur le disque : ${JSON.stringify(surDisque())} ; `
    + `dossier ${DOSSIER} : ${existsSync(DOSSIER) ? readdirSync(DOSSIER).join(", ") || "vide" : "absent"} ; `
    + `terminal : ${JSON.stringify((await lignesAffichees()).slice(-6))} ; `
    // Le flux brut (marqueurs du test seulement) : de quoi rejouer en test
    // unitaire ce que le serveur a vraiment envoyé, ConPTY compris.
    + `flux brut : ${JSON.stringify(String(await browser.execute(() => window.__sortiePty ?? "")).slice(-2500))}`;
}

async function palette(recherche) {
  // Ctrl+Maj+K : la palette, y compris depuis le terminal (Ctrl+K y va au shell).
  await browser.keys(["Control", "Shift", "k"]);
  const input = await $("#palette-input");
  await input.waitForDisplayed({ timeout: 5000 });
  await input.setValue(recherche);
  // Le meilleur résultat est le premier ; d'autres peuvent suivre.
  await browser.waitUntil(async () => (await $$("#palette-results .item")).length > 0, { timeout: 5000 });
  const [premier] = await $$("#palette-results .item");
  const nom = await premier.getText();
  if (!nom.toLowerCase().includes(recherche.toLowerCase())) {
    throw new Error(`palette : « ${recherche} » donne d'abord « ${nom} »`);
  }
  await premier.click();
}

describe("Journal des commandes (SSH)", () => {
  it("note les commandes à l'écran, jamais un secret non affiché ni une ligne sans trace", async () => {
    // Coupé par défaut : on l'allume à la palette, avant toute session.
    const reglage = () => browser.execute(() => localStorage.getItem("avash.journal.commandes"));
    expect(await reglage()).not.toBe("1");
    await palette("Activer le journal");
    await browser.waitUntil(async () => (await reglage()) === "1",
      { timeout: 5000, timeoutMsg: "la palette n'a pas allumé le journal" });

    await ecouterSortiePty();
    await retenirLesNotifications();
    await doubleCliquerHote("test-ssh");
    await attendreSessionLive();
    await browser.waitUntil(async () => (await $$(".tab.journal")).length > 0,
      { timeout: 5000, timeoutMsg: "l'onglet ne montre pas que le journal tourne" });
    // Le shell a fini de démarrer quand plus rien n'arrive depuis un moment :
    // fish tapé pendant ses requêtes au terminal brouillait la ligne.
    let avant = -1;
    await browser.waitUntil(async () => {
      const n = await browser.execute(() => (window.__sortiePty ?? "").length);
      const calme = n > 0 && n === avant;
      avant = n;
      return calme;
    }, { timeout: 15000, interval: 700, timeoutMsg: "le shell n'a jamais fini de démarrer" });

    const commande = marqueur("journal-ok");
    await taperDansLeTerminal(`echo ${commande}`);
    await browser.keys("Enter");
    try {
      await browser.waitUntil(() => ligneVue(commande), { timeout: 10000 });
    } catch {
      throw new Error(`l'écho de ${commande} n'est jamais revenu ; le terminal affiche : ${JSON.stringify((await lignesAffichees()).slice(-8))}`);
    }
    // La commande est sur le disque.
    try {
      await browser.waitUntil(() => surDisque().includes(`echo ${commande}`), { timeout: 5000 });
    } catch {
      throw new Error(`« echo ${commande} » n'est pas dans le journal ; ${await constat()}`);
    }

    // Un secret lu sans écho, comme à l'invite de `sudo`. `read -s` vaut dans
    // bash, zsh et fish (le `read` de fish dessine lui-même la saisie, même
    // après `stty -echo`). On attend que le shell ait affiché la commande, puis
    // un temps pour qu'il se mette à lire.
    const fin = marqueur("lu");
    const secret = marqueur("secret");
    await taperDansLeTerminal(`read -s x; echo ${fin}`);
    await browser.keys("Enter");
    await browser.waitUntil(async () => (await sortiePty()).includes(`echo ${fin}`), { timeout: 10000 });
    await browser.pause(300);
    await taperDansLeTerminal(secret);
    await browser.keys("Enter");
    await browser.waitUntil(() => ligneVue(fin),
      { timeout: 10000, timeoutMsg: "la commande qui lit le secret n'a pas fini" });
    expect(await sortiePty()).not.toContain(secret);

    const sansTrace = marqueur("sans-trace");
    await taperDansLeTerminal(` echo ${sansTrace}`);
    await browser.keys("Enter");
    await browser.waitUntil(() => ligneVue(sansTrace), { timeout: 10000 });

    // Sur le disque : les deux commandes, ni le secret ni la ligne sans trace.
    try {
      await browser.waitUntil(() => surDisque().includes(fin), { timeout: 5000 });
    } catch {
      throw new Error(`la deuxième commande n'est pas dans le journal (« ${surDisque()} ») ; ${await constat()}`);
    }
    const disque = surDisque();
    expect(disque).toContain(`echo ${commande}`);
    expect(disque).toContain(`read -s x; echo ${fin}`);
    expect(disque).not.toContain(secret);
    expect(disque).not.toContain(sansTrace);

    // Dans la fenêtre, ouverte depuis le terminal.
    await palette("Journal des commandes");
    await browser.waitUntil(async () => (await $$("#journal-liste .journal-row")).length >= 2,
      { timeout: 5000, timeoutMsg: "la fenêtre du journal est restée vide" });
    const texte = await browser.execute(() => document.getElementById("journal-liste").textContent);
    expect(texte).toContain(`echo ${commande}`);
    expect(texte).toContain("test-ssh");
    expect(texte).not.toContain(secret);
    expect(texte).not.toContain(sansTrace);
    await browser.keys("Escape");
    await browser.waitUntil(
      () => browser.execute(() => !document.getElementById("journal-modal").classList.contains("open")),
      { timeout: 5000, timeoutMsg: "Échap n'a pas fermé le journal" },
    );
  });
});
