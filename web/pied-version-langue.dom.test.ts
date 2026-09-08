// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : #footer-version portait
// data-i18n="avash-2" (valeur « avash » dans les deux langues), alors que
// dialogues.ts y écrit le texte complet « avash v{version} » une fois
// getVersion() résolu. Chaque bascule de langue appelle appliquerLangue(), qui
// remplaçait le nœud texte de l'élément par « avash » : le pied perdait sa
// version jusqu'au redémarrage. Ce test écrit la version dans le pied, bascule
// en anglais puis en français, et exige que la version survive.
import { describe, it, expect, beforeAll, vi } from "vitest";
import indexHtml from "./index.html?raw";

// dialogues.ts lit la version au chargement via getVersion() : on la mocke.
vi.mock("@tauri-apps/api/app", () => ({ getVersion: () => Promise.resolve("0.0.0-test") }));

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

function $(id: string): HTMLElement {
  return document.getElementById(id) as HTMLElement;
}

let setLangue: (l: "fr" | "en") => void;

beforeAll(async () => {
  // Sous jsdom, dialogues.ts attache ses gestionnaires aux éléments de la page :
  // le corps doit être monté AVANT d'importer le module.
  document.body.innerHTML = corpsIndex();
  const i18n = await import("./i18n");
  setLangue = i18n.setLangue;
  // Importer dialogues déclenche getVersion().then(...) qui écrit le pied.
  await import("./dialogues");
});

describe("pied de page : la version survit à un changement de langue", () => {
  it("garde « avash v{version} » après une bascule anglais puis français", async () => {
    // On attend que la promesse getVersion() résolue ait écrit le pied.
    await vi.waitFor(() => expect($("footer-version").textContent).toBe("avash v0.0.0-test"));

    setLangue("en"); // appliquait « avash » sur #footer-version avant le correctif
    expect($("footer-version").textContent).toBe("avash v0.0.0-test");

    setLangue("fr");
    expect($("footer-version").textContent).toBe("avash v0.0.0-test");
  });
});
