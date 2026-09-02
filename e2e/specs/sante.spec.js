// Santé des hôtes : joignable ou non, sans ouvrir de session.
//
// Le harnais sème « test-ssh » (sshd local, joignable) et « web-1 »
// (10.0.0.1, sans route depuis la machine de test). La sonde, lancée depuis
// la palette, doit colorer les voyants en conséquence et dire pourquoi.
import { HOTES_SEMES } from "../wdio.conf.js";

describe("Santé des hôtes", () => {
  const dot = (cle) => browser.execute((c) => {
    const d = document.querySelector(`#host-list [data-cle="${c}"] .dot`);
    return d ? { classe: d.className, titre: d.title } : null;
  }, cle);

  it("colore les voyants après une sonde lancée depuis la palette", async function () {
    if (!HOTES_SEMES.includes("test-ssh")) this.skip();
    await browser.keys(["Control", "k"]);
    const input = await $("#palette-input");
    await input.waitForDisplayed({ timeout: 5000 });
    await input.setValue("santé");
    const item = await $("#palette-results .item");
    await item.waitForDisplayed({ timeout: 5000 });
    await item.click();

    await browser.waitUntil(async () => (await dot("ssh:test-ssh"))?.classe.includes("up"), {
      timeout: 15000, timeoutMsg: "test-ssh n'est jamais passé joignable",
    });
    const vivant = await dot("ssh:test-ssh");
    expect(vivant.titre).toMatch(/Joignable en \d+ ms/);

    await browser.waitUntil(async () => (await dot("ssh:web-1"))?.classe.includes("down"), {
      timeout: 15000, timeoutMsg: "web-1 n'est jamais passé injoignable",
    });
    const mort = await dot("ssh:web-1");
    expect(mort.titre).toMatch(/^Injoignable : /);

    // La sonde survit au relancement : elle est mémorisée sur la machine.
    const memorise = await browser.execute(() => localStorage.getItem("avash.sante"));
    expect(memorise).toContain("ssh:test-ssh");
    expect(memorise).toContain('"joignable"');
  });
});
