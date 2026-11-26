// @vitest-environment jsdom
// La fenêtre des paramètres (26 novembre 2026, demandée par le mainteneur :
// « un vrai bouton pour les paramètres plutôt que de devoir taper Ctrl+K »).
// Elle montre l'état de chaque réglage à l'ouverture, et chaque contrôle fait
// ce que fait sa bascule de palette.
import { describe, it, expect, beforeAll, beforeEach, vi } from "vitest";
import indexHtml from "./index.html?raw";

const stockage = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", {
  value: {
    getItem: (k: string) => stockage.get(k) ?? null,
    setItem: (k: string, v: string) => void stockage.set(k, String(v)),
    removeItem: (k: string) => void stockage.delete(k),
    clear: () => stockage.clear(),
  },
  configurable: true,
  writable: true,
});

const ws = { readyState: 1 };
const rdp = vi.hoisted(() => ({ annoncerPartageClip: vi.fn(), rdpSessions: new Map() }));
const journal = vi.hoisted(() => {
  let actif = false;
  return {
    basculerJournal: vi.fn(() => {
      actif = !actif;
      localStorage.setItem("avash.journal.commandes", actif ? "1" : "0");
    }),
    journalOpen: vi.fn(),
    majReperesJournal: vi.fn(),
    purgerJournal: vi.fn(),
  };
});
const outils = vi.hoisted(() => ({ setFontSize: vi.fn() }));
vi.mock("./rdp", () => rdp);
vi.mock("./journal-commandes", () => journal);
vi.mock("./terminal-outils", () => outils);
// theme.ts lit renderHosts dans le module d'entrée, qui chargerait toute
// l'application.
vi.mock("./main", () => ({ renderHosts: vi.fn() }));

let parametres: typeof import("./parametres");
let prefs: typeof import("./prefs");
const apresLangue = vi.fn();

const $ = (id: string) => document.getElementById(id) as HTMLInputElement;
function changer(id: string, valeur?: string): void {
  const el = $(id);
  if (valeur === undefined) el.checked = !el.checked;
  else el.value = valeur;
  el.dispatchEvent(new Event("change", { bubbles: true }));
}

beforeAll(async () => {
  // jsdom n'a pas matchMedia, que theme.ts interroge au chargement.
  window.matchMedia = ((): MediaQueryList =>
    ({ matches: false, addEventListener: () => {}, removeEventListener: () => {} }) as unknown as MediaQueryList) as typeof window.matchMedia;
  document.body.innerHTML = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  (await import("./i18n")).setLangue("fr");
  prefs = await import("./prefs");
  parametres = await import("./parametres");
  parametres.brancherParametres({ langue: apresLangue });
  rdp.rdpSessions.set(1, { ws });
});

beforeEach(() => {
  stockage.clear();
  vi.clearAllMocks();
  $("parametres-modal").classList.remove("open");
});

describe("fenêtre des paramètres", () => {
  it("la roue crantée l'ouvre, Échap et « Fermer » la ferment", () => {
    $("parametres-btn").click();
    expect($("parametres-modal").classList.contains("open")).toBe(true);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect($("parametres-modal").classList.contains("open")).toBe(false);
    parametres.parametresOpen();
    $("parametres-close").click();
    expect($("parametres-modal").classList.contains("open")).toBe(false);
  });

  it("à l'ouverture, chaque contrôle montre le réglage en vigueur", () => {
    prefs.setPartageClipboard(false);
    prefs.setSonBureau(false);
    prefs.setSondeAuDemarrage(true);
    prefs.setDureeJournal(365);
    parametres.parametresOpen();
    expect($("param-clip").checked).toBe(false);
    expect($("param-son").checked).toBe(false);
    expect($("param-sante").checked).toBe(true);
    expect($("param-journal-duree").value).toBe("365");
    expect($("param-langue").value).toBe("fr");
  });

  it("une case change le réglage, sans attendre de fermeture", () => {
    parametres.parametresOpen();
    changer("param-son");
    expect(prefs.sonBureau()).toBe(false);
    changer("param-fermeture");
    expect(prefs.confirmerFermetureOnglet()).toBe($("param-fermeture").checked);
    changer("param-ctrl");
    expect(prefs.ctrlCopierColler(false)).toBe($("param-ctrl").checked);
  });

  it("le partage du presse-papiers prévient les bureaux ouverts, comme la palette", () => {
    parametres.parametresOpen();
    changer("param-clip");
    expect(prefs.partageClipboard()).toBe(false);
    expect(rdp.annoncerPartageClip).toHaveBeenCalledWith(ws);
  });

  it("le journal passe par sa bascule, et la durée purge les jours dépassés", () => {
    parametres.parametresOpen();
    changer("param-journal");
    expect(journal.basculerJournal).toHaveBeenCalledTimes(1);
    changer("param-journal-duree", "30");
    expect(prefs.dureeJournal()).toBe(30);
    expect(journal.purgerJournal).toHaveBeenCalled();
  });

  it("la taille du texte passe par setFontSize, qui la borne et la retient", () => {
    parametres.parametresOpen();
    changer("param-taille", "18");
    expect(outils.setFontSize).toHaveBeenCalledWith(18);
  });

  it("changer de langue retraduit la page et la barre latérale", () => {
    parametres.parametresOpen();
    changer("param-langue", "en");
    expect(document.getElementById("parametres-modal-title")!.textContent).toBe("Settings");
    expect(apresLangue).toHaveBeenCalled();
    expect(journal.majReperesJournal).toHaveBeenCalled();
    changer("param-langue", "fr");
  });
});
