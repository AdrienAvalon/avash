// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : dans le flux « Envoyer » d'un snippet,
// l'écouteur d'Échap de snippets.ts fermait send-modal sans arrêter la
// propagation. Comme snippets.ts est évalué avant menu-hote.ts (menu-hote
// l'importe), son écouteur passait en premier et retirait la classe `open` de
// send-modal ; l'écouteur d'Échap de menu-hote.ts s'exécutait ensuite, ne voyait
// plus send-modal ouverte (donc sa garde MODALES_AU_DESSUS ne le protégeait pas),
// et fermait snippets-modal restée derrière. Scénario : Snippets -> « Envoyer »
// -> Échap pour revenir à la liste -> la liste avait disparu. Le bouton Annuler,
// lui, ne fermait que send-modal : les deux gestes divergeaient. Corrigé en
// alignant le flux d'envoi (et la copie SFTP) sur confirm/ask/pass :
// stopImmediatePropagation() dans leur écouteur ET ajout à MODALES_AU_DESSUS,
// pour que le comportement ne dépende pas de l'ordre d'enregistrement.
import { describe, it, expect, beforeAll, beforeEach, vi } from "vitest";
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
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn() }));

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

/** jsdom n'a pas matchMedia, que theme.ts (importé en cascade) appelle. */
function shimsNavigateur(): void {
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
}

let MODALES_AU_DESSUS: readonly string[];

function echap(): void {
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
}

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  // Importer ./main reproduit l'ordre de production : il importe menu-hote, qui
  // importe snippets et sftp — les trois écouteurs d'Échap s'enregistrent dans
  // l'ordre réel (snippets avant menu-hote).
  await import("./main");
  MODALES_AU_DESSUS = (await import("./dialogues")).MODALES_AU_DESSUS;
});

beforeEach(() => {
  invoke.mockClear();
  invoke.mockResolvedValue([]);
  for (const id of ["snippets-modal", "send-modal", "sftp-copier-modal"]) {
    document.getElementById(id)!.classList.remove("open");
  }
});

describe("Échap dans une modale ouverte par-dessus une autre", () => {
  it("echap_dans_envoyer_ne_ferme_pas_la_liste_des_snippets", () => {
    const snippets = document.getElementById("snippets-modal")!;
    const send = document.getElementById("send-modal")!;
    snippets.classList.add("open");
    send.classList.add("open");

    echap();

    expect(send.classList.contains("open")).toBe(false); // le geste voulu : revenir à la liste
    expect(snippets.classList.contains("open")).toBe(true); // la liste doit rester ouverte
  });

  it("echap_dans_copie_sftp_ne_ferme_pas_la_surface_derriere", () => {
    // sftp-copier-modal s'ouvre toujours par-dessus une autre surface ; on place
    // snippets-modal derrière pour exercer la garde du gestionnaire de menu-hote.
    const snippets = document.getElementById("snippets-modal")!;
    const copier = document.getElementById("sftp-copier-modal")!;
    snippets.classList.add("open");
    copier.classList.add("open");

    echap();

    expect(copier.classList.contains("open")).toBe(false);
    expect(snippets.classList.contains("open")).toBe(true);
  });

  it("send-modal et sftp-copier-modal sont dans MODALES_AU_DESSUS, en fin de liste", () => {
    // Garde jumelle du stopImmediatePropagation : elle rend le comportement
    // indépendant de l'ordre d'enregistrement (si menu-hote passait en premier,
    // sa garde verrait la modale encore ouverte et ne fermerait pas le dessous).
    // En fin de liste pour que confirm/ask/pass gardent la priorité du piège de
    // focus s'ils s'ouvrent encore par-dessus.
    expect(MODALES_AU_DESSUS).toContain("send-modal");
    expect(MODALES_AU_DESSUS).toContain("sftp-copier-modal");
    expect(MODALES_AU_DESSUS.slice(0, 3)).toEqual(["confirm-modal", "ask-modal", "pass-modal"]);
  });
});
