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
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ setFullscreen: () => Promise.resolve() }) }));
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
