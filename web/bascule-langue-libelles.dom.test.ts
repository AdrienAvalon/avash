// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : plusieurs libellés produits par le
// code restaient en français quelle que soit la langue. Deux causes distinctes.
//
//  1. Des chaînes écrites en dur (« Enregistrer », « Installation… »,
//     « Connexion… », « Modifier « … » », « Variables : », « N var(s) »)
//     jamais passées par t() : françaises en permanence, y compris au premier
//     lancement d'un système anglais, alors que docs et README annoncent
//     « Anglais — fait ».
//  2. Le formulaire des tunnels lisait ses libellés d'astuce dans une constante
//     de module figée à la langue du chargement : après une bascule en cours de
//     session, tunnelSyncKind les réécrivait en français, écrasant la
//     traduction.
//
// Le volet DOM exerce le formulaire des tunnels après « Switch to English » ; le
// garde-fou sur les sources refuse toute réapparition d'un texte visible écrit
// en dur (le seul angle qui attrape aussi les cas voisins et les récidives).
import { describe, it, expect, beforeAll, vi } from "vitest";
import indexHtml from "./index.html?raw";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    isMaximized: () => Promise.resolve(false),
    minimize: vi.fn(),
    toggleMaximize: () => Promise.resolve(),
    close: vi.fn(),
    onResized: () => Promise.resolve(() => {}),
    onFocusChanged: () => Promise.resolve(() => {}),
    startResizeDragging: vi.fn(),
  }),
}));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn() }));
vi.mock("./dialogues", () => ({
  askPassword: vi.fn(() => Promise.resolve(null)),
  askConfirm: vi.fn(() => Promise.resolve(true)),
  askText: vi.fn(() => Promise.resolve(null)),
  collerDansTerminal: vi.fn(() => Promise.resolve()),
  MODALES_AU_DESSUS: ["confirm-modal", "ask-modal", "pass-modal"],
}));

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

/** jsdom n'a ni matchMedia, ni document.fonts, ni requestIdleCallback. */
function shimsNavigateur(): void {
  // Sous jsdom, navigator.language vaut « en-US » et localStorage est
  // indisponible : lireLangue() choisirait donc l'anglais dès le chargement du
  // module. Le bug de la constante figée (KIND_HINTS gelée à la langue de
  // chargement) serait alors gelé en anglais, invisible à une bascule vers
  // l'anglais. On impose le français au chargement pour que « Switch to
  // English » traverse réellement une bascule de langue (audit du 7 sept. 2026).
  window.__AVASH_LANGUE = "fr";
  window.matchMedia = ((): MediaQueryList =>
    ({
      matches: false,
      media: "",
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    }) as unknown as MediaQueryList) as typeof window.matchMedia;
  Object.defineProperty(document, "fonts", {
    configurable: true,
    value: { load: () => Promise.resolve([]), ready: Promise.resolve(), add: () => {} },
  });
  window.requestIdleCallback = ((cb: () => void) => setTimeout(cb, 0)) as typeof window.requestIdleCallback;
}

function $(id: string): HTMLElement {
  return document.getElementById(id) as HTMLElement;
}

let EN: Record<string, string>;
let setLangue: (l: "fr" | "en") => void;

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  await import("./main");
  await import("./tunnels");
  const i18n = await import("./i18n");
  EN = i18n.EN;
  setLangue = i18n.setLangue;
});

describe("bascule de langue : le formulaire des tunnels suit", () => {
  it("après « Switch to English », le bouton et l'astuce sont en anglais", () => {
    setLangue("en");
    // tunnelFormReset : branché sur #t-reset (on ne l'exporte pas pour ne pas
    // élargir l'API du module).
    $("t-reset").click();
    // Le type « local » donne un libellé de champ déterministe ; le change
    // déclenche tunnelSyncKind, qui lisait la constante figée avant le correctif.
    const local = document.querySelector<HTMLInputElement>('input[name="tkind"][value="local"]')!;
    local.checked = true;
    local.dispatchEvent(new Event("change"));

    // Écrit en dur « Enregistrer » avant le correctif : restait français.
    expect($("t-submit").textContent).toBe("Save");
    expect($("t-submit").textContent).toBe(EN["enregistrer"]);
    // Constante de module figée avant le correctif : restait « Port local
    // d'écoute » malgré la bascule.
    expect($("t-bind-label").textContent).toBe("Local listening port");
    expect($("t-bind-label").textContent).toBe(EN["port-local-d-ecoute"]);

    setLangue("fr");
  });
});

// Toutes les sources TypeScript du dossier (hors tests et déclarations).
const SOURCES_TS = import.meta.glob("./*.ts", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

describe("garde-fou : aucun texte visible écrit en dur", () => {
  it("aucune affectation .textContent/.placeholder ni title= à une chaîne de prose littérale", () => {
    // Une chaîne entre guillemets doubles, portant au moins deux lettres
    // consécutives, affectée à un texte visible ou posée en title= : c'est de la
    // prose qui doit passer par t(). Les cas typographiques purs (« … », « ✓ »,
    // « ⚠ ») n'ont pas deux lettres et ne sont pas visés ; `title="${t(...)}"`
    // non plus (le motif exige des lettres juste après le guillemet).
    const motifs = [
      /\.(?:textContent|placeholder)\s*=\s*"[^"]*[A-Za-zÀ-ÿ]{2}[^"]*"/g,
      /\btitle="[A-Za-zÀ-ÿ]{2}[^"]*"/g,
    ];
    const fautes: string[] = [];
    for (const [fichier, src] of Object.entries(SOURCES_TS)) {
      if (fichier.endsWith(".test.ts") || fichier.endsWith(".d.ts")) continue;
      for (const motif of motifs) {
        for (const m of src.matchAll(motif)) fautes.push(`${fichier} : ${m[0]}`);
      }
    }
    // Aurait listé tunnels.ts « Enregistrer »/title="Modifier", snippets.ts
    // « Enregistrer », cles.ts « Installation… », connexion-directe.ts
    // « Connexion… » avant le correctif.
    expect(fautes).toEqual([]);
  });
});
