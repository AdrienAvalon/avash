// @vitest-environment jsdom
// Audit du 12 septembre 2026 (C-front-5) : la file des transferts SFTP était
// reconstruite à chaque événement de progression (`zone.innerHTML = ""` puis
// recréation, jusqu'à douze fois par seconde avec trois transferts). Le bouton
// « Annuler » disparaissait sous le focus toutes les 80 ms : au clavier, Tab
// jusqu'à lui puis Entrée était impossible ; à la souris, un appui sur l'ancien
// bouton et un relâcher sur le nouveau ne faisaient pas de clic. Les lignes sont
// désormais indexées par transfert et la progression ne touche que leur texte
// et la largeur de leur barre.
import { describe, it, expect, beforeAll, beforeEach, vi } from "vitest";
import indexHtml from "./index.html?raw";
import { state, type Session } from "./etat";
import type { SftpEntry } from "./filters";

const invoke = vi.hoisted(() => vi.fn());
const ecouteurs = vi.hoisted(() => new Map<string, (ev: { payload: unknown }) => void>());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((nom: string, f: (ev: { payload: unknown }) => void) => {
    ecouteurs.set(nom, f);
    return Promise.resolve(() => {});
  }),
}));
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

const ENTREES: SftpEntry[] = [
  { name: "a.bin", is_dir: false, size: 100, modified: null },
  { name: "b.bin", is_dir: false, size: 100, modified: null },
  { name: "c.bin", is_dir: false, size: 100, modified: null },
  { name: "d.bin", is_dir: false, size: 100, modified: null },
];

let sftpOpenAt: (s: Session, path: string) => Promise<void>;
/** Résolveurs des `sftp_download` en vol, par numéro de transfert. */
const enVol = new Map<number, { ok: (m: string) => void; ko: (e: unknown) => void }>();

function $(id: string): HTMLElement {
  return document.getElementById(id) as HTMLElement;
}

function entree(nom: string): HTMLElement {
  return [...document.querySelectorAll<HTMLElement>("#sftp-list .sftp-entry")].find((e) => e.getAttribute("aria-label") === nom)!;
}

function lignes(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>("#sftp-transferts .sftp-transfert")];
}

function ligne(nom: string): HTMLElement {
  return lignes().find((l) => l.querySelector(".nm")!.textContent!.includes(nom))!;
}

/** Télécharge une entrée au clavier (Entrée), comme un utilisateur. */
function telecharger(nom: string): void {
  entree(nom).dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
}

/** Le numéro de transfert choisi par le front pour le téléchargement de `nom`. */
function numero(nom: string): number {
  const appel = invoke.mock.calls.find((c) => c[0] === "sftp_download" && (c[1] as { remote: string }).remote.endsWith(`/${nom}`));
  return (appel![1] as { transfert: number }).transfert;
}

function progression(transfert: number, done: number, total: number): void {
  ecouteurs.get("sftp-progress")!({ payload: { id: 1, transfert, name: "", kind: "download", fichier: "", done, total, termines: 0, nombre: 1 } });
}

async function attendre(): Promise<void> {
  for (let i = 0; i < 4; i++) await new Promise((r) => setTimeout(r, 0));
}

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  await import("./main");
  sftpOpenAt = (await import("./sftp")).sftpOpenAt;
  (await import("./i18n")).setLangue("fr");
});

beforeEach(async () => {
  // Les lignes des tests précédents se retirent d'un clic une fois terminées.
  for (const [, r] of enVol) r.ko(new Error("fin du test"));
  enVol.clear();
  await attendre();
  for (const l of lignes()) l.click();
  invoke.mockReset();
  invoke.mockImplementation((cmd: string, args: { transfert?: number }) => {
    if (cmd === "sftp_list") return Promise.resolve(ENTREES);
    if (cmd === "sftp_download") {
      return new Promise<string>((ok, ko) => enVol.set(args.transfert!, { ok, ko }));
    }
    if (cmd === "sftp_annuler") return Promise.resolve(true);
    return Promise.resolve([]);
  });
  const s = { id: 1, sftpPath: "", serie: false } as unknown as Session;
  state.sessions.set(1, s);
  state.active = 1;
  await sftpOpenAt(s, "/srv");
  await vi.waitFor(() => expect(entree("a.bin")).toBeTruthy());
});

describe("file des transferts SFTP : la progression ne reconstruit pas les lignes", () => {
  it("le_bouton_annuler_garde_le_focus_pendant_la_progression", async () => {
    telecharger("a.bin");
    await attendre();
    const id = numero("a.bin");
    const bouton = ligne("a.bin").querySelector("button")!;
    expect(bouton.hidden).toBe(false);
    bouton.focus();
    expect(document.activeElement).toBe(bouton);

    progression(id, 10, 100);
    progression(id, 50, 100);
    progression(id, 75, 100);

    // Le même nœud, toujours focalisé : Entrée l'atteint.
    expect(document.activeElement).toBe(bouton);
    expect(bouton.isConnected).toBe(true);
    expect((ligne("a.bin").querySelector(".barre > span") as HTMLElement).style.width).toBe("75%");
    expect(ligne("a.bin").querySelector(".det")!.textContent).toContain("/");
  });

  it("la progression d'un transfert ne touche pas les lignes voisines", async () => {
    telecharger("a.bin");
    telecharger("b.bin");
    await attendre();
    const voisine = ligne("b.bin");
    const noeuds = [voisine, ...voisine.querySelectorAll("*")];
    progression(numero("a.bin"), 30, 100);
    expect(ligne("b.bin")).toBe(voisine);
    expect([voisine, ...voisine.querySelectorAll("*")]).toEqual(noeuds);
  });

  it("annuler une ligne en attente la laisse affichée comme annulée, sans l'effacer dans la foulée", async () => {
    for (const nom of ["a.bin", "b.bin", "c.bin", "d.bin"]) telecharger(nom);
    await attendre();
    // Trois partent, la quatrième attend son tour.
    const attente = ligne("d.bin");
    expect(attente.classList.contains("attente")).toBe(true);
    attente.querySelector("button")!.click();
    await attendre();
    expect(ligne("d.bin")).toBe(attente);
    expect(attente.classList.contains("annule")).toBe(true);
    // Elle ne part pas quand une place se libère.
    enVol.get(numero("a.bin"))!.ok("reçu");
    await attendre();
    expect(invoke.mock.calls.some((c) => c[0] === "sftp_download" && (c[1] as { remote: string }).remote.endsWith("/d.bin"))).toBe(false);
  });

  it("une ligne terminée s'efface d'un clic, une ligne en cours non", async () => {
    telecharger("a.bin");
    await attendre();
    ligne("a.bin").click();
    expect(ligne("a.bin")).toBeTruthy();
    enVol.get(numero("a.bin"))!.ko(new Error("Permission refusée"));
    await attendre();
    expect(ligne("a.bin").classList.contains("erreur")).toBe(true);
    expect($("sftp-status").textContent).toContain("Permission refusée");
    ligne("a.bin").click();
    expect(ligne("a.bin")).toBeUndefined();
  });
});
