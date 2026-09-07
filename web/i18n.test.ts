// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from "vitest";
import indexHtml from "./index.html?raw";

// Tout le code TypeScript du dossier, chargé en chaîne par Vite : un nouveau
// fichier entre dans la couverture sans rien changer ici. Les *.test.ts sont
// exclus plus bas (ils posent puis suppriment des clés jetables).
const SOURCES_TS = import.meta.glob("./*.ts", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

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

  it("la langue imposée par l'environnement prime sur la locale, pas sur le choix", () => {
    expect(lireLangue("de-DE", "fr")).toBe("fr");
    expect(lireLangue("fr-FR", "en")).toBe("en");
    expect(lireLangue("fr-FR", "xx")).toBe("fr"); // valeur inattendue : ignorée
    localStorage.setItem(CLE_LANGUE, "en");
    expect(lireLangue("fr-FR", "fr")).toBe("en");
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

// Extrait les littéraux de clé passés en PREMIER argument d'un appel `t(` :
// on scanne caractère par caractère depuis chaque `t(` en suivant la
// profondeur des parenthèses et l'état « dans une chaîne », dans le seul
// premier argument (arrêt à la première virgule de premier niveau, le reste
// étant les variables). Ainsi les appels ternaires `t(vnc ? "a" : "b")` sont
// couverts, là où la regex naïve `t(\s*"…"` les ratait ; les appels à clé
// calculée (`t(el.dataset.i18n!)`, `t(sonBureau(...))`) n'ont aucun littéral
// et restent hors couverture, sans fausse alerte.
//
// Un littéral n'est retenu que s'il est en position de VALEUR — précédé de
// `(`, `?` ou `:` — et jamais opérande d'une comparaison : sinon les
// conditions de ternaire comme `t(themePref === "system" ? …)` ou
// `t(s.etat === "inconnu" ? …)` feraient prendre « system », « light »,
// « inconnu » pour des clés (faux positifs vus le 7 septembre 2026).
function clesAppelees(source: string): string[] {
  const cles: string[] = [];
  const re = /\bt\(/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(source))) {
    let i = m.index + m[0].length;
    let profondeur = 0;
    let prec = "("; // dernier caractère utile ; on vient de consommer `t(`
    for (; i < source.length; i++) {
      const c = source[i];
      if (c === "\"" || c === "'") {
        const j = i + 1;
        let k = j;
        while (k < source.length && source[k] !== c) {
          if (source[k] === "\\") k++;
          k++;
        }
        // Clé de premier niveau en position de valeur uniquement.
        if (profondeur === 0 && (prec === "(" || prec === "?" || prec === ":")) cles.push(source.slice(j, k));
        i = k;
        prec = "\""; // neutre : la chaîne n'ouvre pas une position de valeur
        continue;
      }
      if (c === "`") {
        // Gabarit (dans les variables) : on saute jusqu'au dos-de-guillemet.
        let k = i + 1;
        while (k < source.length && source[k] !== "`") {
          if (source[k] === "\\") k++;
          k++;
        }
        i = k;
        prec = "`";
        continue;
      }
      if (c === "(" || c === "{" || c === "[") profondeur++;
      else if (c === ")" || c === "}" || c === "]") {
        if (profondeur === 0) break; // fin de l'appel `t(`
        profondeur--;
      } else if (c === "," && profondeur === 0) break; // fin du premier argument
      if (!/\s/.test(c)) prec = c;
    }
  }
  return cles;
}

describe("clés i18n effectivement demandées", () => {
  it("chaque clé appelée par t() dans le code et posée dans la page existe en français", () => {
    // Les *.test.ts posent puis suppriment des clés jetables (cle-de-test,
    // cle-inconnue, salut) ; les *.d.ts ne contiennent aucun appel : exclus.
    const sources = Object.entries(SOURCES_TS)
      .filter(([f]) => !f.endsWith(".test.ts") && !f.endsWith(".d.ts"))
      .flatMap(([, src]) => clesAppelees(src));

    const posees: string[] = [];
    // Les quatre attributs lus par appliquerLangue (i18n / -title /
    // -placeholder / -aria).
    for (const m of indexHtml.matchAll(/data-i18n(?:-\w+)?="([^"]+)"/g)) posees.push(m[1]);

    const demandees = [...new Set([...sources, ...posees])];
    const absentes = demandees.filter((k) => !(k in FR)).sort();
    // Aurait listé « connexion-impossible » (échec d'ouverture d'un port série,
    // web/main.ts) : clé jamais définie, affichée brute et sans le message du cœur.
    expect(absentes).toEqual([]);
  });
});
