import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { CLIP_KEY, SANTE_DEMARRAGE_KEY, partageClipboard, setPartageClipboard, sonBureau, setSonBureau, sondeAuDemarrage, setSondeAuDemarrage } from "./prefs";

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
  });

  it("régler ne lève rien", () => {
    expect(() => setPartageClipboard(false)).not.toThrow();
    expect(() => setSonBureau(false)).not.toThrow();
    expect(() => setSondeAuDemarrage(true)).not.toThrow();
  });
});
