// @vitest-environment jsdom
// Régression vue au pre-commit du 28 novembre 2026 : les trois modales
// (saisie, confirmation, mot de passe) focalisent leur champ 30 ms après
// l'ouverture, sans annuler ce minuteur à la fermeture. Sous la charge de la
// suite complète, mot-de-passe-file.dom.test.ts répondait avant les 30 ms et
// le minuteur partait après la destruction de jsdom (« document is not
// defined », deux erreurs, suite rouge). Dans l'application, une modale fermée
// aussi vite volait le focus pour le poser sur un champ caché.
import { describe, it, expect, beforeAll, afterEach, vi } from "vitest";
import indexHtml from "./index.html?raw";

vi.mock("@tauri-apps/api/app", () => ({ getVersion: () => Promise.resolve("1.2.3") }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ readText: vi.fn(), writeText: vi.fn() }));

let dialogues: typeof import("./dialogues");
const el = (id: string) => document.getElementById(id) as HTMLElement;
// Cible en constante : une chaîne passée à askPassword passe pour un mot de
// passe écrit en clair aux yeux de GitGuardian (vu sur cette PR, le 28 novembre
// 2026, comme dans mot-de-passe-file.dom.test.ts).
const CIBLE = "hote-de-test";

beforeAll(async () => {
  document.body.innerHTML = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  dialogues = await import("./dialogues");
});

afterEach(() => {
  vi.useRealTimers();
});

describe("le focus différé d'une modale refermée aussitôt", () => {
  it("la demande de mot de passe ne focalise pas son champ caché", async () => {
    vi.useFakeTimers();
    const reponse = dialogues.askPassword(CIBLE);
    await vi.advanceTimersByTimeAsync(0);
    el("pass-cancel").click();
    expect(await reponse).toBeNull();
    await vi.advanceTimersByTimeAsync(100);
    expect(vi.getTimerCount()).toBe(0);
    expect(document.activeElement).not.toBe(el("pass-input"));
  });

  it("la confirmation ne focalise pas son bouton caché", async () => {
    vi.useFakeTimers();
    const reponse = dialogues.askConfirm("Supprimer ?");
    el("confirm-cancel").click();
    expect(await reponse).toBe(false);
    await vi.advanceTimersByTimeAsync(100);
    expect(vi.getTimerCount()).toBe(0);
    expect(document.activeElement).not.toBe(el("confirm-cancel"));
  });

  it("la saisie de texte ne focalise pas son champ caché", async () => {
    vi.useFakeTimers();
    const reponse = dialogues.askText("Renommer", "Nom", "a.txt");
    el("ask-cancel").click();
    expect(await reponse).toBeNull();
    await vi.advanceTimersByTimeAsync(100);
    expect(vi.getTimerCount()).toBe(0);
    expect(document.activeElement).not.toBe(el("ask-input"));
  });

  it("une modale restée ouverte reçoit toujours son focus", async () => {
    vi.useFakeTimers();
    const reponse = dialogues.askText("Renommer", "Nom", "a.txt");
    await vi.advanceTimersByTimeAsync(30);
    expect(document.activeElement).toBe(el("ask-input"));
    el("ask-cancel").click();
    expect(await reponse).toBeNull();
  });
});
