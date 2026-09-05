// Deux mesures que la feuille de route (axe 3) réclamait avant toute décision :
//
// 1. le démarrage du front : ce que coûtent le chargement et l'exécution du
//    paquet JavaScript entre la navigation et le premier affichage. C'est ce
//    chiffre qui a décidé de charger xterm.js à part (web/xterm-charge.ts) :
//    323 ms de DOMContentLoaded avant, 205 après (feuille de route, axe 3) ;
// 2. la latence à la frappe sur une session SSH réelle (sshd local) : du
//    `keydown` reçu par la page à l'arrivée de l'écho dans la webview
//    (événement Tauri `pty-output`), puis à l'image suivante. Tout le chemin
//    y passe : xterm.js, l'IPC Tauri, russh, le PTY, sshd, le retour.
//
// Les résultats s'écrivent dans e2e/mesures/resultat.json (ignoré par git) et
// sur la sortie standard. Aucune assertion sur les valeurs.
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { doubleCliquerHote, findHostRow } from "../specs/helpers.js";

const NB_DEMARRAGES = 5;
const NB_FRAPPES = 40;
// Des touches toutes différentes de la précédente : l'écho est reconnu par la
// présence de la touche dans la sortie, une répétition serait ambiguë.
const TOUCHES = "abcdefghijklmnopqrstuvwxyz";

const resultat = { demarrages: [], frappes: null };

function stats(valeurs) {
  const t = [...valeurs].sort((a, b) => a - b);
  const q = (p) => t[Math.min(t.length - 1, Math.floor(p * t.length))];
  return { n: t.length, mediane: q(0.5), p95: q(0.95), max: t[t.length - 1] };
}
const ms = (v) => `${v.toFixed(1)} ms`;

/** Chronologie de la navigation, lue par l'API Performance de la webview. */
function chronologieDemarrage() {
  return browser.execute(() => {
    const n = performance.getEntriesByType("navigation")[0];
    const peintures = Object.fromEntries(
      performance.getEntriesByType("paint").map((p) => [p.name, p.startTime]),
    );
    const ressources = performance.getEntriesByType("resource").map((r) => ({
      nom: r.name.split("/").pop(),
      duree: r.duration,
      octets: r.decodedBodySize || r.transferSize || 0,
    }));
    // Repères posés par le front : premier module évalué (avant xterm.js),
    // puis tous les modules évalués (début de l'initialisation d'avash).
    const reperes = Object.fromEntries(
      performance.getEntriesByType("mark").map((m) => [m.name, m.startTime]),
    );
    return {
      // Analyse HTML terminée : les scripts de module s'exécutent ensuite,
      // jusqu'à DOMContentLoaded.
      domInteractive: n.domInteractive,
      modulesDebut: reperes["avash:modules-debut"] ?? null,
      modulesEvalues: reperes["avash:modules-evalues"] ?? null,
      dclDebut: n.domContentLoadedEventStart,
      dclFin: n.domContentLoadedEventEnd,
      chargement: n.loadEventEnd,
      premierePeinture: peintures["first-paint"] ?? null,
      premierContenu: peintures["first-contentful-paint"] ?? null,
      ressources,
    };
  });
}

describe("Mesures du front", () => {
  it("démarrage : chargement et exécution du paquet", async () => {
    for (let i = 0; i < NB_DEMARRAGES; i++) {
      if (i > 0) await browser.reloadSession();
      // La liste d'hôtes vient du cœur : quand elle est là, le front a fini
      // son premier rendu utile.
      await findHostRow("db-1");
      resultat.demarrages.push(await chronologieDemarrage());
    }
    const exec = resultat.demarrages.map((d) => d.dclDebut - d.domInteractive);
    const dcl = resultat.demarrages.map((d) => d.dclFin);
    const fcp = resultat.demarrages.map((d) => d.premierContenu).filter((v) => v !== null);
    console.log(`\nDémarrage du front (${NB_DEMARRAGES} lancements) :`);
    console.log(`  exécution des modules (domInteractive → DOMContentLoaded) : médiane ${ms(stats(exec).mediane)}, max ${ms(stats(exec).max)}`);
    // Le détail, quand les repères sont là : lecture et compilation du paquet
    // (jusqu'au premier module), évaluation des modules (xterm.js compris),
    // puis initialisation d'avash. C'est la part du milieu qu'un chargement
    // différé du terminal ferait gagner, plus la compilation de sa part.
    const avecReperes = resultat.demarrages.filter((d) => d.modulesDebut !== null && d.modulesEvalues !== null);
    if (avecReperes.length) {
      const lecture = avecReperes.map((d) => d.modulesDebut - d.domInteractive);
      const evaluation = avecReperes.map((d) => d.modulesEvalues - d.modulesDebut);
      const init = avecReperes.map((d) => d.dclDebut - d.modulesEvalues);
      console.log(`    dont lecture et compilation du paquet (→ premier module) : médiane ${ms(stats(lecture).mediane)}, max ${ms(stats(lecture).max)}`);
      console.log(`    dont évaluation des modules (xterm.js compris) : médiane ${ms(stats(evaluation).mediane)}, max ${ms(stats(evaluation).max)}`);
      console.log(`    dont initialisation d'avash (→ DOMContentLoaded) : médiane ${ms(stats(init).mediane)}, max ${ms(stats(init).max)}`);
    }
    console.log(`  DOMContentLoaded depuis la navigation : médiane ${ms(stats(dcl).mediane)}, max ${ms(stats(dcl).max)}`);
    if (fcp.length) console.log(`  premier contenu peint : médiane ${ms(stats(fcp).mediane)}, max ${ms(stats(fcp).max)}`);
    const js = resultat.demarrages[0].ressources.find((r) => r.nom.endsWith(".js"));
    if (js) console.log(`  paquet ${js.nom} : ${js.octets} octets, chargé en ${ms(js.duree)}`);
  });

  it("latence à la frappe sur la session SSH locale", async () => {
    await doubleCliquerHote("test-ssh");
    await browser.waitUntil(async () => (await $$(".state.live")).length > 0, {
      timeout: 20000, timeoutMsg: "session SSH jamais live",
    });
    // Sonde posée dans la page : le keydown en phase de capture précède le
    // gestionnaire de xterm.js ; l'écho arrive par l'événement Tauri que le
    // front écoute lui-même, et l'image d'après par requestAnimationFrame.
    // L'application n'expose pas l'API Tauri globale (`window.__TAURI__` est
    // vide) : on s'abonne par les internes de la webview, comme le fait
    // `@tauri-apps/api` lui-même.
    await browser.execute(async () => {
      const m = { frappes: [], attente: null, derniereSortie: performance.now(), sorties: 0 };
      window.__mesure = m;
      document.addEventListener("keydown", (e) => {
        if (e.key.length === 1) m.attente = { t0: performance.now(), touche: e.key };
      }, true);
      const i = window.__TAURI_INTERNALS__;
      await i.invoke("plugin:event|listen", {
        event: "pty-output",
        target: { kind: "Any" },
        handler: i.transformCallback((ev) => {
          m.derniereSortie = performance.now();
          m.sorties++;
          const a = m.attente;
          if (a && String(ev.payload.data).includes(a.touche)) {
            const t1 = performance.now();
            m.attente = null;
            requestAnimationFrame(() => m.frappes.push({ echo: t1 - a.t0, image: performance.now() - a.t0 }));
          }
        }),
      });
    });
    // L'invite du shell : la sortie s'est tue depuis un moment.
    await browser.waitUntil(
      () => browser.execute(() => {
        const m = window.__mesure;
        return m.sorties > 0 && performance.now() - m.derniereSortie > 700;
      }),
      { timeout: 15000, timeoutMsg: "l'invite du shell n'est jamais venue" },
    );
    await $(".xterm textarea").click();
    for (let i = 0; i < NB_FRAPPES; i++) {
      await browser.keys(TOUCHES[i % TOUCHES.length]);
      await browser.waitUntil(
        () => browser.execute((n) => window.__mesure.frappes.length >= n, i + 1),
        { timeout: 5000, timeoutMsg: `écho de la frappe ${i + 1} jamais arrivé` },
      );
    }
    const frappes = await browser.execute(() => window.__mesure.frappes);
    resultat.frappes = {
      echo: stats(frappes.map((f) => f.echo)),
      image: stats(frappes.map((f) => f.image)),
      brut: frappes,
    };
    console.log(`\nLatence à la frappe, SSH local (${NB_FRAPPES} touches) :`);
    console.log(`  keydown → écho reçu : médiane ${ms(resultat.frappes.echo.mediane)}, p95 ${ms(resultat.frappes.echo.p95)}, max ${ms(resultat.frappes.echo.max)}`);
    console.log(`  keydown → image suivante : médiane ${ms(resultat.frappes.image.mediane)}, p95 ${ms(resultat.frappes.image.p95)}, max ${ms(resultat.frappes.image.max)}`);
  });

  after(() => {
    writeFileSync(join(import.meta.dirname, "resultat.json"), JSON.stringify(resultat, null, 2) + "\n");
  });
});
