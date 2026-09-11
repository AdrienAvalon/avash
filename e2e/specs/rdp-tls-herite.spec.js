// Serveur RDP sans suite TLS moderne : le cas de Windows Server 2012 R2 et
// des versions antérieures, trouvé le 11 septembre 2026 contre une vraie
// machine. Après une négociation X.224 normale (HYBRID), Schannel y coupe la
// connexion par un RST dès le ClientHello d'un client qui n'offre qu'ECDHE
// avec AES-GCM ou ChaCha20 — ce que fait rustls. L'application doit alors
// proposer les suites TLS héritées du système, en nommant la cause, respecter
// le refus, et, si l'utilisateur accepte, réessayer avec la pile du système.
//
// Un faux serveur suffit : il répond à la négociation puis coupe au premier
// octet TLS, exactement comme le vrai. Il coupe aussi la seconde tentative,
// ce qui éprouve la fin de chaîne : plus de proposition, une explication.
import { createServer } from "node:net";
import { waitForPort } from "./helpers.js";

const PORT = 33901;
let srv;

/** Réponse X.224 Connection Confirm avec RDP_NEG_RSP (HYBRID), 19 octets. */
const NEGOCIATION_ACCEPTEE = Buffer.from("030000130ed000001234000200080002000000", "hex");

function demarrerFauxServeur(port) {
  const s = createServer((sock) => {
    let etape = 0;
    sock.on("data", () => {
      if (etape === 0) {
        etape = 1;
        sock.write(NEGOCIATION_ACCEPTEE);
      } else {
        // Le ClientHello : un RST, pas une alerte TLS, comme Schannel 2012 R2.
        sock.resetAndDestroy();
      }
    });
    sock.on("error", () => {});
  });
  s.listen(port, "127.0.0.1");
  return s;
}

async function ouvrirConnexionDirecte(port) {
  await $("#manual-btn").click();
  await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
  await browser.execute(() => {
    const r = document.querySelector('input[name="proto"][value="rdp"]');
    r.checked = true;
    r.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await $("#m-addr").setValue("127.0.0.1");
  await $("#m-port").setValue(String(port));
  await $("#m-user").setValue("test");
  await $("#m-password").setValue("test");
  await $("#m-submit").click();
}

async function texteAffiche() {
  return browser.execute(() =>
    [...document.querySelectorAll(".toast, .rdp-closed")].map((e) => e.textContent).join(" | "));
}

describe("RDP — serveur sans suite TLS moderne (Windows Server 2012 R2)", () => {
  before(async () => { srv = demarrerFauxServeur(PORT); await waitForPort(PORT); });
  after(() => { srv?.close(); });

  it("propose les suites TLS héritées en nommant la cause, et respecte le refus", async () => {
    await ouvrirConnexionDirecte(PORT);
    await $("#confirm-modal").waitForDisplayed({ timeout: 20000, timeoutMsg: "aucune proposition de TLS hérité" });
    const texte = await $("#confirm-modal").getText();
    expect(texte).toMatch(/2012 R2/);
    expect(texte).toMatch(/TLS hérité|legacy TLS/);
    await $("#confirm-cancel").click();
    await browser.waitUntil(async () => (await $$(".rdp-closed")).length > 0,
      { timeout: 10000, timeoutMsg: "le refus n'a pas laissé l'onglet fermé" });
    // Refusé : rien n'a été retenu, l'onglet se ferme proprement.
    await browser.execute(() => document.querySelector(".tab.active .close")?.click());
    await browser.waitUntil(async () => (await $$(".rdp-container")).length === 0, { timeout: 5000 });
  });

  it("réessaie avec la pile du système quand on accepte, et dit si ça échoue encore", async () => {
    await ouvrirConnexionDirecte(PORT);
    await $("#confirm-modal").waitForDisplayed({ timeout: 20000, timeoutMsg: "aucune proposition de TLS hérité" });
    await $("#confirm-ok").click();
    let texte = "";
    await browser.waitUntil(async () => {
      texte = await texteAffiche();
      return texte.includes("suites TLS héritées");
    }, { timeout: 20000, timeoutMsg: `la seconde tentative n'a pas expliqué son échec ; à l'écran : ${texte}` });
    // Plus rien à proposer : la boîte ne revient pas, la cause est nommée.
    expect(await $("#confirm-modal").isDisplayed()).toBe(false);
    expect(texte).toMatch(/certificat/);
  });
});
