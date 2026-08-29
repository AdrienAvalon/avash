import { describe, it, expect } from "vitest";
import { humanSize, matchHost, filterHosts, remoteJoin, type Host } from "./filters";

const host = (o: Partial<Host>): Host => ({
  alias: "srv", hostname: null, user: null, port: null,
  identity_file: null, proxy_jump: null, tags: [], ...o,
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
