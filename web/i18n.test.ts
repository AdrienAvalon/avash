// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from "vitest";

// Node déclare un `localStorage` global inerte que jsdom ne remplace pas (voir
// prefs.test.ts) : on installe un stockage mémoire conforme à l'API Storage
// AVANT d'importer le module, qui lit la langue mémorisée à son chargement.
class StockageMemoire implements Storage {
  private m = new Map<string, string>();
  get length() { return this.m.size; }
  clear() { this.m.clear(); }
  getItem(k: string) { return this.m.get(k) ?? null; }
  key(i: number) { return [...this.m.keys()][i] ?? null; }
  removeItem(k: string) { this.m.delete(k); }
  setItem(k: string, v: string) { this.m.set(k, String(v)); }
}
Object.defineProperty(globalThis, "localStorage", { value: new StockageMemoire(), configurable: true });

const { FR, EN, t, setLangue, langue, appliquerLangue, lireLangue, CLE_LANGUE } = await import("./i18n");

describe("dictionnaires", () => {
  it("l'anglais couvre chaque clé du français, et rien de plus", () => {
    const manquantes = Object.keys(FR).filter((k) => !(k in EN));
    const orphelines = Object.keys(EN).filter((k) => !(k in FR));
    expect(manquantes).toEqual([]);
    expect(orphelines).toEqual([]);
  });

  it("aucun texte n'est vide", () => {
    for (const d of [FR, EN]) for (const [k, v] of Object.entries(d)) expect(v.trim(), k).not.toBe("");
  });
});

describe("t()", () => {
  beforeEach(() => {
    localStorage.removeItem(CLE_LANGUE);
    setLangue("fr");
  });

  it("rend le français par défaut, l'anglais après bascule, et mémorise le choix", () => {
    expect(t("annuler")).toBe("Annuler");
    setLangue("en");
    expect(langue()).toBe("en");
    expect(t("annuler")).toBe("Cancel");
    expect(localStorage.getItem(CLE_LANGUE)).toBe("en");
  });

  it("retombe sur le français puis sur la clé : un oubli se voit, il ne casse rien", () => {
    setLangue("en");
    FR["cle-de-test"] = "Seulement en français";
    expect(t("cle-de-test")).toBe("Seulement en français");
    delete FR["cle-de-test"];
    expect(t("cle-inconnue")).toBe("cle-inconnue");
  });

  it("remplace les variables, toutes leurs occurrences", () => {
    FR["salut"] = "Bonjour {nom}, encore {nom} ({n})";
    expect(t("salut", { nom: "Ada", n: 3 })).toBe("Bonjour Ada, encore Ada (3)");
    delete FR["salut"];
  });
});

describe("appliquerLangue()", () => {
  it("ne remplace que le premier texte porteur de lettres et garde la structure", () => {
    document.body.innerHTML = `
      <label data-i18n="nom-du-fichier">Nom du fichier<input id="i" /></label>
      <label data-i18n="rdp"><input type="radio" /> RDP </label>
      <button data-i18n="annuler" data-i18n-title="fermer" title="Fermer"><svg></svg>Annuler</button>
      <input data-i18n-placeholder="filtrer-les-hotes" placeholder="Filtrer les hôtes…" />
      <button data-i18n-aria="rafraichir" aria-label="Rafraîchir"></button>`;
    setLangue("en");
    appliquerLangue();
    const labels = document.querySelectorAll("label");
    expect(labels[0].firstChild?.nodeValue).toBe("File name");
    expect(document.getElementById("i")).not.toBeNull();
    expect(labels[1].lastChild?.nodeValue).toBe(" RDP ");
    expect(labels[1].querySelector("input")).not.toBeNull();
    const bouton = document.querySelector("button")!;
    expect(bouton.querySelector("svg")).not.toBeNull();
    expect(bouton.textContent).toBe("Cancel");
    expect(bouton.title).toBe("Close");
    expect(document.querySelector("input[placeholder]")!.getAttribute("placeholder")).toBe("Filter hosts…");
    expect(document.querySelector("[aria-label]")!.getAttribute("aria-label")).toBe("Refresh");
    expect(document.documentElement.lang).toBe("en");
    setLangue("fr");
    appliquerLangue();
    expect(bouton.textContent).toBe("Annuler");
  });
});

describe("langue au premier lancement", () => {
  beforeEach(() => localStorage.removeItem(CLE_LANGUE));

  it("suit la locale du système : français pour fr*, anglais pour le reste", () => {
    expect(lireLangue("fr-FR")).toBe("fr");
    expect(lireLangue("fr-CA")).toBe("fr");
    expect(lireLangue("FR")).toBe("fr");
    expect(lireLangue("en-US")).toBe("en");
    expect(lireLangue("de-DE")).toBe("en");
    expect(lireLangue("")).toBe("en");
  });

  it("un choix mémorisé prime sur la locale", () => {
    localStorage.setItem(CLE_LANGUE, "fr");
    expect(lireLangue("de-DE")).toBe("fr");
    localStorage.setItem(CLE_LANGUE, "en");
    expect(lireLangue("fr-FR")).toBe("en");
    localStorage.setItem(CLE_LANGUE, "xx");
    expect(lireLangue("fr-FR")).toBe("fr"); // une valeur inattendue vaut « rien »
  });
});
