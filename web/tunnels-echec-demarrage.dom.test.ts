// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : quand tunnel_start échoue (hôte
// injoignable, port local pris) ou quand trois mots de passe sont refusés,
// l'utilisateur ne voyait aucune cause. L'erreur était écrite dans #t-error,
// qui vit dans le <details> « Nouveau tunnel » refermé dès qu'une définition
// existe — donc jamais rendu — et la boucle des trois essais ne disait rien du
// tout. Ces tests exigent que le motif atteigne un élément visible HORS du
// <details> (le bandeau #toasts et la ligne .terr), jamais #t-error.
import { describe, it, expect, beforeAll, beforeEach, vi } from "vitest";
import indexHtml from "./index.html?raw";
import type { Host, TunnelDef } from "./filters";
import { state } from "./etat";

const invoke = vi.hoisted(() => vi.fn());
const askPassword = vi.hoisted(() => vi.fn());
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
// askPassword ouvre une vraie modale qui, sous jsdom, ne se résout jamais
// seule : on la mocke. Les autres exports de ./dialogues doivent rester définis
// car main.ts et ses dépendances les importent au chargement du module.
vi.mock("./dialogues", () => ({
  askPassword,
  askConfirm: vi.fn(() => Promise.resolve(true)),
  askText: vi.fn(() => Promise.resolve(null)),
  collerDansTerminal: vi.fn(() => Promise.resolve()),
  MODALES_AU_DESSUS: ["confirm-modal", "ask-modal", "pass-modal"],
}));

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

let renderTunnels: () => void;
let tunnels: { defs: TunnelDef[]; status: Map<string, unknown>; busy: Set<string>; erreurs: Map<string, string> };

function tunnelDef(id: string): TunnelDef {
  return { id, alias: "srv", kind: "local", bind_port: 8080, target_host: "localhost", target_port: 80, name: "" };
}

/** Le bouton « Démarrer » de l'unique ligne. */
function boutonToggle(): HTMLButtonElement {
  return document.querySelector('#tunnel-list .tunnel-row [data-act="toggle"]') as HTMLButtonElement;
}

function texteToasts(): string {
  return $("toasts")!.textContent ?? "";
}

function $(id: string): HTMLElement {
  return document.getElementById(id) as HTMLElement;
}

/** Laisse les microtâches et les setTimeout(0) se vider. */
async function attendre(): Promise<void> {
  for (let i = 0; i < 8; i++) await new Promise((r) => setTimeout(r, 0));
}

beforeAll(async () => {
  shimsNavigateur();
  document.body.innerHTML = corpsIndex();
  invoke.mockResolvedValue([]);
  await import("./main");
  const mod = await import("./tunnels");
  renderTunnels = mod.renderTunnels;
  tunnels = mod.tunnels as typeof tunnels;
  const i18n = await import("./i18n");
  i18n.setLangue("fr");
});

beforeEach(() => {
  invoke.mockReset();
  askPassword.mockReset();
  tunnels.defs = [tunnelDef("t1")];
  tunnels.status = new Map();
  tunnels.busy = new Set();
  tunnels.erreurs = new Map();
  $("toasts").innerHTML = "";
  $("t-error").hidden = true;
  $("t-error").textContent = "";
  // La modale ouverte laisse le rafraîchissement final redessiner la ligne.
  $("tunnels-modal").classList.add("open");
  ($("tunnel-block") as HTMLDetailsElement).open = false;
});

describe("tunnelStart : rendre visible l'échec de démarrage", () => {
  it("un échec non lié au mot de passe s'affiche hors du <details> refermé, pas dans #t-error", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "host_needs_password") return Promise.resolve(false);
      if (cmd === "tunnel_start") return Promise.reject(new Error("ECONNREFUSED: connexion refusée"));
      if (cmd === "tunnel_defs") return Promise.resolve(tunnels.defs);
      if (cmd === "tunnel_status") return Promise.resolve([]);
      return Promise.resolve([]);
    });

    renderTunnels();
    boutonToggle().click();
    await attendre();

    // Le bandeau (hors du <details>) porte la cause.
    expect(texteToasts()).toContain("Démarrage impossible");
    // La ligne aussi, via .terr — c'est ce que lit la suite bout en bout.
    const terr = document.querySelector("#tunnel-list .tunnel-row .terr") as HTMLElement;
    expect(terr.hidden).toBe(false);
    expect(terr.textContent).toContain("Démarrage impossible");
    // Et le message n'est PAS enfoui dans #t-error (invisible dans le <details>).
    expect($("t-error").hidden).toBe(true);
    // .terr est bien hors du <details> refermé : sinon l'affichage resterait caché.
    expect($("tunnel-block").contains(terr)).toBe(false);
  });

  it("trois mots de passe refusés : un dernier essai n'est plus redemandé et l'échec est annoncé", async () => {
    askPassword.mockResolvedValue({ password: "secret", remember: false });
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "host_needs_password") return Promise.resolve(false);
      if (cmd === "tunnel_start") return Promise.reject(new Error("[AVASH_PASSWORD_REQUIRED] mot de passe requis"));
      if (cmd === "tunnel_defs") return Promise.resolve(tunnels.defs);
      if (cmd === "tunnel_status") return Promise.resolve([]);
      return Promise.resolve([]);
    });

    renderTunnels();
    boutonToggle().click();
    await attendre();

    const essais = invoke.mock.calls.filter((c) => c[0] === "tunnel_start").length;
    expect(essais).toBe(3); // trois tentatives réelles, pas une quatrième jetée
    expect(askPassword).toHaveBeenCalledTimes(2); // redemandé après les 2 premiers échecs seulement
    expect(texteToasts()).toContain("Trois tentatives");
  });
});

// Audit du 12 septembre 2026 (C-SIL-5) : pendant l'ouverture d'un tunnel, la
// ligne montrait « … » et gelait ses trois boutons, sans aucune annulation. Face
// à un hôte muet (pare-feu en DROP, tarpit), l'utilisateur n'avait plus qu'à
// fermer l'application. `tunnel_stop` fonctionne pourtant pendant l'ouverture
// (le cœur note l'arrêt et referme à l'arrivée) : la ligne propose « Annuler ».
describe("tunnelStart : une ouverture en cours peut être annulée", () => {
  it("la ligne en ouverture propose « Annuler », qui appelle tunnel_stop et libère la ligne", async () => {
    let echouer: (e: Error) => void = () => {};
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "host_needs_password") return Promise.resolve(false);
      // Hôte muet : l'ouverture ne rend la main qu'à la fin du test.
      if (cmd === "tunnel_start") return new Promise((_, ko) => { echouer = ko; });
      if (cmd === "tunnel_defs") return Promise.resolve(tunnels.defs);
      if (cmd === "tunnel_status") return Promise.resolve([]);
      return Promise.resolve(undefined);
    });

    renderTunnels();
    boutonToggle().click();
    await attendre();

    const annuler = document.querySelector('#tunnel-list .tunnel-row [data-act="annuler"]') as HTMLButtonElement | null;
    expect(annuler).not.toBeNull();
    expect(annuler!.disabled).toBe(false);
    expect(annuler!.textContent).toContain("Annuler");
    annuler!.click();
    await attendre();

    expect(invoke).toHaveBeenCalledWith("tunnel_stop", { id: "t1" });
    // La ligne n'est plus gelée : « Démarrer » est de nouveau offert.
    expect(tunnels.busy.has("t1")).toBe(false);
    expect(boutonToggle().disabled).toBe(false);

    // Le cœur finit par rendre l'échec de l'ouverture annulée : ce n'est pas une
    // erreur à montrer, l'utilisateur a demandé l'arrêt.
    echouer(new Error("Tunnel arrêté pendant l'ouverture."));
    await attendre();
    expect(texteToasts()).not.toContain("Démarrage impossible");
    expect(tunnels.erreurs.has("t1")).toBe(false);
    expect(tunnels.busy.has("t1")).toBe(false);
  });
});

// Audit du 12 septembre 2026 (C-SIL-6) : l'échec de `password_save` après un
// démarrage réussi était avalé (« facultatif »). L'utilisateur avait coché
// « mémoriser », le trousseau refusait (Secret Service absent, portefeuille
// verrouillé) et rien ne le disait : le mot de passe était redemandé à la
// connexion suivante « sans raison ». Le chemin SSH et la fiche RDP le
// signalaient déjà depuis l'audit du 7 septembre.
describe("tunnelStart : un échec de mémorisation du mot de passe est signalé", () => {
  it("un_echec_de_memorisation_du_mot_de_passe_du_tunnel_est_notifie", async () => {
    state.hosts = [{ alias: "srv", hostname: "10.0.0.9", user: "root", port: 22 } as unknown as Host];
    askPassword.mockResolvedValue({ password: "secret", remember: true });
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "host_needs_password") return Promise.resolve(true);
      if (cmd === "tunnel_start") return Promise.resolve({ id: "t1" });
      if (cmd === "password_save") return Promise.reject(new Error("Secret Service indisponible"));
      if (cmd === "tunnel_defs") return Promise.resolve(tunnels.defs);
      if (cmd === "tunnel_status") return Promise.resolve([]);
      return Promise.resolve(undefined);
    });

    renderTunnels();
    boutonToggle().click();
    await attendre();

    expect(invoke).toHaveBeenCalledWith("password_save", expect.objectContaining({ addr: "10.0.0.9", password: "secret" }));
    expect(texteToasts()).toContain("Mémorisation impossible");
    expect(texteToasts()).toContain("Secret Service indisponible");
    state.hosts = [];
  });
});
