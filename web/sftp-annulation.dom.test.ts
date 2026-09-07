// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : sur un transfert « en-cours »,
// annulerTransfert appelait sftp_annuler mais jetait le booléen rendu. Quand il
// vaut false — aucun drapeau inscrit sous cet id : fenêtre entre le clic et
// inscrire() (ouverture du canal en attente du verrou de session), ou copie
// directe menée par scp chez l'hôte source — le clic « Annuler » restait sans
// le moindre effet ni mot, et la ligne d'une copie directe offrait même ce
// bouton illusoire tout en restant figée à « 0 o ». Ces tests verrouillent que
// le bouton n'est proposé que si l'annulation peut aboutir, et qu'un refus du
// cœur se dit à l'utilisateur au lieu de passer sous silence.
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

let boutonAnnulerVisible: (etat: string, kind: string, direct: boolean) => boolean;
let annulerTransfert: (x: unknown) => Promise<void>;
let t: (cle: string, vars?: Record<string, string | number>) => string;

/** Une ligne de transfert minimale, telle qu'annulerTransfert la lit. */
function transfert(champs: { id: number; etat: string; nom: string; kind: string; direct: boolean }): unknown {
  return { fait: 0, total: 0, message: "", ...champs };
}

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  // ./main d'abord fixe l'ordre d'évaluation du cycle des modules (cf. les
  // autres tests DOM) ; il importe ./sftp, dont on récupère ensuite les exports.
  await import("./main");
  const mod = await import("./sftp");
  boutonAnnulerVisible = mod.boutonAnnulerVisible as typeof boutonAnnulerVisible;
  annulerTransfert = mod.annulerTransfert as typeof annulerTransfert;
  const i18n = await import("./i18n");
  t = i18n.t;
  i18n.setLangue("fr");
});

beforeEach(() => {
  invoke.mockClear();
  invoke.mockResolvedValue([]);
  const status = document.getElementById("sftp-status")!;
  status.textContent = "";
  status.className = "sftp-status";
});

describe("boutonAnnulerVisible : ne proposer « Annuler » que si ça peut aboutir", () => {
  it("cache le bouton d'une copie directe en cours (scp chez la source, non interruptible)", () => {
    expect(boutonAnnulerVisible("en-cours", "copie", true)).toBe(false);
  });

  it("le garde tant qu'une copie directe attend son tour (annulation encore locale)", () => {
    expect(boutonAnnulerVisible("attente", "copie", true)).toBe(true);
  });

  it("le garde pour un transfert relayé, un envoi ou un téléchargement en cours", () => {
    expect(boutonAnnulerVisible("en-cours", "copie", false)).toBe(true);
    expect(boutonAnnulerVisible("en-cours", "upload", false)).toBe(true);
    expect(boutonAnnulerVisible("en-cours", "download", false)).toBe(true);
  });

  it("ne le propose sur aucune ligne terminée", () => {
    for (const etat of ["fini", "erreur", "annule"]) {
      expect(boutonAnnulerVisible(etat, "download", false)).toBe(false);
    }
  });
});

describe("annulerTransfert : réagir au refus du cœur", () => {
  it("affiche un message d'échec quand sftp_annuler rend false (drapeau absent)", async () => {
    invoke.mockResolvedValueOnce(false);
    await annulerTransfert(transfert({ id: 7, etat: "en-cours", nom: "gros dossier", kind: "download", direct: false }));
    const status = document.getElementById("sftp-status")!;
    expect(status.textContent).toContain(t("sftp-annulation-impossible", { nom: "gros dossier" }));
    expect(status.className).toContain("err");
  });

  it("ne crie pas quand le cœur a bien pris l'annulation (sftp_annuler rend true)", async () => {
    invoke.mockResolvedValueOnce(true);
    await annulerTransfert(transfert({ id: 8, etat: "en-cours", nom: "archive", kind: "upload", direct: false }));
    expect(document.getElementById("sftp-status")!.textContent).toBe("");
  });

  it("annule localement une ligne en attente sans passer par le cœur", async () => {
    const x = transfert({ id: 9, etat: "attente", nom: "en file", kind: "copie", direct: true }) as { etat: string };
    await annulerTransfert(x);
    expect(invoke.mock.calls.some((c) => c[0] === "sftp_annuler")).toBe(false);
    expect(x.etat).toBe("annule");
  });
});
