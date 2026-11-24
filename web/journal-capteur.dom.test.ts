// @vitest-environment jsdom
// Journal des commandes (demandé le 24 novembre 2026) : le capteur lit la
// ligne à l'écran sur un vrai xterm, sans l'afficher. Le test joue les deux
// côtés : `taper` est ce que l'utilisateur envoie, `afficher` ce que le
// serveur renvoie (écho, invite, sortie).
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { Terminal } from "@xterm/xterm";
import { CapteurCommandes, estReponseTerminal, tapeSimple } from "./journal-capteur";

let term: Terminal;
let capteur: CapteurCommandes;
let notees: string[];
let codes: number[];
let actif: boolean;

const afficher = (texte: string) => new Promise<void>((r) => term.write(texte, r));
const taper = (data: string) => capteur.frappe(data);
/** Une frappe que le serveur renvoie en écho, comme le fait un shell. */
async function taperAvecEcho(texte: string) {
  for (const c of texte) {
    taper(c);
    await afficher(c);
  }
}
const OSC = (corps: string) => `\x1b]133;${corps}\x07`;

function monter(cols = 80) {
  term = new Terminal({ cols, rows: 24, allowProposedApi: true });
  notees = [];
  codes = [];
  actif = true;
  capteur = new CapteurCommandes(term, (c) => notees.push(c), (c) => codes.push(c), () => actif);
}

beforeEach(() => monter());
afterEach(() => {
  capteur.dispose();
  term.dispose();
});

describe("journal des commandes : ce qui est noté", () => {
  it("note la commande telle qu'elle est à l'écran", async () => {
    await afficher("ada@web:~$ ");
    await taperAvecEcho("ls -la /etc");
    taper("\r");
    await afficher("\r\ntotal 0\r\nada@web:~$ ");
    expect(notees).toEqual(["ls -la /etc"]);
  });

  it("ne note jamais un mot de passe tapé à l'invite de sudo", async () => {
    // La garde promise au mainteneur : le serveur n'affiche pas le mot de
    // passe, il ne peut donc pas entrer dans le journal.
    await afficher("ada@web:~$ ");
    await taperAvecEcho("sudo systemctl restart nginx");
    taper("\r");
    await afficher("\r\n[sudo] Mot de passe de ada : ");
    for (const c of "hunter2") taper(c); // pas d'écho
    taper("\r");
    await afficher("\r\nada@web:~$ ");
    expect(notees).toEqual(["sudo systemctl restart nginx"]);
    expect(notees.join("\n")).not.toContain("hunter2");
  });

  it("note la commande rappelée de l'historique, pas la flèche", async () => {
    await afficher("$ ");
    taper("\x1b[A");
    await afficher("git status"); // le shell redessine la commande rappelée
    taper("\r");
    await afficher("\r\nSur la branche main\r\n$ ");
    expect(notees).toEqual(["git status"]);
  });

  it("note la commande complétée par Tab", async () => {
    await afficher("$ ");
    await taperAvecEcho("cd /et");
    taper("\t");
    await afficher("c/");
    taper("\r");
    await afficher("\r\n$ ");
    expect(notees).toEqual(["cd /etc/"]);
  });

  it("retrouve la commande quand la complétion a listé ses candidats", async () => {
    await afficher("$ ");
    await taperAvecEcho("ls /e");
    taper("\t");
    taper("\t");
    await afficher("\r\netc/  ext/\r\n$ ls /e"); // candidats, puis invite redessinée
    await taperAvecEcho("tc");
    taper("\r");
    await afficher("\r\n$ ");
    expect(notees).toEqual(["ls /etc"]);
  });

  it("attend l'écho sur une liaison lente", async () => {
    await afficher("$ ");
    for (const c of "uptime") taper(c);
    taper("\r"); // Entrée part avant le moindre écho
    expect(notees).toEqual([]);
    await afficher("upt");
    expect(notees).toEqual([]);
    await afficher("ime\r\n 10:00:00 up 3 days\r\n$ ");
    expect(notees).toEqual(["uptime"]);
  });

  it("note entière une commande plus longue que la largeur du terminal", async () => {
    capteur.dispose();
    term.dispose();
    monter(20);
    await afficher("$ ");
    const longue = "echo un-deux-trois-quatre-cinq-six";
    await taperAvecEcho(longue);
    taper("\r");
    await afficher("\r\nun-deux\r\n$ ");
    expect(notees).toEqual([longue]);
  });

  it("laisse de côté l'invite affichée à droite", async () => {
    capteur.dispose();
    term.dispose();
    monter(40);
    await afficher("$ \x1b[35G[main]\x1b[3G"); // RPROMPT de zsh
    await taperAvecEcho("make test");
    taper("\r");
    await afficher("\r\n$ ");
    expect(notees).toEqual(["make test"]);
  });

  it("enchaîne les commandes, chacune à sa ligne", async () => {
    await afficher("$ ");
    await taperAvecEcho("cd /srv");
    taper("\r");
    await afficher("\r\n$ ");
    await taperAvecEcho("ls");
    taper("\r");
    await afficher("\r\napp\r\n$ ");
    expect(notees).toEqual(["cd /srv", "ls"]);
  });
});

describe("journal des commandes : ce qui n'est pas noté", () => {
  it("rien quand le journal est coupé pour l'onglet", async () => {
    actif = false;
    await afficher("$ ");
    await taperAvecEcho("rm -rf build");
    taper("\r");
    await afficher("\r\n$ ");
    expect(notees).toEqual([]);
  });

  it("rien pour une ligne qui commence par une espace", async () => {
    await afficher("$ ");
    await taperAvecEcho(" export SECRET=x");
    taper("\r");
    await afficher("\r\n$ ");
    expect(notees).toEqual([]);
  });

  it("rien pour une ligne vide ou abandonnée par Ctrl+C", async () => {
    await afficher("$ ");
    taper("\r");
    await afficher("\r\n$ ");
    await taperAvecEcho("reboot");
    taper("\x03");
    await afficher("^C\r\n$ ");
    expect(notees).toEqual([]);
  });

  it("rien quand un secret lu sans écho laisse la sortie sur sa ligne", async () => {
    // `stty -echo; read x` : rien ne s'affiche pendant la frappe, et la
    // commande écrit ensuite sa sortie sur la même ligne. Sans la comparaison
    // avec les frappes, cette sortie était notée comme une commande.
    await afficher("$ ");
    await taperAvecEcho("stty -echo; read x; stty echo; echo lu");
    taper("\r");
    await afficher("\r\n");
    for (const c of "hunter2") taper(c);
    taper("\r");
    await afficher("lu\r\n$ ");
    expect(notees).toEqual(["stty -echo; read x; stty echo; echo lu"]);
  });

  it("retrouve la commande quand le curseur n'était pas au bout de l'invite", async () => {
    // ConPTY (sessions SSH vers Windows) redessine par positionnements : le
    // curseur peut être en colonne 1 à la première frappe, et l'écho arriver
    // par un positionnement absolu (suite bout en bout Windows, PR #113).
    await afficher("$ \x1b[1G");
    taper("echo a");
    await afficher("\x1b[3Gecho a");
    taper("\r");
    await afficher("\r\na\r\n$ ");
    expect(notees).toEqual(["echo a"]);
  });

  it("note une commande tapée avant que l'invite ne s'affiche", async () => {
    // Suite bout en bout Windows, PR #113 : la commande part avant que ConPTY
    // n'ait dessiné l'écran ; il l'efface (ED 2), redessine l'invite en haut
    // et l'écho de la commande dessous. Les repères posés à la frappe
    // pointaient sur une ligne vide : rien n'était noté. Flux relevé tel quel.
    taper("echo journal-ok-1");
    taper("\r");
    await afficher("\u001b[?9001h\u001b[?1004h\u001b[?25l\u001b[2J\u001b[m\u001b[H" + "\r\n".repeat(28)
      + "\u001b[H\u001b]0;C:\\Windows\\system32\\conhost.exe\u0007\u001b[?25h\u001b[?2004h\u001b[32m\r\n"
      + "runneradmin@runnervmbpeip \u001b[35mUCRT64 \u001b[33m~\u001b[m\r\n$ \u001b]0;UCRT64:/c/Users/runneradmin\u0007"
      + "echo journal-ok-1\u001b[?2004l\r\njournal-ok-1\r\n\u001b[?2004h\u001b[32m\r\n"
      + "runneradmin@runnervmbpeip \u001b[35mUCRT64 \u001b[33m~\u001b[m\r\n$ ");
    expect(notees).toEqual(["echo journal-ok-1"]);
  });

  it("une correction à l'effacement arrière reste une frappe ordinaire", async () => {
    await afficher("$ ");
    await taperAvecEcho("lss");
    taper("\x7f");
    await afficher("\b \b");
    await taperAvecEcho(" -l");
    taper("\r");
    await afficher("\r\n$ ");
    expect(notees).toEqual(["ls -l"]);
  });

  it("rien de ce qui est tapé en plein écran", async () => {
    await afficher("$ ");
    await taperAvecEcho("vim notes");
    taper("\r");
    await afficher("\r\n\x1b[?1049h"); // vim passe au tampon alternatif
    await taperAvecEcho("iBonjour");
    taper("\x1b");
    await taperAvecEcho(":wq");
    taper("\r");
    await afficher("\x1b[?1049l$ ");
    expect(notees).toEqual(["vim notes"]);
  });

  it("ne prend pas une réponse du terminal pour une frappe", () => {
    for (const r of ["\x1b[12;40R", "\x1b[?1;2c", "\x1b[0n", "\x1b[I", "\x1b[O", "\x1b[<0;10;5M", "\x1b]11;rgb:0000/0000/0000\x07"]) {
      expect(estReponseTerminal(r), JSON.stringify(r)).toBe(true);
    }
    for (const f of ["a", "\x1b[A", "\x1b[1;5C", "\r", "\t", "\x7f"]) {
      expect(estReponseTerminal(f), JSON.stringify(f)).toBe(false);
    }
  });
});

describe("journal des commandes : marqueurs OSC 133", () => {
  it("note à C, une seule fois, et rattache le code de D", async () => {
    await afficher(`${OSC("A")}ada@web:~$ ${OSC("B")}`);
    await taperAvecEcho("make deploy");
    taper("\r");
    await afficher(`\r\n${OSC("C")}Confirmer ? `);
    // Ce qu'on répond au programme qui tourne ne va pas au shell.
    await taperAvecEcho("oui");
    taper("\r");
    await afficher(`\r\nerreur\r\n${OSC("D;2")}${OSC("A")}ada@web:~$ ${OSC("B")}`);
    expect(notees).toEqual(["make deploy"]);
    expect(codes).toEqual([2]);
  });

  it("un code sans commande notée n'est pas transmis", async () => {
    await afficher(`${OSC("A")}$ ${OSC("B")}`);
    taper("\r");
    await afficher(`\r\n${OSC("C")}${OSC("D;0")}${OSC("A")}$ ${OSC("B")}`);
    expect(notees).toEqual([]);
    expect(codes).toEqual([]);
  });

  it("un serveur qui envoie B puis C ne fait rien noter que personne n'a tapé", async () => {
    // Revue de sécurité automatique de la PR #113 : les marqueurs viennent du
    // serveur, qui pouvait faire noter n'importe quel texte de son écran.
    await afficher(`${OSC("A")}$ ${OSC("B")}rm -rf / --no-preserve-root\r\n${OSC("C")}`);
    await afficher(`${OSC("D;0")}${OSC("A")}$ ${OSC("B")}`);
    expect(notees).toEqual([]);
    expect(codes).toEqual([]);
  });

  it("garde ce qui a été tapé avant que l'invite ne s'affiche", async () => {
    // Suite bout en bout, PR #113 : « echo » tapé pendant que fish démarrait
    // était noté « ho » ; le marqueur B effaçait les premières frappes.
    taper("e");
    taper("c");
    await afficher(`${OSC("A")}$ ${OSC("B")}ec`);
    await taperAvecEcho("ho x");
    taper("\r");
    await afficher(`\r\n${OSC("C")}x\r\n${OSC("D;0")}`);
    expect(notees).toEqual(["echo x"]);
  });

  it("pas de mot de passe non plus avec les marqueurs", async () => {
    await afficher(`${OSC("A")}$ ${OSC("B")}`);
    await taperAvecEcho("sudo -v");
    taper("\r");
    await afficher(`\r\n${OSC("C")}[sudo] password for ada: `);
    for (const c of "hunter2") taper(c);
    taper("\r");
    await afficher(`\r\n${OSC("D;0")}${OSC("A")}$ ${OSC("B")}`);
    expect(notees).toEqual(["sudo -v"]);
    expect(codes).toEqual([0]);
  });
});

describe("journal des commandes : frappes ordinaires", () => {
  it("déduit la ligne des caractères tapés, effacements et collage compris", () => {
    expect(tapeSimple("ls -la")).toBe("ls -la");
    expect(tapeSimple("lss\x7f -l")).toBe("ls -l");
    expect(tapeSimple("\x1b[200~git pull\x1b[201~")).toBe("git pull");
    expect(tapeSimple("é\x7fe")).toBe("e");
  });

  it("renonce dès qu'une touche d'édition s'en mêle", () => {
    expect(tapeSimple("\x1b[A")).toBeNull();
    expect(tapeSimple("cd /et\t")).toBeNull();
    expect(tapeSimple("ls\x15pwd")).toBeNull(); // Ctrl+U
  });
});
