// Le lanceur A/B du banc (scripts/banc.mjs) : l'alternance des références
// neutralise la dérive de la machine (chauffe, charge de fond), et les noms
// tirés des références servent de noms de dossiers sur les trois systèmes.
import test from "node:test";
import assert from "node:assert/strict";
import { ordreAlterne, refSure, lignesBanc } from "../banc.mjs";

test("l'ordre alterne base et tête d'une répétition à l'autre", () => {
  assert.deepEqual(ordreAlterne(2), [["base", "tete"], ["tete", "base"]]);
  assert.deepEqual(ordreAlterne(3).map((r) => r[0]), ["base", "tete", "base"]);
});

test("une seule référence : jamais de base", () => {
  assert.deepEqual(ordreAlterne(2, false), [["tete"], ["tete"]]);
});

test("un nom de référence devient un nom de dossier sûr", () => {
  assert.equal(refSure("v0.16.0"), "v0.16.0");
  assert.equal(refSure("HEAD~1"), "HEAD-1");
  assert.ok(!refSure("origin/main").includes("/"));
  assert.ok(!/[\\:*?"<>|^~ ]/.test(refSure("a\\b:c*d?e\"f<g>h|i^j k~l")));
});

test("les lignes BANC des micro-bancs Rust sont récoltées, le reste ignoré", () => {
  const sortie = [
    "  blocs de 64 o : 812.3 Mo/s",
    'BANC {"cle":"M/utf8-blocs-64","unite":"Mo/s","sens":"haut","echantillons":[800.1,812.3]}',
    "BANC pas du json",
    'BANC {"cle":"M/x","unite":"µs","sens":"bas","echantillons":[1,2]}',
  ].join("\n");
  const l = lignesBanc(sortie);
  assert.equal(l.length, 2);
  assert.equal(l[0].cle, "M/utf8-blocs-64");
  assert.deepEqual(l[1].echantillons, [1, 2]);
});
