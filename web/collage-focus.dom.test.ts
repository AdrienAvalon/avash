// @vitest-environment jsdom
// Signalé par le mainteneur le 26 novembre 2026 : « quand on a fait coller on
// perd le focus du terminal ». Un collage par le menu du clic droit ôtait le
// focus à la zone de saisie (le clic sur l'entrée du menu), et la confirmation
// d'un collage de plusieurs lignes le gardait en se fermant : la frappe
// suivante n'allait nulle part. Et « ça copie en double en SSH » : Ctrl+Maj+V
// lisait le presse-papiers ET laissait le navigateur faire son propre collage
// natif (WebView2 : « coller en texte brut »), que l'interception collait une
// seconde fois. Le presse-papiers se lit désormais par le plugin de Tauri, la
// seule lecture qui marche partout (WebKitGTK refuse navigator.clipboard).
import { describe, it, expect, beforeAll, beforeEach, vi } from "vitest";
import indexHtml from "./index.html?raw";

vi.mock("@tauri-apps/api/app", () => ({ getVersion: () => Promise.resolve("0.0.0-test") }));
const lecture = vi.hoisted(() => ({ readText: vi.fn<() => Promise<string>>() }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ readText: lecture.readText, writeText: vi.fn() }));

function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

let dialogues: typeof import("./dialogues");

beforeAll(async () => {
  document.body.innerHTML = corpsIndex();
  (await import("./i18n")).setLangue("fr");
  dialogues = await import("./dialogues");
});

beforeEach(() => {
  lecture.readText.mockReset();
});

function terminal() {
  return { paste: vi.fn<(t: string) => void>(), focus: vi.fn<() => void>() };
}

describe("collage : le terminal garde le focus", () => {
  it("après une ligne collée, le focus revient au terminal", async () => {
    const term = terminal();
    await dialogues.collerDansTerminal(term, "ls -la");
    expect(term.paste).toHaveBeenCalledWith("ls -la");
    expect(term.focus).toHaveBeenCalled();
  });

  it("après la confirmation d'un collage de plusieurs lignes, refusée ou acceptée", async () => {
    for (const bouton of ["confirm-cancel", "confirm-ok"]) {
      const term = terminal();
      const collage = dialogues.collerDansTerminal(term, "a\nb");
      document.getElementById(bouton)!.click();
      await collage;
      expect(term.paste).toHaveBeenCalledTimes(bouton === "confirm-ok" ? 1 : 0);
      expect(term.focus).toHaveBeenCalled();
    }
  });
});

describe("collerPressePapiers : une seule lecture, par le plugin de Tauri", () => {
  it("colle une fois ce que le plugin a lu", async () => {
    lecture.readText.mockResolvedValue("uname -a");
    const term = terminal();
    await dialogues.collerPressePapiers(term);
    expect(lecture.readText).toHaveBeenCalledTimes(1);
    expect(term.paste).toHaveBeenCalledTimes(1);
    expect(term.paste).toHaveBeenCalledWith("uname -a");
    expect(term.focus).toHaveBeenCalled();
  });

  it("un presse-papiers illisible (image, vide) ne colle rien et rend le focus", async () => {
    lecture.readText.mockRejectedValue(new Error("clipboard is empty"));
    const term = terminal();
    await dialogues.collerPressePapiers(term);
    expect(term.paste).not.toHaveBeenCalled();
    expect(term.focus).toHaveBeenCalled();
  });
});
