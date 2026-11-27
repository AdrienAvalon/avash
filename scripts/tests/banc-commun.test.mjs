// La bibliothèque commune du banc de mesures (e2e/banc/commun.js) : ses
// statistiques, son relevé de processus et son format de résultat. Le banc n'a
// pas d'assertion sur les chiffres qu'il mesure ; ce qu'il calcule, lui, doit
// être juste, sans quoi le comparateur juge sur du faux.
//
// Conception : docs/superpowers/specs/2026-11-27-banc-et-audit-design.md.
import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  stats, pente, arbreDeProcessus, releveProcessus,
  nouveauResultat, ajouter, ecrireResultat, lireResultat,
} from "../../e2e/banc/commun.js";

test("stats : médiane, quartiles, bornes", () => {
  const s = stats([5, 1, 3, 2, 4]);
  assert.equal(s.n, 5);
  assert.equal(s.mediane, 3);
  assert.equal(s.p25, 2);
  assert.equal(s.p75, 4);
  assert.equal(s.min, 1);
  assert.equal(s.max, 5);
});

test("stats : une liste vide n'a pas de statistiques, les null sont ignorés", () => {
  assert.equal(stats([]), null);
  assert.equal(stats([null, null]), null);
  assert.equal(stats([null, 2]).n, 1);
});

test("pente : moindres carrés en unités par heure", () => {
  assert.equal(pente([{ t: 0, v: 100 }, { t: 3600, v: 110 }]), 10);
  assert.equal(pente([{ t: 0, v: 5 }, { t: 1800, v: 5 }, { t: 3600, v: 5 }]), 0);
  assert.equal(pente([]), null);
  assert.equal(pente([{ t: 0, v: 1 }]), null);
});

test("ajouter concatène les échantillons d'une même clé", () => {
  const r = nouveauResultat({ reference: "HEAD", commit: "abc", plateforme: "linux" });
  ajouter(r, "S2/echo-1-session", { unite: "ms", sens: "bas" }, [1, 2]);
  ajouter(r, "S2/echo-1-session", { unite: "ms", sens: "bas" }, [3]);
  assert.deepEqual(r.metriques["S2/echo-1-session"], { unite: "ms", sens: "bas", echantillons: [1, 2, 3] });
});

test("un résultat écrit puis relu est identique", () => {
  const dossier = mkdtempSync(join(tmpdir(), "avash-banc-test-"));
  try {
    const r = nouveauResultat({ reference: "v0.16.0", commit: "9a9b303", plateforme: "linux" });
    ajouter(r, "S5/fps-mire", { unite: "i/s", sens: "haut" }, [25, 24, 25]);
    r.absents.push("S4/memoire-30 : onglet jamais ouvert");
    const chemin = join(dossier, "r.json");
    ecrireResultat(r, chemin);
    assert.deepEqual(lireResultat(chemin), r);
  } finally {
    rmSync(dossier, { recursive: true, force: true });
  }
});

test("relevé d'un processus disparu : rien de mesuré, pas un zéro", async () => {
  assert.deepEqual(await releveProcessus([999999999]), { memoireMo: null, processeurS: null, vivants: 0 });
});

test("relevé du processus courant : vivant, avec de la mémoire", async () => {
  const r = await releveProcessus([process.pid]);
  assert.equal(r.vivants, 1);
  assert.ok(r.memoireMo > 0, `mémoire ${r.memoireMo}`);
  assert.ok(r.processeurS >= 0, `processeur ${r.processeurS}`);
});

test("l'arbre de processus contient la racine et ses enfants", async () => {
  const enfant = spawn(process.execPath, ["-e", "setTimeout(() => {}, 5000)"], { stdio: "ignore" });
  try {
    await new Promise((res) => setTimeout(res, 300));
    const arbre = await arbreDeProcessus(process.pid);
    assert.ok(arbre.includes(process.pid));
    assert.ok(arbre.includes(enfant.pid), `enfant ${enfant.pid} absent de ${arbre}`);
  } finally {
    enfant.kill();
  }
});
