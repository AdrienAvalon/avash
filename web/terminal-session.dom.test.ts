// @vitest-environment jsdom
// Onglets de terminal, montés avec un xterm factice : ce que l'audit du
// 12 septembre 2026 a relevé sur leur création, leur clavier, leur flux et leur
// fermeture. Chaque test nomme son constat.
import { describe, it, expect, beforeAll, beforeEach, afterEach, vi } from "vitest";
import indexHtml from "./index.html?raw";
import type { Host } from "./filters";

// Node déclare un `localStorage` inerte que jsdom ne remplace pas (voir
// prefs.test.ts) : un stockage mémoire, avant tout import qui le lit.
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

const invoke = vi.hoisted(() => vi.fn());
const ecouteurs = vi.hoisted(() => new Map<string, (ev: { payload: unknown }) => void>());
const dialogues = vi.hoisted(() => ({
  askPassword: vi.fn(),
  askConfirm: vi.fn(),
  askText: vi.fn(() => Promise.resolve(null)),
  collerDansTerminal: vi.fn(() => Promise.resolve()),
  intercepterCollageNatif: vi.fn(),
}));
const xt = vi.hoisted(() => {
  type Taille = { cols: number; rows: number };
  const terminaux: FauxTerminal[] = [];
  const webgls: FauxWebgl[] = [];
  class FauxTerminal {
    options: Record<string, unknown>;
    cols = 80;
    rows = 24;
    element: HTMLElement | null = null;
    touches: ((e: KeyboardEvent) => boolean) | null = null;
    surDonnees: ((d: string) => void) | null = null;
    surTaille: ((t: Taille) => void) | null = null;
    ecrits: { data: string; fin?: () => void }[] = [];
    constructor(o: Record<string, unknown>) { this.options = { ...o }; terminaux.push(this); }
    open(el: HTMLElement) {
      this.element = document.createElement("div");
      this.element.className = "xterm";
      const ta = document.createElement("textarea");
      ta.className = "xterm-helper-textarea";
      this.element.appendChild(ta);
      el.appendChild(this.element);
    }
    loadAddon(a: { activate(t: unknown): void }) { a.activate(this); }
    attachCustomKeyEventHandler(f: (e: KeyboardEvent) => boolean) { this.touches = f; }
    onData(f: (d: string) => void) { this.surDonnees = f; return { dispose() {} }; }
    onResize(f: (t: Taille) => void) { this.surTaille = f; return { dispose() {} }; }
    write(data: string, fin?: () => void) { this.ecrits.push({ data, fin }); }
    focus() { this.element?.querySelector("textarea")?.focus(); }
    dispose() { this.element?.remove(); this.element = null; }
    paste() {}
    getSelection() { return ""; }
  }
  class FauxFit {
    t: FauxTerminal | null = null;
    activate(t: FauxTerminal) { this.t = t; }
    dispose() {}
    fit() { if (!this.t) return; this.t.cols += 1; this.t.surTaille?.({ cols: this.t.cols, rows: this.t.rows }); }
  }
  class FauxWebgl {
    static echoue = false;
    perte: (() => void) | null = null;
    constructor() { webgls.push(this); }
    activate() { if (FauxWebgl.echoue) throw new Error("WebGL indisponible"); }
    onContextLoss(f: () => void) { this.perte = f; return { dispose() {} }; }
    dispose() {}
  }
  class FauxAddon { activate() {} dispose() {} }
  class FauxLiens { constructor(public gestionnaire: unknown) {} activate() {} dispose() {} }
  return { terminaux, webgls, FauxTerminal, FauxFit, FauxWebgl, FauxAddon, FauxLiens };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((nom: string, f: (ev: { payload: unknown }) => void) => { ecouteurs.set(nom, f); return Promise.resolve(() => {}); }),
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
    setFullscreen: () => Promise.resolve(),
  }),
}));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn() }));
vi.mock("./dialogues", () => ({ ...dialogues, MODALES_AU_DESSUS: ["confirm-modal", "ask-modal", "pass-modal"] }));
vi.mock("./xterm-charge", () => ({
  chargerXterm: () => Promise.resolve({
    Terminal: xt.FauxTerminal, FitAddon: xt.FauxFit, WebglAddon: xt.FauxWebgl,
    SearchAddon: xt.FauxAddon, SerializeAddon: xt.FauxAddon, WebLinksAddon: xt.FauxLiens,
  }),
}));

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

let mouvementReduit = false;
const hote = (alias: string): Host => ({ alias, hostname: "10.0.0.1", user: "root", port: 22, identity_file: null, proxy_jump: null, tags: [], folder: "" });
const pause = (ms: number) => new Promise((r) => setTimeout(r, ms));
const appels = (cmd: string) => invoke.mock.calls.filter((c) => c[0] === cmd);
/** Ce que le cœur rendrait sans rien à dire : aucun mot de passe requis,
 *  des listes vides pour tout ce qui énumère. */
const reponsesParDefaut = (cmd: string): unknown =>
  cmd === "host_needs_password" ? false : ["list_hosts", "rdp_hosts", "folders_list", "onglets_memorises"].includes(cmd) ? [] : undefined;

type Main = typeof import("./main");
let main: Main;
let etat: typeof import("./etat");
let setFontSize: (px: number) => void;
let setConfirmerFermetureOnglet: (b: boolean) => void;

/** La session la plus récente et son terminal factice. */
function derniere() {
  const s = [...etat.state.sessions.values()].at(-1)!;
  return { s, term: s.term as unknown as InstanceType<typeof xt.FauxTerminal> };
}

beforeAll(async () => {
  window.__AVASH_LANGUE = "fr";
  window.matchMedia = ((q: string): MediaQueryList =>
    ({ matches: q.includes("reduced-motion") ? mouvementReduit : false, media: q, addEventListener: () => {}, removeEventListener: () => {} }) as unknown as MediaQueryList) as typeof window.matchMedia;
  Object.defineProperty(document, "fonts", { configurable: true, value: { load: () => Promise.resolve([]), ready: Promise.resolve() } });
  window.requestIdleCallback = ((cb: () => void) => setTimeout(cb, 0)) as typeof window.requestIdleCallback;
  document.body.innerHTML = corpsIndex();
  invoke.mockImplementation((cmd: string) => Promise.resolve(reponsesParDefaut(cmd)));
  main = await import("./main");
  etat = await import("./etat");
  ({ setFontSize } = await import("./terminal-outils"));
  ({ setConfirmerFermetureOnglet } = await import("./prefs"));
});

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation((cmd: string) => Promise.resolve(reponsesParDefaut(cmd)));
  for (const f of Object.values(dialogues)) f.mockClear();
  dialogues.askConfirm.mockResolvedValue(true);
  mouvementReduit = false;
  setConfirmerFermetureOnglet(true);
});

afterEach(() => {
  for (const id of [...etat.state.sessions.keys()]) main.closeSession(id);
});

describe("création d'un terminal", () => {
  it("le_moteur_de_rendu_est_note_au_premier_terminal_et_a_la_perte_du_contexte", async () => {
    // C-front-14, contrat K8 : sous WebKitGTK sans compositing, personne ne
    // savait si le terminal rendait en WebGL ou en DOM ; le diagnostic le dit.
    await main.openSession(hote("srv-1"));
    expect(appels("diagnostic_noter_rendu")).toEqual([["diagnostic_noter_rendu", { rendu: "webgl" }]]);
    await main.openSession(hote("srv-2"));
    expect(appels("diagnostic_noter_rendu")).toHaveLength(1);
    xt.webgls.at(-1)!.perte!();
    expect(appels("diagnostic_noter_rendu").at(-1)).toEqual(["diagnostic_noter_rendu", { rendu: "dom" }]);
    expect(etat.state.rendu).toBe("dom");
  });

  it("un_lien_osc8_passe_par_open_external", async () => {
    // FS-3 : sans `linkHandler`, xterm activait un lien OSC 8 par un confirm()
    // natif en anglais puis window.open, hors de la liste blanche de Rust.
    const ouvrir = vi.spyOn(window, "open").mockImplementation(() => null);
    await main.openSession(hote("srv-1"));
    const { term } = derniere();
    const lien = term.options.linkHandler as { activate: (e: MouseEvent, uri: string) => void; allowNonHttpProtocols?: boolean };
    lien.activate(new MouseEvent("click"), "https://exemple.org/doc");
    expect(appels("open_external")).toEqual([["open_external", { url: "https://exemple.org/doc" }]]);
    expect(lien.allowNonHttpProtocols).toBe(false);
    expect(ouvrir).not.toHaveBeenCalled();
    ouvrir.mockRestore();
  });

  it("le_curseur_ne_clignote_pas_sous_prefers_reduced_motion", async () => {
    // C-front-10 : `cursorBlink: true` en dur, alors que la CSS coupe toute
    // animation pour qui l'a demandé au système.
    mouvementReduit = true;
    await main.openSession(hote("srv-1"));
    expect(derniere().term.options.cursorBlink).toBe(false);
    mouvementReduit = false;
    await main.openSession(hote("srv-2"));
    expect(derniere().term.options.cursorBlink).toBe(true);
  });

  it("le_collage_natif_passe_par_la_confirmation", async () => {
    // FS-2, contrat K9 : Maj+Inser, clic du milieu et l'événement `paste`
    // passaient à xterm puis au PTY sans la confirmation de Ctrl+Maj+V.
    await main.openSession(hote("srv-1"));
    const { term } = derniere();
    expect(dialogues.intercepterCollageNatif).toHaveBeenCalledTimes(1);
    const [conteneur, t] = dialogues.intercepterCollageNatif.mock.calls[0] as unknown as [HTMLElement, unknown];
    expect(conteneur.classList.contains("xterm-container")).toBe(true);
    expect(t).toBe(term);
  });

  it("les_libelles_d_onglet_perdent_leurs_controles_bidi", async () => {
    // FS-10 : un alias portant U+202E réordonnait le libellé et le titre.
    await main.openSession(hote("prod‮bd"));
    const { s } = derniere();
    expect(s.tab.querySelector(".label")!.textContent).toBe("prodbd");
    expect(document.getElementById("tb-name")!.textContent).toBe("prodbd — Avash");
  });
});

describe("clavier dans un terminal", () => {
  const touche = (key: string, mods: KeyboardEventInit = {}) => new KeyboardEvent("keydown", { key, cancelable: true, bubbles: true, ...mods });

  it("le_terminal_laisse_ctrl_b_au_shell_et_retient_ctrl_tab", async () => {
    // C-front-2 et C-front-3 : `false` = xterm n'envoie rien au shell et la
    // touche remonte aux raccourcis de la fenêtre.
    await main.openSession(hote("srv-1"));
    const { term } = derniere();
    const tab = touche("Tab", { ctrlKey: true });
    expect(term.touches!(tab)).toBe(false);
    expect(tab.defaultPrevented).toBe(true); // le focus ne quitte pas le terminal
    expect(term.touches!(touche("w", { ctrlKey: true }))).toBe(false);
    expect(term.touches!(touche("3", { ctrlKey: true }))).toBe(false);
    expect(term.touches!(touche("b", { ctrlKey: true }))).toBe(true);
    expect(term.touches!(touche("k", { ctrlKey: true }))).toBe(true);
    expect(term.touches!(touche("B", { ctrlKey: true, shiftKey: true }))).toBe(false);
    expect(term.touches!(touche("K", { ctrlKey: true, shiftKey: true }))).toBe(false);
  });

  it("ctrl_k_dans_un_terminal_va_au_shell_ctrl_maj_k_ouvre_la_palette", async () => {
    await main.openSession(hote("srv-1"));
    const ta = derniere().term.element!.querySelector("textarea")!;
    ta.focus();
    const palette = document.getElementById("palette")!;
    ta.dispatchEvent(touche("k", { ctrlKey: true }));
    expect(palette.classList.contains("open")).toBe(false);
    ta.dispatchEvent(touche("K", { ctrlKey: true, shiftKey: true }));
    expect(palette.classList.contains("open")).toBe(true);
    document.getElementById("palette-input")!.dispatchEvent(touche("Escape"));
  });
});

describe("flux du PTY", () => {
  it("la_sortie_pty_est_accusee_une_fois_apres_ecriture", async () => {
    // C-front-4, contrat K6 : sans accusé, rien ne retenait Rust ; la file
    // d'évaluation de la webview et celle de xterm grossissaient sans borne.
    await main.openSession(hote("srv-1"));
    await main.openSession(hote("srv-2"));
    const premiere = [...etat.state.sessions.values()].at(-2)!; // inactive
    const term = premiere.term as unknown as InstanceType<typeof xt.FauxTerminal>;
    ecouteurs.get("pty-output")!({ payload: { id: premiere.id, data: "abc", seq: 7 } });
    const ecrit = term.ecrits.at(-1)!;
    expect(ecrit.data).toBe("abc");
    expect(appels("pty_ack")).toHaveLength(0); // pas avant que xterm l'ait traité
    ecrit.fin!();
    expect(appels("pty_ack")).toEqual([["pty_ack", { id: premiere.id, seq: 7 }]]);
  });

  it("le_zoom_n_envoie_qu_un_redimensionnement_par_session", async () => {
    // C-front-13 : `setFontSize` appelait `fit()` (donc `onResize`, donc
    // `pty_resize` différé) puis `pty_resize` une seconde fois.
    await main.openSession(hote("srv-1"));
    await main.openSession(hote("srv-2"));
    await pause(150);
    invoke.mockClear();
    setFontSize(etat.state.terminalFontSize + 1);
    await pause(150);
    expect(appels("pty_resize")).toHaveLength(etat.state.sessions.size);
  });
});

describe("connexion", () => {
  it("une_cle_d_hote_changee_ne_consomme_pas_d_essai_et_chaque_mot_de_passe_saisi_est_essaye", async () => {
    // C-front-13 : la clé d'hôte oubliée puis réapprise consommait un des trois
    // essais, et le troisième mot de passe saisi n'était jamais essayé.
    const refus = ["[AVASH_HOST_KEY_CHANGED] la clé a changé", "[AVASH_PASSWORD_REQUIRED]", "[AVASH_PASSWORD_REQUIRED] refusé", "[AVASH_PASSWORD_REQUIRED] refusé", "[AVASH_PASSWORD_REQUIRED] refusé"];
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "pty_open") { const r = refus.shift(); return r ? Promise.reject(new Error(r)) : Promise.resolve(); }
      return Promise.resolve(reponsesParDefaut(cmd));
    });
    dialogues.askPassword.mockResolvedValue({ password: "secret", remember: false });
    await main.openSession(hote("srv-1"));
    expect(appels("known_hosts_forget")).toHaveLength(1);
    expect(appels("pty_open")).toHaveLength(5);
    expect(dialogues.askPassword).toHaveBeenCalledTimes(3);
    const { s } = derniere();
    expect(s.closed).toBe(true);
    expect(s.tab.title).toMatch(/trois/i);
  });

  it("la_reconnexion_serie_ne_rejette_pas", async () => {
    // C-SIL-11 : Entrée sur un onglet série mort relançait une connexion dont
    // l'échec partait en promesse rejetée non gérée.
    invoke.mockImplementation((cmd: string) => cmd === "serie_open" ? Promise.reject(new Error("port occupé")) : Promise.resolve(reponsesParDefaut(cmd)));
    await expect(main.openSerie({ chemin: "/dev/ttyUSB9", vitesse: 9600 })).rejects.toThrow("port occupé");
    const { s } = derniere();
    await expect(s.reconnect!()).resolves.toBeUndefined();
    expect(s.closed).toBe(true);
  });

  it("un_trousseau_indisponible_est_signale", () => {
    // C-SIL-8, contrat K1 : un trousseau en panne passait pour « pas de mot de
    // passe », et CredSSP échouait en « mot de passe refusé ».
    const message = "Le trousseau du système ne répond pas (org.freedesktop.secrets introuvable). Les mots de passe mémorisés ne peuvent pas être relus : saisis-les.";
    ecouteurs.get("trousseau-indisponible")!({ payload: { message } });
    expect(document.getElementById("toasts")!.textContent).toContain(message);
  });
});

// C-front-6 : fermer un onglet vivant ne demandait rien.
describe("fermeture d'un onglet", () => {
  const croix = () => document.querySelector<HTMLElement>("#tabs .tab.active .close")!;

  it("fermer_un_onglet_vivant_demande_confirmation", async () => {
    await main.openSession(hote("srv-1"));
    const { s } = derniere();
    expect(s.etat).toBe("live");
    dialogues.askConfirm.mockResolvedValue(false);
    croix().click();
    await pause(0);
    expect(dialogues.askConfirm).toHaveBeenCalledTimes(1);
    expect(String(dialogues.askConfirm.mock.calls[0][0])).toContain("srv-1");
    expect(appels("pty_close")).toHaveLength(0);
    expect(etat.state.sessions.has(s.id)).toBe(true);
    dialogues.askConfirm.mockResolvedValue(true);
    croix().click();
    await pause(0);
    expect(appels("pty_close")).toEqual([["pty_close", { id: s.id }]]);
    expect(etat.state.sessions.has(s.id)).toBe(false);
  });

  it("ctrl_w_sur_un_onglet_vivant_demande_confirmation", async () => {
    await main.openSession(hote("srv-1"));
    dialogues.askConfirm.mockResolvedValue(false);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "w", ctrlKey: true }));
    await pause(0);
    expect(dialogues.askConfirm).toHaveBeenCalledTimes(1);
    expect(appels("pty_close")).toHaveLength(0);
  });

  it("un_onglet_ferme_ou_en_connexion_se_ferme_sans_question", async () => {
    await main.openSession(hote("srv-1"));
    const { s } = derniere();
    ecouteurs.get("pty-closed")!({ payload: { id: s.id } });
    expect(s.etat).toBe("closed");
    croix().click();
    await pause(0);
    expect(dialogues.askConfirm).not.toHaveBeenCalled();
    expect(etat.state.sessions.has(s.id)).toBe(false);

    // En connexion : `pty_open` n'a pas encore répondu.
    invoke.mockImplementation((cmd: string) => cmd === "pty_open" ? new Promise(() => {}) : Promise.resolve(reponsesParDefaut(cmd)));
    void main.openSession(hote("srv-2"));
    await vi.waitFor(() => expect(derniere().s.etat).toBe("connecting"));
    croix().click();
    await pause(0);
    expect(dialogues.askConfirm).not.toHaveBeenCalled();
    expect(etat.state.sessions.size).toBe(0);
  });

  it("ne_plus_demander_ferme_sans_question", async () => {
    setConfirmerFermetureOnglet(false);
    await main.openSession(hote("srv-1"));
    croix().click();
    await pause(0);
    expect(dialogues.askConfirm).not.toHaveBeenCalled();
    expect(etat.state.sessions.size).toBe(0);
  });
});
