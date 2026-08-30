import { describe, it, expect, beforeEach } from "vitest";
import { CLIP_KEY, partageClipboard, setPartageClipboard } from "./prefs";

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
