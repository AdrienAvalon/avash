import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ctrlCopierColler, setCtrlCopierColler, tailleTexte, setTailleTexte, TAILLE_TEXTE_DEFAUT, CLIP_KEY, FERMETURE_KEY, SANTE_DEMARRAGE_KEY, confirmerFermetureOnglet, partageClipboard, setConfirmerFermetureOnglet, setPartageClipboard, sonBureau, setSonBureau, sondeAuDemarrage, setSondeAuDemarrage, JOURNAL_HOTES_KEY, JOURNAL_KEY, basculerJournalPour, dureeJournal, exceptionJournal, journalActifPour, journalGlobal, setDureeJournal, setJournalGlobal } from "./prefs";

// Node 26 déclare un `localStorage` global inerte que jsdom ne remplace pas :
// l'environnement DOM ne suffit donc pas ici. On installe un stockage conforme
// à l'API Storage — ce qui est testé est la politique (défaut, révocation,
// valeur inattendue), pas le moteur de stockage du navigateur.
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

// Ce réglage décide si le presse-papiers local part vers un serveur distant.
// Le défaut doit être explicite et la révocation doit survivre au redémarrage.
describe("partage du presse-papiers avec les bureaux RDP", () => {
  beforeEach(() => stockage.clear());

  it("vaut « partagé » quand rien n'a jamais été réglé", () => {
    expect(partageClipboard()).toBe(true);
  });

  it("retient le refus d'un lancement à l'autre", () => {
    setPartageClipboard(false);
    expect(stockage.getItem(CLIP_KEY)).toBe("0");
    expect(partageClipboard()).toBe(false);
  });

  it("retient le retour au partage", () => {
    setPartageClipboard(false);
    setPartageClipboard(true);
    expect(partageClipboard()).toBe(true);
  });

  it("ne tient une valeur inattendue que pour un refus explicite", () => {
    stockage.setItem(CLIP_KEY, "peut-être");
    expect(partageClipboard()).toBe(true);
  });
});

describe("sonde de santé au démarrage", () => {
  beforeEach(() => localStorage.removeItem(SANTE_DEMARRAGE_KEY));

  it("est coupée quand rien n'a jamais été réglé : une sonde n'est pas anodine", () => {
    expect(sondeAuDemarrage()).toBe(false);
  });

  it("retient l'activation, puis le retour au repos", () => {
    setSondeAuDemarrage(true);
    expect(sondeAuDemarrage()).toBe(true);
    expect(localStorage.getItem(SANTE_DEMARRAGE_KEY)).toBe("1");
    setSondeAuDemarrage(false);
    expect(sondeAuDemarrage()).toBe(false);
  });

  it("ne tient une valeur inattendue que pour un repos", () => {
    localStorage.setItem(SANTE_DEMARRAGE_KEY, "oui");
    expect(sondeAuDemarrage()).toBe(false);
  });
});

// Le son se négocie à l'ouverture de la session : le réglage vaut pour les
// connexions suivantes, joué par défaut, coupé depuis la palette.
describe("son des bureaux distants", () => {
  beforeEach(() => stockage.clear());

  it("est joué quand rien n'a jamais été réglé", () => {
    expect(sonBureau()).toBe(true);
  });

  it("retient la coupure, puis le retour du son", () => {
    setSonBureau(false);
    expect(sonBureau()).toBe(false);
    setSonBureau(true);
    expect(sonBureau()).toBe(true);
  });

  it("ne tient une valeur inattendue que pour du son", () => {
    stockage.setItem("avash.rdp.son", "muet ?");
    expect(sonBureau()).toBe(true);
  });
});

// Audit du 12 septembre 2026 (C-front-6) : fermer un onglet vivant ne demandait
// rien, alors que tout le reste de destructif passe par une confirmation. Le
// défaut demande ; « Ne plus demander » se retient et se règle à la palette.
describe("confirmation avant de fermer un onglet vivant", () => {
  beforeEach(() => stockage.clear());

  it("demande quand rien n'a jamais été réglé", () => {
    expect(confirmerFermetureOnglet()).toBe(true);
  });

  it("retient « ne plus demander », puis le retour à la question", () => {
    setConfirmerFermetureOnglet(false);
    expect(stockage.getItem(FERMETURE_KEY)).toBe("0");
    expect(confirmerFermetureOnglet()).toBe(false);
    setConfirmerFermetureOnglet(true);
    expect(confirmerFermetureOnglet()).toBe(true);
  });

  it("ne tient une valeur inattendue que pour une question", () => {
    stockage.setItem(FERMETURE_KEY, "jamais ?");
    expect(confirmerFermetureOnglet()).toBe(true);
  });
});

// Un navigateur en mode privé strict refuse tout accès au stockage, en lecture
// comme en écriture : chaque réglage retombe sur son défaut, et régler ne fait
// pas échouer l'appelant (le choix vaut alors pour la session en cours).
describe("stockage refusé par le navigateur", () => {
  class StockageRefuse implements Storage {
    get length(): number { throw new Error("stockage refusé"); }
    clear(): void { throw new Error("stockage refusé"); }
    getItem(): string | null { throw new Error("stockage refusé"); }
    key(): string | null { throw new Error("stockage refusé"); }
    removeItem(): void { throw new Error("stockage refusé"); }
    setItem(): void { throw new Error("stockage refusé"); }
  }

  beforeEach(() => {
    Object.defineProperty(globalThis, "localStorage", { value: new StockageRefuse(), configurable: true, writable: true });
  });
  afterEach(() => {
    Object.defineProperty(globalThis, "localStorage", { value: stockage, configurable: true, writable: true });
  });

  it("chaque réglage vaut son défaut : partagé, joué, sonde au repos", () => {
    expect(partageClipboard()).toBe(true);
    expect(sonBureau()).toBe(true);
    expect(sondeAuDemarrage()).toBe(false);
    expect(confirmerFermetureOnglet()).toBe(true);
  });

  it("régler ne lève rien", () => {
    expect(() => setConfirmerFermetureOnglet(false)).not.toThrow();
    expect(() => setPartageClipboard(false)).not.toThrow();
    expect(() => setSonBureau(false)).not.toThrow();
    expect(() => setSondeAuDemarrage(true)).not.toThrow();
  });
});

// Journal des commandes (24 novembre 2026) : il écrit sur le disque ce qu'on
// fait sur des serveurs, il doit donc être coupé tant qu'on ne l'a pas allumé,
// et une exception par connexion doit valoir dans les deux sens.
describe("journal des commandes : réglage et exceptions", () => {
  beforeEach(() => stockage.clear());

  it("est coupé quand rien n'a jamais été réglé", () => {
    expect(journalGlobal()).toBe(false);
    expect(journalActifPour("prod")).toBe(false);
  });

  it("une connexion peut être journalisée journal coupé, et exclue journal allumé", () => {
    basculerJournalPour("prod");
    expect(journalActifPour("prod")).toBe(true);
    expect(journalActifPour("autre")).toBe(false);
    setJournalGlobal(true);
    expect(stockage.getItem(JOURNAL_KEY)).toBe("1");
    expect(journalActifPour("autre")).toBe(true);
    basculerJournalPour("autre");
    expect(journalActifPour("autre")).toBe(false);
    expect(exceptionJournal("autre")).toBe(false);
  });

  it("une exception revenue au réglage général disparaît", () => {
    basculerJournalPour("prod"); // allumée, journal coupé
    basculerJournalPour("prod"); // revient à « coupé », comme le réglage
    expect(exceptionJournal("prod")).toBeNull();
    setJournalGlobal(true);
    // Elle suit désormais le réglage général.
    expect(journalActifPour("prod")).toBe(true);
  });

  it("des exceptions abîmées ne valent rien", () => {
    stockage.setItem(JOURNAL_HOTES_KEY, "{pas du json");
    expect(journalActifPour("prod")).toBe(false);
    stockage.setItem(JOURNAL_HOTES_KEY, JSON.stringify({ prod: "oui", web: true }));
    expect(exceptionJournal("prod")).toBeNull();
    expect(journalActifPour("web")).toBe(true);
    stockage.setItem(JOURNAL_HOTES_KEY, JSON.stringify(["prod"]));
    expect(journalActifPour("prod")).toBe(false);
  });

  it("la durée de conservation vaut 90 jours par défaut et reste bornée", () => {
    expect(dureeJournal()).toBe(90);
    setDureeJournal(365);
    expect(dureeJournal()).toBe(365);
    setDureeJournal(0);
    expect(dureeJournal()).toBe(1);
    setDureeJournal(1e9);
    expect(dureeJournal()).toBe(3650);
  });
});

// Ctrl+C / Ctrl+V « comme Windows Terminal » (26 novembre 2026) : d'office
// sous Windows, où c'est l'usage ; ailleurs, Ctrl+V sert au shell et à vim.
describe("Ctrl+C et Ctrl+V copient et collent", () => {
  beforeEach(() => stockage.clear());

  it("actif d'office sous Windows seulement", () => {
    expect(ctrlCopierColler(true)).toBe(true);
    expect(ctrlCopierColler(false)).toBe(false);
  });

  it("le choix de l'utilisateur prime sur le défaut du système, dans les deux sens", () => {
    setCtrlCopierColler(false);
    expect(ctrlCopierColler(true)).toBe(false);
    setCtrlCopierColler(true);
    expect(ctrlCopierColler(false)).toBe(true);
  });
});

// La taille du texte ne survivait pas à un redémarrage (26 novembre 2026).
describe("taille du texte des terminaux", () => {
  beforeEach(() => stockage.clear());

  it("vaut 14 sans réglage, et retient la valeur choisie", () => {
    expect(tailleTexte()).toBe(TAILLE_TEXTE_DEFAUT);
    setTailleTexte(18);
    expect(tailleTexte()).toBe(18);
  });

  it("une valeur abîmée ou hors bornes retombe sur le défaut", () => {
    for (const v of ["", "abc", "3", "500", "12.5"]) {
      stockage.setItem("avash.terminal.taille", v);
      expect(tailleTexte()).toBe(TAILLE_TEXTE_DEFAUT);
    }
  });
});

// Audit du 27 novembre 2026 (ME-2) : une valeur abîmée ou hors liste dans le
// stockage ne donne pas un historique démesuré, elle retombe sur 10 000.
describe("historique du terminal", () => {
  it("vaut 10 000 lignes par défaut et refuse une valeur hors liste", async () => {
    const prefs = await import("./prefs");
    localStorage.removeItem("avash.terminal.historique");
    expect(prefs.historiqueTerminal()).toBe(10000);
    prefs.setHistoriqueTerminal(1000);
    expect(prefs.historiqueTerminal()).toBe(1000);
    prefs.setHistoriqueTerminal(999_999);
    expect(prefs.historiqueTerminal()).toBe(1000);
    localStorage.setItem("avash.terminal.historique", "1e9");
    expect(prefs.historiqueTerminal()).toBe(10000);
  });
});
