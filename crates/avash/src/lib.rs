//! Avash — parseur ~/.ssh/config v0.1, avec serialisation pour le front.

#[cfg(test)]
pub(crate) mod testutil;

pub mod enregistrement;
pub mod folders;
pub mod import;
pub mod keys;
pub mod osinfo;
pub mod rdphost;
pub mod sante;
pub mod secrets;
pub mod serie;
pub mod sftp;
pub mod snippet;
pub mod ssh;
pub mod tunnel;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Dossier de rangement Avash (ex. « prod/web »), vide = racine.
    #[serde(default)]
    pub folder: String,
}

#[must_use]
pub fn ssh_config_path() -> std::path::PathBuf {
    repertoire_personnel()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".ssh/config")
}

pub fn parse_ssh_config() -> anyhow::Result<Vec<SshHost>> {
    let path = ssh_config_path();
    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Impossible de lire {}: {e}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    Ok(parse_config_str(&resolve_includes(&content, &base, 0)))
}

/// Comme [`parse_ssh_config`], mais sur un chemin explicite (testable), en
/// résolvant les `Include` relativement au dossier de ce fichier. Un fichier
/// principal absent ou illisible rend une liste vide.
///
/// Sert au registre des dossiers (`folders`) pour repérer les hôtes déclarés
/// dans un fichier inclus : ils ne sont pas dans le fichier principal que
/// `set_host_folder_at` réécrit, mais figurent bien dans l'arbre affiché.
#[must_use]
pub(crate) fn parse_config_resolu_at(path: &Path) -> Vec<SshHost> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    parse_config_str(&resolve_includes(&content, &base, 0))
}

/// Profondeur maximale de resolution des `Include`.
///
/// OpenSSH s'arrete a 16 ; on fait de meme. Sans borne, deux fichiers qui
/// s'incluent mutuellement boucleraient indefiniment.
const MAX_INCLUDE_DEPTH: usize = 16;

/// Resout les directives `Include` et rend le contenu aplati.
///
/// Les chemins relatifs sont resolus depuis `~/.ssh`, comme le fait OpenSSH.
/// `~` est developpe. Les motifs (`config.d/*`) sont etendus par ordre
/// alphabetique. Un fichier illisible est ignore en silence : OpenSSH se
/// comporte ainsi, et une configuration partielle vaut mieux qu'aucune.
fn resolve_includes(content: &str, base: &Path, depth: usize) -> String {
    if depth >= MAX_INCLUDE_DEPTH {
        return content.to_string();
    }
    let mut out = String::with_capacity(content.len());
    for raw in content.lines() {
        let line = raw.trim();
        let is_include = line
            .split_once(char::is_whitespace)
            .is_some_and(|(k, _)| k.eq_ignore_ascii_case("include"));
        if !is_include {
            out.push_str(raw);
            out.push('\n');
            continue;
        }
        let Some((_, patterns)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        for pattern in patterns.split_whitespace() {
            for path in expand_include(pattern, base) {
                if let Ok(inner) = std::fs::read_to_string(&path) {
                    let parent = path.parent().unwrap_or(base).to_path_buf();
                    out.push_str(&resolve_includes(&inner, &parent, depth + 1));
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// Developpe un motif d'`Include` en liste de fichiers existants.
fn expand_include(pattern: &str, base: &Path) -> Vec<PathBuf> {
    let expanded = if let Some(rest) = pattern.strip_prefix("~/") {
        repertoire_personnel()
            .unwrap_or_else(|| base.to_path_buf())
            .join(rest)
    } else if pattern.starts_with('/') {
        PathBuf::from(pattern)
    } else {
        base.join(pattern)
    };

    let s = expanded.to_string_lossy().into_owned();
    if !s.contains(['*', '?']) {
        return if expanded.is_file() {
            vec![expanded]
        } else {
            vec![]
        };
    }
    // Motif : on liste le repertoire parent et on filtre a la main plutot
    // que d'ajouter une dependance de glob pour ce seul usage.
    let (dir, pat) = match expanded.parent().zip(expanded.file_name()) {
        Some((d, f)) => (d.to_path_buf(), f.to_string_lossy().into_owned()),
        None => return vec![],
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| glob_match(&pat, &n.to_string_lossy()))
        })
        .collect();
    // Ordre stable : OpenSSH lit dans l'ordre lexicographique.
    found.sort();
    found
}

/// Correspondance de motif minimale : `*` et `?`, sans classes.
///
/// Trouvé par l'audit du 7 septembre 2026 : la version récursive (deux appels
/// sur `*`) faisait un retour arrière exponentiel : un motif d'`Include` à
/// plusieurs étoiles (`conf.d/*a*a…*b`) sur un long nom sans correspondance
/// figeait `parse_ssh_config` à chaque rafraîchissement. On balaie désormais les
/// octets une fois, avec un seul point de repli (dernière étoile vue et position
/// à reprendre) : O(n*m), même résultat.
///
/// Exposée (fonction pure) pour la cible cargo-fuzz `glob_match_pur`, qui rejoue
/// le cas pathologique décrit ci-dessus sous `-timeout`.
#[must_use]
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p = pattern.as_bytes();
    let n = name.as_bytes();
    let (mut pi, mut ni) = (0, 0);
    // `etoile` = index dans `p` de la dernière `*` rencontrée ; `repli` = index
    // dans `n` à partir duquel cette `*` reprendra en avalant un octet de plus.
    let mut etoile: Option<usize> = None;
    let mut repli = 0;
    while ni < n.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            etoile = Some(pi);
            repli = ni;
            pi += 1;
        } else if let Some(e) = etoile {
            // Pas de correspondance ici : l'étoile avale un octet de plus.
            pi = e + 1;
            repli += 1;
            ni = repli;
        } else {
            return false;
        }
    }
    // Nom épuisé : le reste du motif ne doit être que des étoiles.
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

/// Retire une paire de guillemets doubles entourant une valeur de directive.
///
/// OpenSSH permet de guillemeter une valeur qui contient une espace
/// (`IdentityFile "~/ma clé"`, ou un chemin Windows `C:\Users\Jean Dupont\…`).
/// Trouvé par l'audit du 7 septembre 2026 : le parseur gardait les guillemets
/// littéralement, rendant la clé (ou le `HostName`) introuvable. On les retire à
/// la lecture ; l'écriture les remet quand c'est nécessaire.
fn dequote(value: &str) -> &str {
    let v = value.trim();
    v.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(v)
}

/// Blocs `Host` du fichier, motifs bruts et champs parsés, dans l'ordre.
///
/// L'`alias` de chaque bloc est la ligne `Host` telle quelle (`db bastion`,
/// `*`, `!prod *`) : [`parse_config_str`] la découpe ensuite pour la liste
/// éditable, tandis que [`resoudre_hote_dans`] a besoin des motifs entiers pour
/// appliquer les valeurs par défaut d'un bloc à joker.
fn blocs_bruts(content: &str) -> Vec<SshHost> {
    let mut hosts: Vec<SshHost> = Vec::new();
    let mut current: Option<SshHost> = None;

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        // Convention Avash : un commentaire `#Tags: a, b` DANS un bloc Host
        // etiquette l'hote. Reste un commentaire pour OpenSSH (ignore).
        if let Some(rest) = line.strip_prefix('#') {
            if let Some(list) = rest
                .trim_start()
                .strip_prefix("Tags:")
                .or_else(|| rest.trim_start().strip_prefix("tags:"))
            {
                // Premier `#Tags:` du bloc gagne, comme les directives SSH
                // ci-dessous : une liste vide ne compte pas, la suivante sert.
                if let Some(h) = current.as_mut().filter(|h| h.tags.is_empty()) {
                    h.tags = list
                        .split(',')
                        .map(|t| t.trim().to_string())
                        .filter(|t| !t.is_empty())
                        .collect();
                }
            } else if let Some(path) = rest
                .trim_start()
                .strip_prefix("Folder:")
                .or_else(|| rest.trim_start().strip_prefix("folder:"))
            {
                // Idem pour `#Folder:` : premier gagnant, valeur vide ignorée.
                if let Some(h) = current.as_mut().filter(|h| h.folder.is_empty()) {
                    h.folder = path.trim().trim_matches('/').to_string();
                }
            }
            continue;
        }
        let (key, value) = match line.split_once(char::is_whitespace) {
            Some((k, v)) => (k.to_lowercase(), v.trim().to_string()),
            None => (line.to_lowercase(), String::new()),
        };

        match key.as_str() {
            "host" => {
                if let Some(h) = current.take() {
                    hosts.push(h);
                }
                current = Some(SshHost {
                    alias: value.clone(),
                    ..Default::default()
                });
            }
            // Un bloc `Match` ferme le bloc `Host` courant. Sans cela ses
            // directives etaient attribuees au dernier hote rencontre : un
            // `Match exec ...` jamais satisfait pouvait ainsi changer
            // silencieusement l'utilisateur et le port d'un hote reel.
            //
            // Avash n'evalue pas les conditions de `Match` (elles dependent de
            // l'hote cible, de l'utilisateur courant, voire d'une commande) :
            // on ferme le bloc et on ignore ce qu'il contient, plutot que de
            // l'appliquer a tort.
            "match" => {
                if let Some(h) = current.take() {
                    hosts.push(h);
                }
                current = None;
            }
            _ => {
                if let Some(h) = current.as_mut() {
                    // « La première valeur obtenue est retenue » vaut aussi
                    // À L'INTÉRIEUR d'un bloc, pas seulement entre blocs.
                    // Trouvé par l'audit du 9 septembre 2026 : chaque directive
                    // écrasait la précédente, donc un bloc issu d'une fusion
                    // manuelle (`User adrien` puis `User root`) faisait afficher
                    // et connecter Avash en « root » là où `ssh prod` prend
                    // « adrien ». On ne remplit donc qu'un champ encore `None`.
                    //
                    // Une valeur VIDE ne compte pas comme première valeur : la
                    // même fusion manuelle laisse des résidus (`HostName` sans
                    // argument, `User ""`) et les retenir aurait masqué la vraie
                    // valeur écrite juste après. Avash aurait alors visé une
                    // adresse vide, que rien ne rattrape en aval
                    // (`hostname.unwrap_or(alias)` laisse passer `""`). Même
                    // raison que pour `Port 0` ci-dessous.
                    let valeur = dequote(&value);
                    match key.as_str() {
                        "hostname" if h.hostname.is_none() && !valeur.is_empty() => {
                            h.hostname = Some(valeur.to_string());
                        }
                        "user" if h.user.is_none() && !valeur.is_empty() => {
                            h.user = Some(valeur.to_string());
                        }
                        // OpenSSH refuse « Port 0 » (« Bad port ») : le lire
                        // comme un port menait à une connexion vouée à l'échec
                        // sur un message opaque. Trouvé par le fuzzing. Un port
                        // rejeté ne vaut pas première valeur : on laisse le
                        // champ vide, une occurrence valide suivante servira.
                        "port" if h.port.is_none() => {
                            h.port = value.parse::<u16>().ok().filter(|p| *p != 0);
                        }
                        "identityfile" if h.identity_file.is_none() && !valeur.is_empty() => {
                            h.identity_file = Some(valeur.to_string());
                        }
                        "proxyjump" if h.proxy_jump.is_none() && !valeur.is_empty() => {
                            h.proxy_jump = Some(valeur.to_string());
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    if let Some(h) = current.take() {
        hosts.push(h);
    }
    hosts
}

/// Parse `~/.ssh/config` en liste ÉDITABLE : un hôte par alias littéral, les
/// blocs à joker (`Host *`, `Host !prod`) écartés — ils ne désignent pas un
/// hôte connectable. Les valeurs par défaut qu'ils posent ne sont pas perdues
/// pour autant : elles sont appliquées à la résolution ([`resoudre_hote`]), pas
/// ici, pour que la liste et le formulaire d'édition ne montrent que ce que
/// l'utilisateur a réellement écrit dans le bloc.
#[must_use]
pub fn parse_config_str(content: &str) -> Vec<SshHost> {
    let mut expanded = Vec::new();
    for h in blocs_bruts(content) {
        for alias in h.alias.split_whitespace() {
            expanded.push(SshHost {
                alias: alias.to_string(),
                ..h.clone()
            });
        }
    }
    expanded.retain(|h| !h.alias.contains('*') && !h.alias.starts_with('!'));
    expanded
}

/// Un bloc `Host` s'applique-t-il à `alias` ? (sémantique OpenSSH)
///
/// Le bloc s'applique si au moins un motif positif correspond ET qu'aucun motif
/// de négation `!` ne correspond : un `!motif` qui matche annule tout le bloc,
/// un bloc de négations seules ne matche jamais. La comparaison est insensible à
/// la casse (`match_pattern_list` avec `dolower=1`) et porte sur l'alias tapé,
/// pas sur le `HostName`.
fn bloc_s_applique(motifs: &str, alias: &str) -> bool {
    let alias = alias.to_ascii_lowercase();
    let mut positif = false;
    for jeton in motifs.split_whitespace() {
        if let Some(negatif) = jeton.strip_prefix('!') {
            if glob_match(&negatif.to_ascii_lowercase(), &alias) {
                return false;
            }
        } else if glob_match(&jeton.to_ascii_lowercase(), &alias) {
            positif = true;
        }
    }
    positif
}

/// Résout un alias en appliquant les valeurs par défaut posées par les blocs à
/// motif (`Host *`, `Host *.interne`…), comme le fait `ssh <alias>`.
///
/// Trouvé par l'audit du 7 septembre 2026 : [`parse_config_str`] jetait les
/// blocs à joker, si bien qu'un `User`/`IdentityFile`/`Port`/`ProxyJump` posé
/// dans `Host *` ne s'appliquait pas. Avash résolvait alors l'hôte avec
/// l'utilisateur courant, sans clé et sur le port 22, là où `ssh` faisait autre
/// chose — mauvais utilisateur, mot de passe demandé à tort faute de clé, ou
/// sonde de santé lancée en direct vers un hôte pourtant derrière un rebond.
///
/// On reproduit « la première valeur obtenue est retenue » d'OpenSSH : parcours
/// des blocs dans l'ordre du fichier, en ne remplissant qu'un champ encore
/// `None`. Un `Host *` placé en fin de fichier (disposition recommandée par le
/// man) ne peut donc pas écraser un bloc littéral placé avant. On n'hérite que
/// `user`, `port`, `identity_file` et `proxy_jump` : jamais `hostname` (Avash ne
/// développe aucun jeton `%h`, un `HostName` de bloc à motif remonterait faux),
/// ni les conventions Avash `tags`/`folder` (propres au bloc littéral). Comme
/// chez OpenSSH `IdentityFile` s'accumule et toutes les clés sont tentées alors
/// que [`SshHost`] n'en garde qu'une, « première valeur » est ici une
/// approximation acceptable.
///
/// Rend `None` si aucun bloc littéral ne déclare exactement `alias` : ce n'est
/// pas un hôte connu d'Avash.
#[must_use]
pub fn resoudre_hote_dans(content: &str, alias: &str) -> Option<SshHost> {
    let blocs = blocs_bruts(content);
    let mut resolu = SshHost {
        alias: alias.to_string(),
        ..Default::default()
    };
    let mut trouve = false;
    for bloc in &blocs {
        if !bloc_s_applique(&bloc.alias, alias) {
            continue;
        }
        // Un bloc qui liste EXACTEMENT cet alias est son bloc littéral : lui
        // seul porte le `HostName` propre de l'hôte et ses conventions Avash.
        // Un bloc purement à motif ne fournit que les défauts de connexion.
        if bloc.alias.split_whitespace().any(|jeton| jeton == alias) {
            trouve = true;
            if resolu.hostname.is_none() {
                resolu.hostname.clone_from(&bloc.hostname);
            }
            if resolu.tags.is_empty() {
                resolu.tags.clone_from(&bloc.tags);
            }
            if resolu.folder.is_empty() {
                resolu.folder.clone_from(&bloc.folder);
            }
        }
        if resolu.user.is_none() {
            resolu.user.clone_from(&bloc.user);
        }
        if resolu.port.is_none() {
            resolu.port = bloc.port;
        }
        if resolu.identity_file.is_none() {
            resolu.identity_file.clone_from(&bloc.identity_file);
        }
        if resolu.proxy_jump.is_none() {
            resolu.proxy_jump.clone_from(&bloc.proxy_jump);
        }
    }
    trouve.then_some(resolu)
}

/// Comme [`resoudre_hote_dans`], en lisant `~/.ssh/config` (Include résolus).
#[must_use]
pub fn resoudre_hote(alias: &str) -> Option<SshHost> {
    let path = ssh_config_path();
    let content = std::fs::read_to_string(&path).ok()?;
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    resoudre_hote_dans(&resolve_includes(&content, &base, 0), alias)
}

#[cfg(test)]
#[path = "tests_lib/tests.rs"]
mod tests;

/// Ajoute un hote a `~/.ssh/config`.
///
/// On ecrit dans le fichier standard plutot que dans un format propre a
/// Avash : l'hote enregistre devient utilisable avec `ssh`, `scp`, `rsync`
/// et tout l'ecosysteme, pas seulement ici.
///
/// ⚠️ Aucun mot de passe n'est enregistre — ce fichier est en clair. Pour se
/// passer de saisie, la voie propre est de deployer une cle.
/// Un maillon d'une chaine `ProxyJump`, tel qu'ecrit dans `~/.ssh/config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HopSpec {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

/// Decoupe une valeur `ProxyJump` (`a,b`, `user@host:port`, un alias…) en
/// maillons, dans l'ordre. Ne resout rien : la resolution (alias -> hote)
/// se fait ensuite avec la config.
#[must_use]
pub fn split_proxy_jump(spec: &str) -> Vec<HopSpec> {
    spec.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case("none"))
        .map(|token| {
            // Chaque morceau est rogné : `user @hote :port` n'est pas une
            // syntaxe valide, mais un fichier édité à la main peut la
            // contenir, et un espace collé au nom d'hôte rendait un rebond
            // introuvable sans que le message ne le laisse voir (trouvé par
            // le test de mutation).
            let (user, rest) = match token.split_once('@') {
                Some((u, r)) => (Some(u.trim()).filter(|u| !u.is_empty()), r.trim()),
                None => (None, token),
            };
            // `host:port`. Une IPv6 littérale s'écrit entre crochets
            // (`[2001:db8::1]:2222`) : c'est la seule syntaxe qu'OpenSSH
            // accepte en ProxyJump, et la seule que nous sachions découper sans
            // ambiguïté (une IPv6 nue partage le `:` avec le port). Trouvé par
            // l'audit du 7 septembre 2026 : le `rsplit_once(':')` gardait les
            // crochets dans l'hôte, si bien que russh recevait `"[2001:db8::1]"`
            // (ni IP analysable ni nom résolvable) et que le rebond échouait là
            // où `ssh -J` marchait. On retire donc les crochets comme le fait
            // `cleanhostname` d'OpenSSH et on ne lit un `:port` que derrière `]`.
            // Hors crochets, on ne coupe que si la partie après le dernier `:`
            // est un port : zéro n'en est pas un, le morceau reste entier et la
            // résolution le dira introuvable plutôt que de viser le port 0.
            let (host, port) = if let Some(reste) = rest.strip_prefix('[') {
                match reste.split_once(']') {
                    Some((hote, apres)) => {
                        let port = apres
                            .strip_prefix(':')
                            .and_then(|p| p.trim().parse::<u16>().ok())
                            .filter(|p| *p != 0);
                        (hote.trim().to_string(), port)
                    }
                    // Crochet ouvrant sans fermant : morceau malformé. On retire
                    // quand même le `[` de tête (russh ne sait pas le lire, et
                    // l'invariant « pas de crochet dans l'hôte » doit tenir, y
                    // compris pour la cible fuzz) ; la résolution refusera cet
                    // hôte de toute façon.
                    None => (reste.trim().to_string(), None),
                }
            } else {
                match rest.rsplit_once(':') {
                    Some((h, p))
                        if !h.trim().is_empty()
                            && p.trim().parse::<u16>().is_ok_and(|p| p != 0) =>
                    {
                        (h.trim().to_string(), p.trim().parse::<u16>().ok())
                    }
                    _ => (rest.to_string(), None),
                }
            };
            // Filet de sécurité : aucune paire de crochets ne doit subsister
            // dans l'hôte, y compris pour des entrées pathologiques à crochets
            // imbriqués (`[[h]:22` → `[h` gardait un crochet, trouvé par
            // cargo-fuzz après le premier correctif). OpenSSH (`cleanhostname`)
            // retire les crochets du nom d'hôte ; on fait de même, ce qui tient
            // l'invariant « pas de crochet dans l'hôte » quoi qu'on reçoive.
            // On re-rogne APRÈS le retrait des crochets : un `[` ou `]` en bord
            // collé à une espace (`] a`) la mettrait à découvert, cassant
            // l'invariant « hôte rogné » (second cas trouvé par cargo-fuzz).
            let host = host.replace(['[', ']'], "").trim().to_string();
            HopSpec {
                user: user.map(str::to_string),
                host,
                port,
            }
        })
        .filter(|h| !h.host.is_empty())
        .collect()
}

/// Séparateur de ligne à réémettre pour préserver la fin de ligne du fichier.
///
/// Trouvé par l'audit du 7 septembre 2026 : `content.lines()` retire `\r\n`, et
/// `remove_host`/`update_host`/`set_host_folder_at` réémettaient chaque ligne en
/// `\n`. Sous Windows (Bloc-notes, éditeurs en CRLF), un simple déplacement de
/// dossier ou une édition convertissait donc TOUT le fichier en LF, remplissant
/// `git diff` d'un dépôt de dotfiles versionné et masquant le vrai changement.
/// On garde le séparateur du fichier ; sur un fichier déjà mixte on suit la fin
/// de la PREMIÈRE ligne plutôt qu'un `contains("\r\n")` qui basculerait tout en
/// CRLF. Un fichier sans `\n` (une seule ligne, ou vide) reste en LF.
fn fin_de_ligne(content: &str) -> &'static str {
    match content.find('\n') {
        Some(i) if i > 0 && content.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

/// Dit si `alias` est déjà déclaré dans la configuration SSH, `Include` résolus.
///
/// Trouvé par l'audit du 31 août 2026 pour `append_host` (commit 664d45e), puis
/// par celui du 9 septembre 2026 pour `update_host` : la vérification faite sur
/// le seul fichier principal ne voyait pas les alias d'un fichier inclus, si
/// bien qu'on écrivait dans `~/.ssh/config` un second bloc pour un alias déjà
/// pris. OpenSSH retenant la PREMIÈRE occurrence, l'hôte joint restait celui du
/// fichier inclus et les modifications semblaient sans effet, deux entrées de
/// même nom apparaissant dans la liste. La duplication du contrôle entre les
/// deux fonctions est ce qui avait laissé `update_host` en arrière : il est
/// désormais écrit une seule fois.
/// `principal` sert de repli quand la configuration complète est illisible.
fn alias_deja_declare(alias: &str, principal: &str) -> bool {
    let pris = |hotes: &[SshHost]| hotes.iter().any(|h| h.alias.eq_ignore_ascii_case(alias));
    parse_ssh_config().map_or_else(|_| pris(&parse_config_str(principal)), |hotes| pris(&hotes))
}

pub fn append_host(host: &SshHost) -> anyhow::Result<()> {
    use std::io::Write as _;

    let alias = host.alias.trim();
    validate_host(host)?;

    let path = ssh_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("Création de {} : {e}", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }

    // Trouvé par l'audit du 7 septembre 2026 : un `unwrap_or_default()` ici
    // avalait l'erreur de lecture d'un `~/.ssh/config` existant mais non UTF-8
    // (un commentaire Latin-1 `# R\xe9seau` d'un vieil éditeur). `existing`
    // devenait vide, le contrôle d'unicité ne voyait plus aucun alias (doublon
    // possible) et, le fichier étant cru vide, le bloc était collé au dernier
    // octet, soudant `Host x` à la directive précédente si elle ne finissait
    // pas par un saut de ligne. Seul `NotFound` (fichier absent) vaut `""` ;
    // toute autre erreur est propagée comme le font `remove_host` et
    // `parse_ssh_config`.
    let existing = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(anyhow::anyhow!("Lecture de {} : {e}", path.display())),
    };
    // Unicité vérifiée sur la configuration COMPLÈTE, Include résolus (voir
    // `alias_deja_declare`), avec repli sur le fichier principal.
    if alias_deja_declare(alias, &existing) {
        return Err(anyhow::anyhow!(
            "Un hôte « {alias} » est déjà déclaré dans votre configuration SSH."
        ));
    }

    // Fin de ligne du fichier existant : `render_host_block` produit du LF, on
    // convertit le bloc ajouté juste avant l'écriture (voir `fin_de_ligne`).
    // Sans cela un `~/.ssh/config` en CRLF se retrouvait mixte, le bloc Avash y
    // étant collé en LF.
    let fin = fin_de_ligne(&existing);
    let mut block = String::new();
    // Une ligne vide avant le bloc, sauf si le fichier est vide ou en finit
    // deja par une : sinon le `Host` se colle a la directive precedente et
    // en devient une sous-directive. La détection tient compte du CRLF (une
    // ligne vide de fin y vaut `\r\n\r\n`).
    let finit_par_ligne_vide = existing.ends_with("\n\n") || existing.ends_with("\r\n\r\n");
    if !existing.is_empty() && !finit_par_ligne_vide {
        if !existing.ends_with('\n') {
            block.push('\n');
        }
        block.push('\n');
    }
    block.push_str(&render_host_block(host));
    if fin == "\r\n" {
        block = block.replace('\n', "\r\n");
    }

    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| anyhow::anyhow!("Ouverture de {} : {e}", path.display()))?;
    f.write_all(block.as_bytes())
        .map_err(|e| anyhow::anyhow!("Écriture dans {} : {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Si `alias` figure dans un bloc `Host` à plusieurs noms (`Host a b`), rend la
/// liste d'alias telle qu'écrite (« a b ») ; sinon `None`.
///
/// La détection suit `parse_config_str` : mot-clé insensible à la casse,
/// séparateur espace OU tabulation (`HoSt\ta b` compte). La comparaison de
/// l'alias est exacte, comme celle qui décide `in_target`/`matche` dans les
/// éditeurs de blocs.
fn bloc_multi_alias_citant(content: &str, alias: &str) -> Option<String> {
    for line in content.lines() {
        let Some((key, value)) = line.trim_start().split_once(char::is_whitespace) else {
            continue;
        };
        if !key.eq_ignore_ascii_case("host") {
            continue;
        }
        let noms: Vec<&str> = value.split_whitespace().collect();
        if noms.len() > 1 && noms.contains(&alias) {
            return Some(noms.join(" "));
        }
    }
    None
}

/// Message d'échec quand un alias visé n'a pas pu être édité en place.
///
/// Trouvé par l'audit du 7 septembre 2026 : `set_host_folder_at`, `remove_host`
/// et `update_host` rendaient « Hôte « a » introuvable » pour un alias déclaré
/// dans un bloc `Host a b`. L'hôte est pourtant listé (`parse_config_str` éclate
/// `Host a b` en deux entrées) et glissable dans l'arbre : le message affirmait
/// un fait faux. Ces trois chemins refusent volontairement d'éditer un bloc à
/// plusieurs alias (on ne saurait où poser le marqueur ni quel bloc réécrire
/// sans changer le sens pour les autres noms) ; le message le dit désormais, et
/// ne garde « introuvable » que si aucun bloc ne cite l'alias.
fn erreur_alias_non_editable(content: &str, alias: &str, path: &std::path::Path) -> anyhow::Error {
    if let Some(bloc) = bloc_multi_alias_citant(content, alias) {
        anyhow::anyhow!("Le bloc « Host {bloc} » a plusieurs alias : rangez-le à la main.")
    } else {
        anyhow::anyhow!("Hôte « {alias} » introuvable dans {}.", path.display())
    }
}

/// Supprime un hôte de `~/.ssh/config`, en préservant tout le reste.
///
/// On retire le bloc `Host <alias>` et ses directives indentées, jusqu'au
/// prochain `Host`/`Match` ou la fin du fichier. Les commentaires et les
/// autres hôtes de l'utilisateur restent intacts.
pub fn remove_host(alias: &str) -> anyhow::Result<()> {
    let alias = alias.trim();
    if alias.is_empty() {
        return Err(anyhow::anyhow!("Nom d'hôte vide."));
    }
    let path = ssh_config_path();
    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Lecture de {} : {e}", path.display()))?;

    let mut out = String::with_capacity(content.len());
    let mut skipping = false;
    let mut removed = false;
    // Trouvé par l'audit du 7 septembre 2026 : le saut du bloc emportait les
    // lignes vides et les commentaires libres qui le SUIVENT, alors que dans une
    // config éditée à la main ils annoncent le bloc suivant (« # Staging » avant
    // `Host staging`) ; la docstring promettait pourtant de les garder. On les
    // met en tampon : réémis au prochain `Host`/`Match` ou en fin de fichier,
    // mais jetés (donc supprimés avec le bloc) dès qu'une directive du bloc sauté
    // suit. Les marqueurs Avash `#Tags:`/`#Folder:` appartiennent au bloc et
    // partent avec lui.
    let mut tampon: Vec<&str> = Vec::new();
    let vider = |out: &mut String, tampon: &mut Vec<&str>| {
        for l in tampon.drain(..) {
            out.push_str(l);
            out.push('\n');
        }
    };
    for line in content.lines() {
        let trimmed = line.trim_start();
        let (key, value) = trimmed
            .split_once(char::is_whitespace)
            .map_or((trimmed, ""), |(k, v)| (k, v.trim()));
        let key_lower = key.to_lowercase();

        if key_lower == "host" {
            // Un bloc Host commence : on saute celui qui matche exactement.
            // Les alias multiples (`Host a b`) : on ne retire que si l'alias
            // vise est le seul du bloc — sinon on toucherait aux autres.
            let matche = value.split_whitespace().eq(std::iter::once(alias));
            // Ce que le tampon gardait annonçait ce nouveau bloc (ou la fin du
            // bloc précédent non supprimé) : on le rend avant de décider.
            vider(&mut out, &mut tampon);
            skipping = matche;
            if matche {
                removed = true;
                continue;
            }
        } else if key_lower == "match" {
            vider(&mut out, &mut tampon);
            skipping = false;
        }
        if skipping {
            if trimmed.is_empty()
                || (trimmed.starts_with('#') && !is_tags_comment(line) && !is_folder_comment(line))
            {
                // Vide ou commentaire libre : peut annoncer le bloc suivant.
                tampon.push(line);
            } else {
                // Directive du bloc sauté (marqueur Avash compris) : ce qui était
                // en tampon lui était intérieur, on le jette avec.
                tampon.clear();
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    // Bloc cible en fin de fichier : le tampon garde d'éventuels commentaires ou
    // lignes vides de fin qui n'appartiennent pas au bloc supprimé.
    vider(&mut out, &mut tampon);

    if !removed {
        return Err(erreur_alias_non_editable(&content, alias, &path));
    }
    // Compacter les lignes vides en trop laissees par la suppression. On
    // travaille en LF (la compaction `\n\n\n` ne verrait rien en CRLF), puis on
    // rétablit la fin de ligne d'origine juste avant l'écriture.
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    let sortie = out.trim_start_matches('\n');
    let sortie = if fin_de_ligne(&content) == "\r\n" {
        sortie.replace('\n', "\r\n")
    } else {
        sortie.to_string()
    };
    ecrire_atomiquement(&path, sortie.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Modifie un hôte de `~/.ssh/config` : remplace son bloc, en préservant sa
/// position et tout le reste du fichier.
///
/// Si l'alias change, l'ancien bloc est retiré et le nouveau écrit à la même
/// place. On refuse de renommer vers un alias déjà pris (hors l'hôte modifié
/// lui-même). Les blocs à alias multiples ne sont pas modifiables ici — même
/// raison que pour la suppression.
pub fn update_host(old_alias: &str, host: &SshHost) -> anyhow::Result<()> {
    let old_alias = old_alias.trim();
    validate_host(host)?;

    let path = ssh_config_path();
    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Lecture de {} : {e}", path.display()))?;

    // Renommage vers un alias existant (autre que celui qu'on modifie) : refus,
    // sur la configuration COMPLÈTE et non sur le seul fichier principal (voir
    // `alias_deja_declare`, audit du 9 septembre 2026).
    if !host.alias.eq_ignore_ascii_case(old_alias)
        && alias_deja_declare(host.alias.trim(), &content)
    {
        return Err(anyhow::anyhow!(
            "Un hôte « {} » existe déjà.",
            host.alias.trim()
        ));
    }

    let mut out = String::with_capacity(content.len());
    let mut skipping = false;
    let mut replaced = false;
    // Directives du bloc cible qu'Avash ne régénère pas : on les reconduit après
    // le bloc réécrit, au lieu de les perdre.
    let mut preserves: Vec<&str> = Vec::new();
    // Trouvé par l'audit du 7 septembre 2026 : lignes vides et commentaires
    // libres en attente. À la FIN du bloc ils séparent/annoncent le bloc suivant
    // (la note « # Staging » avant `Host staging`, la ligne vide de séparation)
    // et doivent lui rester ; suivis d'une autre directive du bloc, ils lui sont
    // intérieurs (commentaires reconduits, lignes vides régénérées). Sans ce
    // tampon, le séparateur était avalé et le bloc réécrit se collait au suivant.
    let mut tampon: Vec<&str> = Vec::new();
    // Émet le bloc cible (régénéré), ses directives préservées, puis le tampon de
    // fin (séparateur et commentaire annonçant le bloc suivant).
    let emettre = |out: &mut String, preserves: &mut Vec<&str>, tampon: &mut Vec<&str>| {
        out.push_str(render_host_block(host).trim_end());
        out.push('\n');
        for l in preserves.drain(..) {
            out.push_str(l);
            out.push('\n');
        }
        for l in tampon.drain(..) {
            out.push_str(l);
            out.push('\n');
        }
    };
    for line in content.lines() {
        let trimmed = line.trim_start();
        let (key, value) = trimmed
            .split_once(char::is_whitespace)
            .map_or((trimmed, ""), |(k, v)| (k, v.trim()));
        let key_lower = key.to_lowercase();

        if key_lower == "host" {
            // Un nouveau bloc met fin au bloc cible : on l'émet d'abord.
            if skipping {
                emettre(&mut out, &mut preserves, &mut tampon);
                skipping = false;
            }
            if value.split_whitespace().eq(std::iter::once(old_alias)) {
                skipping = true;
                replaced = true;
                continue;
            }
        } else if key_lower == "match" && skipping {
            emettre(&mut out, &mut preserves, &mut tampon);
            skipping = false;
        }
        if skipping {
            if trimmed.is_empty()
                || (trimmed.starts_with('#') && !is_tags_comment(line) && !is_folder_comment(line))
            {
                // Vide ou commentaire libre : en attente, on ne tranche entre
                // « intérieur » et « annonce du bloc suivant » qu'à la ligne d'après.
                tampon.push(line);
            } else {
                // Directive réelle (ou marqueur Avash) : le bloc continue, donc le
                // tampon lui est intérieur — commentaires reconduits, vides jetés.
                for l in tampon.drain(..) {
                    if !l.trim().is_empty() {
                        preserves.push(l);
                    }
                }
                // Ligne interne au bloc cible : préservée si Avash ne la régénère pas.
                if !directive_regeneree(line) {
                    preserves.push(line);
                }
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    // Bloc cible en fin de fichier : rien après lui ne l'a émis.
    if skipping {
        emettre(&mut out, &mut preserves, &mut tampon);
    }

    if !replaced {
        return Err(erreur_alias_non_editable(&content, old_alias, &path));
    }
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    // On a travaillé en LF (`render_host_block` et la compaction sont en LF) :
    // rétablir la fin de ligne d'origine juste avant l'écriture, sans quoi une
    // édition convertissait tout un `~/.ssh/config` CRLF en LF.
    let sortie = out.trim_start_matches('\n');
    let sortie = if fin_de_ligne(&content) == "\r\n" {
        sortie.replace('\n', "\r\n")
    } else {
        sortie.to_string()
    };
    ecrire_atomiquement(&path, sortie.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Rend un bloc `Host` au format OpenSSH.
/// Vrai si la ligne est un commentaire `#Tags:` d'Avash.
fn is_tags_comment(line: &str) -> bool {
    line.trim_start().strip_prefix('#').is_some_and(|r| {
        let r = r.trim_start();
        r.strip_prefix("Tags:")
            .or_else(|| r.strip_prefix("tags:"))
            .is_some()
    })
}

/// Une ligne du bloc `Host` que `render_host_block` régénère déjà, et qu'il ne
/// faut donc pas reconduire telle quelle lors d'un `update_host` — sinon on
/// dupliquerait la directive. Tout le reste (directives qu'Avash ne gère pas :
/// `ForwardAgent`, `LocalForward`, `IdentitiesOnly`, `Ciphers`… et les
/// commentaires libres) est au contraire à préserver.
///
/// Trouvé par l'audit du 7 septembre 2026 : `update_host` réécrivait le bloc
/// depuis `render_host_block`, qui ne connaît qu'un jeu fixe de directives —
/// toute autre directive du bloc disparaissait en silence dès qu'on éditait
/// l'hôte depuis l'interface.
fn directive_regeneree(line: &str) -> bool {
    let t = line.trim_start();
    if t.is_empty() {
        return true; // les lignes vides internes au bloc sont régénérées
    }
    if is_folder_comment(line) || is_tags_comment(line) {
        return true;
    }
    let mot = t.split_once(char::is_whitespace).map_or(t, |(k, _)| k);
    matches!(
        mot.to_ascii_lowercase().as_str(),
        "hostname" | "user" | "port" | "identityfile" | "proxyjump"
    )
}

/// Vrai si la ligne est un commentaire `#Folder:` d'Avash.
fn is_folder_comment(line: &str) -> bool {
    line.trim_start().strip_prefix('#').is_some_and(|r| {
        let r = r.trim_start();
        r.strip_prefix("Folder:")
            .or_else(|| r.strip_prefix("folder:"))
            .is_some()
    })
}

/// Émet un bloc accumulé ; pour le bloc cible, retire l'ancienne ligne
/// `#Folder:` et insère la nouvelle après la dernière directive (avant les
/// éventuelles lignes vides de fin de bloc). Le reste est préservé tel quel.
fn flush_folder_block(out: &mut String, block: &mut Vec<String>, is_target: bool, folder: &str) {
    if is_target {
        block.retain(|l| !is_folder_comment(l));
        if !folder.is_empty() {
            let last = block.iter().rposition(|l| !l.trim().is_empty());
            let pos = last.map_or(block.len(), |i| i + 1);
            block.insert(pos, format!("    #Folder: {folder}"));
        }
    }
    for l in block.drain(..) {
        out.push_str(&l);
        out.push('\n');
    }
}

/// Range un hôte dans un dossier (commentaire `#Folder:`), en **place** :
/// seule la ligne `#Folder:` du bloc est ajoutée/remplacée/retirée, toutes les
/// autres directives sont préservées (contrairement à `update_host`).
///
/// # Errors
/// Si le fichier est illisible/inscriptible, ou l'alias introuvable.
pub fn set_host_folder(alias: &str, folder: &str) -> anyhow::Result<()> {
    set_host_folder_at(&ssh_config_path(), alias, folder)
}

/// Comme [`set_host_folder`], sur un chemin explicite (testable).
///
/// # Errors
/// Si le fichier est illisible/inscriptible, ou l'alias introuvable.
pub fn set_host_folder_at(path: &std::path::Path, alias: &str, folder: &str) -> anyhow::Result<()> {
    let alias = alias.trim();
    let folder = folder.trim().trim_matches('/');
    validate_config_value("Folder", folder)?;
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Lecture de {} : {e}", path.display()))?;

    let mut out = String::with_capacity(content.len() + 32);
    let mut block: Vec<String> = Vec::new();
    let mut in_target = false;
    let mut found = false;

    for line in content.lines() {
        let trimmed = line.trim_start();
        let (key, value) = trimmed
            .split_once(char::is_whitespace)
            .map_or((trimmed, ""), |(k, v)| (k, v.trim()));
        let key_lower = key.to_lowercase();
        if key_lower == "host" || key_lower == "match" {
            flush_folder_block(&mut out, &mut block, in_target, folder);
            in_target = key_lower == "host" && value.split_whitespace().eq(std::iter::once(alias));
            if in_target {
                found = true;
            }
        }
        block.push(line.to_string());
    }
    flush_folder_block(&mut out, &mut block, in_target, folder);

    if !found {
        return Err(erreur_alias_non_editable(&content, alias, path));
    }
    // La (re)pose du seul `#Folder:` ne doit pas convertir tout le fichier :
    // `flush_folder_block` réémet chaque ligne en LF, on rétablit la fin de
    // ligne d'origine juste avant l'écriture (voir `fin_de_ligne`).
    let sortie = if fin_de_ligne(&content) == "\r\n" {
        out.replace('\n', "\r\n")
    } else {
        out
    };
    ecrire_atomiquement(path, sortie.as_bytes())?;
    Ok(())
}

#[must_use]
pub fn render_host_block(host: &SshHost) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "Host {}", host.alias.trim());
    if let Some(v) = host.hostname.as_deref().filter(|v| !v.trim().is_empty()) {
        let _ = writeln!(out, "    HostName {}", v.trim());
    }
    if let Some(v) = host.user.as_deref().filter(|v| !v.trim().is_empty()) {
        let _ = writeln!(out, "    User {}", v.trim());
    }
    if let Some(p) = host.port.filter(|p| *p != 22) {
        let _ = writeln!(out, "    Port {p}");
    }
    if let Some(v) = host
        .identity_file
        .as_deref()
        .filter(|v| !v.trim().is_empty())
    {
        let v = v.trim();
        // Un chemin avec une espace (typique sous Windows : `C:\Users\Jean
        // Dupont\…`) doit être guillemeté, sinon OpenSSH lit « extra arguments »
        // et rejette TOUTE la configuration. Trouvé par l'audit du 7 sept. 2026.
        if v.contains(char::is_whitespace) {
            let _ = writeln!(out, "    IdentityFile \"{v}\"");
        } else {
            let _ = writeln!(out, "    IdentityFile {v}");
        }
    }
    if let Some(v) = host.proxy_jump.as_deref().filter(|v| !v.trim().is_empty()) {
        let _ = writeln!(out, "    ProxyJump {}", v.trim());
    }
    let clean: Vec<&str> = host
        .tags
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect();
    if !clean.is_empty() {
        let _ = writeln!(out, "    #Tags: {}", clean.join(", "));
    }
    let folder = host.folder.trim().trim_matches('/');
    if !folder.is_empty() {
        let _ = writeln!(out, "    #Folder: {folder}");
    }
    out
}

/// Développe un chemin commençant par `~/` (ou valant `~`) en chemin absolu
/// dans le répertoire personnel, comme le fait OpenSSH pour `IdentityFile`.
///
/// Trouvé par l'audit du 7 septembre 2026 : `~/.ssh/id_ed25519`, la forme de la
/// quasi-totalité des configs écrites à la main (et le libellé même du champ clé
/// de l'interface), restait littéral et le fichier de clé était introuvable —
/// l'hôte devenait inconnectable alors que `ssh` s'y connectait. On développe à
/// la RÉSOLUTION, jamais au parseur ni à l'écriture, pour que le fichier
/// conserve `~/` et reste lisible par `ssh`. Un chemin sans tilde de tête est
/// rendu inchangé. Le séparateur Windows `~\` est traité comme `~/`.
#[must_use]
pub fn developper_tilde(chemin: &str) -> PathBuf {
    if chemin == "~" {
        if let Some(home) = repertoire_personnel() {
            return home;
        }
    } else if let Some(rest) = chemin
        .strip_prefix("~/")
        .or_else(|| chemin.strip_prefix("~\\"))
    {
        if let Some(home) = repertoire_personnel() {
            return home.join(rest);
        }
    }
    PathBuf::from(chemin)
}

/// Répertoire personnel de l'utilisateur, avec un point d'entrée unique.
///
/// Deux raisons de ne pas appeler `repertoire_personnel()` directement partout.
///
/// **La cohérence avec russh.** Sous Windows, `repertoire_personnel()` interroge
/// `SHGetKnownFolderPath(FOLDERID_Profile)` alors que `std::env::home_dir()` —
/// celui dont russh se sert pour `known_hosts` — consulte d'abord `USERPROFILE`.
/// Les deux peuvent différer : nous vérifiions alors un fichier pendant que
/// russh en lisait un autre, ce qui vide de sens la vérification de clé d'hôte.
/// Tout passe désormais par ici, et les chemins `known_hosts` sont donnés
/// explicitement à russh plutôt que laissés à sa propre résolution.
///
/// **L'isolation des tests.** Elle reposait sur le remplacement de `HOME`, que
/// Windows ignore : les tests y travaillaient sur le vrai profil, tous en
/// parallèle sur les mêmes fichiers. `AVASH_HOME` sert de dérogation explicite,
/// honorée sur toutes les plateformes. Ce n'est pas une porte dérobée : qui
/// peut poser une variable d'environnement dans le processus peut déjà
/// beaucoup plus.
#[must_use]
pub fn repertoire_personnel() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("AVASH_HOME") {
        let p = std::path::PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    dirs::home_dir()
}

/// Répertoire de configuration d'Avash (`~/.config/avash` ou son équivalent).
///
/// Suit `AVASH_HOME` quand il est posé, pour que les tests isolent aussi les
/// fichiers d'état (dossiers, bureaux RDP, snippets, tunnels).
#[must_use]
pub fn repertoire_configuration() -> Option<std::path::PathBuf> {
    if std::env::var_os("AVASH_HOME").is_some() {
        return repertoire_personnel().map(|h| h.join(".config"));
    }
    dirs::config_dir()
}

/// Écrit un fichier de configuration **atomiquement** et sans fenêtre lisible.
///
/// Deux défauts corrigés d'un coup :
///
/// 1. `std::fs::write` tronque le fichier **puis** écrit. Une coupure entre les
///    deux — disque plein, arrêt brutal — laissait un fichier vide. Pour
///    `~/.ssh/config` c'est toute la configuration SSH de l'utilisateur, pas
///    seulement celle d'Avash, qui disparaissait sur un simple renommage de
///    dossier (une réécriture complète par hôte déplacé). Pour
///    `rdp_known_hosts` c'était pire qu'une perte : sans empreintes, chaque
///    serveur redevient un « premier contact » et tout certificat est réaccepté.
/// 2. Le temporaire naissait avec l'umask, souvent 0644, et n'était resserré
///    qu'après le renommage : `snippets.yaml` — qui contient des commandes
///    d'administration, parfois avec un jeton dedans — était brièvement lisible
///    par les autres comptes de la machine.
///
/// Le temporaire est créé dans le **même répertoire** que la cible, sans quoi
/// `rename` franchirait un point de montage et échouerait.
///
/// # Errors
/// Si le répertoire, l'écriture, la synchronisation ou le renommage échouent.
pub fn ecrire_atomiquement(path: &std::path::Path, contenu: &[u8]) -> anyhow::Result<()> {
    use std::io::Write as _;
    // Le temporaire doit être unique par APPEL, pas seulement par processus :
    // `folders::rename_core` réécrit ~/.ssh/config une fois par hôte, et une
    // autre commande peut y toucher au même moment. Deux appels ouvrant le même
    // `.tmp` en troncature produisaient un fichier mêlant les deux contenus —
    // exactement la perte que cette fonction doit empêcher.
    static SUITE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            // Seul un répertoire que CET appel crée est resserré à 0700. On
            // resserrait aussi celui qui existait déjà : la suite de tests
            // lancée en root (2026-09-03), dont plusieurs cas écrivent un
            // fichier directement sous /tmp, a passé /tmp en 0700 — plus aucun
            // utilisateur du poste ne pouvait y entrer. Un compte ordinaire
            // subissait la même chose, en silence, sur tout répertoire à lui
            // où Avash déposait un fichier : un export dans ~/Documents rendait
            // ~/Documents privé.
            #[cfg(unix)]
            let existait = dir.exists();
            std::fs::create_dir_all(dir)
                .map_err(|e| anyhow::anyhow!("Création de {} : {e}", dir.display()))?;
            #[cfg(unix)]
            if !existait {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
        }
    }
    // Un renommage remplace le **lien** symbolique, là où `std::fs::write` le
    // suivait et écrivait dans sa cible. Une configuration de dotfiles —
    // `~/.ssh/config` pointant vers un dépôt versionné, cas très courant —
    // aurait vu son lien transformé en fichier ordinaire au premier
    // déplacement d'hôte : le dépôt devenait silencieusement orphelin, sans
    // que `git status` n'ait rien à dire. On écrit donc dans la cible réelle.
    // `canonicalize` échoue si le fichier n'existe pas encore : c'est alors le
    // chemin demandé qui convient.
    let resolu = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let path = resolu.as_path();

    // Écrire par renommage remplace la cible même si **elle** est en lecture
    // seule : seul le droit d'écriture du répertoire compte. Un utilisateur qui
    // a délibérément passé son ~/.ssh/config en 0400 ne s'attend pas à le voir
    // réécrit ; on refuse plutôt que de passer outre.
    #[cfg(unix)]
    if let Ok(meta) = std::fs::metadata(path) {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o200 == 0 {
            return Err(anyhow::anyhow!("{} est en lecture seule.", path.display()));
        }
    }
    let tmp = path.with_extension(format!(
        "{}tmp{}.{}",
        path.extension().map_or("", |_| "."),
        std::process::id(),
        SUITE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let ecrire = || -> std::io::Result<()> {
        let mut f = options.open(&tmp)?;
        f.write_all(contenu)?;
        // Sans cette synchronisation, le renommage peut être visible avant le
        // contenu : on retrouverait un fichier de la bonne taille, rempli de
        // zéros, après une coupure de courant.
        f.sync_all()
    };
    if let Err(e) = ecrire() {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::anyhow!("Écriture de {} : {e}", tmp.display()));
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        anyhow::anyhow!("Renommage vers {} : {e}", path.display())
    })?;
    restreindre_au_proprietaire(path);
    Ok(())
}

/// Restreint un fichier de configuration à son seul propriétaire.
///
/// Ces fichiers ne contiennent pas de mot de passe — ceux-ci vivent dans le
/// trousseau — mais bien l'inventaire de l'infrastructure : bureaux RDP,
/// tunnels, dossiers, snippets, donc utilisateurs, hôtes internes, ports et
/// commandes d'administration. Ils héritaient de l'umask (souvent lisible par
/// tous), alors que `~/.ssh/config` est déjà resserré depuis longtemps. Sans
/// effet sous Windows, où les droits viennent des ACL du profil.
pub fn restreindre_au_proprietaire(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        // Le répertoire, lui, n'est resserré que s'il est à Avash. On
        // resserrait le parent de TOUT fichier écrit : un export déposé dans
        // ~/Documents rendait ~/Documents privé sans un mot, et la suite de
        // tests lancée en root sur le poste du mainteneur (2026-09-03) a passé
        // /tmp en 0700 par les cas qui y écrivent directement — plus aucun
        // utilisateur ne pouvait y entrer.
        if let Some(parent) = path.parent() {
            if est_un_repertoire_d_avash(parent) {
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// `~/.ssh` et `~/.config/avash` (avec ses sous-répertoires, dont les
/// enregistrements) : les seuls répertoires qu'Avash s'autorise à resserrer,
/// parce qu'il les tient pour siens. Comparés une fois résolus, pour qu'un
/// `~/.ssh` en lien symbolique vers un dépôt de dotfiles compte aussi.
#[cfg(unix)]
fn est_un_repertoire_d_avash(dir: &std::path::Path) -> bool {
    let reel = |p: std::path::PathBuf| std::fs::canonicalize(&p).unwrap_or(p);
    let dir = reel(dir.to_path_buf());
    let ssh = repertoire_personnel().map(|h| reel(h.join(".ssh")));
    let config = repertoire_configuration().map(|c| reel(c.join("avash")));
    ssh.is_some_and(|s| s == dir) || config.is_some_and(|c| dir.starts_with(c))
}

/// Remplace par une espace tout caractère de contrôle d'un texte avant de
/// l'imprimer sur un terminal.
///
/// Pendant du refus à l'écriture, pour le chemin de lecture. Durcir la seule
/// écriture d'Avash ne protège de rien si `~/.ssh/config` a été piégé par un
/// autre outil (import maison, éditeur, dotfiles partagés) : `avash list`
/// rejouerait la séquence à chaque exécution. Trouvé par l'audit du
/// 9 septembre 2026.
#[must_use]
pub fn sans_controle(texte: &str) -> String {
    texte
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Refuse tout caractère de contrôle dans une valeur destinée à
/// `~/.ssh/config`.
///
/// Le saut de ligne était le seul vrai danger connu (il ouvre une directive
/// arbitraire, `ProxyCommand` compris). L'audit du 9 septembre 2026 a montré
/// que la liste `\n \r \0` laissait passer tout le reste du plan C0 : un
/// `HostName srv\x1b]0;PWNED\x07` venu d'un export `PuTTY` hostile était écrit
/// tel quel par [`render_host_block`], qui ne fait qu'un `.trim()`, et la
/// séquence repartait vers le terminal à chaque `avash list` sans que
/// personne ouvre le fichier. La tabulation tombe avec le reste : elle sépare
/// la clé de la valeur pour OpenSSH, aucun champ n'en a l'usage.
fn validate_config_value(label: &str, value: &str) -> anyhow::Result<()> {
    if let Some(c) = value.chars().find(|c| c.is_control()) {
        return Err(anyhow::anyhow!(
            "{label} contient un caractère interdit (contrôle U+{:04X}).",
            c as u32
        ));
    }
    // Un guillemet double casserait le round-trip : on s'en sert pour entourer
    // une valeur à espace, et OpenSSH le traite comme délimiteur de citation.
    if value.contains('"') {
        return Err(anyhow::anyhow!(
            "{label} ne doit pas contenir de guillemet double."
        ));
    }
    Ok(())
}

/// Comme [`validate_config_value`], en refusant aussi l'espace : pour un champ
/// où rien de légitime n'en contient (`HostName`, `User`, `ProxyJump`). Trouvé
/// par l'audit du 7 septembre 2026 : une espace y était écrite telle quelle et
/// faisait rejeter toute la configuration par OpenSSH.
fn validate_config_value_sans_espace(label: &str, value: &str) -> anyhow::Result<()> {
    validate_config_value(label, value)?;
    if value.contains(char::is_whitespace) {
        return Err(anyhow::anyhow!(
            "{label} ne doit pas contenir d'espace : « {value} »"
        ));
    }
    Ok(())
}

/// Valide une valeur `ProxyJump`, maillon par maillon.
///
/// La forme canonique d'une chaîne à plusieurs sauts s'écrit
/// « bastion, relais:2200 » : virgule PUIS espace. C'est ce que
/// [`split_proxy_jump`] découpe, c'est l'exemple que l'interface affiche dans
/// son champ `ProxyJump`, et c'est la valeur du corpus dit « réaliste » des
/// tests de mutation. Refuser l'espace sur la chaîne entière rendait un hôte
/// déjà enregistré ainsi impossible à réenregistrer, même en ne changeant
/// qu'un tag : trouvé par l'audit du 9 septembre 2026.
///
/// L'espace reste refusé à l'intérieur d'un maillon, mais par prudence
/// d'Avash, pas parce qu'OpenSSH le rejetterait. Mesuré sur cette machine avec
/// `OpenSSH_10.5p1` : `ProxyJump saut un.invalid` passe `ssh -G` sans un mot et
/// en rc=0, parce que `oProxyJump` avale la fin de ligne, là où
/// `HostName un hote` sort bien « keyword hostname extra arguments at end of
/// line ». Le maillon est ensuite recollé tel quel dans la `ProxyCommand`
/// implicite (`ssh … -W '[%h]:%p' saut un.invalid`), où l'espace redevient une
/// frontière d'argument : ssh cherche alors à résoudre « saut » seul et prend
/// « un.invalid » pour une commande distante. Se tromper en silence est pire
/// qu'échouer franchement, d'où le refus ici.
fn validate_proxy_jump(value: &str) -> anyhow::Result<()> {
    validate_config_value("ProxyJump", value)?;
    for maillon in value.split(',') {
        let maillon = maillon.trim();
        // Un maillon vide (« a,,b », ou la chaîne vide) ne vaut rien à écrire
        // mais n'a rien de dangereux : `split_proxy_jump` l'ignore de son côté.
        if maillon.is_empty() {
            continue;
        }
        validate_config_value_sans_espace("ProxyJump", maillon)?;
    }
    Ok(())
}

/// Valide tous les champs d'un hote avant ecriture.
fn validate_host(host: &SshHost) -> anyhow::Result<()> {
    validate_alias(host.alias.trim())?;
    if let Some(v) = &host.hostname {
        validate_config_value_sans_espace("HostName", v)?;
    }
    if let Some(v) = &host.user {
        validate_config_value_sans_espace("User", v)?;
    }
    if let Some(v) = &host.identity_file {
        // IdentityFile peut contenir une espace (chemin Windows) : on la
        // guillemète à l'écriture. Restent refusés le guillemet et les
        // caractères de contrôle.
        validate_config_value("IdentityFile", v)?;
    }
    if let Some(v) = &host.proxy_jump {
        validate_proxy_jump(v)?;
    }
    for t in &host.tags {
        validate_config_value("Tags", t)?;
    }
    validate_config_value("Folder", &host.folder)?;
    Ok(())
}

fn validate_alias(alias: &str) -> anyhow::Result<()> {
    if alias.is_empty() {
        return Err(anyhow::anyhow!("Le nom de l'hôte est vide."));
    }
    // Un saut de ligne permettrait d'injecter n'importe quelle directive
    // dans la configuration SSH, y compris ProxyCommand. Les autres codes de
    // contrôle tombent avec lui depuis l'audit du 9 septembre 2026 : l'alias
    // finit sur la ligne `Host`, que le terminal relit à chaque `avash list`
    // au même titre que les autres champs.
    if let Some(c) = alias.chars().find(|c| c.is_control()) {
        return Err(anyhow::anyhow!(
            "Nom d'hôte invalide : caractère interdit (contrôle U+{:04X}).",
            c as u32
        ));
    }
    if alias.contains(char::is_whitespace) {
        return Err(anyhow::anyhow!(
            "Le nom d'hôte ne doit pas contenir d'espace : « {alias} »"
        ));
    }
    // `Host *` s'appliquerait a toutes les connexions.
    if alias.contains(['*', '?', '!']) {
        return Err(anyhow::anyhow!(
            "Le nom d'hôte ne doit pas contenir de joker (* ? !)."
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests_lib/tests_ecriture_atomique.rs"]
mod tests_ecriture_atomique;

#[cfg(test)]
#[path = "tests_lib/save_tests.rs"]
mod save_tests;

/// Fuzzing par mutation du parseur `~/.ssh/config`.
///
/// C'est la surface d'entrée la plus exposée du cœur : un fichier que
/// l'utilisateur édite à la main, qu'un outil tiers réécrit, ou qu'un dépôt de
/// dotfiles fournit. Le même principe que pour le processus RDP : muter un
/// contenu authentique atteint des chemins que des octets aléatoires ne
/// touchent jamais, parce que le tout premier `Host` filtre déjà tout ce qui ne
/// ressemble pas à une configuration.
#[cfg(test)]
#[path = "tests_lib/tests_mutation.rs"]
mod tests_mutation;
