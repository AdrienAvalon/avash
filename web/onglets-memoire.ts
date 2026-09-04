// Mémoire des onglets, la partie pure : ce qui vaut la peine d'être retenu et
// ce qui vaut encore la peine d'être proposé. Le cœur garde la liste dans le
// répertoire de configuration ; l'écran d'accueil est dans
// onglets-restauration.ts.

export type OngletMemorise = { kind: "ssh"; alias: string } | { kind: "rdp"; host_id: string };

/** Un onglet ouvert, tel que `orderedTabs` le décrit, avec ce qui l'identifie. */
export type OngletOuvert =
  | { kind: "ssh"; alias: string }
  | { kind: "rdp"; hostId?: string };

/**
 * Ce qui se rouvre : un hôte encore déclaré dans `~/.ssh/config`, un bureau
 * enregistré. Une connexion directe (alias qui n'est pas un hôte connu,
 * bureau sans identifiant) n'a ni configuration ni mot de passe rejouable.
 * Les doublons sont gardés : deux onglets sur le même hôte se rouvrent à deux.
 */
export function listeAMemoriser(
  ouverts: OngletOuvert[],
  aliasConnus: ReadonlySet<string>,
  bureauxConnus: ReadonlySet<string>,
): OngletMemorise[] {
  const out: OngletMemorise[] = [];
  for (const o of ouverts) {
    if (o.kind === "ssh") {
      if (aliasConnus.has(o.alias)) out.push({ kind: "ssh", alias: o.alias });
    } else if (o.hostId && bureauxConnus.has(o.hostId)) {
      out.push({ kind: "rdp", host_id: o.hostId });
    }
  }
  return out;
}

/** Ce que la proposition doit dire, avant traduction : rien, ou combien. */
export function nombreARestaurer(memorises: OngletMemorise[], aliasConnus: ReadonlySet<string>, bureauxConnus: ReadonlySet<string>): number {
  return memorises.filter((m) => (m.kind === "ssh" ? aliasConnus.has(m.alias) : bureauxConnus.has(m.host_id))).length;
}
