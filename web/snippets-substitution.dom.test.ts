// @vitest-environment jsdom
// Audit du 12 septembre 2026 (couverture.md, section 5, point 2) : la
// substitution des variables `{{nom}}` d'un snippet n'avait que deux cas testés
// (filters.test.ts), et le flux d'envoi de snippets.ts qui la câble (champs de
// variables, aperçu, commande envoyée) aucun. Or c'est la commande exécutée sur
// un ou plusieurs serveurs : une valeur réinterprétée (motif `$&` d'un
// remplacement, variable imbriquée) y partirait telle quelle.
import { describe, it, expect, beforeAll, vi } from "vitest";
import indexHtml from "./index.html?raw";
import { renderSnippet, snippetVars, type Snippet } from "./filters";

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

function $(id: string): HTMLElement {
  return document.getElementById(id) as HTMLElement;
}

async function attendre(): Promise<void> {
  for (let i = 0; i < 6; i++) await new Promise((r) => setTimeout(r, 0));
}

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  await import("./main");
  (await import("./i18n")).setLangue("fr");
});

describe("substitution des variables d'un snippet", () => {
  it("une variable répétée prend partout la même valeur", () => {
    expect(renderSnippet("journalctl -u {{svc}} && systemctl status {{svc}}", { svc: "nginx" }))
      .toBe("journalctl -u nginx && systemctl status nginx");
  });

  it("les espaces autour du nom sont tolérés, à l'extraction comme à la substitution", () => {
    expect(snippetVars("ping {{ hote }}")).toEqual(["hote"]);
    expect(renderSnippet("ping {{ hote }}", { hote: "srv" })).toBe("ping srv");
  });

  it("une valeur n'est jamais lue comme un motif de remplacement", () => {
    // Avec une chaîne de remplacement au lieu d'une fonction, « $& » recopierait
    // « {{v}} » et « $$ » deviendrait « $ ».
    expect(renderSnippet("echo {{v}}", { v: "$& $1 $$ $`" })).toBe("echo $& $1 $$ $`");
  });

  it("une valeur qui contient {{…}} n'est pas substituée à son tour", () => {
    expect(renderSnippet("echo {{a}}", { a: "{{b}}", b: "secret" })).toBe("echo {{b}}");
  });

  it("une variable non renseignée devient vide, le reste de la commande est gardé", () => {
    expect(renderSnippet("tar czf {{archive}}.tgz {{dossier}}", { archive: "sauvegarde" })).toBe("tar czf sauvegarde.tgz ");
  });
});

describe("flux d'envoi : les champs de variables font la commande envoyée", () => {
  it("l'aperçu et snippet_send portent la commande substituée", async () => {
    const sn: Snippet = { id: "s1", name: "Service", command: "systemctl {{action}} {{svc}}", run: true, category: "" };
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "snippet_list") return Promise.resolve([sn]);
      if (cmd === "open_sessions") return Promise.resolve([{ id: 5, label: "root@srv" }]);
      if (cmd === "snippet_send") return Promise.resolve(1);
      return Promise.resolve([]);
    });

    $("snippets-btn").click();
    await attendre();
    ($("snippet-list").querySelector('[data-act="send"]') as HTMLButtonElement).click();
    await attendre();

    const champs = [...$("send-vars").querySelectorAll<HTMLInputElement>("input[data-var]")];
    expect(champs.map((c) => c.dataset.var)).toEqual(["action", "svc"]);
    champs[0].value = "restart";
    champs[1].value = "nginx";
    champs[1].dispatchEvent(new Event("input", { bubbles: true }));
    expect($("send-preview").textContent).toBe("systemctl restart nginx");

    $("send-form").dispatchEvent(new Event("submit", { cancelable: true }));
    await attendre();

    expect(invoke).toHaveBeenCalledWith("snippet_send", { sessionIds: [5], command: "systemctl restart nginx", run: true });
    expect($("send-modal").classList.contains("open")).toBe(false);
  });
});
