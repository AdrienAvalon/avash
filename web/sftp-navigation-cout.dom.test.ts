// @vitest-environment jsdom
// Audit du 12 septembre 2026 (C-front-7) : dans le panneau SFTP, chaque flèche
// réécrivait le tabindex de TOUTES les entrées (`focusin`) et reparcourait la
// liste entière (`querySelectorAll` puis `indexOf` dans `keydown`). Sur
// 10 000 entrées (/usr/lib, un dépôt de sauvegardes), la navigation au clavier
// saccadait. Le coût d'une frappe ne doit plus dépendre de la taille de la
// liste : on compare une liste de 100 entrées à une de 10 000.
import { describe, it, expect, beforeAll, vi } from "vitest";
import indexHtml from "./index.html?raw";
import { state, type Session } from "./etat";
import type { SftpEntry } from "./filters";

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

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

/** jsdom n'a ni matchMedia, ni document.fonts, ni requestIdleCallback. */
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
  Object.defineProperty(document, "fonts", {
    configurable: true,
    value: { load: () => Promise.resolve([]), ready: Promise.resolve(), add: () => {} },
  });
  window.requestIdleCallback = ((cb: () => void) => setTimeout(cb, 0)) as typeof window.requestIdleCallback;
}

let sftpOpenAt: (s: Session, path: string) => Promise<void>;

function entreesFactices(n: number): SftpEntry[] {
  return Array.from({ length: n }, (_, i) => ({ name: `f${String(i).padStart(5, "0")}`, is_dir: false, size: i, modified: 1_700_000_000 }));
}

/** Monte une liste de `n` entrées (plus « .. ») et attend son rendu. */
async function ouvrir(n: number): Promise<HTMLElement> {
  invoke.mockImplementation((cmd: string) => Promise.resolve(cmd === "sftp_list" ? entreesFactices(n) : []));
  const s = { id: 1, sftpPath: "", serie: false } as unknown as Session;
  state.sessions.set(1, s);
  state.active = 1;
  await sftpOpenAt(s, `/liste-${n}`);
  const list = document.getElementById("sftp-list")!;
  await vi.waitFor(() => expect(list.children.length).toBe(n + 1), { timeout: 20_000 });
  return list;
}

function fleche(key: "ArrowDown" | "ArrowUp"): void {
  (document.activeElement as HTMLElement).dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
}

type Mesure = { ecrituresParFrappe: number; parcours: number; msParFrappe: number };

/** Navigue aux flèches et relève, par frappe : les écritures d'attributs dans
 *  la liste (MutationObserver : tabindex et classe), les parcours complets de la
 *  liste (`querySelectorAll` sur le conteneur) et le temps (meilleure de cinq
 *  séries, pour écarter une pause du ramasse-miettes). Les séries alternent Bas
 *  et Haut pour rester loin des bords, où une flèche n'écrit rien : sur
 *  100 entrées, cinq séries de Bas d'affilée touchaient le fond de la liste.
 *
 *  Le temps mesuré est celui du panneau : un écouteur posé sur la liste, après
 *  ceux du panneau, arrête la touche avant la fenêtre. Mesuré le 12 septembre
 *  2026 sous jsdom avec 10 000 entrées : `focus()` coûte 0,15 ms quelle que
 *  soit la taille, mais une touche quelconque sur `body` coûte 88 ms, soit le
 *  prix d'un `document.querySelector(".modal-backdrop.open, …")` balayant tout
 *  le document (78 ms). Ce balayage vient des raccourcis globaux d'autres
 *  modules, qui interrogent le document avant de regarder la touche ; il est
 *  signalé à part et ne dit rien du coût propre du panneau. */
function mesurer(list: HTMLElement): Mesure {
  const arreter = (e: Event) => e.stopPropagation();
  list.addEventListener("keydown", arreter);
  (list.firstElementChild as HTMLElement).focus();
  for (let i = 0; i < 5; i++) fleche("ArrowDown"); // échauffement
  const qsa = vi.spyOn(list, "querySelectorAll");
  const obs = new MutationObserver(() => {});
  obs.observe(list, { attributes: true, subtree: true });
  const FRAPPES = 20;
  let meilleure = Infinity;
  for (let serie = 0; serie < 5; serie++) {
    const t0 = performance.now();
    const sens = serie % 2 === 0 ? "ArrowDown" : "ArrowUp";
    for (let i = 0; i < FRAPPES; i++) fleche(sens);
    meilleure = Math.min(meilleure, (performance.now() - t0) / FRAPPES);
  }
  const ecritures = obs.takeRecords().length;
  obs.disconnect();
  const parcours = qsa.mock.calls.length;
  qsa.mockRestore();
  list.removeEventListener("keydown", arreter);
  return { ecrituresParFrappe: ecritures / (5 * FRAPPES), parcours, msParFrappe: meilleure };
}

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  await import("./main");
  sftpOpenAt = (await import("./sftp")).sftpOpenAt;
  (await import("./i18n")).setLangue("fr");
});

describe("panneau SFTP : navigation au clavier à coût constant", () => {
  it("dix_mille_entrees_naviguent_a_cout_constant", async () => {
    const petit = mesurer(await ouvrir(100));
    const grand = mesurer(await ouvrir(10_000));

    // Écritures DOM : l'ancienne entrée et la nouvelle, quelle que soit la taille.
    expect(petit.ecrituresParFrappe).toBeLessThanOrEqual(4);
    expect(grand.ecrituresParFrappe).toBe(petit.ecrituresParFrappe);
    // Aucun parcours de toute la liste pendant la navigation.
    expect(grand.parcours).toBe(0);
    // Et le temps par frappe ne dépend pas de n (ratio < 3 entre 100 et 10 000).
    expect(grand.msParFrappe / Math.max(petit.msParFrappe, 0.001)).toBeLessThan(3);
  }, 60_000);
});
