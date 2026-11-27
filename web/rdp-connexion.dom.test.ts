// @vitest-environment jsdom
// Onglets de bureau distant (RDP, VNC) : ce que l'audit du 12 septembre 2026 a
// relevé sur leur connexion, leurs messages et leur état. Monté avec une
// WebSocket factice, comme rdp-diagnostic-ordre.dom.test.ts.
import { describe, it, expect, beforeAll, beforeEach, vi } from "vitest";
import indexHtml from "./index.html?raw";

const invoke = vi.hoisted(() => vi.fn());
const presse = vi.hoisted(() => ({ readText: vi.fn(() => Promise.resolve("")), writeText: vi.fn(() => Promise.resolve()) }));
const dialogue = vi.hoisted(() => ({ open: vi.fn() }));
const dialogues = vi.hoisted(() => ({ askPassword: vi.fn(), askConfirm: vi.fn(() => Promise.resolve(false)) }));
const avis = vi.hoisted(() => ({ notify: vi.fn(), notifyErreur: vi.fn() }));
const principal = vi.hoisted(() => ({
  loadHosts: vi.fn(), renderHosts: vi.fn(), rafraichirLignes: vi.fn(), moveHostTo: vi.fn(), setupFolderDrop: vi.fn(),
  closeSession: vi.fn(), focusSession: vi.fn(), openSession: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }) }));
// La fenêtre : `isMinimized` et ses deux événements sont pilotés par les tests
// de la pause (audit du 27 novembre 2026, PR-5).
const fenetre = vi.hoisted(() => ({
  reduite: false,
  ecouteurs: [] as (() => void)[],
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    setFullscreen: () => Promise.resolve(),
    isMinimized: () => Promise.resolve(fenetre.reduite),
    onResized: (f: () => void) => { fenetre.ecouteurs.push(f); return Promise.resolve(() => {}); },
    onFocusChanged: (f: () => void) => { fenetre.ecouteurs.push(f); return Promise.resolve(() => {}); },
  }),
}));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => presse);
vi.mock("@tauri-apps/plugin-dialog", () => dialogue);
vi.mock("./main", () => principal);
vi.mock("./dialogues", () => dialogues);
vi.mock("./notifications", () => avis);
vi.mock("./verrous", () => ({ currentLocks: () => Promise.resolve(null) }));
vi.mock("./raccourcis", () => ({ orderedTabs: () => [], focusTab: vi.fn(), fermerOnglet: vi.fn() }));
vi.mock("./onglets-restauration", () => ({ majMemoireOnglets: vi.fn() }));
vi.mock("./dossiers", () => ({ openMoveModal: vi.fn() }));

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

class FauxWebSocket {
  static OPEN = 1;
  static toutes: FauxWebSocket[] = [];
  readyState = 1;
  binaryType = "";
  envoyes: Uint8Array[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: ArrayBuffer }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(public url: string) { FauxWebSocket.toutes.push(this); }
  send(m: Uint8Array): void { this.envoyes.push(m); }
  close(): void { this.readyState = 3; }
  /** Un message du processus de bureau distant. */
  recoit(...octets: number[]): void { this.onmessage!({ data: new Uint8Array(octets).buffer }); }
  recoitJson(code: number, v: unknown): void {
    const corps = new TextEncoder().encode(JSON.stringify(v));
    const m = new Uint8Array(1 + corps.length);
    m[0] = code;
    m.set(corps, 1);
    this.onmessage!({ data: m.buffer });
  }
}

const contexte = { putImageData: vi.fn(), drawImage: vi.fn() };
const u16 = (n: number) => [n & 0xff, (n >> 8) & 0xff];
const pause = (ms = 0) => new Promise((r) => setTimeout(r, ms));
const appels = (cmd: string) => invoke.mock.calls.filter((c) => c[0] === cmd);

type Rdp = typeof import("./rdp");
let rdp: Rdp;
let state: typeof import("./etat").state;

/** Ouvre un bureau et rend sa WebSocket, une fois `rdp_open` répondu. */
async function ouvrir(hostId = "b1", nom = "bureau-1"): Promise<{ id: number; ws: FauxWebSocket }> {
  await rdp.openRdp({ host: "10.0.0.9", port: 3389, user: "moi", password: "", hostId, name: nom });
  const id = [...rdp.rdpSessions.keys()].at(-1)!;
  return { id, ws: FauxWebSocket.toutes.at(-1)! };
}

beforeAll(async () => {
  window.__AVASH_LANGUE = "fr"; // jsdom annonce en-US : les textes attendus sont français
  (globalThis as unknown as { WebSocket: unknown }).WebSocket = FauxWebSocket;
  (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class { observe() {} disconnect() {} };
  (globalThis as unknown as { ImageData: unknown }).ImageData = class { constructor(public data: Uint8ClampedArray, public width: number, public height: number) {} };
  HTMLCanvasElement.prototype.getContext = (() => contexte) as unknown as typeof HTMLCanvasElement.prototype.getContext;
  window.matchMedia = ((): MediaQueryList =>
    ({ matches: false, addEventListener: () => {}, removeEventListener: () => {} }) as unknown as MediaQueryList) as typeof window.matchMedia;
  document.body.innerHTML = corpsIndex();
  rdp = await import("./rdp");
  state = (await import("./etat")).state;
});

beforeEach(() => {
  for (const id of [...rdp.rdpSessions.keys()]) rdp.closeRdp(id);
  FauxWebSocket.toutes = [];
  invoke.mockReset();
  invoke.mockImplementation((cmd: string) => Promise.resolve(cmd === "rdp_open" ? { port: 4000, token: "jeton" } : undefined));
  for (const f of [...Object.values(presse), ...Object.values(dialogues), ...Object.values(avis), principal.rafraichirLignes, dialogue.open, contexte.putImageData]) f.mockClear();
});

describe("état d'un bureau", () => {
  it("la_pastille_rdp_s_eteint_quand_la_websocket_se_ferme", async () => {
    // C-SIL-1 : la pastille se calculait sur la seule présence de la session ;
    // une coupure serveur la laissait verte tant que l'onglet restait ouvert.
    const { id, ws } = await ouvrir("b1");
    expect(rdp.etatBureau("b1")).toBe("connecting");
    ws.recoit(1, ...u16(1280), ...u16(800));
    expect(rdp.etatBureau("b1")).toBe("live");
    expect(principal.rafraichirLignes).toHaveBeenLastCalledWith(["rdp:b1"]);
    principal.rafraichirLignes.mockClear();
    ws.onclose!();
    expect(rdp.etatBureau("b1")).toBe("");
    expect(rdp.rdpSessions.get(id)!.etat).toBe("closed");
    expect(principal.rafraichirLignes).toHaveBeenCalledWith(["rdp:b1"]);
    expect(rdp.rdpSessions.get(id)!.tab.querySelector(".state")!.className).toBe("state closed");
  });

  it("une_reprise_repasse_l_onglet_en_connexion", async () => {
    // C-SIL-10, contrat K7 : pendant une redirection ou une reprise du canal
    // graphique, le processus refait toute la connexion après un premier [1] :
    // l'onglet restait « connecté », le canvas noir, sans un mot.
    const { id, ws } = await ouvrir("b2");
    ws.recoit(1, ...u16(1280), ...u16(800));
    const onglet = rdp.rdpSessions.get(id)!;
    ws.recoit(23);
    expect(onglet.etat).toBe("connecting");
    expect(onglet.tab.querySelector(".state")!.className).toBe("state connecting");
    const incrustation = onglet.canvas.parentElement!.querySelector(".rdp-connexion");
    expect(incrustation?.textContent).toContain("Reprise de la connexion");
    ws.recoit(1, ...u16(1280), ...u16(800));
    expect(onglet.etat).toBe("live");
    expect(onglet.canvas.parentElement!.querySelector(".rdp-connexion")).toBeNull();
  });

  it("une_incrustation_dit_que_la_connexion_est_en_cours", async () => {
    // C-front-15 : un bureau en connexion (TLS, NLA, repli TLS hérité : des
    // secondes) montrait un rectangle noir ; on double-cliquait une seconde fois.
    let repondre: (v: unknown) => void = () => {};
    invoke.mockImplementation((cmd: string) => cmd === "rdp_open" ? new Promise((r) => { repondre = r; }) : Promise.resolve(undefined));
    const ouverture = rdp.openRdp({ host: "10.0.0.9", port: 3389, user: "moi", password: "", hostId: "b3", name: "lent" });
    const id = [...rdp.rdpSessions.keys()].at(-1)!;
    const zone = rdp.rdpSessions.get(id)!.canvas.parentElement!;
    const incrustation = zone.querySelector(".rdp-connexion")!;
    expect(incrustation.textContent).toContain("Connexion à");
    expect(incrustation.textContent).toContain("lent");
    repondre({ port: 4000, token: "jeton" });
    await ouverture;
    expect(zone.querySelector(".rdp-connexion")).not.toBeNull(); // pas avant [1]
    FauxWebSocket.toutes.at(-1)!.recoit(1, ...u16(1280), ...u16(800));
    expect(zone.querySelector(".rdp-connexion")).toBeNull();
  });

  it("une_coupure_annule_les_minuteurs_de_redimensionnement", async () => {
    // Le rejeu de 120 ms posé par [1] survivait à la coupure : `sendResize`
    // relisait la taille d'un onglet mort (vu le 12 octobre 2026 en CI, où il
    // partait après la fin du fichier de test, `window` déjà détruit).
    const { ws } = await ouvrir("b5");
    vi.useFakeTimers();
    try {
      ws.recoit(1, ...u16(1280), ...u16(800));
      const mesure = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect");
      ws.onclose!();
      vi.advanceTimersByTime(5000);
      expect(mesure).not.toHaveBeenCalled();
      mesure.mockRestore();
    } finally {
      vi.useRealTimers();
    }
  });

  it("annuler_depuis_l_incrustation_ferme_l_onglet", () => {
    invoke.mockImplementation((cmd: string) => cmd === "rdp_open" ? new Promise(() => {}) : Promise.resolve(undefined));
    void rdp.openRdp({ host: "10.0.0.9", port: 3389, user: "moi", password: "", hostId: "b4", name: "lent" });
    const id = [...rdp.rdpSessions.keys()].at(-1)!;
    const zone = rdp.rdpSessions.get(id)!.canvas.parentElement!;
    zone.querySelector<HTMLButtonElement>('.rdp-connexion [data-act="annuler"]')!.click();
    expect(rdp.rdpSessions.has(id)).toBe(false);
    expect(appels("rdp_close")).toEqual([["rdp_close", { id }]]);
  });
});

describe("messages du bureau", () => {
  it("le_presse_papiers_d_un_bureau_en_arriere_plan_attend_son_focus", async () => {
    // FS-4 : un bureau hostile en arrière-plan écrasait le presse-papiers
    // pendant qu'on copiait une commande dans un onglet SSH.
    const a = await ouvrir("pa");
    const b = await ouvrir("pb");
    expect(state.active).toBe(b.id);
    a.ws.recoit(8, ...new TextEncoder().encode("rm -rf ~"));
    await pause();
    expect(presse.writeText).not.toHaveBeenCalled();
    b.ws.recoit(8, ...new TextEncoder().encode("ls"));
    await pause();
    expect(presse.writeText).toHaveBeenLastCalledWith("ls");
    rdp.focusRdp(a.id);
    await pause();
    expect(presse.writeText).toHaveBeenLastCalledWith("rm -rf ~");
    expect(presse.writeText).toHaveBeenCalledTimes(2);
  });

  it("un_json_de_forme_inattendue_n_empeche_pas_la_trame_suivante", async () => {
    // FS-8 : `[15]` + `null` levait dans `onmessage` (lecture de `.fichiers`).
    const { ws } = await ouvrir("j1");
    ws.recoit(15, ...new TextEncoder().encode("null"));
    ws.recoit(15, ...new TextEncoder().encode('{"fichiers":"a.txt"}'));
    ws.recoit(2, ...u16(0), ...u16(0), ...u16(1), ...u16(1), 1, 2, 3, 255);
    expect(contexte.putImageData).toHaveBeenCalledTimes(1);
    expect(ws.envoyes.at(-1)).toEqual(new Uint8Array([6])); // accusé de rendu
  });

  it("une_liste_de_fichiers_copies_trop_longue_reste_bornee_dans_la_pastille", async () => {
    const { id, ws } = await ouvrir("j2");
    ws.recoitJson(17, { fichier: "x".repeat(5000), fait: 1, total: 2, termines: 0, nombre: 1 });
    const pastille = rdp.rdpSessions.get(id)!.badge!;
    expect(pastille.hidden).toBe(false);
    expect(pastille.textContent!.length).toBeLessThan(400);
  });
});

describe("connexion d'un bureau enregistré", () => {
  it("un_echec_de_memorisation_du_mot_de_passe_rdp_est_notifie", async () => {
    // C-SIL-6 : « mémoriser » coché, trousseau en panne : rien n'était dit, et
    // le mot de passe était redemandé « sans raison » à la connexion suivante.
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "rdp_password_known") return Promise.resolve(false);
      if (cmd === "rdp_password_save") return Promise.reject(new Error("Secret Service absent"));
      return Promise.resolve(cmd === "rdp_open" ? { port: 4000, token: "jeton" } : undefined);
    });
    dialogues.askPassword.mockResolvedValue({ password: "secret", remember: true });
    await rdp.connectRdpSaved({ id: "m1", name: "bureau", host: "10.0.0.9", port: 3389, user: "moi", width: 0, height: 0, folder: "" });
    expect(avis.notifyErreur).toHaveBeenCalledWith(expect.stringContaining("Secret Service absent"));
    // Non mémorisé : le mot de passe saisi reste celui de la connexion.
    expect((appels("rdp_open")[0] as unknown[])[1]).toMatchObject({ password: "secret" });
  });

  it("la_raison_d_un_repli_sans_nla_est_nettoyee_avant_la_confirmation", async () => {
    // C-front-13 : la raison, reprise du serveur par le processus, partait
    // telle quelle dans la boîte ; ses retours à la ligne repoussaient les
    // boutons hors de l'écran.
    invoke.mockImplementation((cmd: string) =>
      cmd === "rdp_open" ? Promise.reject(new Error("[AVASH_RDP_SANS_NLA] pas de NLA\n\n\n\n\x1b[2Jici")) : Promise.resolve(undefined));
    await rdp.openRdp({ host: "10.0.0.9", port: 3389, user: "moi", password: "", name: "vieux" });
    const texte = String((dialogues.askConfirm.mock.calls[0] as unknown[])[0]);
    const titre = texte.split("\n\n")[0];
    expect(titre).toBe("vieux — pas de NLA [2Jici");
  });
});

describe("dossier partagé d'un bureau enregistré (contrat K12)", () => {
  it("le_dossier_partage_se_choisit_par_la_boite_native_et_se_retire", async () => {
    // C-ipc-1 : le champ libre et la boîte du greffon JavaScript laissaient la
    // page choisir quel dossier du poste servir au distant. Le chemin vient
    // désormais de la boîte native, qui le désigne côté Rust.
    invoke.mockImplementation((cmd: string) => Promise.resolve(cmd === "choisir_fichiers_locaux" ? ["/home/moi/Partage"] : undefined));
    const champ = document.getElementById("re-partage") as HTMLInputElement;
    expect(champ.readOnly).toBe(true);
    document.getElementById("re-partage-choisir")!.click();
    await vi.waitFor(() => expect(champ.value).toBe("/home/moi/Partage"));
    expect((appels("choisir_fichiers_locaux")[0] as unknown[])[1]).toMatchObject({ dossiers: true });
    expect(dialogue.open).not.toHaveBeenCalled();
    document.getElementById("re-partage-retirer")!.click();
    expect(champ.value).toBe("");
  });

  it("une_boite_annulee_ne_touche_pas_au_champ", async () => {
    invoke.mockImplementation((cmd: string) => Promise.resolve(cmd === "choisir_fichiers_locaux" ? [] : undefined));
    const champ = document.getElementById("re-partage") as HTMLInputElement;
    champ.value = "/srv/partage";
    document.getElementById("re-partage-choisir")!.click();
    await vi.waitFor(() => expect(appels("choisir_fichiers_locaux")).toHaveLength(1));
    await pause();
    expect(champ.value).toBe("/srv/partage");
  });
});

describe("collage au clavier dans un bureau RDP", () => {
  // Signalé sur Windows Server 2012 R2 le 24 novembre 2026 : copier dans une
  // autre application, revenir dans Avash sans cliquer dans le bureau, Ctrl+V :
  // le serveur collait l'ancien contenu. Le texte du poste n'était annoncé
  // qu'au clic.
  const texte = (m: Uint8Array) => new TextDecoder().decode(m.slice(1));
  const touche = (canvas: HTMLCanvasElement, type: "keydown" | "keyup", code: string, init: KeyboardEventInit = {}) =>
    canvas.dispatchEvent(new KeyboardEvent(type, { code, bubbles: true, cancelable: true, ...init }));
  const estV = (m: Uint8Array, enfonce: number) => m[0] === 4 && m[1] === 0x2f && m[3] === enfonce;

  it("ctrl_v_annonce_le_texte_du_poste_avant_la_touche", async () => {
    const { id, ws } = await ouvrir("c1");
    ws.recoit(1, ...u16(1280), ...u16(800));
    presse.readText.mockResolvedValue("copié ailleurs");
    const canvas = rdp.rdpSessions.get(id)!.canvas;
    touche(canvas, "keydown", "ControlLeft", { ctrlKey: true });
    touche(canvas, "keydown", "KeyV", { ctrlKey: true });
    // Relâchée pendant l'annonce : elle doit partir APRÈS l'enfoncement.
    touche(canvas, "keyup", "KeyV", { ctrlKey: true });
    await vi.waitFor(() => expect(ws.envoyes.some((m) => estV(m, 0))).toBe(true), { timeout: 1000 });
    const iAnnonce = ws.envoyes.findIndex((m) => m[0] === 8);
    const iEnfonce = ws.envoyes.findIndex((m) => estV(m, 1));
    const iRelache = ws.envoyes.findIndex((m) => estV(m, 0));
    expect(iAnnonce).toBeGreaterThanOrEqual(0);
    expect(texte(ws.envoyes[iAnnonce])).toBe("copié ailleurs");
    expect(iAnnonce).toBeLessThan(iEnfonce);
    expect(iEnfonce).toBeLessThan(iRelache);
  });

  it("un_texte_deja_sur_le_bureau_n_est_pas_reannonce", async () => {
    // Le bureau vient de copier ce texte : il en est le propriétaire, le lui
    // réannoncer ne servirait à rien et la touche partirait 150 ms plus tard.
    const { id, ws } = await ouvrir("c2");
    ws.recoit(1, ...u16(1280), ...u16(800));
    const corps = new TextEncoder().encode("copié sur le bureau");
    ws.recoit(8, ...corps);
    presse.readText.mockResolvedValue("copié sur le bureau");
    touche(rdp.rdpSessions.get(id)!.canvas, "keydown", "KeyV", { ctrlKey: true });
    await vi.waitFor(() => expect(ws.envoyes.some((m) => estV(m, 1))).toBe(true), { timeout: 100 });
    expect(ws.envoyes.some((m) => m[0] === 8)).toBe(false);
  });

  it("le_partage_coupe_n_annonce_rien_au_collage", async () => {
    localStorage.setItem("avash.rdp.clipboard", "0");
    try {
      const { id, ws } = await ouvrir("c3");
      ws.recoit(1, ...u16(1280), ...u16(800));
      presse.readText.mockResolvedValue("secret");
      touche(rdp.rdpSessions.get(id)!.canvas, "keydown", "Insert", { shiftKey: true });
      await vi.waitFor(() => expect(ws.envoyes.some((m) => m[0] === 4)).toBe(true), { timeout: 1000 });
      expect(ws.envoyes.some((m) => m[0] === 8)).toBe(false);
      expect(presse.readText).not.toHaveBeenCalled();
    } finally {
      localStorage.removeItem("avash.rdp.clipboard");
    }
  });

  it("une_frappe_ordinaire_part_sans_attendre", async () => {
    const { id, ws } = await ouvrir("c4");
    ws.recoit(1, ...u16(1280), ...u16(800));
    touche(rdp.rdpSessions.get(id)!.canvas, "keydown", "KeyA");
    expect(ws.envoyes.some((m) => m[0] === 4 && m[1] === 0x1e)).toBe(true);
    expect(presse.readText).not.toHaveBeenCalled();
  });
});

describe("texte copié sur le bureau distant", () => {
  const OCCUPE = "The native clipboard is not accessible due to being held by another party.";

  it("un_presse_papiers_occupe_est_reessaye_puis_ecrit", async () => {
    const { ws } = await ouvrir("d1");
    ws.recoit(1, ...u16(1280), ...u16(800));
    presse.writeText.mockRejectedValueOnce(OCCUPE).mockRejectedValueOnce(OCCUPE).mockResolvedValue(undefined);
    ws.recoit(8, ...new TextEncoder().encode("copié sur le serveur"));
    await vi.waitFor(() => expect(presse.writeText).toHaveBeenCalledTimes(3), { timeout: 1000 });
    expect(presse.writeText).toHaveBeenLastCalledWith("copié sur le serveur");
    expect(avis.notifyErreur).not.toHaveBeenCalled();
  });

  it("un_echec_d_ecriture_se_dit_au_lieu_de_disparaitre", async () => {
    const { ws } = await ouvrir("d2");
    ws.recoit(1, ...u16(1280), ...u16(800));
    presse.writeText.mockRejectedValue(OCCUPE);
    ws.recoit(8, ...new TextEncoder().encode("copié sur le serveur"));
    await vi.waitFor(() => expect(avis.notifyErreur).toHaveBeenCalledTimes(1), { timeout: 2000 });
    expect(String(avis.notifyErreur.mock.calls[0][0])).toContain("presse-papiers du poste");
    presse.writeText.mockReset();
    presse.writeText.mockImplementation(() => Promise.resolve());
  });
});

describe("pause d'un bureau que personne ne regarde (audit du 27 novembre 2026)", () => {
  const pauses = (ws: FauxWebSocket) => ws.envoyes.filter((m) => m[0] === 11).map((m) => m[1]);

  it("un_bureau_masque_avant_l_ouverture_de_sa_websocket_est_en_pause_des_qu_elle_s_ouvre", async () => {
    // PF-3 : `marquerVisibilite` ne peut rien envoyer avant l'ouverture, et
    // `onopen` ne disait que le jeton et le presse-papiers : un bureau masqué
    // pendant la poignée de main restait en flux plein.
    const { id, ws } = await ouvrir("v1");
    const s = rdp.rdpSessions.get(id)!;
    ws.envoyes = [];
    ws.readyState = 0; // CONNECTING
    rdp.marquerVisibilite(s, false);
    expect(pauses(ws)).toEqual([]);
    ws.readyState = 1;
    ws.envoyes = [];
    ws.onopen!();
    expect(ws.envoyes.map((m) => m[0])).toEqual([...new TextEncoder().encode("jeton").slice(0, 1), 12, 11]);
    expect(pauses(ws)).toEqual([1]);
  });

  it("apres_une_reprise_le_bureau_reannonce_sa_pause_et_le_reglage_du_presse_papiers", async () => {
    // PF-3 et SE-6 : une reprise (redirection RDS, GNOME Remote Desktop, canal
    // graphique) refait la session distante, et le processus repartait avec le
    // partage actif et sans pause. Le [1] qui la termine fait tout redire.
    localStorage.setItem("avash.rdp.clipboard", "0");
    try {
      const { id, ws } = await ouvrir("v2");
      ws.recoit(1, ...u16(1280), ...u16(800));
      rdp.marquerVisibilite(rdp.rdpSessions.get(id)!, false);
      ws.recoit(23); // reprise
      ws.envoyes = [];
      ws.recoit(1, ...u16(1280), ...u16(800));
      expect(ws.envoyes.find((m) => m[0] === 12)).toEqual(new Uint8Array([12, 0]));
      expect(pauses(ws)).toEqual([1]);
    } finally {
      localStorage.removeItem("avash.rdp.clipboard");
    }
  });

  it("reduire_la_fenetre_met_en_pause_le_bureau_affiche_et_la_restaurer_le_relance", async () => {
    // PR-5, PF-2 : la pause ne suivait que l'onglet affiché ; Avash réduit avec
    // un bureau au premier plan le décodait et le peignait comme regardé.
    const { ws } = await ouvrir("v3");
    ws.recoit(1, ...u16(1280), ...u16(800));
    ws.envoyes = [];
    vi.useFakeTimers();
    try {
      fenetre.reduite = true;
      for (const f of fenetre.ecouteurs) f();
      await vi.advanceTimersByTimeAsync(200);
      expect(pauses(ws)).toEqual([1]);
      fenetre.reduite = false;
      for (const f of fenetre.ecouteurs) f();
      await vi.advanceTimersByTimeAsync(200);
      expect(pauses(ws)).toEqual([1, 0]);
      // Un événement de plus sans changement d'état n'envoie rien.
      for (const f of fenetre.ecouteurs) f();
      await vi.advanceTimersByTimeAsync(200);
      expect(pauses(ws)).toEqual([1, 0]);
    } finally {
      vi.useRealTimers();
      fenetre.reduite = false;
    }
  });

  it("une_page_masquee_met_aussi_les_bureaux_en_pause", async () => {
    const { ws } = await ouvrir("v4");
    ws.envoyes = [];
    rdp.surEtatFenetre({ cachee: true });
    expect(pauses(ws)).toEqual([1]);
    // Un onglet affiché dans une fenêtre cachée reste en pause.
    rdp.marquerVisibilite(rdp.rdpSessions.get([...rdp.rdpSessions.keys()].at(-1)!)!, true);
    expect(pauses(ws)).toEqual([1, 1]);
    rdp.surEtatFenetre({ cachee: false });
    expect(pauses(ws)).toEqual([1, 1, 0]);
  });

  it("le_partage_coupe_part_avec_le_lancement_du_processus", async () => {
    // SE-6 : le processus naissait partage actif et ne l'apprenait coupé qu'au
    // [12] de l'ouverture de la WebSocket, puis plus du tout après une reprise.
    await ouvrir("v7");
    expect((appels("rdp_open").at(-1) as unknown[])[1]).toMatchObject({ options: { sansPressePapiers: false } });
    localStorage.setItem("avash.rdp.clipboard", "0");
    try {
      await ouvrir("v8");
      expect((appels("rdp_open").at(-1) as unknown[])[1]).toMatchObject({ options: { sansPressePapiers: true } });
    } finally {
      localStorage.removeItem("avash.rdp.clipboard");
    }
  });

  it("partage_coupe_la_liste_des_fichiers_copies_sur_le_distant_ne_s_affiche_pas", async () => {
    // PF-3 : le bras « fichiers copiés » ne regardait pas le réglage ; la
    // pastille apparaissait et la réception restait possible.
    const liste = { dossier: "/tmp", fichiers: [{ chemin: "a.txt", taille: 3, dossier: false }], octets: 3 };
    // Contrôle : partage actif, la pastille apparaît.
    const actif = await ouvrir("v5");
    actif.ws.recoitJson(15, liste);
    expect(rdp.rdpSessions.get(actif.id)!.badge!.hidden).toBe(false);
    localStorage.setItem("avash.rdp.clipboard", "0");
    try {
      const { id, ws } = await ouvrir("v6");
      ws.recoitJson(15, liste);
      const s = rdp.rdpSessions.get(id)!;
      expect(s.badge!.hidden).toBe(true);
      expect(s.fichiers ?? null).toBeNull();
      await rdp.recevoirFichiers(id);
      expect(ws.envoyes.some((m) => m[0] === 16)).toBe(false);
    } finally {
      localStorage.removeItem("avash.rdp.clipboard");
    }
  });
});
