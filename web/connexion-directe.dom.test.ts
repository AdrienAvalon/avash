// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : dans « Connexion directe », le catch
// de `manualSubmit` affichait le message d'échec du cœur tel quel, marqueur
// interne compris. Résultat : « [AVASH_HOST_KEY_CHANGED] LA CLÉ D'HÔTE A
// CHANGÉ… » s'affichait brut et se resoumettre redonnait la même chose — aucune
// sortie, alors que le chemin par alias propose d'oublier l'ancienne clé. Ces
// tests verrouillent le nouveau flux (proposition d'oubli + un seul nouvel
// essai) et l'absence de tout marqueur `[AVASH_…]` dans le message affiché.
import { describe, it, expect, beforeEach, vi } from "vitest";
import { t } from "./i18n";

const invoke = vi.hoisted(() => vi.fn());
const openManualSession = vi.hoisted(() => vi.fn());
const askConfirm = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
// Les modules testés câblent des écouteurs sur ./main et ./rdp à l'import ;
// on les remplace pour n'exercer que la soumission du formulaire.
vi.mock("./main", () => ({
  loadHosts: vi.fn(),
  openManualSession,
  openSerie: vi.fn(),
}));
vi.mock("./rdp", () => ({
  choisirDossierPartage: vi.fn(),
  openRdp: vi.fn(),
}));
vi.mock("./dialogues", () => ({ askConfirm }));

/** Formulaire manuel complet : `manualSubmit` et sa fermeture lisent beaucoup
 *  de champs (proto, auth, lignes cachées) ; on reproduit la structure. */
function monterDom() {
  document.body.innerHTML = `
    <button id="manual-btn"></button>
    <div id="manual-modal">
      <form id="manual-form">
        <label class="radio"><input type="radio" name="proto" value="ssh" checked /></label>
        <label class="radio"><input type="radio" name="proto" value="rdp" /></label>
        <label class="radio"><input type="radio" name="proto" value="vnc" /></label>
        <label class="radio"><input type="radio" name="proto" value="serie" /></label>
        <p id="m-vnc-hint" hidden></p>
        <label id="m-addr-row"><input id="m-addr" value="" /></label>
        <label id="m-port-row"><input id="m-port" value="" /></label>
        <label id="m-user-row"><input id="m-user" value="" /></label>
        <label id="m-serie-row" hidden><input id="m-serie-chemin" /><select id="m-serie-vitesse"><option value="115200">115200</option></select><span id="m-serie-hint"></span></label>
        <label id="m-serie-vitesse-row" hidden></label>
        <div id="m-auth-switch">
          <label class="radio"><input type="radio" name="auth" value="password" checked /></label>
          <label class="radio"><input type="radio" name="auth" value="key" /></label>
        </div>
        <label id="m-password-row"><input id="m-password" value="" /></label>
        <label id="m-key-row" hidden><input id="m-key" value="" /></label>
        <label id="m-rdp-remember-row" hidden><input type="checkbox" id="m-rdp-remember" /></label>
        <label id="m-rdp-save-row" hidden><input type="checkbox" id="m-rdp-save" /></label>
        <label id="m-rdp-name-row" hidden><input id="m-rdp-name" /></label>
        <label id="m-rdp-partage-row" hidden><input id="m-rdp-partage" /><button type="button" id="m-rdp-partage-choisir"></button></label>
        <label class="check" id="m-save-row"><input type="checkbox" id="m-save" /></label>
        <label id="m-alias-row" hidden><input id="m-alias" value="" /></label>
        <p id="m-error" hidden></p>
        <button id="m-cancel"></button>
        <button type="submit" id="m-submit">Se connecter</button>
      </form>
    </div>`;
}

const HOST_KEY_CHANGED = "[AVASH_HOST_KEY_CHANGED] LA CLÉ D'HÔTE A CHANGÉ pour 10.0.0.7:22.";
const PASSWORD_REQUIRED = "[AVASH_PASSWORD_REQUIRED] Aucune méthode d'authentification n'a abouti.";

/** Renseigne l'adresse et déclenche la soumission SSH (sans enregistrement). */
async function soumettre(manualSubmit: (ev: Event) => Promise<void>) {
  (document.getElementById("m-addr") as HTMLInputElement).value = "10.0.0.7";
  (document.getElementById("m-user") as HTMLInputElement).value = "root";
  await manualSubmit(new Event("submit"));
}

describe("connexion directe : clé d'hôte changée", () => {
  beforeEach(() => {
    invoke.mockReset();
    openManualSession.mockReset();
    askConfirm.mockReset();
    monterDom();
  });

  it("propose d'oublier l'ancienne clé, l'oublie et relance une fois", async () => {
    const { manualSubmit } = await import("./connexion-directe");
    // Premier essai refusé (clé changée), second essai accepté après oubli.
    openManualSession.mockRejectedValueOnce(new Error(HOST_KEY_CHANGED)).mockResolvedValueOnce(undefined);
    askConfirm.mockResolvedValue(true);
    invoke.mockResolvedValue(1);

    await soumettre(manualSubmit);

    expect(askConfirm).toHaveBeenCalledTimes(1);
    // Le marqueur ne doit pas apparaître dans la question posée.
    expect(askConfirm.mock.calls[0][0]).not.toContain("[AVASH_");
    expect(invoke).toHaveBeenCalledWith("known_hosts_forget", { addr: "10.0.0.7", port: null });
    // Un seul nouvel essai : deux appels au total (échec + reprise).
    expect(openManualSession).toHaveBeenCalledTimes(2);
    // Pas de ré-enregistrement de l'hôte (case décochée) : aucun host_save.
    expect(invoke).not.toHaveBeenCalledWith("host_save", expect.anything());
  });

  it("refus de l'oubli : message propre, sans marqueur, pas d'oubli ni de reprise", async () => {
    const { manualSubmit } = await import("./connexion-directe");
    openManualSession.mockRejectedValueOnce(new Error(HOST_KEY_CHANGED));
    askConfirm.mockResolvedValue(false);

    await soumettre(manualSubmit);

    const erreur = document.getElementById("m-error") as HTMLElement;
    expect(erreur.hidden).toBe(false);
    expect(erreur.textContent).not.toContain("[AVASH_");
    expect(erreur.textContent).toContain("LA CLÉ D'HÔTE A CHANGÉ");
    expect(invoke).not.toHaveBeenCalledWith("known_hosts_forget", expect.anything());
    expect(openManualSession).toHaveBeenCalledTimes(1);
  });
});

describe("connexion directe : autres marqueurs nettoyés", () => {
  beforeEach(() => {
    invoke.mockReset();
    openManualSession.mockReset();
    askConfirm.mockReset();
    monterDom();
  });

  it("un mot de passe requis n'affiche jamais le marqueur brut", async () => {
    const { manualSubmit } = await import("./connexion-directe");
    openManualSession.mockRejectedValueOnce(new Error(PASSWORD_REQUIRED));

    await soumettre(manualSubmit);

    const erreur = document.getElementById("m-error") as HTMLElement;
    expect(erreur.hidden).toBe(false);
    expect(erreur.textContent).not.toContain("[AVASH_");
    // askConfirm ne concerne que la clé d'hôte : pas de proposition d'oubli ici.
    expect(askConfirm).not.toHaveBeenCalled();
  });
});

// Trouvé par l'audit du 7 septembre 2026 : host_save précède la connexion. En
// cas d'échec (mauvais mot de passe), la case « Enregistrer cet hôte » restait
// cochée et le second submit rappelait host_save, refusé par append_host
// (« déjà déclaré »), ce qui bloquait toute reconnexion depuis la modale.
describe("connexion directe : enregistrement puis connexion échouée", () => {
  beforeEach(() => {
    invoke.mockReset();
    openManualSession.mockReset();
    askConfirm.mockReset();
    monterDom();
  });

  /** Coche « Enregistrer », saisit un alias, adresse et utilisateur, soumet. */
  async function soumettreAvecEnregistrement(manualSubmit: (ev: Event) => Promise<void>) {
    (document.getElementById("m-addr") as HTMLInputElement).value = "10.0.0.7";
    (document.getElementById("m-user") as HTMLInputElement).value = "root";
    (document.getElementById("m-save") as HTMLInputElement).checked = true;
    (document.getElementById("m-alias") as HTMLInputElement).value = "prod";
    await manualSubmit(new Event("submit"));
  }

  it("décoche la case après l'enregistrement et ne rappelle plus host_save au second essai", async () => {
    const { manualSubmit } = await import("./connexion-directe");
    // host_save réussit ; la connexion échoue une fois (mot de passe faux) puis aboutit.
    invoke.mockResolvedValue(1);
    openManualSession
      .mockRejectedValueOnce(new Error("Authentification refusée."))
      .mockResolvedValueOnce(undefined);

    await soumettreAvecEnregistrement(manualSubmit);

    const save = document.getElementById("m-save") as HTMLInputElement;
    const erreur = document.getElementById("m-error") as HTMLElement;
    // L'hôte est enregistré : la case est décochée pour ne pas le réécrire…
    expect(save.checked).toBe(false);
    // …et le message rappelle que l'enregistrement a bien eu lieu.
    expect(erreur.hidden).toBe(false);
    expect(erreur.textContent).toContain(t("cd-hote-enregistre"));
    expect(erreur.textContent).toContain("Authentification refusée.");
    const appelsSave1 = invoke.mock.calls.filter((c) => c[0] === "host_save");
    expect(appelsSave1).toHaveLength(1);
    // L'alias saisi est passé à la session : l'onglet porte « prod ».
    expect(openManualSession).toHaveBeenLastCalledWith(expect.anything(), "prod");

    // Second submit (mot de passe corrigé) : plus de host_save, la connexion passe.
    await manualSubmit(new Event("submit"));
    const appelsSave2 = invoke.mock.calls.filter((c) => c[0] === "host_save");
    expect(appelsSave2).toHaveLength(1);
    expect(openManualSession).toHaveBeenCalledTimes(2);
    // L'onglet du nouvel essai garde « prod » : l'hôte est enregistré sous ce
    // nom même si la case a été décochée entre-temps.
    expect(openManualSession).toHaveBeenLastCalledWith(expect.anything(), "prod");
  });
});
