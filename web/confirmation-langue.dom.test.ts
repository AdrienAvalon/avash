// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : askConfirm posait son libellé par
// défaut « Confirmer » écrit en dur, ce qui écrasait à chaque appel la
// traduction que data-i18n avait mise sur #confirm-ok. En interface anglaise,
// toute confirmation destructive sans option `ok` (supprimer un hôte, un
// dossier, un tunnel…) affichait donc « Confirmer » face à « Cancel ». Ce test
// bascule en anglais, ouvre la modale de confirmation et exige « Confirm ».
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

let askConfirm: (t: string, o?: { ok?: string; danger?: boolean }) => Promise<boolean>;
let setLangue: (l: "fr" | "en") => void;
let EN: Record<string, string>;

beforeAll(async () => {
  // Sous jsdom, dialogues.ts attache ses gestionnaires aux éléments de la page :
  // le corps doit être monté AVANT d'importer le module.
  document.body.innerHTML = corpsIndex();
  const i18n = await import("./i18n");
  setLangue = i18n.setLangue;
  EN = i18n.EN;
  askConfirm = (await import("./dialogues")).askConfirm;
});

describe("confirmation : le bouton par défaut suit la langue", () => {
  it("en interface anglaise, le bouton de confirmation affiche « Confirm »", () => {
    setLangue("en"); // appliquerLangue pose « Confirm » via data-i18n="confirmer"
    // On n'attend pas la promesse (elle ne se tient qu'au clic) : askConfirm
    // écrit le libellé de façon synchrone, il suffit de le lire aussitôt.
    void askConfirm("Delete the host?");
    // Écrit en dur « Confirmer » avant le correctif : restait français.
    expect($("confirm-ok").textContent).toBe("Confirm");
    expect($("confirm-ok").textContent).toBe(EN["confirmer"]);
    setLangue("fr");
  });
});
