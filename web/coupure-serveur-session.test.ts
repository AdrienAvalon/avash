// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : à la coupure côté serveur, `ws.onclose`
// ne coupait que l'observateur de taille (`ro.disconnect()`) ; le contexte
// WebAudio (créé au premier bloc de son, message [20]) et les écouteurs
// d'invalidation de rect (resize/scroll/visibilitychange) n'étaient relâchés que
// dans `closeRdp`. Un bureau avec son dont le serveur coupait, laissé ouvert sur
// l'incrustation « Connexion RDP fermée », gardait donc un flux de sortie audio
// ouvert sur le périphérique tant que l'onglet n'était pas fermé ou reconnecté.
//
// Le correctif extrait `terminerSession(s)` (ro + detachRect + audio), appelé et
// par `ws.onclose` et par `closeRdp`. Ce test verrouille ce que la fonction
// relâche : sans le correctif, `fermer()` du contexte audio n'était jamais
// appelé sur ce chemin.
import { describe, it, expect, beforeAll, vi } from "vitest";
import indexHtml from "./index.html?raw";

// rdp.ts câble des écouteurs et importe le cœur du front à l'évaluation ; on
// remplace ./main (effets de bord lourds) et on monte index.html pour que les
// autres modules trouvent leurs éléments, afin de n'exercer que
// `terminerSession`.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }) }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ setFullscreen: () => Promise.resolve() }) }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ readText: vi.fn(), writeText: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("./main", () => ({
  loadHosts: vi.fn(), renderHosts: vi.fn(), moveHostTo: vi.fn(), setupFolderDrop: vi.fn(),
  closeSession: vi.fn(), focusSession: vi.fn(), openSession: vi.fn(),
}));

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

let terminerSession: (s: { ro?: { disconnect(): void }; detachRect?: () => void; audio?: { fermer(): void } }) => void;

beforeAll(async () => {
  document.body.innerHTML = corpsIndex();
  ({ terminerSession } = await import("./rdp") as unknown as { terminerSession: typeof terminerSession });
});

describe("terminerSession — relâche les ressources locales d'un bureau coupé", () => {
  it("la coupure serveur ferme le contexte audio", () => {
    let fermetures = 0;
    const s = { audio: { fermer: () => { fermetures += 1; } } };
    // C'est exactement ce qu'appelle `ws.onclose` : sans le correctif, le
    // contexte audio n'était jamais fermé sur ce chemin.
    terminerSession(s);
    expect(fermetures).toBe(1);
  });

  it("coupe aussi l'observateur de taille et détache les écouteurs de rect", () => {
    let disconnects = 0, detaches = 0, fermetures = 0;
    const s = {
      ro: { disconnect: () => { disconnects += 1; } },
      detachRect: () => { detaches += 1; },
      audio: { fermer: () => { fermetures += 1; } },
    };
    terminerSession(s);
    expect([disconnects, detaches, fermetures]).toEqual([1, 1, 1]);
  });

  it("ne suppose aucune ressource présente : un bureau sans son ni observateur ne casse pas", () => {
    // `audio` n'existe qu'après un premier bloc de son ; un bureau lancé sans
    // RDPSND ou en `--sans-son` n'en a pas. Chaque relâchement est optionnel.
    expect(() => terminerSession({})).not.toThrow();
  });
});
