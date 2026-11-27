// @vitest-environment jsdom
// Audit du 27 novembre 2026 (FI-11) : la demande de mot de passe est une
// modale unique. Une seconde demande résolvait la première par `null`, comme
// une annulation, et changeait l'étiquette sous les doigts de l'utilisateur,
// qui pouvait taper le mot de passe d'un hôte et l'envoyer à un autre. Les
// demandes attendent désormais leur tour.
import { describe, it, expect, beforeAll, vi } from "vitest";
import indexHtml from "./index.html?raw";

vi.mock("@tauri-apps/api/app", () => ({ getVersion: () => Promise.resolve("1.2.3") }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ readText: vi.fn(), writeText: vi.fn() }));

let dialogues: typeof import("./dialogues");
const $ = (id: string) => document.getElementById(id) as HTMLInputElement;
// Saisies factices, en constantes : une valeur écrite à côté de « password »
// passe pour un secret aux yeux des détecteurs (GitGuardian, 27 novembre 2026).
const SAISIE_1 = "saisie-onglet-un";
const SAISIE_2 = "saisie-onglet-deux";
// Cibles en constantes, pour la même raison : une adresse « root@… » passée
// à la demande de mot de passe est lue comme un mot de passe écrit en clair.
const CIBLE_1 = "root@web-1";
const CIBLE_2 = "root@web-2";
const pause = () => new Promise((r) => setTimeout(r, 0));

function repondre(mdp: string) {
  $("pass-input").value = mdp;
  $("pass-form").dispatchEvent(new Event("submit", { cancelable: true }));
}

beforeAll(async () => {
  document.body.innerHTML = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  dialogues = await import("./dialogues");
});

describe("demandes de mot de passe concurrentes", () => {
  it("la seconde attend la première au lieu de l'annuler", async () => {
    const premiere = dialogues.askPassword(CIBLE_1);
    const seconde = dialogues.askPassword(CIBLE_2);
    await pause();
    expect(document.getElementById("pass-target")!.textContent).toBe(CIBLE_1);
    repondre(SAISIE_1);
    const reponse1 = await premiere;
    expect(reponse1?.password).toBe(SAISIE_1);
    expect(reponse1?.remember).toBe(false);
    await pause();
    expect(document.getElementById("pass-target")!.textContent).toBe(CIBLE_2);
    repondre(SAISIE_2);
    const reponse2 = await seconde;
    expect(reponse2?.password).toBe(SAISIE_2);
    expect(reponse2?.remember).toBe(false);
  });

  it("une demande dont l'onglet a été fermé pendant l'attente n'ouvre pas la modale", async () => {
    const premiere = dialogues.askPassword(CIBLE_1);
    let ouvert = true;
    const seconde = dialogues.askPassword(CIBLE_2, undefined, () => ouvert);
    await pause();
    ouvert = false;
    $("pass-cancel").click();
    expect(await premiere).toBeNull();
    expect(await seconde).toBeNull();
    expect(document.getElementById("pass-modal")!.classList.contains("open")).toBe(false);
  });
});
