// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : un mot de passe avec une espace de
// tête ou de fin était rogné (trim) avant d'être envoyé au cœur, qui le
// refusait. Ces tests verrouillent que le mot de passe garde ses espaces de
// bord, alors que les autres champs (adresse, utilisateur, port) restent
// rognés à dessein.
import { describe, it, expect, beforeEach, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
// Les modules testés câblent des écouteurs sur ./main et ./rdp à l'import ;
// on les remplace pour n'exercer que la lecture des formulaires.
vi.mock("./main", () => ({
  loadHosts: vi.fn(),
  openManualSession: vi.fn(),
  openSerie: vi.fn(),
}));
vi.mock("./rdp", () => ({
  choisirDossierPartage: vi.fn(),
  openRdp: vi.fn(),
}));
// connexion-directe importe askConfirm depuis ./dialogues, qui câble des
// écouteurs à l'import (ask-form…) absents de ce DOM minimal ; on le remplace.
vi.mock("./dialogues", () => ({ askConfirm: vi.fn() }));

/** Formulaires minimaux : seulement les champs lus par les deux modules. */
function monterDom() {
  document.body.innerHTML = `
    <button id="keys-btn"></button>
    <button id="k-close"></button>
    <form id="keygen-form"></form>
    <div id="k-ok" hidden></div>
    <div id="k-error" hidden></div>
    <form id="deploy-form">
      <input id="d-addr" value="" />
      <input id="d-port" value="" />
      <input id="d-user" value="" />
      <input id="d-password" value="" />
      <select id="d-key"><option value="ssh-rsa AAAA">clé</option></select>
      <button id="d-submit"></button>
    </form>

    <button id="manual-btn"></button>
    <button id="m-cancel"></button>
    <form id="manual-form">
      <input id="m-addr" value="" />
      <input id="m-port" value="" />
      <input id="m-user" value="" />
      <input id="m-password" value="" />
      <input id="m-key" value="" />
      <label><input type="radio" name="auth" value="password" checked /></label>
      <label><input type="radio" name="auth" value="key" /></label>
      <div id="m-password-row"></div>
      <div id="m-key-row"></div>
      <div id="m-alias-row"></div>
      <input id="m-save" type="checkbox" />
      <button id="m-rdp-save"></button>
      <button id="m-rdp-partage-choisir"></button>
    </form>`;
}

describe("le mot de passe garde ses espaces de bord", () => {
  beforeEach(() => {
    invoke.mockReset();
    monterDom();
  });

  it("deploySubmit (« Installer la clé ») envoie le mot de passe brut", async () => {
    invoke.mockResolvedValue("Clé installée.");
    const { deploySubmit } = await import("./cles");
    (document.getElementById("d-addr") as HTMLInputElement).value = " host ";
    (document.getElementById("d-user") as HTMLInputElement).value = " root ";
    (document.getElementById("d-port") as HTMLInputElement).value = " 22 ";
    (document.getElementById("d-password") as HTMLInputElement).value = "a ";

    await deploySubmit(new Event("submit"));

    expect(invoke).toHaveBeenCalledTimes(1);
    const args = invoke.mock.calls[0][1] as Record<string, unknown>;
    expect(args.password).toBe("a ");
    // Les autres champs restent rognés : le cœur les rogne aussi, mais le front
    // ne doit pas régresser dessus.
    expect(args.addr).toBe("host");
    expect(args.user).toBe("root");
    expect(args.port).toBe(22);
  });

  it("manualReadForm (« Connexion directe » SSH) garde les espaces du mot de passe", async () => {
    const { manualReadForm } = await import("./connexion-directe");
    (document.getElementById("m-addr") as HTMLInputElement).value = " srv ";
    (document.getElementById("m-user") as HTMLInputElement).value = " user ";
    (document.getElementById("m-password") as HTMLInputElement).value = " secret ";

    const cible = manualReadForm();

    expect(cible.password).toBe(" secret ");
    expect(cible.addr).toBe("srv");
    expect(cible.user).toBe("user");
  });

  it("manualReadForm : un mot de passe fait d'espaces reste un mot de passe (pas null)", async () => {
    // Sous-cas : « espaces seuls » passait la validation HTML `required` puis
    // devenait null/"" après trim, et le cœur répondait « mot de passe ou clé »
    // sur un champ visiblement rempli.
    const { manualReadForm } = await import("./connexion-directe");
    (document.getElementById("m-password") as HTMLInputElement).value = "  ";

    expect(manualReadForm().password).toBe("  ");
  });

  it("manualReadForm : un champ mot de passe vide reste null", async () => {
    const { manualReadForm } = await import("./connexion-directe");
    (document.getElementById("m-password") as HTMLInputElement).value = "";

    expect(manualReadForm().password).toBeNull();
  });
});
