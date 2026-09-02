// La barre latérale au clavier.
//
// Les lignes étaient de simples `div` : la connexion passait par un double-clic,
// les options par un clic droit. Tab sautait du champ de recherche aux boutons
// du bas — la liste entière était hors d'atteinte, et l'on ne pouvait ni éditer,
// ni déplacer, ni supprimer un hôte sans souris.
import { EMBARQUE } from "../wdio.conf.js";

describe("Liste d'hôtes — atteignable au clavier", () => {
  const lignes = () => $$("#host-list [data-cle]");
  const focalisee = () => browser.execute(() => document.activeElement?.dataset?.cle ?? null);
  const poserFocus = () =>
    browser.execute(() => document.querySelector("#host-list [data-cle]").focus());

  before(async function () {
    // Le serveur WebDriver embarqué (chemin Windows) synthétise les touches en
    // JavaScript : Origine et Fin n'y sont pas traduites, les flèches n'y sont
    // gérées que sur des boutons radio, et les modificateurs ne s'appliquent
    // pas aux touches de fonction (Maj+F10). Rien de cela ne concerne
    // l'application : ce fichier reste joué sous Linux, avec de vraies touches.
    if (EMBARQUE) this.skip();
    await browser.waitUntil(async () => (await lignes()).length > 0,
      { timeout: 10000, timeoutMsg: "aucune ligne focalisable" });
  });

  it("n'offre qu'un seul arrêt de tabulation pour toute la liste", async () => {
    // Un tabindex=0 par ligne demandait deux cents pressions de Tab pour
    // traverser une barre latérale bien remplie : le champ de recherche et les
    // boutons du bas devenaient inatteignables. On entre en un Tab, on se
    // déplace aux flèches.
    const valeurs = await browser.execute(() =>
      [...document.querySelectorAll("#host-list [data-cle]")].map((e) => e.getAttribute("tabindex")),
    );
    expect(valeurs.length).toBeGreaterThan(1);
    expect(valeurs.filter((v) => v === "0")).toHaveLength(1);
    expect(valeurs.filter((v) => v === "-1")).toHaveLength(valeurs.length - 1);
  });

  it("les flèches déplacent le focus, Origine et Fin vont aux extrémités", async () => {
    await poserFocus();
    const premier = await focalisee();
    expect(premier).not.toBe(null);

    await browser.keys(["ArrowDown"]);
    const second = await focalisee();
    expect(second).not.toBe(premier);

    await browser.keys(["ArrowUp"]);
    expect(await focalisee()).toBe(premier);

    await browser.keys(["End"]);
    const dernier = await focalisee();
    expect(dernier).not.toBe(premier);

    await browser.keys(["Home"]);
    expect(await focalisee()).toBe(premier);
  });

  it("le focus vaut sélection : un seul curseur, pas deux", async () => {
    // Les flèches déplaçaient le focus sans toucher au cadre de sélection :
    // l'utilisateur voyait deux « sélections » et ne savait pas laquelle agit.
    await poserFocus();
    await browser.keys(["ArrowDown"]);
    const coherent = await browser.execute(() => {
      const actif = document.activeElement;
      const marquees = [...document.querySelectorAll("#host-list .picked")];
      return marquees.length === 1 && marquees[0] === actif;
    });
    expect(coherent).toBe(true);
  });

  it("Maj+F10 ouvre le menu, les flèches y naviguent, Échap referme et rend le focus", async () => {
    await poserFocus();
    const origine = await focalisee();
    await browser.keys(["Shift", "F10"]);

    const menu = await browser.waitUntil(
      async () => {
        for (const id of ["host-context", "rdp-context", "folder-context"]) {
          if (await $(`#${id}`).isDisplayed()) return id;
        }
        return false;
      },
      { timeout: 5000, timeoutMsg: "menu contextuel absent au clavier" },
    );

    // Le focus doit être ENTRÉ dans le menu, sinon les flèches continueraient de
    // déplacer la sélection derrière lui et Entrée relancerait une connexion.
    const dansLeMenu = await browser.execute((m) =>
      document.getElementById(m).contains(document.activeElement), menu);
    expect(dansLeMenu).toBe(true);

    await browser.keys(["ArrowDown"]);
    const aBouge = await browser.execute((m) => {
      const items = [...document.getElementById(m).querySelectorAll("[data-act]")].filter((i) => !i.hidden);
      return items.indexOf(document.activeElement) === 1;
    }, menu);
    expect(aBouge).toBe(true);

    await browser.keys(["Escape"]);
    await browser.waitUntil(async () => !(await $(`#${menu}`).isDisplayed()),
      { timeout: 5000, timeoutMsg: "Échap n'a pas refermé le menu" });
    // Et le focus revient d'où il venait.
    expect(await focalisee()).toBe(origine);
  });
});
