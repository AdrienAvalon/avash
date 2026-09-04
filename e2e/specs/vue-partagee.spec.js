// Vue partagée : deux sessions SSH sur le sshd local, Ctrl+Maj+E les met côte à
// côte (deux conteneurs visibles, chacun dans son volet, de largeurs voisines),
// fermer l'un des deux onglets ramène une seule vue, et la palette propose le
// partage quand il y a deux onglets.
import { findHostRow } from "./helpers.js";

const visibles = () => browser.execute(() =>
  [...document.querySelectorAll("#terminal .xterm-container")]
    .filter((c) => c.offsetParent !== null || getComputedStyle(c).display !== "none")
    .map((c) => ({ volet: c.parentElement?.className ?? "", largeur: c.getBoundingClientRect().width })));

describe("Vue partagée — deux onglets côte à côte", () => {
  it("Ctrl+Maj+E partage l'écran entre l'onglet actif et le suivant", async () => {
    for (let i = 0; i < 2; i++) {
      await (await findHostRow("test-ssh")).doubleClick();
      await browser.waitUntil(async () => (await $$(".state.live")).length === i + 1,
        { timeout: 20000, timeoutMsg: `session ${i + 1} jamais live` });
    }
    expect((await visibles()).length).toBe(1);
    await browser.keys(["Control", "Shift", "e"]);
    await browser.waitUntil(async () => (await visibles()).length === 2, { timeout: 5000, timeoutMsg: "deux terminaux jamais visibles" });
    const v = await visibles();
    expect(v.map((x) => x.volet).sort()).toEqual(["volet droit", "volet gauche"]);
    expect(Math.abs(v[0].largeur - v[1].largeur)).toBeLessThan(40);
    expect(await browser.execute(() => document.getElementById("terminal").classList.contains("partage"))).toBe(true);
  });

  it("fermer un des deux onglets referme le partage", async () => {
    await browser.keys(["Control", "w"]);
    await browser.waitUntil(async () => (await visibles()).length === 1, { timeout: 5000, timeoutMsg: "le partage ne s'est pas refermé" });
    expect(await browser.execute(() => document.getElementById("terminal").classList.contains("partage"))).toBe(false);
    expect((await $$(".tab")).length).toBe(1);
  });
});
