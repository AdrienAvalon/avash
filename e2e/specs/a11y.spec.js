// Accessibilité : rôles ARIA des boîtes de dialogue et comportement du focus.
// Vérifié sur l'application réelle — c'est le seul endroit où le focus se
// comporte comme chez l'utilisateur.

import { trouverLigne } from "./helpers.js";

describe("Accessibilité des boîtes de dialogue", () => {
  it("la modale porte role=dialog et un titre accessible existant", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    const box = await $('#manual-modal [role="dialog"]');
    expect(await box.isExisting()).toBe(true);
    expect(await box.getAttribute("aria-modal")).toBe("true");
    // aria-labelledby doit pointer sur un élément réellement présent.
    const id = await box.getAttribute("aria-labelledby");
    expect(await $(`#${id}`).isExisting()).toBe(true);
    await browser.keys("Escape");
  });

  it("Tab reste enfermé dans la modale (ne fuit pas vers la page derrière)", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    // Assez de Tab pour faire plusieurs fois le tour de la boîte.
    for (let i = 0; i < 25; i++) await browser.keys("Tab");
    const dansLaModale = await browser.execute(
      () => !!document.activeElement?.closest("#manual-modal"),
    );
    expect(dansLaModale).toBe(true);
    await browser.keys("Escape");
  });

  it("le focus revient au bouton déclencheur après fermeture", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.keys("Escape");
    await browser.waitUntil(
      async () => browser.execute(() => document.activeElement?.id === "manual-btn"),
      { timeout: 5000, timeoutMsg: "le focus n'est pas revenu sur le déclencheur" },
    );
  });

  it("les boutons icône-seule ont un nom accessible", async () => {
    const sans = await browser.execute(() =>
      [...document.querySelectorAll("button")]
        .filter((b) => !b.textContent.trim() && b.querySelector("svg"))
        .filter((b) => !b.getAttribute("aria-label") && !b.getAttribute("title"))
        .map((b) => b.id || b.className),
    );
    expect(sans).toEqual([]);
  });

  // Trouvé par l'audit du 7 septembre 2026 : `.modal input:focus { outline:
  // none }` écrasait l'anneau :focus-visible des radios cachées (0×0) des
  // interrupteurs segmentés. En Maj+Tab depuis « Adresse ou IP » le focus
  // arrivait sur la radio SSH sans qu'aucun indicateur n'apparaisse (WCAG
  // 2.4.7). On met l'état complet DANS l'assertion (comme axe.spec.js) : un
  // simple booléen rendrait un échec de CI indéchiffrable, et closest() renvoie
  // null si le focus n'est pas où on croit.
  it("l'interrupteur segmenté montre le focus clavier", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.keys(["Shift", "Tab"]);
    const etat = await browser.execute(() => {
      const a = document.activeElement;
      const pastille = a && a.closest(".auth-switch .radio");
      if (!pastille) return { type: a && a.type, trouve: false };
      return { type: a.type, trouve: true, contour: getComputedStyle(pastille).outlineStyle };
    });
    expect(etat).toEqual({ type: "radio", trouve: true, contour: "solid" });
    await browser.keys("Escape");
  });
});

// Trouvé par l'audit du 7 septembre 2026 : renderTunnels vide #tunnel-list et
// recrée chaque ligne toutes les 1,5 s (le minuteur de tunnelsOpen) ; un bouton
// de ligne focalisé au clavier était détruit, le focus retombait sur <body> et
// le piège de focus renvoyait le Tab suivant en haut de la modale. C'est le
// seul endroit qui reproduit le vrai minuteur (l'application tourne pour de
// bon). On crée une définition, on focalise un bouton de ligne, on laisse
// passer plus d'un tick, et on exige que le focus soit resté sur ce bouton.
describe("Accessibilité de la liste des tunnels", () => {
  const findRow = () => trouverLigne("#tunnel-list .tunnel-row", ".tname", "Tunnel focus");

  it("le focus survit au rafraîchissement de la liste des tunnels", async () => {
    await $("#tunnels-btn").click();
    await $("#tunnels-modal").waitForDisplayed({ timeout: 5000 });
    await browser.execute(() => document.getElementById("tunnel-block").setAttribute("open", ""));
    await $("#t-alias").selectByIndex(0); // un hôte semé (web-1/db-1)
    await $("#t-bind").setValue("18082");
    await $("#t-host").setValue("localhost");
    await $("#t-port").setValue("5432");
    await $("#t-name").setValue("Tunnel focus");
    await $("#t-submit").click();
    await browser.waitUntil(async () => (await findRow()) !== null, { timeout: 8000, timeoutMsg: "tunnel non listé" });

    // Focaliser « Modifier » comme le ferait un utilisateur au clavier (sans
    // cliquer : un clic ouvrirait la fiche d'édition et déplacerait le focus).
    await browser.execute(() =>
      document.querySelector('#tunnel-list .tunnel-row [data-act="edit"]')?.focus(),
    );
    // Plus long qu'un tick (1,5 s) : la liste est reconstruite au moins une fois.
    await browser.pause(2000);
    const etat = await browser.execute(() => {
      const a = document.activeElement;
      return { dansLaListe: !!a?.closest("#tunnel-list"), act: a?.dataset?.act };
    });
    expect(etat).toEqual({ dansLaListe: true, act: "edit" });

    // Ménage : supprimer la définition et refermer, pour ne rien laisser au
    // scénario suivant.
    await (await findRow()).$('[data-act="delete"]').click();
    await $("#confirm-modal").waitForDisplayed({ timeout: 5000 });
    await $("#confirm-ok").click();
    await browser.waitUntil(async () => (await findRow()) === null, { timeout: 8000, timeoutMsg: "tunnel pas supprimé" });
    await $("#t-close").click();
    await browser.waitUntil(async () => !(await $("#tunnels-modal").isDisplayed()), { timeout: 5000 });
  });
});
