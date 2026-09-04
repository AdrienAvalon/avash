// Connexion VNC réelle contre un serveur de test DÉDIÉ (rustvncserver, mot de
// passe « test », ZRLE) : formulaire → processus avash-rdp --vnc → WebSocket →
// canvas. Le serveur sert une image connue et RÉAGIT aux entrées, ce qui
// permet de vérifier sur les pixels tout le chemin : décodage et peinture
// (rouge à gauche, bleu à droite), souris (un clic pose un carré magenta),
// clavier (« g » repeint en vert, « é » arrive comme le keysym 0xe9).
import { startVncServer, waitForPort, attendreBureauConnecte } from "./helpers.js";
const VNC_PORT = 35900;
let srv;
let journal = "";

/** La couleur d'un pixel du bureau, lue dans le canvas. */
const pixel = (x, y) =>
  browser.execute((px, py) => {
    const c = document.querySelector(".rdp-container canvas");
    const d = c.getContext("2d").getImageData(px, py, 1, 1).data;
    return [d[0], d[1], d[2]];
  }, x, y);

async function attendreCouleur(x, y, attendue, quoi) {
  let vue = null;
  try {
    await browser.waitUntil(async () => {
      vue = await pixel(x, y);
      return JSON.stringify(vue) === JSON.stringify(attendue);
    }, { timeout: 10000 });
  } catch (e) {
    throw new Error(`${quoi} : pixel (${x}, ${y}) = ${JSON.stringify(vue)}, attendu ${JSON.stringify(attendue)}`, { cause: e });
  }
}

describe("VNC — connexion réelle au serveur de test", () => {
  before(async () => {
    srv = startVncServer(VNC_PORT, (ligne) => { journal += ligne; });
    await waitForPort(VNC_PORT);
  });
  after(() => { if (srv) srv.kill(); });

  it("se connecte, affiche le bureau, et réagit à la souris et au clavier", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.execute(() => {
      const r = document.querySelector('input[name="proto"][value="vnc"]');
      r.checked = true;
      r.dispatchEvent(new Event("change", { bubbles: true }));
    });
    // Pas d'utilisateur : l'authentification VNC classique n'en a pas.
    await $("#m-addr").setValue("127.0.0.1");
    await $("#m-port").setValue(String(VNC_PORT));
    await $("#m-password").setValue("test");
    await $("#m-submit").click();

    await $(".rdp-container canvas").waitForExist({ timeout: 20000, timeoutMsg: "aucun canvas VNC" });
    await attendreBureauConnecte("le bureau VNC");
    expect(await $(".rdp-closed").isExisting()).toBe(false);

    // Le bureau du serveur de test : 640×480, rouge à gauche, bleu à droite.
    await attendreCouleur(100, 100, [255, 0, 0], "moitié gauche");
    await attendreCouleur(540, 380, [0, 0, 255], "moitié droite");

    // Un clic au centre du canvas : le serveur pose un carré magenta à
    // l'endroit reçu. Le pixel central prouve le mappage souris (letterbox),
    // le masque de boutons, et la mise à jour incrémentale. Les événements
    // sont dispatchés à la main : le canvas écoute mousedown et mouseup, et le
    // serveur WebDriver embarqué (Windows, macOS) ne produit qu'un `click`
    // synthétique, sans les deux, ce qui ne faisait jamais partir le clic.
    await browser.execute(() => {
      const c = document.querySelector(".rdp-container canvas");
      const r = c.getBoundingClientRect();
      const x = r.left + r.width / 2, y = r.top + r.height / 2;
      for (const type of ["mousedown", "mouseup"]) {
        c.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, buttons: type === "mousedown" ? 1 : 0, clientX: x, clientY: y }));
      }
    });
    try {
      await attendreCouleur(325, 245, [255, 0, 255], "carré magenta au point du clic");
    } catch (e) {
      // Ce que le serveur a reçu dit si c'est le clic ou l'image qui manque.
      throw new Error(`${e.message} ; journal du serveur : ${journal.replace(/\n/g, " / ")}`, { cause: e });
    }
    expect(journal).toContain("clic gauche en");

    // « é » : le keysym Latin-1 0xe9, tel que le serveur le reçoit.
    await browser.keys("é");
    await browser.waitUntil(() => journal.includes("touche 0xe9 enfoncée"), {
      timeout: 5000, timeoutMsg: `le serveur n'a pas reçu le keysym 0xe9 ; journal : ${journal}`,
    });
    // « g » : le serveur repeint tout en vert.
    await browser.keys("g");
    await attendreCouleur(100, 100, [0, 255, 0], "bureau vert après « g »");
    await attendreCouleur(540, 380, [0, 255, 0], "bureau vert, moitié droite");
    console.log(`journal du serveur VNC : ${journal.replace(/\n/g, " / ")}`);
  });

  it("un mauvais mot de passe est refusé, et dit pourquoi", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.execute(() => {
      const r = document.querySelector('input[name="proto"][value="vnc"]');
      r.checked = true;
      r.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await $("#m-addr").setValue("127.0.0.1");
    await $("#m-port").setValue(String(VNC_PORT));
    await $("#m-password").setValue("faux");
    await $("#m-submit").click();
    // L'onglet reste, avec l'incrustation « connexion fermée » et une
    // notification qui porte la raison du processus.
    await browser.waitUntil(async () => (await $$(".rdp-closed")).length > 0, {
      timeout: 20000, timeoutMsg: "aucune incrustation de fermeture",
    });
    let texte = "";
    try {
      await browser.waitUntil(async () => {
        texte = await browser.execute(() => [...document.querySelectorAll(".toast, .rdp-closed")].map((e) => e.textContent).join(" | "));
        return texte.includes("Mot de passe VNC refusé");
      }, { timeout: 5000 });
    } catch (e) {
      throw new Error(`la raison du refus n'est pas affichée ; à l'écran : ${texte}`, { cause: e });
    }
  });
});
