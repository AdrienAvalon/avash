// Enregistrement d'une session au format asciicast (asciinema).
//
// Sur la session SSH réelle : on démarre l'enregistrement depuis le menu
// contextuel du terminal, on fait afficher un mot connu, on arrête, puis on
// relit le fichier annoncé — en-tête v2 et un événement de sortie portant le
// mot. Seule la sortie est enregistrée : le fichier ne doit contenir aucun
// événement de frappe.
import { readFileSync } from "node:fs";
import { doubleCliquerHote, ecouterSortiePty, sortiePty } from "./helpers.js";
import { EMBARQUE } from "../wdio.conf.js";

describe("Enregistrement de session (asciicast)", () => {
  const menu = async (act) => {
    const zone = await $("#terminal");
    await browser.execute((el) => {
      el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: 200, clientY: 200 }));
    }, zone);
    const item = await $(`#term-context [data-act="${act}"]`);
    await item.waitForDisplayed({ timeout: 5000, timeoutMsg: `entrée ${act} absente du menu` });
    await item.click();
  };
  const dernierToast = () => browser.execute(() => {
    const t = [...document.querySelectorAll("#toasts .toast")].pop();
    return t ? t.textContent : null;
  });

  const taper = async (texte) => {
    // xterm écoute sur son textarea caché : on le focalise avant de taper.
    // Pas de lettre doublée dans ce qu'on tape : une frappe synthétique répétée
    // trop vite est parfois perdue par le composant (« asciicast » arrivait
    // « ascicast »), et ce n'est pas ce que le scénario vérifie.
    // Le serveur WebDriver embarqué (Windows, macOS) ne synthétise que keydown
    // et keyup ; or xterm.js prend les caractères imprimables sur l'événement
    // `input` de son textarea (le chemin d'une saisie IME ou d'un collage), et
    // Entrée sur keydown. Sur ce chemin, le texte est donc donné par `input`
    // et Entrée part comme touche nommée (sixième passage Windows, 05/09/2026,
    // où « le premier marqueur n'est jamais arrivé »).
    await browser.execute(() => document.querySelector(".xterm-helper-textarea")?.focus());
    const corps = texte.replace(/\n$/, "");
    if (EMBARQUE) {
      await browser.execute((t) => {
        document.querySelector(".xterm-helper-textarea")
          ?.dispatchEvent(new InputEvent("input", { data: t, inputType: "insertText", bubbles: true }));
      }, corps);
    } else {
      await browser.keys(corps);
    }
    if (texte.endsWith("\n")) await browser.keys("Enter");
  };
  // Le chemin annoncé commence par « / » sous Unix et par une lettre de lecteur
  // sous Windows (`C:\…\x.cast`) : le motif ne présume pas de sa forme.
  // Où est le focus, et ce qui est ouvert par-dessus le terminal : pour les
  // messages d'échec des attentes d'écho.
  const ouLeFocus = () => browser.execute(() => {
    const e = document.activeElement;
    const ouverts = [...document.querySelectorAll(".modal-backdrop.open, #palette.open, #term-context.open")].map((m) => m.id);
    return `${e?.tagName ?? "aucun"}#${e?.id ?? ""}.${String(e?.className ?? "").slice(0, 60)} ; ouverts : ${ouverts.join(",") || "rien"}`;
  });
  const cheminDu = (toast) => /: (\S+\.cast)/.exec(toast ?? "")?.[1];
  // Un marqueur unique SANS deux caractères identiques consécutifs : la frappe
  // synthétique en perd un sur deux (« 1788 » arrivait « 178 », « asciicast »
  // « ascicast ») ; on intercale une lettre entre deux jumeaux.
  const marqueur = (base) => `${base}-${String(Date.now()).replace(/(.)(?=\1)/g, "$1x")}`;

  it("enregistre la sortie du terminal dans un fichier relisible", async () => {
    // On écoute `pty-output` AVANT de connecter : l'assertion finale (l'écho
    // d'après l'arrêt n'entre pas dans le .cast) doit être prise après un état
    // observable prouvant que le cœur a vu les octets, pas après un délai fixe.
    await ecouterSortiePty();
    await doubleCliquerHote("test-ssh");
    await browser.waitUntil(async () => (await $$(".state.live")).length > 0,
      { timeout: 20000, timeoutMsg: "session SSH jamais live" });

    // Premier enregistrement : il prouve que le shell traite bien ce qu'on tape
    // (les frappes envoyées pendant son démarrage ne le sont qu'après).
    await menu("record");
    await browser.waitUntil(async () => (await $$(".tab.rec")).length > 0,
      { timeout: 5000, timeoutMsg: "le voyant d'enregistrement n'est pas apparu" });
    const premierFichier = cheminDu(await dernierToast());
    expect(premierFichier).toBeDefined();
    const avant = marqueur("avant-du-lancement");
    await taper(`echo ${avant}\n`);
    // Sur échec, on dit ce que le fichier contient : « jamais arrivé » ne
    // distingue pas une frappe perdue (le marqueur y est, amputé d'un
    // caractère) d'un shell qui n'a rien reçu ou d'un enregistreur qui n'a
    // rien écrit. Vu en intégration le 22 novembre 2026, une fois, sans que
    // le journal permette de trancher.
    try {
      await browser.waitUntil(() => readFileSync(premierFichier, "utf8").includes(avant),
        { timeout: 10000 });
    } catch {
      const contenu = readFileSync(premierFichier, "utf8");
      throw new Error(`le premier marqueur (${avant}) n'est jamais arrivé ; `
        + `fin de l'enregistrement (${contenu.length} octets) : ${contenu.slice(-600)}`);
    }
    await menu("record-stop");
    await browser.waitUntil(async () => (await $$(".tab.rec")).length === 0, { timeout: 5000 });

    // Second enregistrement, en cours de session : il doit s'ouvrir sur l'état
    // de l'écran — le marqueur y est déjà — et non sur du noir.
    await menu("record");
    await browser.waitUntil(async () => (await $$(".tab.rec")).length > 0, { timeout: 5000 });
    const chemin = cheminDu(await dernierToast());
    expect(chemin).toBeDefined();
    expect(chemin).not.toBe(premierFichier);
    const premier = JSON.parse(readFileSync(chemin, "utf8").split("\n")[1]);
    expect(premier[1]).toBe("o");
    expect(premier[2]).toContain(avant);

    await taper("echo bonjour-cast-ok\n");
    // Le fichier est écrit au fil de l'eau : l'écho du serveur y arrive sans
    // attendre l'arrêt.
    // Sur échec, on dit où était le focus, ce que le shell a renvoyé et ce que
    // le fichier contient : vu en intégration le 24 novembre 2026 (sur main,
    // fce15c1), une fois, sans rien pour trancher entre une frappe perdue
    // (l'écho est amputé dans la sortie), des frappes parties ailleurs que dans
    // le terminal (le menu vient de se fermer) et un enregistreur qui n'écrit
    // pas (l'écho est dans la sortie, pas dans le fichier).
    try {
      await browser.waitUntil(() => readFileSync(chemin, "utf8").includes("bonjour-cast-ok"),
        { timeout: 10000 });
    } catch {
      throw new Error(`l'écho n'est jamais arrivé dans l'enregistrement ; focus : ${await ouLeFocus()} ; `
        + `fin de la sortie du terminal : ${(await sortiePty()).slice(-300)} ; `
        + `fin du fichier : ${readFileSync(chemin, "utf8").slice(-300)}`);
    }

    await menu("record-stop");
    await browser.waitUntil(async () => (await $$(".tab.rec")).length === 0, { timeout: 5000 });
    expect(cheminDu(await dernierToast())).toBe(chemin);

    const contenu = readFileSync(chemin, "utf8");
    const lignes = contenu.trim().split("\n");
    const entete = JSON.parse(lignes[0]);
    expect(entete.version).toBe(2);
    expect(entete.title).toBe("test-ssh");
    expect(entete.width).toBeGreaterThan(0);
    const evenements = lignes.slice(1).map((l) => JSON.parse(l));
    expect(evenements.length).toBeGreaterThan(1);
    expect(evenements.every((e) => e[1] === "o" || e[1] === "r")).toBe(true);
    expect(evenements.some((e) => e[1] === "o" && e[2].includes("bonjour-cast-ok"))).toBe(true);

    // La liste des enregistrements, depuis la palette, montre les deux fichiers.
    // Le terminal a le focus : Ctrl+K y est le kill-line de readline, la
    // palette s'ouvre par Ctrl+Maj+K (audit du 12 septembre 2026, C-front-3).
    await browser.keys(["Control", "Shift", "k"]);
    const input = await $("#palette-input");
    await input.waitForDisplayed({ timeout: 5000 });
    await input.setValue("Enregistrements");
    const item = await $("#palette-results .item");
    await item.waitForDisplayed({ timeout: 5000 });
    await item.click();
    await browser.waitUntil(async () => (await $$("#enregistrements-liste .enregistrement-row")).length > 0,
      { timeout: 5000, timeoutMsg: "la liste des enregistrements est restée vide" });
    const listes = await browser.execute(() => [...document.querySelectorAll("#enregistrements-liste .enregistrement-row")].map((r) => r.dataset.chemin));
    expect(listes).toContain(chemin);
    expect(listes).toContain(premierFichier);
    expect(listes[0]).toBe(chemin, "le plus récent d'abord");
    // Le focus dans le terminal, fenêtre ouverte : c'est le cas qui a fait
    // échouer ce scénario deux fois en intégration (22 novembre 2026). Selon
    // l'instant où la palette rendait le focus, Échap fermait la fenêtre ou
    // partait au shell derrière elle, qui avalait ensuite la commande suivante.
    // On se place du mauvais côté de la course : Échap doit fermer la fenêtre,
    // et rien de ce qui est tapé pendant qu'elle est ouverte n'atteint le shell.
    await browser.execute(() => document.querySelector(".xterm-helper-textarea")?.focus());
    const derriere = marqueur("derriere-la-fenetre");
    await browser.keys([...derriere]);
    await browser.keys("Escape");
    await browser.waitUntil(
      () => browser.execute(() => !document.getElementById("enregistrements-modal").classList.contains("open")),
      { timeout: 5000, timeoutMsg: "Échap n'a pas fermé la liste des enregistrements (focus dans le terminal)" },
    );
    expect(await sortiePty()).not.toContain(derriere);

    // Arrêté, l'enregistrement ne reprend pas : ce qui suit n'y entre pas. Le
    // marqueur est unique : le shell garde l'historique des passages précédents
    // et le proposerait en autosuggestion — donc dans la sortie — avant l'arrêt.
    const apres = marqueur("apres-stop");
    await taper(`echo ${apres}\n`);
    // Trouvé par l'audit du 7 septembre 2026 : une pause fixe de 800 ms jugeait
    // l'assertion négative avant que l'écho n'ait fait le trajet
    // xterm → IPC → russh → sshd → shell → retour ; sous charge l'écho arrive
    // après le délai, le fichier est encore vide du marqueur au moment de la
    // lecture et le test passe même si l'enregistrement continue. On attend
    // plutôt que le cœur ait vu l'écho (même trajet, 10 s accordées aux
    // l. 76-77 et 95-96) : `e.sortie(&text)` (sessions.rs) capte l'octet AVANT
    // l'émission `pty-output`, donc voir `apres` dans `sortiePty()` prouve que
    // la pompe a franchi le point de captation. Si l'enregistreur était resté
    // en place, `apres` serait déjà sur disque (flush à chaque ligne). Le
    // marqueur est unique (Date.now), donc `includes` ne fait pas de faux
    // positif avec un écho précédent.
    // Sur échec, on dit où était le focus et ce que le shell a renvoyé en
    // dernier : vu en intégration le 22 novembre 2026 (PR #95), « jamais
    // revenu » ne dit pas si les frappes sont parties ailleurs que dans le
    // terminal (la palette vient de se fermer) ou si le shell n'a pas répondu.
    try {
      await browser.waitUntil(async () => (await sortiePty()).includes(apres), { timeout: 10000 });
    } catch {
      throw new Error(`l'écho après arrêt (${apres}) n'est jamais revenu ; focus : ${await ouLeFocus()} ; `
        + `fin de la sortie du terminal : ${(await sortiePty()).slice(-300)}`);
    }
    expect(readFileSync(chemin, "utf8")).not.toContain(apres);
  });
});
