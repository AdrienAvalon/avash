#!/usr/bin/env node
// Lanceur du banc de mesures : construit une ou deux références d'Avash, les
// fait tourner en alternance sur la même machine avec le harnais de l'arbre
// courant, puis compare.
//
// Usage :
//   node scripts/banc.mjs [--base <ref>] [--tete <ref>] [--jeu rapide|complet|endurance]
//                         [--repetitions N] [--sortie <dossier>] [--cache <dossier>]
//                         [--latence-ms N] [--informatif] [--forcer] [--garder]
//
//   --tete        référence mesurée (HEAD par défaut) ;
//   --base        référence de comparaison ; sans elle, une seule référence
//                 est mesurée (ligne de base) ;
//   --jeu         rapide (S1, S2, S3, S4 à 10 sessions, S5 mire, comme en PR),
//                 complet (tout sauf l'endurance, paliers 10/20/30, SFTP et
//                 micro-bancs Rust), endurance (S8) ;
//   --latence-ms  Linux : une passe SFTP de plus, avec cette latence
//                 aller-retour injectée sur la boucle locale (netem, sudo) ;
//   --informatif  publie la comparaison sans jamais échouer ;
//   --forcer      mesure même sur une machine chargée (chiffres douteux) ;
//   --garder      garde les worktrees de construction.
//
// Variables passées aux scénarios : BANC_XRDP, BANC_XRDP_MDP, BANC_RDP_WINDOWS,
// BANC_RDP_WINDOWS_MDP (voir e2e/banc/sessions.spec.js).
//
// Les binaires construits restent dans le cache de la machine (jamais publiés :
// sous Windows ils portent la fonctionnalité `webdriver`). Le profil est celui
// de la publication (`--release`) : on mesure ce qui est livré.
//
// Conception : docs/superpowers/specs/2026-11-27-banc-et-audit-design.md.
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const WINDOWS = process.platform === "win32";
const RACINE = resolve(import.meta.dirname, "..");
const exe = (nom) => (WINDOWS ? `${nom}.exe` : nom);

/** Ordre des passes : base puis tête, puis tête puis base, etc. Alterner
 *  répartit la dérive de la machine (chauffe, tâches de fond) entre les deux. */
export function ordreAlterne(n, avecBase = true) {
  return Array.from({ length: n }, (_, i) => (!avecBase ? ["tete"] : i % 2 === 0 ? ["base", "tete"] : ["tete", "base"]));
}

/** Un nom de dossier sûr sur les trois systèmes, tiré d'une référence git. */
export function refSure(ref) {
  return ref.replace(/~/g, "-").replace(/[^A-Za-z0-9._-]/g, "-");
}

/** Les lignes `BANC {json}` qu'impriment les micro-bancs Rust. */
export function lignesBanc(texte) {
  const out = [];
  for (const l of texte.split(/\r?\n/)) {
    if (!l.startsWith("BANC ")) continue;
    try { out.push(JSON.parse(l.slice(5))); } catch { /* ligne tronquée : ignorée */ }
  }
  return out;
}

function options(argv) {
  const o = { tete: "HEAD", base: null, jeu: "rapide", repetitions: 3, sortie: null, cache: null, latenceMs: null, informatif: false, forcer: false, garder: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const valeur = () => { const v = argv[++i]; if (v === undefined) throw new Error(`${a} attend une valeur`); return v; };
    if (a === "--tete") o.tete = valeur();
    else if (a === "--base") o.base = valeur();
    else if (a === "--jeu") o.jeu = valeur();
    else if (a === "--repetitions") o.repetitions = Number(valeur());
    else if (a === "--sortie") o.sortie = valeur();
    else if (a === "--cache") o.cache = valeur();
    else if (a === "--latence-ms") o.latenceMs = Number(valeur());
    else if (a === "--informatif") o.informatif = true;
    else if (a === "--forcer") o.forcer = true;
    else if (a === "--garder") o.garder = true;
    else throw new Error(`option inconnue : ${a}`);
  }
  if (!["rapide", "complet", "endurance"].includes(o.jeu)) throw new Error(`jeu inconnu : ${o.jeu}`);
  if (!(o.repetitions >= 1)) throw new Error("--repetitions attend un entier positif");
  return o;
}

function lancer(cmd, args, opts = {}) {
  console.log(`\n$ ${cmd} ${args.join(" ")}${opts.cwd ? `   (dans ${opts.cwd})` : ""}`);
  const r = spawnSync(cmd, args, { stdio: "inherit", shell: WINDOWS, ...opts });
  if (r.status !== 0) throw new Error(`${cmd} ${args.join(" ")} : code ${r.status}`);
}

function capturer(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { encoding: "utf8", shell: WINDOWS, maxBuffer: 64 * 1024 * 1024, ...opts });
  if (r.status !== 0) throw new Error(`${cmd} ${args.join(" ")} : code ${r.status}\n${r.stderr}`);
  return r.stdout;
}

function tripletHote() {
  return /host: (\S+)/.exec(capturer("rustc", ["-vV"]))[1];
}

/** L'outillage de mesure vient de l'arbre courant : une référence plus
 *  ancienne ne l'a pas (exemple SFTP, micro-bancs), ou dans une version qui
 *  n'imprime pas de ligne BANC. Ce ne sont que des programmes de mesure : ils
 *  ne changent rien au binaire mesuré. */
function apporterOutillage(arbre) {
  for (const f of ["crates/avash/examples/banc_sftp.rs", "crates/avash/benches/config_ssh.rs", "crates/avash-ui/benches/utf8.rs"]) {
    mkdirSync(join(arbre, f, ".."), { recursive: true });
    copyFileSync(join(RACINE, f), join(arbre, f));
  }
  const manifeste = join(arbre, "crates/avash/Cargo.toml");
  const s = readFileSync(manifeste, "utf8");
  if (!s.includes('name = "config_ssh"')) writeFileSync(manifeste, `${s.trimEnd()}\n\n[[bench]]\nname = "config_ssh"\nharness = false\n`);
}

/** Construit une référence (profil release) et rend le dossier de ses binaires. */
function construire(ref, o) {
  const commit = capturer("git", ["rev-parse", "--verify", `${ref}^{commit}`], { cwd: RACINE }).trim();
  const court = commit.slice(0, 9);
  const binDir = join(o.cache, "bin", court);
  const arbre = join(o.cache, "arbres", `${refSure(ref)}-${court}`);
  const info = { ref, commit, binDir, arbre };
  if (existsSync(join(binDir, exe("avash-ui"))) && existsSync(join(binDir, exe("banc_sftp")))) {
    console.log(`\n${ref} (${court}) : binaires déjà construits dans ${binDir}`);
    return info;
  }
  if (!existsSync(arbre)) lancer("git", ["worktree", "add", "--detach", arbre, commit], { cwd: RACINE });
  apporterOutillage(arbre);
  const cible = { ...process.env, CARGO_TARGET_DIR: join(o.cache, "cible") };
  const cibleSidecar = { ...process.env, CARGO_TARGET_DIR: join(o.cache, "cible-sidecar") };
  lancer("npm", ["ci", "--no-audit", "--no-fund"], { cwd: join(arbre, "web") });
  lancer("npx", ["vite", "build"], { cwd: join(arbre, "web") });
  lancer("cargo", ["build", "--locked", "--release", "--manifest-path", join(arbre, "rdp-sidecar", "Cargo.toml")], { env: cibleSidecar });
  // avash-ui déclare le processus RDP en ressource embarquée (externalBin) :
  // il doit exister avant la compilation (comme dans ci.yml).
  mkdirSync(join(arbre, "crates/avash-ui/binaries"), { recursive: true });
  copyFileSync(join(o.cache, "cible-sidecar", "release", exe("avash-rdp")),
    join(arbre, "crates/avash-ui/binaries", `avash-rdp-${tripletHote()}${WINDOWS ? ".exe" : ""}`));
  // Sous Windows, le harnais passe par le serveur WebDriver embarqué : c'est
  // la seule fois où cette fonctionnalité est posée, et ce binaire ne quitte
  // pas la machine (cache du banc, jamais publié).
  lancer("cargo", ["build", "--locked", "--release", "-p", "avash-ui", ...(WINDOWS ? ["--features", "webdriver"] : [])], { cwd: arbre, env: cible });
  lancer("cargo", ["build", "--locked", "--release", "--example", "banc_sftp", "-p", "avash"], { cwd: arbre, env: cible });
  mkdirSync(binDir, { recursive: true });
  copyFileSync(join(o.cache, "cible", "release", exe("avash-ui")), join(binDir, exe("avash-ui")));
  copyFileSync(join(o.cache, "cible-sidecar", "release", exe("avash-rdp")), join(binDir, exe("avash-rdp")));
  copyFileSync(join(o.cache, "cible", "release", "examples", exe("banc_sftp")), join(binDir, exe("banc_sftp")));
  return info;
}

/** Les serveurs de test du harnais (arbre courant). */
function serveursDeTest() {
  for (const s of ["test-rdp-server"]) {
    if (!existsSync(join(RACINE, s, "target", "release", exe(s)))) {
      lancer("cargo", ["build", "--locked", "--release", "--manifest-path", join(RACINE, s, "Cargo.toml")]);
    }
  }
  if (!existsSync(join(RACINE, "test-rdp-server", "cert.pem"))) {
    throw new Error("test-rdp-server/cert.pem absent : voir e2e/README.md (certificat du serveur RDP de test)");
  }
}

const SPECS = {
  rapide: { specs: ["./banc/terminal.spec.js", "./banc/sessions.spec.js", "./banc/rdp.spec.js"], paliers: "10" },
  complet: { specs: ["./banc/terminal.spec.js", "./banc/sessions.spec.js", "./banc/rdp.spec.js", "./banc/sftp.spec.js"], paliers: "10,20,30" },
  endurance: { specs: ["./banc/endurance.spec.js"], paliers: "10" },
};

async function charge() {
  const { chargeMachine } = await import("../e2e/banc/commun.js");
  return chargeMachine();
}

function chargeExcessive(c) {
  return c.type === "loadavg-1min" ? c.valeur > 4 : c.valeur > 50;
}

/** Une passe du harnais sur un binaire, échantillons versés dans `json`. */
async function passe(info, json, o, extra = {}) {
  const { lireResultat, ecrireResultat } = await import("../e2e/banc/commun.js");
  const avant = await charge();
  const env = {
    ...process.env,
    AVASH_E2E_APP: join(info.binDir, exe("avash-ui")),
    BANC_SORTIE: json,
    BANC_REFERENCE: info.ref,
    BANC_COMMIT: info.commit,
    BANC_SPECS: SPECS[o.jeu].specs.join(","),
    BANC_PALIERS: SPECS[o.jeu].paliers,
    BANC_SFTP_BIN: join(info.binDir, exe("banc_sftp")),
    ...extra,
  };
  const args = ["wdio", "run", "wdio.banc.conf.js"];
  const r = WINDOWS
    ? spawnSync("npx", args, { cwd: join(RACINE, "e2e"), env, stdio: "inherit", shell: true })
    : spawnSync("xvfb-run", ["-a", "-s", "-screen 0 1440x900x24", "npx", ...args], { cwd: join(RACINE, "e2e"), env, stdio: "inherit" });
  if (r.status !== 0) console.warn(`\n⚠ passe ${info.ref} : le harnais a rendu ${r.status} (scénarios en échec consignés dans « absents »)`);
  if (existsSync(json)) {
    const res = lireResultat(json);
    res.machine.chargeDepart ??= avant;
    res.machine.chargeFin = await charge();
    ecrireResultat(res, json);
  }
}

/** Micro-bancs Rust (jeu complet), versés dans `json`. */
async function microBancs(info, json, o) {
  const { verser } = await import("../e2e/banc/commun.js");
  const env = { ...process.env, BANC_SORTIE: json, BANC_REFERENCE: info.ref, BANC_COMMIT: info.commit };
  const arbre = info.arbre;
  const sorties = [];
  const cible = { ...process.env, CARGO_TARGET_DIR: join(o.cache, "cible") };
  const cibleSidecar = { ...process.env, CARGO_TARGET_DIR: join(o.cache, "cible-sidecar") };
  try {
    sorties.push(capturer("cargo", ["bench", "--locked", "-p", "avash", "--bench", "config_ssh"], { cwd: arbre, env: cible }));
    sorties.push(capturer("cargo", ["bench", "--locked", "-p", "avash-ui", "--bench", "utf8"], { cwd: arbre, env: cible }));
    sorties.push(capturer("cargo", ["test", "--locked", "--release", "-p", "ironrdp-graphics", "--", "--ignored", "--nocapture", "vitesse"], { cwd: join(arbre, "rdp-sidecar"), env: cibleSidecar }));
  } catch (e) {
    console.warn(`\n⚠ micro-bancs ${info.ref} : ${e.message.split("\n")[0]}`);
  }
  const ancien = process.env.BANC_SORTIE;
  Object.assign(process.env, env);
  for (const l of lignesBanc(sorties.join("\n"))) verser(l.cle, { unite: l.unite, sens: l.sens }, l.echantillons);
  if (ancien === undefined) delete process.env.BANC_SORTIE; else process.env.BANC_SORTIE = ancien;
}

async function principal() {
  const o = options(process.argv.slice(2));
  o.cache ??= WINDOWS ? join(process.env.LOCALAPPDATA ?? homedir(), "avash-banc") : join(homedir(), ".cache", "avash-banc");
  o.sortie ??= join(RACINE, "e2e", "banc", "resultats", new Date().toISOString().replace(/[:.]/g, "-"));
  mkdirSync(o.sortie, { recursive: true });

  const depart = await charge();
  if (chargeExcessive(depart) && !o.forcer) {
    console.error(`machine chargée (${depart.valeur} ${depart.type}) : les chiffres seraient faux. Attendre, ou --forcer.`);
    return 1;
  }

  serveursDeTest();
  const refs = { tete: construire(o.tete, o) };
  if (o.base) refs.base = construire(o.base, o);
  const json = (role) => join(o.sortie, `${role}.json`);

  for (const [i, ordre] of ordreAlterne(o.repetitions, !!o.base).entries()) {
    for (const role of ordre) {
      console.log(`\n══ répétition ${i + 1}/${o.repetitions} : ${role} (${refs[role].ref}) ══`);
      await passe(refs[role], json(role), o);
    }
  }
  if (o.jeu === "complet") {
    for (const role of Object.keys(refs)) await microBancs(refs[role], json(role), o);
    if (o.latenceMs && !WINDOWS) {
      const delai = `${o.latenceMs / 2}ms`;
      lancer("sudo", ["tc", "qdisc", "add", "dev", "lo", "root", "netem", "delay", delai]);
      try {
        for (const role of Object.keys(refs)) {
          await passe(refs[role], json(role), o, { BANC_SPECS: "./banc/sftp.spec.js", BANC_LATENCE_MS: String(o.latenceMs) });
        }
      } finally {
        lancer("sudo", ["tc", "qdisc", "del", "dev", "lo", "root"]);
      }
    }
  }

  if (!o.garder) {
    for (const r of Object.values(refs)) {
      if (existsSync(r.arbre)) spawnSync("git", ["worktree", "remove", "--force", r.arbre], { cwd: RACINE, stdio: "inherit" });
    }
  }

  if (!o.base) {
    const { lireResultat, stats } = await import("../e2e/banc/commun.js");
    const r = lireResultat(json("tete"));
    console.log(`\nLigne de base ${r.reference} (${r.commit.slice(0, 9)}), ${r.plateforme} :`);
    for (const [cle, m] of Object.entries(r.metriques)) {
      const s = stats(m.echantillons);
      console.log(`  ${cle.padEnd(34)} ${s ? `médiane ${s.mediane.toFixed(1)} ${m.unite}, p95 ${s.p95.toFixed(1)}, n=${s.n}` : "aucun échantillon"}`);
    }
    if (r.absents.length) console.log(`  non mesurés : ${r.absents.join(" ; ")}`);
    console.log(`\nRésultat : ${json("tete")}`);
    // Une référence seule se juge sur les plafonds absolus (fuite de mémoire
    // en endurance) : rien à comparer, mais une pente trop forte échoue.
    const { depassementsAbsolus } = await import("../e2e/banc/comparer.js");
    const seuils = JSON.parse(readFileSync(join(RACINE, "e2e", "banc", "seuils.json"), "utf8"));
    const d = depassementsAbsolus(r, seuils);
    for (const x of d) console.error(`✗ plafond franchi : ${x}`);
    return d.length && !o.informatif ? 1 : 0;
  }
  const args = [join(RACINE, "e2e", "banc", "comparer.js"), json("base"), json("tete"), "--markdown", join(o.sortie, "comparaison.md")];
  if (o.informatif) args.push("--informatif");
  const r = spawnSync(process.execPath, args, { stdio: "inherit" });
  console.log(`\nComparaison : ${join(o.sortie, "comparaison.md")}`);
  return r.status ?? 1;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  principal().then((code) => { process.exitCode = code; }, (e) => { console.error(e.message); process.exitCode = 1; });
}

