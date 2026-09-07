// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : ouvrirMenuAuClavier attachait un
// écouteur keydown en capture sur le menu et ne le retirait que via fermer(),
// déclenché uniquement au clavier (Échap, Entrée, Tab). Une fermeture à la
// souris (clic global window, clic sur un item, perte de focus fenêtre) retire
// seulement la classe `open` sans appeler fermer() : l'écouteur restait
// attaché. Les menus étant des éléments statiques réutilisés, chaque cycle
// « ouverture clavier -> fermeture souris » empilait un écouteur de plus, si
// bien qu'une flèche Bas déplaçait le focus de plusieurs crans et Entrée
// déclenchait une action parasite (clic sur items[0], « Se connecter »).
// Ces tests verrouillent que l'écouteur ne s'accumule pas et que la navigation
// reste d'un cran par frappe après une fermeture souris.
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

/** jsdom n'a pas matchMedia, que theme.ts (importé en cascade) appelle au chargement. */
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

let ouvrirMenuAuClavier: (menu: HTMLElement, origine: HTMLElement) => void;

function fleche(menu: HTMLElement, key: string): void {
  menu.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
}

/** Simule une fermeture souris : retrait de `.open` puis clic global window,
 *  exactement ce que fait hideHostMenu (menu-hote.ts:90-91). */
function fermetureSouris(menu: HTMLElement): void {
  menu.classList.remove("open");
  window.dispatchEvent(new MouseEvent("click", { bubbles: true }));
}

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  // Importer ./main d'abord fixe l'ordre d'évaluation du cycle main <-> menu-hote
  // (sinon un binding est lu avant son initialisation, cf. onglets-restauration).
  await import("./main");
  const mod = await import("./menu-hote");
  ouvrirMenuAuClavier = mod.ouvrirMenuAuClavier;
});

beforeEach(() => {
  invoke.mockClear();
  invoke.mockResolvedValue([]);
});

describe("fuite d'écouteur clavier des menus contextuels", () => {
  it("garde une navigation d'un cran par frappe après une fermeture souris", () => {
    const menu = document.getElementById("host-context")!;
    const origine = document.createElement("div");
    origine.tabIndex = -1;
    document.body.appendChild(origine);
    const items = [...menu.querySelectorAll<HTMLElement>("[data-act]")].filter((i) => !i.hidden);

    // 1er cycle : ouverture clavier puis fermeture souris (sans passer par fermer()).
    menu.classList.add("open");
    ouvrirMenuAuClavier(menu, origine);
    expect(document.activeElement).toBe(items[0]);
    fermetureSouris(menu);

    // 2e ouverture clavier : si l'écouteur du 1er cycle est resté attaché, une
    // flèche Bas fait avancer le focus de deux crans (items[2]) au lieu d'un.
    menu.classList.add("open");
    ouvrirMenuAuClavier(menu, origine);
    expect(document.activeElement).toBe(items[0]);
    fleche(menu, "ArrowDown");
    expect(document.activeElement).toBe(items[1]);
  });

  it("ne déclenche qu'une seule action sur Entrée après un cycle clavier -> souris -> clavier", () => {
    const menu = document.getElementById("host-context")!;
    const origine = document.createElement("div");
    origine.tabIndex = -1;
    document.body.appendChild(origine);
    const items = [...menu.querySelectorAll<HTMLElement>("[data-act]")].filter((i) => !i.hidden);
    const clics: string[] = [];
    for (const it of items) it.addEventListener("click", () => clics.push(it.getAttribute("data-act")!));

    menu.classList.add("open");
    ouvrirMenuAuClavier(menu, origine);
    fermetureSouris(menu);

    menu.classList.add("open");
    ouvrirMenuAuClavier(menu, origine);
    // items[0] a le focus : Entrée doit cliquer « connect » une seule fois. Un
    // écouteur périmé du 1er cycle en aurait déclenché un second, parasite.
    fleche(menu, "Enter");
    expect(clics).toEqual(["connect"]);
  });

  it("ne clique rien sur Entrée quand le focus a quitté les items du menu", () => {
    const menu = document.getElementById("host-context")!;
    const origine = document.createElement("div");
    origine.tabIndex = -1;
    document.body.appendChild(origine);
    const items = [...menu.querySelectorAll<HTMLElement>("[data-act]")].filter((i) => !i.hidden);
    const clics: string[] = [];
    for (const it of items) it.addEventListener("click", () => clics.push(it.getAttribute("data-act")!));

    menu.classList.add("open");
    ouvrirMenuAuClavier(menu, origine);
    // Le focus part hors des items : sans garde, le repli `items[i] ?? items[0]`
    // cliquait items[0] (« Se connecter »), ouvrant une session non demandée.
    origine.focus();
    fleche(menu, "Enter");
    expect(clics).toEqual([]);
  });
});
