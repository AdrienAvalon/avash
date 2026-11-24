// @vitest-environment jsdom
// Fenêtre du journal des commandes (24 novembre 2026) : ce qu'elle montre, ce
// qu'elle demande au cœur, et le repère d'onglet qui dit que le journal tourne.
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import indexHtml from "./index.html?raw";
import { state, type Session } from "./etat";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

// Même stockage que prefs.test.ts : le `localStorage` global de Node est inerte.
class StockageMemoire implements Storage {
  private m = new Map<string, string>();
  get length() { return this.m.size; }
  clear() { this.m.clear(); }
  getItem(k: string) { return this.m.get(k) ?? null; }
  key(i: number) { return [...this.m.keys()][i] ?? null; }
  removeItem(k: string) { this.m.delete(k); }
  setItem(k: string, v: string) { this.m.set(k, String(v)); }
}
const stockage = new StockageMemoire();
Object.defineProperty(globalThis, "localStorage", { value: stockage, configurable: true, writable: true });

const ENTREES = [
  { t: 1_795_500_060, s: 2, session: "prod-db", cible: "ada@10.0.0.6:22", commande: "psql -c 'select 1'", code: 1 },
  { t: 1_795_500_000, s: 1, session: "prod-web", cible: "ada@10.0.0.5:22", commande: "echo <img src=x onerror=window.piege=1>" },
];

let mod: typeof import("./journal-commandes");
let prefs: typeof import("./prefs");

beforeAll(async () => {
  // Le corps de la page tel quel : un script inséré par innerHTML ne s'exécute
  // jamais, inutile de le filtrer (le filtre faisait lever deux alertes CodeQL
  // sur la PR #113, sans objet ici).
  document.body.innerHTML = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  mod = await import("./journal-commandes");
  prefs = await import("./prefs");
});

beforeEach(() => {
  stockage.clear();
  invoke.mockReset();
  invoke.mockImplementation((cmd: string) => Promise.resolve(cmd === "journal_lister" ? ENTREES : null));
  document.getElementById("journal-modal")!.classList.remove("open");
  (document.getElementById("journal-filtre") as HTMLInputElement).value = "";
  state.sessions.clear();
});

const lignes = () => [...document.querySelectorAll("#journal-liste .journal-row")];

describe("fenêtre du journal des commandes", () => {
  it("montre les commandes en texte, jamais en HTML", async () => {
    await mod.journalOpen();
    expect(lignes()).toHaveLength(2);
    expect(document.querySelector("#journal-liste img")).toBeNull();
    expect(lignes()[1].querySelector(".commande")!.textContent).toBe("echo <img src=x onerror=window.piege=1>");
    expect(lignes()[0].querySelector(".code")!.textContent).toContain("1");
    expect(lignes()[0].querySelector(".ou")!.textContent).toBe("prod-db · ada@10.0.0.6:22");
  });

  it("filtre sur chaque mot, connexion et commande confondues", async () => {
    await mod.journalOpen();
    const filtre = document.getElementById("journal-filtre") as HTMLInputElement;
    filtre.value = "PROD-DB select";
    filtre.dispatchEvent(new Event("input"));
    expect(lignes()).toHaveLength(1);
    filtre.value = "introuvable";
    filtre.dispatchEvent(new Event("input"));
    expect(lignes()).toHaveLength(0);
    expect(document.querySelector("#journal-liste .empty")).not.toBeNull();
  });

  it("dit que le journal est coupé, et l'allume", async () => {
    await mod.journalOpen();
    const bascule = document.getElementById("journal-bascule")!;
    expect(prefs.journalGlobal()).toBe(false);
    const avant = bascule.textContent;
    bascule.click();
    expect(prefs.journalGlobal()).toBe(true);
    expect(bascule.textContent).not.toBe(avant);
  });

  it("purge selon la durée choisie", async () => {
    await mod.journalOpen();
    const duree = document.getElementById("journal-duree") as HTMLSelectElement;
    expect(duree.value).toBe("90");
    duree.value = "30";
    duree.dispatchEvent(new Event("change"));
    expect(prefs.dureeJournal()).toBe(30);
    expect(invoke).toHaveBeenCalledWith("journal_purger", { jours: 30 });
  });

  it("exporte avec le décalage horaire du poste, sans chemin venu de la page", async () => {
    await mod.journalOpen();
    document.getElementById("journal-exporter")!.click();
    expect(invoke).toHaveBeenCalledWith("journal_exporter", { decalageMinutes: -new Date().getTimezoneOffset() });
  });
});

describe("repère d'onglet du journal", () => {
  function onglet(alias: string, serie = false): Session {
    const tab = document.createElement("div");
    tab.innerHTML = `<span class="state"></span><span class="label"></span>`;
    const s = { id: state.sessions.size + 1, alias, tab, closed: false, etat: "live", serie } as unknown as Session;
    state.sessions.set(s.id, s);
    return s;
  }

  it("suit le réglage et les exceptions, jamais sur une console série", () => {
    const web = onglet("prod-web");
    const db = onglet("prod-db");
    const serie = onglet("/dev/ttyUSB0", true);
    mod.majReperesJournal();
    expect(web.tab.classList.contains("journal")).toBe(false);
    prefs.setJournalGlobal(true);
    prefs.basculerJournalPour("prod-db");
    mod.majReperesJournal();
    expect(web.tab.classList.contains("journal")).toBe(true);
    expect(web.tab.querySelector<HTMLElement>(".journal-repere")!.hidden).toBe(false);
    expect(db.tab.classList.contains("journal")).toBe(false);
    expect(serie.tab.classList.contains("journal")).toBe(false);
    prefs.setJournalGlobal(false);
    mod.majReperesJournal();
    expect(web.tab.querySelector<HTMLElement>(".journal-repere")!.hidden).toBe(true);
  });

  it("le capteur d'un onglet note par le cœur, avec l'identifiant de l'onglet", async () => {
    const { Terminal } = await import("@xterm/xterm");
    const term = new Terminal({ cols: 80, rows: 24 });
    const s = onglet("prod-web");
    prefs.setJournalGlobal(true);
    const capteur = mod.brancherJournal(s, term)!;
    expect(capteur).toBeDefined();
    const afficher = (x: string) => new Promise<void>((r) => term.write(x, r));
    await afficher("$ ");
    for (const c of "uptime") { capteur.frappe(c); await afficher(c); }
    capteur.frappe("\r");
    await afficher("\r\n$ ");
    expect(invoke).toHaveBeenCalledWith("journal_noter", { id: s.id, commande: "uptime" });
    capteur.dispose();
    term.dispose();
  });
});
