// @vitest-environment jsdom
// Audit du 12 septembre 2026 (C-SIL-13) : `setupWindowControls` attendait
// `isMaximized()` avant de câbler quoi que ce soit. Si l'appel rejetait
// (permission retirée, fenêtre en cours de destruction), réduire, agrandir et
// fermer restaient trois boutons qui ne faisaient rien, sans un message, sur
// une fenêtre sans décorations : plus aucun moyen de la fermer à la souris.
import { describe, it, expect, beforeAll, vi } from "vitest";

const fenetre = vi.hoisted(() => ({
  isMaximized: vi.fn(() => Promise.reject(new Error("permission refusée"))),
  minimize: vi.fn(() => Promise.resolve()),
  toggleMaximize: vi.fn(() => Promise.resolve()),
  close: vi.fn(() => Promise.resolve()),
  onResized: vi.fn(() => Promise.reject(new Error("permission refusée"))),
  onFocusChanged: vi.fn(() => Promise.reject(new Error("permission refusée"))),
  startResizeDragging: vi.fn(() => Promise.resolve()),
}));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => fenetre }));
vi.mock("./rdp", () => ({ rdpSessions: new Map() }));

beforeAll(() => {
  document.body.innerHTML = `<button id="win-min"></button><button id="win-max"></button><button id="win-close"></button><div id="resize-handles"></div><span id="tb-name"></span>`;
});

describe("contrôles de fenêtre", () => {
  it("les_boutons_de_fenetre_sont_cables_meme_si_l_etat_maximise_est_illisible", async () => {
    const { setupWindowControls } = await import("./titre");
    await setupWindowControls();
    document.getElementById("win-min")!.click();
    document.getElementById("win-close")!.click();
    document.getElementById("win-max")!.click();
    expect(fenetre.minimize).toHaveBeenCalledTimes(1);
    expect(fenetre.close).toHaveBeenCalledTimes(1);
    expect(fenetre.toggleMaximize).toHaveBeenCalledTimes(1);
    // Le bouton garde une icône, celle de la fenêtre non agrandie.
    expect(document.getElementById("win-max")!.innerHTML).not.toBe("");
    expect(document.querySelectorAll("#resize-handles > div").length).toBe(8);
  });
});
