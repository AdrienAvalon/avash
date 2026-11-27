// Relevé des commandes passées dans un terminal SSH, pour le journal des
// commandes (demandé par le mainteneur le 24 novembre 2026).
//
// On ne note pas les frappes : un `↑ ↑ ← ← Suppr 2 Entrée` ne dit pas quelle
// commande est partie. On note la ligne telle que le terminal l'affiche quand
// elle part, complétion et rappel d'historique compris. Ce qui ne s'affiche
// pas n'est donc jamais noté : un mot de passe tapé à l'invite de `sudo`, que
// le serveur n'écrit pas à l'écran, n'entre pas dans le journal.
//
// Deux façons de savoir où commence la commande :
// - les marqueurs OSC 133 (intégration du shell des terminaux modernes) :
//   `B` à la fin de l'invite, `C` quand la commande part, `D;code` quand elle
//   finit. Exacts, et donnent le code de retour ; entre `C` et `D`, ce qu'on
//   tape va au programme qui tourne, pas au shell, et n'est pas noté ;
// - sans eux, la position du curseur à la première frappe : c'est la fin de
//   l'invite, la commande va de là à la fin de la ligne (et des lignes qui la
//   prolongent quand elle dépasse la largeur).
//
// La ligne est lue une fois l'écho revenu : sur une liaison lente, Entrée part
// avant que l'écho des dernières lettres ne soit affiché. On attend que le
// curseur ait quitté la ligne de la commande (le serveur renvoie le retour à
// la ligne après la commande), au plus `ATTENTE_ECHO_MS`.
//
// Rien n'est noté en plein écran (vim, less, top : tampon alternatif), ni une
// ligne qui commence par une espace (le cœur la refuse aussi, comme
// `HISTCONTROL=ignorespace`).

import type { IMarker, Terminal } from "@xterm/xterm";

/** Au-delà, la ligne est lue telle qu'elle est, écho complet ou non. */
const ATTENTE_ECHO_MS = 1500;

/** Au plus, les lignes (prolongements compris) parcourues pour retrouver une
 *  commande à l'écran : une sortie abondante ne coûte pas un parcours entier
 *  de l'historique du terminal. */
const GROUPES_MAX = 200;

/** Ce que le capteur lit du terminal : de quoi tester sur un vrai xterm sans
 *  l'afficher. */
export type TerminalLu = Pick<Terminal, "buffer" | "cols" | "registerMarker" | "parser" | "onWriteParsed">;

/** Les réponses que le terminal envoie de lui-même au serveur (position du
 *  curseur, attributs, couleurs, focus, souris) : elles passent par le même
 *  canal que les frappes mais ne sont pas tapées. */
export function estReponseTerminal(data: string): boolean {
  // Les séquences du terminal sont faites de caractères de contrôle : c'est
  // précisément ce qu'on cherche à reconnaître.
  /* eslint-disable no-control-regex */
  return /^\x1b\[\??[\d;]*[Rcn]$/.test(data) // position, attributs, état
    || /^\x1b\[[IO]$/.test(data) // focus
    || /^\x1b\[<[\d;]+[Mm]$/.test(data) || /^\x1b\[M/.test(data) // souris
    || /^\x1b[\]P][\s\S]*(\x07|\x1b\\)$/.test(data); // OSC et DCS
  /* eslint-enable no-control-regex */
}

/** Où commence une commande : un repère d'xterm, qui suit la ligne quand
 *  l'écran défile, et la ligne absolue au moment de la pose, qui sert quand le
 *  repère a disparu (un effacement d'écran, ED 2, supprime les repères des
 *  lignes effacées). */
type Depart = { marqueur: IMarker; x: number; y: number };
type EnAttente = {
  depart: Depart | null;
  finale: IMarker;
  /** La ligne absolue de `finale` à la pose. */
  yFinale: number;
  /** Ce qui a été tapé, quand on peut en déduire la ligne (voir `tapeSimple`). */
  attendu: string | null;
  limite: number;
  minuteur: ReturnType<typeof setTimeout>;
};

/** La ligne que des frappes produisent quand elles ne sont que des caractères
 *  ordinaires (effacement arrière compris, collage compris) ; `null` dès
 *  qu'une touche d'édition, une flèche ou une tabulation s'en mêle : la ligne
 *  ne se déduit plus des frappes, seul l'écran la connaît. */
export function tapeSimple(frappes: string): string | null {
  const sansCollage = frappes.replaceAll("\x1b[200~", "").replaceAll("\x1b[201~", "");
  let ligne = "";
  for (const c of sansCollage) {
    if (c === "\x7f" || c === "\b") ligne = [...ligne].slice(0, -1).join("");
    else if (c < " " || c === "\x1b") return null;
    else ligne += c;
  }
  return ligne;
}

/** Rend le repère d'un départ à xterm. Un repère vit tant que sa ligne est
 *  dans l'historique et abonne trois écouteurs à la liste des lignes : un
 *  repère oublié sans `dispose()` coûte à chaque ligne qui défile (audit du
 *  27 novembre 2026, ME-1). */
function liberer(depart: Depart | null): void {
  depart?.marqueur.dispose();
}

export class CapteurCommandes {
  private debut: Depart | null = null;
  private invite: Depart | null = null; // OSC 133;B
  private attente: EnAttente | null = null;
  /** Les frappes depuis le début de la ligne en cours. */
  private tape = "";
  /** Ce que le capteur a vu et décidé, en nombres, sans rien de ce qui a été
   *  tapé : de quoi diagnostiquer un journal qui reste vide (suite bout en
   *  bout Windows, PR #113 : commandes exécutées, rien de noté, sans erreur).
   *  `formes` garde les dernières frappes avec chaque caractère imprimable
   *  remplacé par « x » : leur forme (une Entrée en `\r`, une séquence),
   *  jamais leur contenu. */
  readonly diagnostic: { compteurs: Record<string, number>; formes: string[] } = { compteurs: {}, formes: [] };

  private compter(cle: string): void {
    this.diagnostic.compteurs[cle] = (this.diagnostic.compteurs[cle] ?? 0) + 1;
  }
  /** Le shell envoie `C` : la fin de l'invite et le départ sont exacts. */
  private oscComplet = false;
  /** Entre `C` et `D` : une commande tourne, ce qu'on tape ne va pas au shell. */
  private enCommande = false;
  /** Une commande a été notée et son code n'est pas encore venu. */
  private codeAttendu = false;
  private readonly abonnements: { dispose(): void }[] = [];

  constructor(
    private readonly term: TerminalLu,
    private readonly noter: (commande: string) => void,
    private readonly noterCode: (code: number) => void,
    private readonly actif: () => boolean,
  ) {
    this.abonnements.push(
      term.parser.registerOscHandler(133, (data) => {
        this.surOsc133(data);
        return true;
      }),
      term.onWriteParsed(() => this.verifierAttente(false)),
    );
  }

  dispose(): void {
    for (const a of this.abonnements) a.dispose();
    if (this.attente) {
      clearTimeout(this.attente.minuteur);
      liberer(this.attente.depart);
      this.attente.finale.dispose();
    }
    this.attente = null;
    liberer(this.debut);
    liberer(this.invite);
    this.debut = null;
    this.invite = null;
  }

  /** À appeler pour chaque donnée que le terminal envoie au serveur. */
  frappe(data: string): void {
    this.compter("frappes");
    if (!this.actif()) {
      this.compter("inactif");
      this.oublierLigne();
      return;
    }
    // La forme des frappes seulement journal allumé (audit du 27 novembre
    // 2026, SE-8) : elle était retenue pour toute session, et sa longueur
    // disait celle d'un mot de passe tapé à l'invite de `sudo`. Le diagnostic
    // n'en a besoin que pour un journal actif qui reste vide.
    // eslint-disable-next-line no-control-regex -- la forme garde justement les contrôles
    this.diagnostic.formes.push(JSON.stringify(data.replace(/[^\x00-\x1f\x7f]/gu, "x").slice(0, 24)));
    if (this.diagnostic.formes.length > 12) this.diagnostic.formes.shift();
    if (estReponseTerminal(data)) return this.compter("reponse-terminal");
    if (this.term.buffer.active.type === "alternate") {
      this.compter("plein-ecran");
      this.oublierLigne();
      return;
    }
    if (this.oscComplet && this.enCommande) return this.compter("commande-en-cours");
    // Ctrl+C abandonne la ligne, Ctrl+D quitte : rien à noter.
    if (data === "\x03" || data === "\x04") {
      this.compter("abandon");
      this.oublierLigne();
      return;
    }
    if (!this.debut && !this.invite) this.debut = this.ici();
    const entree = data.indexOf("\r");
    if (entree < 0) {
      this.tape += data;
      return;
    }
    // Entrée : la ligne sera lue quand l'écho sera revenu.
    this.compter("entrees");
    if (this.attente) this.verifierAttente(true);
    const depart = this.invite ?? this.debut;
    // Le repère qui ne sert pas de départ est rendu (audit du 27 novembre
    // 2026, ME-1) : voir `liberer`.
    if (depart !== this.debut) liberer(this.debut);
    const attendu = tapeSimple(this.tape + data.slice(0, entree));
    this.debut = null;
    this.invite = null;
    this.tape = "";
    const finale = this.term.registerMarker(0);
    const yFinale = this.ligneDuCurseur();
    this.attente = {
      depart,
      finale,
      yFinale,
      attendu,
      limite: Date.now() + ATTENTE_ECHO_MS,
      minuteur: setTimeout(() => this.verifierAttente(true), ATTENTE_ECHO_MS),
    };
    // Un collage de plusieurs lignes : celles qui suivent la première partent
    // avant que le terminal n'ait affiché leur invite, on ne sait pas où elles
    // commencent. Avec les marqueurs OSC 133, chacune est notée à son `C`.
  }

  private oublierLigne(): void {
    liberer(this.debut);
    this.debut = null;
    this.tape = "";
  }

  private ligneDuCurseur(): number {
    const buf = this.term.buffer.normal;
    return buf.baseY + buf.cursorY;
  }

  private ici(): Depart {
    return { marqueur: this.term.registerMarker(0), x: this.term.buffer.active.cursorX, y: this.ligneDuCurseur() };
  }

  private surOsc133(data: string): void {
    const [genre, ...reste] = data.split(";");
    if (genre === "A") {
      this.enCommande = false;
    } else if (genre === "B") {
      this.enCommande = false;
      // Le repère remplacé est rendu, et aucun n'est posé journal coupé (audit
      // du 27 novembre 2026, ME-1 et PF-7) : chaque `B` posait un repère
      // jamais libéré. 100 000 `B` sur une ligne (800 Ko de sortie) en
      // laissaient 100 000 vivants, 233 Mo de tas, puis 8,7 s de fil principal
      // au défilement, chaque ligne sortie de l'historique notifiant tous
      // leurs écouteurs ; l'interface entière gelait, pas seulement l'onglet.
      // Si le journal s'allume en cours de ligne, la première frappe retombe
      // sur la position du curseur, comme sans OSC 133.
      liberer(this.invite);
      this.invite = this.actif() ? this.ici() : null;
      // Ce qui a été tapé avant que l'invite ne s'affiche appartient à la ligne
      // qui vient : l'oublier ici faisait noter « ho … » pour « echo … » tapé
      // pendant que fish démarrait (suite bout en bout, PR #113).
      liberer(this.debut);
      this.debut = null;
    } else if (genre === "C") {
      this.oscComplet = true;
      // La commande part : si Entrée a été vue, elle attend son écho, qui est
      // revenu puisque le shell l'exécute. Sans Entrée, rien : les marqueurs
      // viennent du serveur, et un serveur hostile qui envoie `B` puis `C`
      // ferait sinon noter un texte que personne n'a tapé (revue de sécurité
      // automatique de la PR #113).
      if (this.attente) this.verifierAttente(true);
      liberer(this.invite);
      this.invite = null;
      this.oublierLigne();
      this.enCommande = true;
    } else if (genre === "D") {
      this.enCommande = false;
      const code = Number.parseInt(reste[0] ?? "", 10);
      if (this.codeAttendu && Number.isInteger(code)) this.noterCode(code);
      this.codeAttendu = false;
    }
  }

  /** Lit la commande en attente si son écho est revenu, ou si `forcer`. */
  private verifierAttente(forcer: boolean): void {
    const a = this.attente;
    if (!a) return;
    if (!forcer && Date.now() < a.limite && !this.pret(a)) return;
    clearTimeout(a.minuteur);
    this.attente = null;
    this.compter(forcer ? "lectures-forcees" : "lectures");
    if (this.actif()) this.lireEtNoter(a.depart, a.finale, a.attendu, a.yFinale);
    else this.compter("lecture-inactif");
    a.finale.dispose();
    liberer(a.depart);
  }

  /** La commande en attente peut-elle être lue ? Quand les frappes donnent
   *  leur ligne, dès qu'elle se lit à l'écran : xterm traite une grosse
   *  écriture par tranches, et ConPTY déplace le curseur (effacement, sauts de
   *  ligne) avant d'y dessiner l'écho ; le curseur sous les repères ne disait
   *  pas que l'écho était là (PR #113). Sinon, quand le curseur a quitté la
   *  ligne de la commande. Au plus `ATTENTE_ECHO_MS` dans les deux cas. */
  private pret(a: EnAttente): boolean {
    if (a.attendu !== null) return this.retrouver(a.attendu, a.depart, a.finale, a.yFinale) !== null;
    return this.echoRevenu(a);
  }

  /** Le curseur a-t-il quitté la ligne (prolongements compris) où se trouvait
   *  la commande quand Entrée est partie ? */
  private echoRevenu(a: EnAttente): boolean {
    if (this.term.buffer.active.type === "alternate") return true;
    if (a.finale.isDisposed) return true;
    const buf = this.term.buffer.normal;
    const curseur = buf.baseY + buf.cursorY;
    return curseur > this.finDuGroupe(a.finale.line);
  }

  /** La dernière ligne du groupe qui commence ou passe par `y` : les lignes
   *  suivantes qui ne sont que son prolongement (`isWrapped`). */
  private finDuGroupe(y: number): number {
    const buf = this.term.buffer.normal;
    let fin = y;
    while (buf.getLine(fin + 1)?.isWrapped) fin++;
    return fin;
  }

  private debutDuGroupe(y: number): number {
    const buf = this.term.buffer.normal;
    let debut = y;
    while (debut > 0 && buf.getLine(debut)?.isWrapped) debut--;
    return debut;
  }

  private lireEtNoter(depart: Depart | null, finale: IMarker | null, attendu: string | null, yFinale?: number): void {
    const commande = attendu !== null ? this.retrouver(attendu, depart, finale, yFinale) : this.lire(depart, finale);
    this.compter(attendu !== null ? "lu-par-frappes" : "lu-a-l-ecran");
    if (commande === null) this.compter("non-retrouvee");
    // Une ligne qui commence par une espace ne laisse pas de trace : le cœur la
    // refuse aussi, mais elle n'a pas à quitter la page.
    if (!commande || /^\s/.test(commande) || commande.trim() === "") return;
    this.noter(commande);
    this.compter("notees");
    this.codeAttendu = true;
  }

  /** Des frappes ordinaires donnent leur ligne : on la note si l'écran la
   *  montre, en fin de la ligne où était la commande, et rien sinon.
   *
   *  En fin de ligne plutôt qu'à la colonne de la première frappe : sous
   *  Windows, la session passe par ConPTY, qui redessine l'écran par
   *  positionnements et ne place pas toujours le curseur au bout de l'invite
   *  avant la première frappe (suite bout en bout Windows, PR #113 : commandes
   *  exécutées, journal vide). Et rien si l'écran montre autre chose : c'est le
   *  cas d'un secret lu sans écho (`read` après `stty -echo`) dont la commande
   *  écrit sa sortie sur la même ligne, qu'on noterait comme si on l'avait
   *  tapée ; un mot de passe non affiché n'est jamais à l'écran. */
  private retrouver(attendu: string, depart: Depart | null, finale: IMarker | null, yFinale?: number): string | null {
    const voulu = attendu.trimEnd();
    if (voulu.trim() === "") return null;
    const ligne = (m: IMarker | null | undefined, secours: number | undefined) =>
      m && !m.isDisposed && m.line >= 0 ? m.line : secours;
    const reperes = [ligne(finale, yFinale), ligne(depart?.marqueur, depart?.y)]
      .filter((y): y is number => y !== undefined && y >= 0);
    if (reperes.length === 0) return null;
    // On parcourt ce qui s'est affiché depuis la frappe, du premier repère
    // jusqu'au curseur : une commande tapée avant que l'invite ne soit
    // dessinée s'affiche plus bas que ses repères. Sous Windows, ConPTY efface
    // l'écran (ED 2) et redessine l'invite en haut à l'ouverture : les repères
    // posés pendant la connexion pointaient sur une ligne vide, rien n'était
    // noté (suite bout en bout Windows, PR #113, diagnostiqué par les
    // compteurs du capteur).
    const buf = this.term.buffer.normal;
    const fin = Math.max(...reperes, buf.baseY + buf.cursorY);
    let y = this.debutDuGroupe(Math.min(...reperes));
    for (let groupes = 0; y <= fin && groupes < GROUPES_MAX; groupes++) {
      if (this.texteDuGroupe(y, 0).endsWith(voulu)) return voulu;
      y = this.finDuGroupe(y) + 1;
    }
    return null;
  }

  /** Le texte de la commande, de la fin de l'invite à la fin de sa ligne. */
  private lire(depart: Depart | null, finale: IMarker | null): string | null {
    const buf = this.term.buffer.normal;
    const valide = (m: IMarker | null | undefined): m is IMarker => !!m && !m.isDisposed && m.line >= 0;
    if (!depart || !valide(depart.marqueur)) {
      // Écran effacé (Ctrl+L) depuis la première frappe : l'invite a été
      // redessinée en haut, à la même colonne.
      if (!depart || !valide(finale)) return null;
      return this.texteDuGroupe(this.debutDuGroupe(finale.line), depart.x);
    }
    let y = depart.marqueur.line;
    // La complétion qui liste ses candidats (double Tab) les affiche sous la
    // ligne puis redessine l'invite et la commande plus bas : la commande est
    // alors dans le groupe où était le curseur à Entrée, à la même colonne.
    if (valide(finale) && this.debutDuGroupe(finale.line) > this.finDuGroupe(y)) {
      y = this.debutDuGroupe(finale.line);
    }
    if (!buf.getLine(y)) return null;
    return this.texteDuGroupe(y, depart.x);
  }

  private texteDuGroupe(y: number, x: number): string {
    const buf = this.term.buffer.normal;
    let texte = buf.getLine(y)?.translateToString(false, x) ?? "";
    let derniere = buf.getLine(y)?.translateToString(false) ?? "";
    let i = y + 1;
    for (let l = buf.getLine(i); l?.isWrapped; l = buf.getLine(++i)) {
      derniere = l.translateToString(false);
      texte += derniere;
    }
    // Une invite à droite (RPROMPT de zsh) reste sur la ligne : du texte collé
    // au bord droit, séparé de la commande par au moins deux espaces.
    if (derniere.length >= this.term.cols && derniere[this.term.cols - 1] !== " ") {
      const m = /^(.*\S)\s{2,}\S.*$/.exec(texte.trimEnd());
      if (m) return m[1];
    }
    return texte.trimEnd();
  }
}
