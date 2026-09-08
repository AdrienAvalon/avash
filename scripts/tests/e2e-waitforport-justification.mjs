// Contrôle reproductible de la justification de `waitForPort` (e2e/specs/helpers.js).
//
// Trouvé par l'audit du 8 septembre 2026 : le commentaire de `waitForPort` et la
// puce correspondante d'e2e/README.md justifiaient les DEUX connexions
// successives par « la seconde n'est tentée qu'après fermeture de la première, ce
// qui vérifie que la boucle d'acceptation a bouclé » — le serveur serait *revenu*
// écouter. C'est faux : l'événement `connect` d'un client TCP est émis dès que le
// noyau a terminé la poignée de main, ce qu'il fait pour toute connexion en file
// d'attente (backlog) SANS que le serveur ait appelé accept(). Deux connexions
// réussissent donc sur n'importe quel socket en écoute, même si le serveur ne
// sert jamais aucun client. La stabilité observée venait du délai et du plafond
// porté à 15 s, pas de la propriété annoncée ; un relecteur qui croyait la
// garantie acquise ne cherchait pas ailleurs si l'aléa revenait.
//
// Ce contrôle : (1) démontre le fait — deux sondes du corps de `waitForPort`
// réussissent contre un serveur qui n'accepte applicativement AUCUN client
// (`maxConnections = 0`) ; (2) exige que le code et le README aient retiré la
// fausse garantie et disent la vérité (backlog, simple délai supplémentaire).
import test from "node:test";
import assert from "node:assert/strict";
import { createServer, connect } from "node:net";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const ici = dirname(fileURLToPath(import.meta.url));
const racine = resolve(ici, "../..");
const helpers = readFileSync(resolve(racine, "e2e/specs/helpers.js"), "utf8");
const readme = readFileSync(resolve(racine, "e2e/README.md"), "utf8");

// Une sonde identique à `uneConnexion()` de waitForPort : connect puis end.
function uneConnexion(port) {
  return new Promise((ok, ko) => {
    const sock = connect(port, "127.0.0.1");
    sock.once("connect", () => { sock.end(); ok(); });
    sock.once("error", (e) => { sock.destroy(); ko(e); });
  });
}

test("deux connexions successives réussissent sur un serveur qui ne sert personne", async () => {
  // `maxConnections = 0` : le serveur laisse le noyau terminer la poignée de main
  // (backlog) puis coupe aussitôt — il ne « revient » jamais servir un client.
  // Si deux connexions prouvaient que la boucle d'acceptation a bouclé, elles
  // devraient échouer ici ; elles réussissent, donc la garantie était fausse.
  const srv = createServer();
  srv.maxConnections = 0;
  await new Promise((r) => srv.listen(0, "127.0.0.1", r));
  const port = srv.address().port;
  try {
    await uneConnexion(port);
    await uneConnexion(port); // seconde poignée de main : backlog, pas un accept servi
    // On est arrivé ici : les deux sondes ont « connecté » sans service réel.
    assert.ok(true);
  } finally {
    srv.close();
  }
});

test("waitForPort enchaîne bien deux connexions (le code n'a pas régressé)", () => {
  const nb = (helpers.match(/uneConnexion\(\)/g) || []).length;
  assert.ok(nb >= 2, "waitForPort doit toujours enchaîner deux connexions successives");
});

test("le commentaire de helpers.js ne prétend plus prouver le retour à accept()", () => {
  assert.doesNotMatch(
    helpers,
    /vérifie que la boucle d'acceptation a bouclé/,
    "helpers.js affirme encore que deux connexions vérifient le retour de la boucle d'acceptation",
  );
  assert.doesNotMatch(
    helpers,
    /le serveur est revenu accepter/,
    "le commentaire de la seconde sonde promet encore, à tort, le retour à accept()",
  );
  assert.match(
    helpers,
    /ne prouve PAS que la boucle d'acceptation/,
    "helpers.js doit dire explicitement que deux connexions ne prouvent pas le retour de la boucle d'acceptation",
  );
  assert.match(
    helpers,
    /backlog/,
    "helpers.js doit expliquer le vrai mécanisme (poignée de main du noyau via le backlog)",
  );
});

test("e2e/README.md ne présente plus les deux connexions comme une preuve du retour à écouter", () => {
  assert.doesNotMatch(
    readme,
    /pas que le serveur est \*revenu\* l'écouter/,
    "e2e/README.md justifie encore les deux connexions par le retour du serveur à écouter",
  );
  assert.match(
    readme,
    /backlog/,
    "e2e/README.md doit dire que le `connect` vient du backlog du noyau, sans accept() serveur",
  );
});
