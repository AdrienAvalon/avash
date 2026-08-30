import { describe, it, expect } from "vitest";
import { humanSize, matchHost, filterHosts, remoteJoin, type Host } from "./filters";

const host = (o: Partial<Host>): Host => ({
  alias: "srv", hostname: null, user: null, port: null,
  identity_file: null, proxy_jump: null, tags: [], folder: "", ...o,
});

describe("humanSize", () => {
  it("affiche les octets bruts sous 1 Kio", () => {
    expect(humanSize(0)).toBe("0 o");
    expect(humanSize(1023)).toBe("1023 o");
  });

  it("bascule sur l'unite superieure a 1024", () => {
    expect(humanSize(1024)).toBe("1.0 Ko");
    expect(humanSize(1536)).toBe("1.5 Ko");
    expect(humanSize(1024 ** 2)).toBe("1.0 Mo");
    expect(humanSize(1024 ** 3)).toBe("1.0 Go");
    expect(humanSize(1024 ** 4)).toBe("1.0 To");
  });

  it("ne deborde pas au-dela du dernier prefixe", () => {
    // Sans le garde-fou, l'index sortait du tableau et affichait "undefined".
    expect(humanSize(1024 ** 6)).not.toContain("undefined");
  });

  it("refuse les valeurs absurdes plutot que d'afficher NaN", () => {
    expect(humanSize(-1)).toBe("—");
    expect(humanSize(NaN)).toBe("—");
    expect(humanSize(Infinity)).toBe("—");
  });
});

describe("matchHost", () => {
  const h = host({ alias: "prod-web", hostname: "10.0.0.42", user: "deploy" });

  it("accepte tout quand la recherche est vide", () => {
    expect(matchHost(h, "")).toBe(true);
    expect(matchHost(h, "   ")).toBe(true);
  });

  it("trouve par alias, sans tenir compte de la casse", () => {
    expect(matchHost(h, "PROD")).toBe(true);
    expect(matchHost(h, "web")).toBe(true);
  });

  it("trouve aussi par adresse — regression : la palette ne cherchait que l'alias", () => {
    expect(matchHost(h, "10.0.0")).toBe(true);
  });

  it("trouve par utilisateur", () => {
    expect(matchHost(h, "deploy")).toBe(true);
  });

  it("ne trouve rien qui ne corresponde pas", () => {
    expect(matchHost(h, "staging")).toBe(false);
  });

  it("supporte un hostname absent sans planter", () => {
    expect(matchHost(host({ alias: "local" }), "local")).toBe(true);
    expect(matchHost(host({ alias: "local" }), "10.0")).toBe(false);
  });
});

describe("filterHosts", () => {
  const hosts = [
    host({ alias: "prod", hostname: "10.0.0.1" }),
    host({ alias: "staging", hostname: "10.0.0.2" }),
    host({ alias: "backup", hostname: "192.168.1.9" }),
  ];

  it("conserve l'ordre d'origine", () => {
    expect(filterHosts(hosts, "10.0.0").map((h) => h.alias)).toEqual(["prod", "staging"]);
  });

  it("rend la liste complete sans filtre", () => {
    expect(filterHosts(hosts, "")).toHaveLength(3);
  });
});

describe("remoteJoin", () => {
  it("ne double pas le separateur a la racine", () => {
    expect(remoteJoin("/", "rapport.md")).toBe("/rapport.md");
  });

  it("assemble un sous-repertoire", () => {
    expect(remoteJoin("/srv/data", "rapport.md")).toBe("/srv/data/rapport.md");
  });

  it("tolere un chemin deja termine par un slash", () => {
    // Cas que les trois copies inline de cette logique ne geraient pas.
    expect(remoteJoin("/srv/data/", "rapport.md")).toBe("/srv/data/rapport.md");
  });
});

import { parentDir, isPasswordRequired, stripHtml } from "./filters";

describe("parentDir", () => {
  it("remonte d'un niveau", () => {
    expect(parentDir("/srv/data")).toBe("/srv");
    expect(parentDir("/srv/data/logs")).toBe("/srv/data");
  });
  it("s'arrête à la racine", () => {
    expect(parentDir("/srv")).toBe("/");
    expect(parentDir("/")).toBe("/");
    expect(parentDir("")).toBe("/");
  });
  it("tolère un slash final", () => {
    expect(parentDir("/srv/data/")).toBe("/srv");
    expect(parentDir("/srv/")).toBe("/");
  });
  it("ne remonte jamais au-dessus de la racine", () => {
    // Cas piège : quel que soit le chemin, on reste dans l'arborescence.
    for (const p of ["/", "//", "/a", "/a/", "a"]) {
      const r = parentDir(p);
      expect(r.startsWith("/")).toBe(true);
      expect(r).not.toContain("..");
    }
  });
});

describe("isPasswordRequired", () => {
  it("reconnaît le marqueur du backend", () => {
    expect(isPasswordRequired("[AVASH_PASSWORD_REQUIRED] blabla")).toBe(true);
  });
  it("ignore les autres erreurs", () => {
    expect(isPasswordRequired("Connection refused")).toBe(false);
    expect(isPasswordRequired("host key changed")).toBe(false);
  });
});

describe("stripHtml", () => {
  it("retire les caractères d'injection", () => {
    expect(stripHtml("<img onerror=x>")).toBe("img onerror=x");
    expect(stripHtml("a & b < c > d")).toBe("a  b  c  d");
  });
  it("laisse le texte normal intact", () => {
    expect(stripHtml("prod-web 10.0.0.1")).toBe("prod-web 10.0.0.1");
  });
  it("neutralise une tentative de script", () => {
    const injection = "<script>window.invoke('rm')</script>";
    const propre = stripHtml(injection);
    expect(propre).not.toContain("<");
    expect(propre).not.toContain(">");
  });
});

import {
  describeTunnel, tunnelFlag, tunnelTraffic, activeTunnelsByHost,
  type TunnelDef, type TunnelStatus,
} from "./filters";

const tdef = (o: Partial<TunnelDef>): TunnelDef => ({
  id: "t-1", alias: "prod", kind: "local", bind_port: 8080,
  target_host: "db.interne", target_port: 5432, name: "", ...o,
});
const tstatus = (o: Partial<TunnelStatus>): TunnelStatus => ({
  id: "t-1", bound_port: 8080, active: 0, total: 0, bytes_up: 0, bytes_down: 0,
  alive: true, last_error: null, ...o,
});

describe("describeTunnel", () => {
  it("suit le sens du trafic pour les trois types", () => {
    expect(describeTunnel(tdef({}))).toBe("localhost:8080 → prod → db.interne:5432");
    expect(describeTunnel(tdef({ kind: "remote" }))).toBe("prod:8080 → localhost → db.interne:5432");
    expect(describeTunnel(tdef({ kind: "dynamic" }))).toBe("SOCKS5 localhost:8080 → prod");
  });
  it("donne la lettre ssh", () => {
    expect(tunnelFlag("local")).toBe("-L");
    expect(tunnelFlag("remote")).toBe("-R");
    expect(tunnelFlag("dynamic")).toBe("-D");
  });
});

describe("tunnelTraffic", () => {
  it("montre les connexions en cours quand il y en a, sinon le cumul", () => {
    expect(tunnelTraffic(tstatus({ active: 2, total: 5, bytes_up: 1024, bytes_down: 3481 })))
      .toBe("2 conn · ↑1.0 Ko ↓3.4 Ko");
    expect(tunnelTraffic(tstatus({ total: 5 }))).toBe("5 au total · ↑0 o ↓0 o");
  });
});

describe("activeTunnelsByHost", () => {
  it("ne compte que les tunnels vivants", () => {
    const defs = [tdef({ id: "a" }), tdef({ id: "b" }), tdef({ id: "c", alias: "dev" })];
    const status = new Map([
      ["a", tstatus({ id: "a" })],
      ["b", tstatus({ id: "b", alive: false })],
    ]);
    const m = activeTunnelsByHost(defs, status);
    expect(m.get("prod")).toBe(1);
    expect(m.has("dev")).toBe(false);
  });
});

import { hostInitials, hostHue } from "./filters";

describe("hostInitials", () => {
  it("prend une lettre par mot quand le nom en a plusieurs", () => {
    expect(hostInitials("prod-web")).toBe("PW");
    expect(hostInitials("nas_maison")).toBe("NM");
  });
  it("sinon les deux premiers caracteres", () => {
    expect(hostInitials("192.168.2.40")).toBe("19");
    expect(hostInitials("ava")).toBe("AV");
    expect(hostInitials("")).toBe("?");
  });
});

describe("hostHue", () => {
  it("est stable et differe entre deux noms", () => {
    expect(hostHue("prod")).toBe(hostHue("prod"));
    expect(hostHue("prod")).not.toBe(hostHue("dev"));
    expect(hostHue("x")).toMatch(/^hsl\(\d+ 60% 62%\)$/);
  });
});

import { osBadge } from "./filters";

describe("osBadge", () => {
  it("connait les distributions courantes", () => {
    expect(osBadge({ id: "debian", like: [], pretty: "" }).glyph).toBe("");
    expect(osBadge({ id: "ubuntu", like: ["debian"], pretty: "" }).glyph).toBe("");
  });
  it("retombe sur la famille pour une derivee inconnue", () => {
    expect(osBadge({ id: "cachyos", like: ["arch"], pretty: "" }).glyph).toBe("");
    expect(osBadge({ id: "inconnue", like: ["rhel", "fedora"], pretty: "" }).glyph).toBe("");
  });
  it("sinon Tux", () => {
    expect(osBadge({ id: "mystere", like: [], pretty: "" }).glyph).toBe("");
  });
});


import { shortDate, shellQuote, validFileName } from "./filters";
import { fileIconName } from "./icons";

describe("fileIconName", () => {
  it("distingue dossier, image, archive, code, cle", () => {
    expect(fileIconName("x", true)).toBe("folder");
    expect(fileIconName("photo.JPG", false)).toBe("image");
    expect(fileIconName("site.tar.gz", false)).toBe("fileArchive");
    expect(fileIconName("deploy.sh", false)).toBe("fileCode");
    expect(fileIconName("id_ed25519", false)).toBe("key");
    expect(fileIconName("notes", false)).toBe("file");
  });
});

describe("shortDate", () => {
  it("heure le jour meme, date sinon, rien sans valeur", () => {
    const now = new Date(2026, 7, 29, 15, 0);
    const today = new Date(2026, 7, 29, 14, 7).getTime() / 1000;
    expect(shortDate(today, now)).toBe("14:07");
    const old = new Date(2026, 2, 12, 9, 0).getTime() / 1000;
    expect(shortDate(old, now)).toBe("12/03/26");
    expect(shortDate(null, now)).toBe("");
  });
});

describe("shellQuote", () => {
  it("protege espaces et apostrophes", () => {
    expect(shellQuote("/srv/mon dossier")).toBe("'/srv/mon dossier'");
    expect(shellQuote("/srv/l'ami")).toBe("'/srv/l'\\''ami'");
  });
});

describe("validFileName", () => {
  it("refuse vide, ., .. et les slashs", () => {
    expect(validFileName("ok.txt")).toBe(true);
    for (const bad of ["", ".", "..", "a/b", "../x"]) expect(validFileName(bad)).toBe(false);
  });
});

import { snippetPreview, snippetVars, renderSnippet } from "./filters";

describe("snippetVars / renderSnippet", () => {
  it("liste les variables sans doublon et dans l'ordre", () => {
    expect(snippetVars("systemctl {{action}} {{svc}} && journalctl -u {{svc}}"))
      .toEqual(["action", "svc"]);
    expect(snippetVars("echo {{ hote }}")).toEqual(["hote"]);
    expect(snippetVars("rien ici")).toEqual([]);
  });
  it("substitue, l'inconnue devient vide", () => {
    expect(renderSnippet("ssh {{user}}@{{host}}", { user: "root", host: "srv" })).toBe("ssh root@srv");
    expect(renderSnippet("a {{x}} b", {})).toBe("a  b");
  });
});

describe("snippetPreview", () => {
  it("aplati les sauts de ligne et tronque", () => {
    expect(snippetPreview("cd /x\ngit pull")).toBe("cd /x ⏎ git pull");
    expect(snippetPreview("x".repeat(80)).endsWith("…")).toBe(true);
  });
});

import { allTags } from "./filters";

describe("tags", () => {
  const th = (alias: string, tags: string[]): Host => host({ alias, tags });
  it("allTags dédupliqué et trié", () => {
    expect(allTags([th("a", ["prod", "web"]), th("b", ["web", "db"])])).toEqual(["db", "prod", "web"]);
  });
  it("filterHosts filtre par tag", () => {
    const hosts = [th("a", ["prod"]), th("b", ["dev"]), th("c", ["prod", "web"])];
    expect(filterHosts(hosts, "", "prod").map((h) => h.alias)).toEqual(["a", "c"]);
    expect(filterHosts(hosts, "", null).length).toBe(3);
  });
  it("matchHost trouve par tag via la recherche", () => {
    expect(matchHost(th("srv", ["staging"]), "stag")).toBe(true);
  });
});

import { sortSftpEntries, type SftpEntry } from "./filters";

describe("sortSftpEntries", () => {
  const e = (name: string, is_dir: boolean): SftpEntry => ({ name, is_dir, size: 0, modified: null });
  it("dossiers d'abord, puis alphabétique", () => {
    const got = sortSftpEntries([e("b.txt", false), e("zeta", true), e("a.txt", false), e("alpha", true)]);
    expect(got.map((x) => x.name)).toEqual(["alpha", "zeta", "a.txt", "b.txt"]);
  });
  it("ne mute pas l'entrée", () => {
    const src = [e("b", false), e("a", true)];
    sortSftpEntries(src);
    expect(src.map((x) => x.name)).toEqual(["b", "a"]);
  });
});

import { buildFolderTree, folderNodeCount, ensureFolderNode, rdpScancode, le16, rdpMousePos , choisirVerrous } from "./filters";

describe("buildFolderTree", () => {
  it("range les éléments à la racine et dans des dossiers imbriqués", () => {
    const root = buildFolderTree<string>([], [
      { folder: "", item: "racine" },
      { folder: "prod", item: "p1" },
      { folder: "prod/web", item: "w1" },
      { folder: "prod/web", item: "w2" },
    ]);
    expect(root.items).toEqual(["racine"]);
    const prod = root.children.get("prod")!;
    expect(prod.path).toBe("prod");
    expect(prod.items).toEqual(["p1"]);
    expect(prod.children.get("web")!.items).toEqual(["w1", "w2"]);
  });

  it("crée les dossiers vides du registre, et ceux dérivés des éléments", () => {
    const root = buildFolderTree<string>(["a/b/c"], [{ folder: "x", item: "h" }]);
    // Dossier vide du registre, avec toute sa chaîne d'ancêtres.
    expect(root.children.get("a")!.children.get("b")!.children.get("c")).toBeTruthy();
    // Dossier dérivé d'un élément, même absent du registre.
    expect(root.children.get("x")!.items).toEqual(["h"]);
  });

  it("un dossier vide ('') équivaut à la racine", () => {
    const root = buildFolderTree<number>([], [{ folder: "", item: 1 }, { folder: "", item: 2 }]);
    expect(root.items).toEqual([1, 2]);
    expect(root.children.size).toBe(0);
  });
});

describe("folderNodeCount", () => {
  it("compte récursivement les éléments d'un nœud et de ses descendants", () => {
    const root = buildFolderTree<string>([], [
      { folder: "prod", item: "a" },
      { folder: "prod/web", item: "b" },
      { folder: "prod/web", item: "c" },
      { folder: "autre", item: "d" },
    ]);
    expect(folderNodeCount(root)).toBe(4);
    expect(folderNodeCount(root.children.get("prod")!)).toBe(3);
    expect(folderNodeCount(root.children.get("prod")!.children.get("web")!)).toBe(2);
  });
});

describe("ensureFolderNode", () => {
  it("est idempotent : deux appels renvoient le même nœud", () => {
    const root = buildFolderTree<number>([], []);
    const a = ensureFolderNode(root, "x/y");
    const b = ensureFolderNode(root, "x/y");
    expect(a).toBe(b);
    expect(root.children.get("x")!.children.get("y")).toBe(a);
  });
});

describe("rdpScancode", () => {
  it("mappe les touches usuelles et renvoie null pour l'inconnu", () => {
    expect(rdpScancode("Enter")).toBe(0x1c);
    expect(rdpScancode("KeyV")).toBe(0x2f);
    expect(rdpScancode("ControlLeft")).toBe(0x1d);
    expect(rdpScancode("Escape")).toBe(0x01);
    expect(rdpScancode("F13")).toBeNull();
    expect(rdpScancode("")).toBeNull();
  });
});

describe("le16", () => {
  it("encode un u16 en petit-boutiste", () => {
    expect(le16(0)).toEqual([0, 0]);
    expect(le16(1)).toEqual([1, 0]);
    expect(le16(256)).toEqual([0, 1]);
    expect(le16(0x1234)).toEqual([0x34, 0x12]);
    expect(le16(65535)).toEqual([0xff, 0xff]);
  });
});

describe("rdpMousePos", () => {
  const rect = { left: 0, top: 0, width: 800, height: 600 };

  it("mappe 1:1 quand le canvas remplit exactement (même ratio)", () => {
    // 800x600 affiché dans 800x600 : pas de letterbox.
    expect(rdpMousePos(0, 0, rect, 800, 600)).toEqual([0, 0]);
    expect(rdpMousePos(400, 300, rect, 800, 600)).toEqual([400, 300]);
    expect(rdpMousePos(799, 599, rect, 800, 600)).toEqual([799, 599]);
  });

  it("tient compte des bandes (letterbox) quand les ratios diffèrent", () => {
    // Bureau 400x600 (portrait) dans une zone 800x600 : bandes à gauche/droite.
    // scale = min(800/400, 600/600) = 1 ; image 400 de large, offset X = 200.
    expect(rdpMousePos(200, 0, rect, 400, 600)).toEqual([0, 0]); // bord gauche de l'image
    expect(rdpMousePos(400, 300, rect, 400, 600)).toEqual([200, 300]); // centre
  });

  it("borne au bureau (jamais hors [0, w-1] x [0, h-1])", () => {
    expect(rdpMousePos(-50, -50, rect, 800, 600)).toEqual([0, 0]);
    expect(rdpMousePos(9999, 9999, rect, 800, 600)).toEqual([799, 599]);
  });
});

describe("rdpScancode — clavier complet", () => {
  it("le pavé numérique est transmis", () => {
    // Sans ces codes, le pavé numérique est inerte dans le bureau distant.
    expect(rdpScancode("Numpad0")).toBe(0x52);
    expect(rdpScancode("Numpad5")).toBe(0x4c);
    expect(rdpScancode("Numpad9")).toBe(0x49);
    expect(rdpScancode("NumpadAdd")).toBe(0x4e);
    expect(rdpScancode("NumpadSubtract")).toBe(0x4a);
    expect(rdpScancode("NumpadMultiply")).toBe(0x37);
    expect(rdpScancode("NumpadDecimal")).toBe(0x53);
    expect(rdpScancode("NumLock")).toBe(0x45);
  });

  it("AltGr est transmis (touche étendue)", () => {
    // Sur un clavier français, l'antislash s'obtient par AltGr+8 : sans AltGr,
    // le caractère est impossible à saisir dans le bureau distant.
    expect(rdpScancode("AltRight")).toBe(0xe038);
  });

  it("les touches étendues portent le préfixe 0xE0", () => {
    expect(rdpScancode("ArrowUp")).toBe(0xe048);
    expect(rdpScancode("ArrowDown")).toBe(0xe050);
    expect(rdpScancode("ArrowLeft")).toBe(0xe04b);
    expect(rdpScancode("ArrowRight")).toBe(0xe04d);
    expect(rdpScancode("Delete")).toBe(0xe053);
    expect(rdpScancode("Home")).toBe(0xe047);
    expect(rdpScancode("NumpadEnter")).toBe(0xe01c);
    expect(rdpScancode("NumpadDivide")).toBe(0xe035);
    expect(rdpScancode("ControlRight")).toBe(0xe01d);
  });

  it("les touches de fonction sont transmises", () => {
    expect(rdpScancode("F1")).toBe(0x3b);
    expect(rdpScancode("F10")).toBe(0x44);
    expect(rdpScancode("F11")).toBe(0x57);
    expect(rdpScancode("F12")).toBe(0x58);
  });

  it("la touche à gauche de Maj des claviers européens est transmise", () => {
    // Sur AZERTY, c'est « < / > » : absente de la table, elle ne passait pas.
    expect(rdpScancode("IntlBackslash")).toBe(0x56);
  });

  it("une touche inconnue ne produit rien", () => {
    expect(rdpScancode("ToucheImaginaire")).toBeNull();
  });
});

describe("choisirVerrous — priorité des sources", () => {
  it("le système prime sur les événements clavier", () => {
    // WebKitGTK rapporte NumLock éteint alors qu'il est allumé : laisser les
    // événements primer éteignait le pavé numérique du bureau distant.
    expect(choisirVerrous(1, 0)).toBe(1);
    expect(choisirVerrous(0, 1)).toBe(0);
  });

  it("les événements servent de secours quand le système ne sait pas", () => {
    expect(choisirVerrous(null, 5)).toBe(5);
  });

  it("le système peut légitimement répondre « tout éteint »", () => {
    // 0 est une réponse, pas une absence de réponse.
    expect(choisirVerrous(0, 7)).toBe(0);
  });

  it("aucune source : on n'impose rien au distant", () => {
    expect(choisirVerrous(null, null)).toBeNull();
  });
});

// ── Partage du presse-papiers avec les bureaux RDP ────────────────────────────
// Le presse-papiers local était transmis à tout bureau RDP ouvert sans le
// moindre geste, y compris au simple retour sur la fenêtre. Le réglage doit
// être révocable et retenu ; par défaut il reste actif (parité avec les autres
// clients RDP), mais un « 0 » enregistré doit couper la transmission.
describe("partage du presse-papiers RDP", () => {
  // Reproduit la lecture faite dans main.ts, qui n'est pas exportable telle
  // quelle (elle vit dans le module d'entrée, chargé avec le DOM de l'app).
  const partage = (brut: string | null) => brut !== "0";

  it("est actif par défaut, avant tout réglage", () => {
    expect(partage(null)).toBe(true);
  });
  it("reste actif si le réglage vaut 1", () => {
    expect(partage("1")).toBe(true);
  });
  it("est coupé par un réglage à 0", () => {
    expect(partage("0")).toBe(false);
  });
});
