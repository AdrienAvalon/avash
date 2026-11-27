// Le comparateur du banc (e2e/banc/comparer.js) : c'est lui qui bloque une PR.
// Il doit voir une vraie régression, ignorer le bruit de la machine, ne jamais
// diviser par zéro et ne pas juger une métrique mesurée d'un seul côté.
import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { comparer, enMarkdown, depassementsAbsolus } from "../../e2e/banc/comparer.js";
import { nouveauResultat, ajouter, ecrireResultat } from "../../e2e/banc/commun.js";

const SANS_GARDE = { defaut: { seuil: 0.1 }, metriques: {} };

function paire(cle, meta, ea, eb) {
  const a = ajouter(nouveauResultat({ reference: "base", commit: "a", plateforme: "linux" }), cle, meta, ea);
  const b = ajouter(nouveauResultat({ reference: "tete", commit: "b", plateforme: "linux" }), cle, meta, eb);
  return [a, b];
}

test("une latence qui passe de 10 à 13 ms est une régression", () => {
  const [a, b] = paire("S2/echo-1-session", { unite: "ms", sens: "bas" }, [10, 10, 10, 10, 10], [13, 13, 13, 13, 13]);
  const [l] = comparer(a, b, SANS_GARDE);
  assert.equal(l.verdict, "pire");
  assert.ok(Math.abs(l.ecart - 0.3) < 1e-9, `écart ${l.ecart}`);
});

test("des images par seconde qui baissent sont une régression (sens haut)", () => {
  const [a, b] = paire("S5/fps-mire", { unite: "i/s", sens: "haut" }, [25, 25, 25], [20, 20, 20]);
  assert.equal(comparer(a, b, SANS_GARDE)[0].verdict, "pire");
});

test("des images par seconde qui montent sont un progrès", () => {
  const [a, b] = paire("S5/fps-mire", { unite: "i/s", sens: "haut" }, [10, 10, 10], [25, 25, 25]);
  assert.equal(comparer(a, b, SANS_GARDE)[0].verdict, "mieux");
});

test("des dispersions qui se chevauchent sont du bruit", () => {
  const [a, b] = paire("S1/dcl", { unite: "ms", sens: "bas" }, [10, 11, 12, 20], [11, 12, 13, 21]);
  assert.equal(comparer(a, b, SANS_GARDE)[0].verdict, "bruit");
});

test("un écart sous le seuil est du bruit, même sans chevauchement", () => {
  const [a, b] = paire("S1/dcl", { unite: "ms", sens: "bas" }, [100, 100, 100], [105, 105, 105]);
  assert.equal(comparer(a, b, SANS_GARDE)[0].verdict, "bruit");
});

test("zéro contre zéro : du bruit, pas de NaN", () => {
  const [a, b] = paire("S2/echo-1-session", { unite: "ms", sens: "bas" }, [0, 0, 0], [0, 0, 0]);
  const [l] = comparer(a, b, SANS_GARDE);
  assert.equal(l.verdict, "bruit");
  assert.ok(Number.isFinite(l.ecart));
});

test("une métrique mesurée d'un seul côté est absente, pas jugée", () => {
  const a = nouveauResultat({ reference: "base", commit: "a", plateforme: "linux" });
  const b = ajouter(nouveauResultat({ reference: "tete", commit: "b", plateforme: "linux" }), "S4/memoire-30", { unite: "Mo", sens: "bas" }, [1, 2, 3]);
  const [l] = comparer(a, b, SANS_GARDE);
  assert.equal(l.verdict, "absent");
  assert.equal(l.a, null);
});

test("moins de trois échantillons d'un côté : absent", () => {
  const [a, b] = paire("S3/flot-50mo", { unite: "ms", sens: "bas" }, [100, 100], [500, 500, 500]);
  assert.equal(comparer(a, b, SANS_GARDE)[0].verdict, "absent");
});

test("le seuil propre à une métrique prime sur le défaut", () => {
  const [a, b] = paire("S1/dcl", { unite: "ms", sens: "bas" }, [100, 100, 100], [125, 125, 125]);
  const seuils = { defaut: { seuil: 0.1 }, metriques: { "S1/dcl": { seuil: 0.3 } } };
  assert.equal(comparer(a, b, seuils)[0].verdict, "bruit");
});

test("la CLI ne bloque que sur une régression gardée", () => {
  const dossier = mkdtempSync(join(tmpdir(), "avash-banc-test-"));
  try {
    const [a, b] = paire("S2/echo-1-session", { unite: "ms", sens: "bas" }, [10, 10, 10], [20, 20, 20]);
    ecrireResultat(a, join(dossier, "a.json"));
    ecrireResultat(b, join(dossier, "b.json"));
    const cli = join(import.meta.dirname, "..", "..", "e2e", "banc", "comparer.js");
    const lancer = (seuils) => {
      writeFileSync(join(dossier, "s.json"), JSON.stringify(seuils));
      try {
        execFileSync(process.execPath, [cli, join(dossier, "a.json"), join(dossier, "b.json"), "--seuils", join(dossier, "s.json")], { stdio: "pipe" });
        return 0;
      } catch (e) {
        return e.status;
      }
    };
    assert.equal(lancer(SANS_GARDE), 0);
    assert.equal(lancer({ defaut: { seuil: 0.1 }, metriques: { "S2/echo-1-session": { garde: true } } }), 1);
  } finally {
    rmSync(dossier, { recursive: true, force: true });
  }
});

test("le tableau Markdown a une ligne par métrique et dit la charge des machines", () => {
  const [a, b] = paire("S2/echo-1-session", { unite: "ms", sens: "bas" }, [1, 1, 1], [1, 1, 1]);
  ajouter(a, "S1/dcl", { unite: "ms", sens: "bas" }, [130, 131, 132]);
  ajouter(b, "S1/dcl", { unite: "ms", sens: "bas" }, [130, 131, 132]);
  a.machine.chargeDepart = { type: "loadavg-1min", valeur: 0.5 };
  b.machine.chargeFin = { type: "loadavg-1min", valeur: 1.25 };
  const md = enMarkdown(comparer(a, b, SANS_GARDE), a, b);
  assert.match(md, /S2\/echo-1-session/);
  assert.match(md, /S1\/dcl/);
  assert.match(md, /0\.5/);
  assert.match(md, /1\.25/);
});

test("un plafond absolu (endurance) se juge sans référence", () => {
  const r = ajouter(nouveauResultat({ reference: "main", commit: "c", plateforme: "linux" }), "S8/pente-memoire", { unite: "Mo/h", sens: "bas" }, [120]);
  assert.deepEqual(depassementsAbsolus(r, { absolus: { "S8/pente-memoire": { max: 200 } } }), []);
  const d = depassementsAbsolus(r, { absolus: { "S8/pente-memoire": { max: 50 } } });
  assert.equal(d.length, 1);
  assert.match(d[0], /S8\/pente-memoire/);
  assert.deepEqual(depassementsAbsolus(r, { absolus: { "S9/inconnue": { max: 1 } } }), []);
});
