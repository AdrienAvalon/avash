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
mod tests {
    use super::*;

    #[test]
    fn une_cle_avec_espace_est_guillemetee_et_se_relit() {
        // Trouvé par l'audit du 7 septembre 2026 : une valeur avec espace était
        // écrite sans guillemets et faisait rejeter TOUTE la configuration par
        // OpenSSH. Elle doit être guillemetée à l'écriture et déguillemetée à la
        // lecture (round-trip).
        let mut h = SshHost {
            alias: "prod".into(),
            hostname: Some("prod.exemple.com".into()),
            ..Default::default()
        };
        h.identity_file = Some(r"C:\Users\Jean Dupont\.ssh\id".into());
        let bloc = render_host_block(&h);
        assert!(
            bloc.contains(r#"IdentityFile "C:\Users\Jean Dupont\.ssh\id""#),
            "clé avec espace non guillemetée :\n{bloc}"
        );
        let relu = &parse_config_str(&bloc)[0];
        assert_eq!(
            relu.identity_file.as_deref(),
            Some(r"C:\Users\Jean Dupont\.ssh\id"),
            "la clé doit se relire sans les guillemets"
        );
        // Une clé sans espace n'est pas guillemetée.
        h.identity_file = Some("/home/u/.ssh/id".into());
        assert!(render_host_block(&h).contains("IdentityFile /home/u/.ssh/id"));
    }

    #[test]
    fn validate_host_refuse_l_espace_dans_hostname_user_proxyjump() {
        let base = SshHost {
            alias: "a".into(),
            ..Default::default()
        };
        let avec = |f: fn(&mut SshHost)| {
            let mut h = base.clone();
            f(&mut h);
            validate_host(&h)
        };
        assert!(avec(|h| h.hostname = Some("un hote".into())).is_err());
        assert!(avec(|h| h.user = Some("jean dupont".into())).is_err());
        assert!(avec(|h| h.proxy_jump = Some("a b".into())).is_err());
        // Un guillemet est refusé partout ; l'espace dans IdentityFile passe.
        assert!(avec(|h| h.hostname = Some("a\"b".into())).is_err());
        assert!(avec(|h| h.identity_file = Some("/home/u/ma clé".into())).is_ok());
    }

    #[test]
    fn validate_host_accepte_un_rebond_a_plusieurs_sauts() {
        // Trouvé par l'audit du 9 septembre 2026 : « bastion, relais:2200 »
        // (virgule PUIS espace) est la forme que l'interface donne en exemple,
        // celle que `split_proxy_jump` sait découper, et celle dont `ssh -vv`
        // montre qu'OpenSSH enchaîne bien les deux sauts. La validation
        // refusait pourtant tout espace : un hôte déjà enregistré ainsi ne
        // pouvait plus être réenregistré, le seul fait de changer un tag
        // faisait échouer l'enregistrement.
        let avec_rebond = |v: &str| {
            validate_host(&SshHost {
                alias: "prod".into(),
                proxy_jump: Some(v.into()),
                ..Default::default()
            })
        };
        for bon in [
            "bastion, relais:2200",
            "bastion,deploy@10.0.0.1:2222",
            " bastion , relais ",
            "u@[2001:db8::1]:2222, bastion",
        ] {
            assert!(avec_rebond(bon).is_ok(), "devrait passer : {bon}");
        }
        // L'espace à l'intérieur d'un maillon reste refusé, par prudence : ssh
        // ne s'en plaint pas (mesuré avec OpenSSH_10.5p1), il le lit de
        // travers, « saut un.invalid » devenant l'hôte « saut » suivi d'une
        // commande distante « un.invalid ».
        for mauvais in ["bastion relais", "bastion, un relais", "a b"] {
            assert!(avec_rebond(mauvais).is_err(), "devrait échouer : {mauvais}");
        }
    }

    #[test]
    fn validate_host_refuse_les_caracteres_de_controle() {
        // Trouvé par l'audit du 9 septembre 2026 : la validation ne refusait
        // que `\n`, `\r`, `\0` et le guillemet, et la variante « sans espace »
        // n'ajoutait que `char::is_whitespace`, qui ignore les codes de
        // contrôle C0. Un `HostName srv\x1b]0;PWNED\x07` importé depuis un
        // export PuTTY hostile passait donc jusque dans `~/.ssh/config`, où
        // `render_host_block` l'écrit tel quel (il ne fait qu'un `.trim()`).
        // La séquence repartait ensuite vers le terminal à chaque `avash list`,
        // sans que personne ouvre le fichier : titre de fenêtre réécrit,
        // presse-papiers manipulé par OSC 52, voire réponse d'une requête
        // d'état réinjectée comme une frappe sur certains émulateurs.
        let base = SshHost {
            alias: "prod".into(),
            ..Default::default()
        };
        let avec = |f: &dyn Fn(&mut SshHost)| {
            let mut h = base.clone();
            f(&mut h);
            validate_host(&h)
        };
        // ESC (début de toute séquence ANSI), BEL (fin d'un OSC), DEL, un C1
        // (0x9B, CSI sur un octet) et la tabulation, qui sépare la clé de la
        // valeur pour OpenSSH : aucun n'a sa place dans un champ.
        for c in ['\u{1b}', '\u{7}', '\u{7f}', '\u{9b}', '\t'] {
            let charge = format!("srv{c}x");
            assert!(
                avec(&|h| h.hostname = Some(charge.clone())).is_err(),
                "HostName devrait refuser U+{:04X}",
                c as u32
            );
            assert!(
                avec(&|h| h.user = Some(charge.clone())).is_err(),
                "User devrait refuser U+{:04X}",
                c as u32
            );
            assert!(
                avec(&|h| h.proxy_jump = Some(charge.clone())).is_err(),
                "ProxyJump devrait refuser U+{:04X}",
                c as u32
            );
            assert!(
                avec(&|h| h.identity_file = Some(format!("/home/u/{charge}"))).is_err(),
                "IdentityFile devrait refuser U+{:04X}",
                c as u32
            );
            assert!(
                avec(&|h| h.tags = vec![charge.clone()]).is_err(),
                "Tags devrait refuser U+{:04X}",
                c as u32
            );
            assert!(
                avec(&|h| h.folder = charge.clone()).is_err(),
                "Folder devrait refuser U+{:04X}",
                c as u32
            );
            // L'alias a sa propre validation, avec le même trou : il finit sur
            // la ligne `Host`, tout aussi lue par le terminal.
            assert!(
                validate_host(&SshHost {
                    alias: charge.clone(),
                    ..Default::default()
                })
                .is_err(),
                "l'alias devrait refuser U+{:04X}",
                c as u32
            );
        }
        // Rien de légitime ne doit être devenu invalide au passage.
        assert!(avec(&|h| h.hostname = Some("prod.exemple.com".into())).is_ok());
        assert!(avec(&|h| h.identity_file = Some("/home/u/ma clé".into())).is_ok());
        assert!(avec(&|h| h.folder = "Prod/Bases".into()).is_ok());
        assert!(avec(&|h| h.tags = vec!["été".into(), "bases".into()]).is_ok());
    }

    #[test]
    fn sans_controle_neutralise_une_sequence_ansi_avant_affichage() {
        // Trouvé par l'audit du 9 septembre 2026 : durcir la seule écriture
        // d'Avash ne protège pas d'un `~/.ssh/config` déjà piégé par un autre
        // outil. `avash list` imprimait alias, hôte et rebond bruts, donc
        // chaque exécution rejouait la séquence dans le terminal.
        let propre = sans_controle("srv\u{1b}]0;PWNED\u{7}");
        assert!(
            !propre.chars().any(char::is_control),
            "il reste un caractère de contrôle : {propre:?}"
        );
        assert_eq!(propre, "srv ]0;PWNED ");
        // Le texte inoffensif, accents compris, ne bouge pas.
        assert_eq!(sans_controle("prod.exemple.com"), "prod.exemple.com");
        assert_eq!(sans_controle("relais été"), "relais été");
    }

    #[test]
    fn developper_tilde_resout_dans_le_repertoire_personnel() {
        // Trouvé par l'audit du 7 septembre 2026 : la forme `~/…` d'IdentityFile
        // n'était jamais développée. `~` seul et `~/x` visent le HOME ; un chemin
        // absolu ou relatif sans tilde reste inchangé.
        let g = crate::testutil::temp_home();
        assert_eq!(developper_tilde("~/.ssh/k"), g.dir().join(".ssh").join("k"));
        assert_eq!(developper_tilde("~"), g.dir().to_path_buf());
        assert_eq!(developper_tilde("/tmp/k"), PathBuf::from("/tmp/k"));
        assert_eq!(developper_tilde("k"), PathBuf::from("k"));
        // Un tilde qui n'est pas en tête n'est pas un raccourci de HOME.
        assert_eq!(developper_tilde("/a/~/b"), PathBuf::from("/a/~/b"));
    }

    #[test]
    fn tags_lus_et_reecrits() {
        let cfg = "Host prod\n  HostName 10.0.0.1\n  #Tags: prod, web\n";
        let h = &parse_config_str(cfg)[0];
        assert_eq!(h.tags, vec!["prod", "web"]);
        // Round-trip : render puis relit les memes tags.
        let rendered = render_host_block(h);
        assert!(rendered.contains("#Tags: prod, web"), "{rendered}");
        assert_eq!(parse_config_str(&rendered)[0].tags, vec!["prod", "web"]);
    }

    #[test]
    fn tags_hors_bloc_host_ignores() {
        // Un #Tags avant tout Host ne s'attache a rien.
        let h = parse_config_str("#Tags: orphelin\nHost a\n  HostName x\n");
        assert!(h[0].tags.is_empty());
    }

    #[test]
    fn split_proxy_jump_decoupe_une_chaine() {
        let v = split_proxy_jump("bastion, deploy@10.0.0.1:2222");
        assert_eq!(v.len(), 2);
        assert_eq!(
            v[0],
            HopSpec {
                user: None,
                host: "bastion".into(),
                port: None
            }
        );
        assert_eq!(
            v[1],
            HopSpec {
                user: Some("deploy".into()),
                host: "10.0.0.1".into(),
                port: Some(2222)
            }
        );
    }

    #[test]
    fn split_proxy_jump_gere_none_et_vide() {
        assert!(split_proxy_jump("none").is_empty());
        assert!(split_proxy_jump("").is_empty());
        assert!(split_proxy_jump("  ,  ").is_empty());
    }

    // Trouvé par l'audit du 7 septembre 2026 : un bastion IPv6 littéral s'écrit
    // entre crochets (`[2001:db8::1]:2222`), la seule syntaxe qu'OpenSSH accepte
    // en ProxyJump (`hpdelim` coupe une IPv6 nue au premier `:`, « Bad
    // ProxyJump »). Le `rsplit_once(':')` gardait les crochets dans l'hôte, si
    // bien que russh recevait `"[2001:db8::1]"`, ni IP analysable ni nom
    // résolvable : le rebond échouait là où `ssh -J` marchait.
    #[test]
    fn split_proxy_jump_retire_les_crochets_ipv6() {
        let v = split_proxy_jump("u@[2001:db8::1]:2222");
        assert_eq!(v.len(), 1);
        assert_eq!(
            v[0],
            HopSpec {
                user: Some("u".into()),
                host: "2001:db8::1".into(),
                port: Some(2222)
            }
        );

        // Même adresse sans port : les crochets tombent, port `None`.
        let sans_port = split_proxy_jump("[2001:db8::1]");
        assert_eq!(
            sans_port,
            vec![HopSpec {
                user: None,
                host: "2001:db8::1".into(),
                port: None
            }]
        );

        // Un port nul derrière les crochets reste refusé : morceau sans port,
        // la résolution le dira introuvable plutôt que de viser le port 0.
        let port_nul = split_proxy_jump("[2001:db8::1]:0");
        assert_eq!(port_nul[0].host, "2001:db8::1");
        assert_eq!(port_nul[0].port, None);

        // Crochet ouvrant sans fermant (saisie malformée) : on retire quand même
        // le `[` de tête pour tenir l'invariant « pas de crochet dans l'hôte »
        // gardé par la cible fuzz et le test de mutation. La résolution refusera
        // cet hôte de toute façon.
        let non_ferme = split_proxy_jump("[2001:db8::1");
        assert_eq!(non_ferme[0].host, "2001:db8::1");
        assert_eq!(non_ferme[0].port, None);
        assert!(!non_ferme[0].host.starts_with('['));

        // Crochets imbriqués (`[[h]:22`), trouvés par cargo-fuzz après le
        // premier correctif : le premier `[` retiré, `split_once(']')` laissait
        // `[h`. Aucun crochet ne doit subsister dans l'hôte.
        for pathologique in [
            "[[h]:22",
            "[[2001:db8::1]]",
            "[]",
            "[a]b]",
            "] a",
            "[ a",
            "a ]",
            "] a ]:2",
        ] {
            for hop in split_proxy_jump(pathologique) {
                assert!(
                    !hop.host.contains(['[', ']']),
                    "crochet gardé pour {pathologique:?} : {hop:?}"
                );
                assert_eq!(
                    hop.host.trim(),
                    hop.host,
                    "hôte non rogné pour {pathologique:?} : {hop:?}"
                );
            }
        }
    }

    // Comportement documenté d'une IPv6 littérale SANS crochets : OpenSSH la
    // refuse en ProxyJump, il n'y a donc pas de résultat « correct » à viser.
    // On note ce que rend `split_proxy_jump` (découpe au dernier `:`, jamais
    // entre crochets) pour que le choix soit visible et gardé : la forme entre
    // crochets reste la seule voie vers un bastion IPv6 littéral.
    #[test]
    fn split_proxy_jump_ipv6_nue_reste_une_erreur_de_config() {
        let v = split_proxy_jump("2001:db8::1");
        // Le dernier groupe décimal (`1`) est pris pour un port : résultat
        // volontairement faux, comme une IPv6 nue l'est déjà pour OpenSSH.
        assert_eq!(v[0].host, "2001:db8:");
        assert_eq!(v[0].port, Some(1));
        // Aucun morceau ne conserve de crochet : invariant tenu même ici.
        assert!(!v[0].host.starts_with('['));
    }

    #[test]
    fn parses_basic_config() {
        let cfg = r"
# commentaire
Host web
    HostName 10.0.0.5
    User adrien
    Port 2222
    IdentityFile ~/.ssh/id_ed25519

Host db bastion
    HostName 10.0.0.9
    User root
";
        let hosts = parse_config_str(cfg);
        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[0].alias, "web");
        assert_eq!(hosts[0].port, Some(2222));
        assert_eq!(hosts[1].alias, "db");
        assert_eq!(hosts[2].alias, "bastion");
        assert_eq!(hosts[2].user, Some("root".into()));
    }

    #[test]
    fn les_blocs_a_motif_sont_absents_de_la_liste_editable() {
        // La liste éditable ne montre que des hôtes connectables : un bloc à
        // joker (`Host db*`) n'en est pas un, on ne le liste donc pas. Il n'est
        // pas ignoré pour autant — ses valeurs par défaut sont appliquées à la
        // résolution (`resoudre_hote_dans`), comme le fait `ssh`.
        let cfg = "Host db*\n  User admin\nHost prod-1\n  User root";
        let hosts = parse_config_str(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "prod-1");
    }

    #[test]
    fn les_valeurs_par_defaut_de_host_etoile_s_appliquent() {
        // Trouvé par l'audit du 7 septembre 2026 : un `User`/`IdentityFile` posé
        // dans `Host *` s'applique à chaque hôte. `parse_config_str` jetait le
        // bloc à joker, et Avash résolvait `prod` avec l'utilisateur courant et
        // sans clé, là où `ssh prod` prenait l'utilisateur et la clé du `Host *`.
        let cfg = "Host *\n  User adrien\n  IdentityFile ~/.ssh/id_ed25519\n\n\
                   Host prod\n  HostName 10.0.0.1\n";
        let h = resoudre_hote_dans(cfg, "prod").expect("prod est un hôte littéral");
        assert_eq!(h.user.as_deref(), Some("adrien"));
        assert_eq!(h.identity_file.as_deref(), Some("~/.ssh/id_ed25519"));
        assert_eq!(h.hostname.as_deref(), Some("10.0.0.1"));
        // Le tilde n'est PAS développé ici : la résolution le laisse au fichier,
        // les appelants (Target::from_alias…) appellent `developper_tilde`. Le
        // port reste vide, le défaut 22 étant appliqué par les appelants.
        assert_eq!(h.port, None);
    }

    #[test]
    fn la_premiere_valeur_obtenue_est_retenue() {
        // Ordre du fichier : `Host *` en tête pose `User root` AVANT que le bloc
        // littéral `Host prod` ne pose `User adrien`. OpenSSH retient la première
        // valeur obtenue : root l'emporte.
        let cfg = "Host *\n  User root\n\nHost prod\n  HostName 10.0.0.1\n  User adrien\n";
        assert_eq!(
            resoudre_hote_dans(cfg, "prod").unwrap().user.as_deref(),
            Some("root"),
            "première valeur = Host *"
        );
        // Bloc littéral d'abord : c'est lui qui l'emporte alors (disposition
        // recommandée par le man, `Host *` en fin de fichier).
        let cfg2 = "Host prod\n  HostName 10.0.0.1\n  User adrien\n\nHost *\n  User root\n";
        assert_eq!(
            resoudre_hote_dans(cfg2, "prod").unwrap().user.as_deref(),
            Some("adrien"),
            "le bloc littéral vient avant"
        );
    }

    #[test]
    fn la_premiere_occurrence_dans_un_meme_bloc_est_retenue() {
        // Trouvé par l'audit du 9 septembre 2026 : la règle « la première valeur
        // obtenue est retenue » n'était appliquée qu'ENTRE blocs. À l'intérieur
        // d'un même bloc, chaque directive écrasait la précédente, si bien qu'un
        // bloc issu d'une fusion manuelle (`User adrien` puis `User root`) faisait
        // afficher et connecter Avash en « root » là où `ssh prod` se connecte en
        // « adrien ». Aucun message ne signalait l'ambiguïté.
        let cfg = "Host prod\n  HostName 10.0.0.1\n  HostName 10.0.0.2\n  \
                   User adrien\n  User root\n  Port 22\n  Port 2222\n  \
                   IdentityFile ~/.ssh/premiere\n  IdentityFile ~/.ssh/seconde\n  \
                   ProxyJump bastion\n  ProxyJump autre\n";

        let liste = parse_config_str(cfg);
        assert_eq!(liste.len(), 1);
        let h = &liste[0];
        assert_eq!(h.hostname.as_deref(), Some("10.0.0.1"), "HostName dupliqué");
        assert_eq!(h.user.as_deref(), Some("adrien"), "User dupliqué");
        assert_eq!(h.port, Some(22), "Port dupliqué");
        assert_eq!(
            h.identity_file.as_deref(),
            Some("~/.ssh/premiere"),
            "IdentityFile dupliqué"
        );
        assert_eq!(
            h.proxy_jump.as_deref(),
            Some("bastion"),
            "ProxyJump dupliqué"
        );

        // Même règle à la résolution : les deux chemins partagent `blocs_bruts`.
        let r = resoudre_hote_dans(cfg, "prod").unwrap();
        assert_eq!(r.hostname.as_deref(), Some("10.0.0.1"));
        assert_eq!(r.user.as_deref(), Some("adrien"));
        assert_eq!(r.port, Some(22));
        assert_eq!(r.identity_file.as_deref(), Some("~/.ssh/premiere"));
        assert_eq!(r.proxy_jump.as_deref(), Some("bastion"));

        // Un port refusé par OpenSSH (`Port 0`, « Bad port ») ne compte pas comme
        // une première valeur : Avash l'ignore, et le port suivant valide sert.
        // L'ordre inverse est le cas qui discrimine : sans la règle « premier
        // gagnant », le `Port 0` final effaçait le 2222 déjà lu.
        let cfg_port_nul = "Host prod\n  HostName 10.0.0.1\n  Port 0\n  Port 2222\n";
        assert_eq!(
            resoudre_hote_dans(cfg_port_nul, "prod").unwrap().port,
            Some(2222)
        );
        let cfg_port_nul_apres = "Host prod\n  HostName 10.0.0.1\n  Port 2222\n  Port 0\n";
        assert_eq!(
            resoudre_hote_dans(cfg_port_nul_apres, "prod").unwrap().port,
            Some(2222)
        );
    }

    #[test]
    fn une_valeur_vide_ne_compte_pas_comme_premiere_valeur() {
        // Trouvé à la relecture de l'audit du 9 septembre 2026 : la règle
        // « premier gagnant » posée juste au-dessus faisait, appliquée sans
        // nuance, qu'un résidu de fusion manuelle (`HostName` sans argument,
        // `User ""`) masquait la vraie valeur écrite juste en dessous. Avash
        // aurait visé une adresse vide, que rien ne rattrape en aval : le
        // binaire fait `hostname.unwrap_or(alias)`, qui laisse passer `""`.
        let cfg = "Host prod\n  HostName\n  HostName 10.0.0.1\n  \
                   User \"\"\n  User adrien\n  IdentityFile\n  \
                   IdentityFile ~/.ssh/prod\n  ProxyJump \"\"\n  ProxyJump bastion\n";

        let h = resoudre_hote_dans(cfg, "prod").unwrap();
        assert_eq!(h.hostname.as_deref(), Some("10.0.0.1"), "HostName vide");
        assert_eq!(h.user.as_deref(), Some("adrien"), "User vide");
        assert_eq!(
            h.identity_file.as_deref(),
            Some("~/.ssh/prod"),
            "IdentityFile vide"
        );
        assert_eq!(h.proxy_jump.as_deref(), Some("bastion"), "ProxyJump vide");

        // Seule : une directive vide laisse le champ absent, jamais `Some("")`.
        // Le repli sur l'alias peut alors jouer.
        let seule = parse_config_str("Host prod\n  HostName\n  User \"\"\n");
        assert_eq!(seule[0].hostname, None);
        assert_eq!(seule[0].user, None);
    }

    #[test]
    fn la_premiere_convention_avash_du_bloc_est_retenue() {
        // Même audit : les conventions `#Tags:`/`#Folder:` restaient en
        // dernier-gagne dans un bloc alors que la résolution leur applique le
        // premier-gagne entre blocs. Un bloc recollé à la main affichait donc
        // l'étiquette du morceau collé, pas celle d'origine.
        let cfg = "Host prod\n  #Tags: prod, linux\n  #Tags: brouillon\n  \
                   #Folder: Client/Prod\n  #Folder: Corbeille\n  HostName 10.0.0.1\n";
        let h = &parse_config_str(cfg)[0];
        assert_eq!(h.tags, vec!["prod".to_string(), "linux".to_string()]);
        assert_eq!(h.folder, "Client/Prod");

        // Une liste vide ne compte pas comme première valeur non plus.
        let vide = "Host prod\n  #Tags:\n  #Tags: prod\n  #Folder:\n  \
                    #Folder: Client\n  HostName 10.0.0.1\n";
        let h = &parse_config_str(vide)[0];
        assert_eq!(h.tags, vec!["prod".to_string()]);
        assert_eq!(h.folder, "Client");
    }

    #[test]
    fn un_motif_de_negation_annule_le_bloc() {
        // `Host !prod *` : le `!prod` matche `prod` et annule tout le bloc, donc
        // `prod` n'hérite pas de son `User`. Un autre hôte, lui, en hérite.
        let cfg = "Host !prod *\n  User root\n\nHost prod\n  HostName 10.0.0.1\n\n\
                   Host web\n  HostName 10.0.0.2\n";
        assert_eq!(resoudre_hote_dans(cfg, "prod").unwrap().user, None);
        assert_eq!(
            resoudre_hote_dans(cfg, "web").unwrap().user.as_deref(),
            Some("root")
        );
    }

    #[test]
    fn un_alias_inconnu_ne_se_resout_pas() {
        // Sans bloc littéral, ce n'est pas un hôte connu d'Avash, même si
        // `Host *` le couvrirait : `resoudre_hote` rend alors `None`.
        let cfg = "Host *\n  User root\n";
        assert!(resoudre_hote_dans(cfg, "inexistant").is_none());
    }
}

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
mod tests_ecriture_atomique {
    use super::ecrire_atomiquement;
    use crate::testutil::temp_home;

    /// Le contenu doit être intégralement lisible, et le fichier ne doit jamais
    /// avoir été lisible par un autre compte — le temporaire naissait avec
    /// l'umask et n'était resserré qu'après le renommage.
    #[test]
    fn le_fichier_ecrit_est_complet_et_prive() {
        let home = temp_home();
        let cible = home.dir().join("secrets.yaml");
        ecrire_atomiquement(&cible, b"contenu complet\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&cible).unwrap(),
            "contenu complet\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&cible).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "lisible par d'autres comptes : {mode:o}");
        }
    }

    /// Réécrire remplace le contenu sans laisser d'intermédiaire : aucun
    /// résidu `.tmp` ne doit subsister dans le répertoire.
    #[test]
    fn la_reecriture_ne_laisse_aucun_residu() {
        let home = temp_home();
        let cible = home.dir().join("liste.yaml");
        ecrire_atomiquement(&cible, b"premier").unwrap();
        ecrire_atomiquement(&cible, b"second").unwrap();
        assert_eq!(std::fs::read_to_string(&cible).unwrap(), "second");
        let restants: Vec<_> = std::fs::read_dir(home.dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            restants,
            vec!["liste.yaml".to_owned()],
            "résidu : {restants:?}"
        );
    }

    /// Le répertoire manquant est créé, et en 0700 : `~/.config/avash` naissait
    /// lui aussi avec l'umask.
    #[test]
    fn le_repertoire_absent_est_cree_et_prive() {
        let home = temp_home();
        let cible = home.dir().join("neuf/sous/fichier.yaml");
        ecrire_atomiquement(&cible, b"x").unwrap();
        assert!(cible.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(cible.parent().unwrap())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "répertoire ouvert : {mode:o}");
        }
    }

    /// Un répertoire qui existait déjà garde ses droits : on le resserrait à
    /// 0700 comme s'il venait d'être créé. Vu le 2026-09-03 quand la suite,
    /// lancée en root, a passé /tmp en 0700 par les cas qui y écrivent
    /// directement ; un compte ordinaire subissait la même chose sur ses
    /// propres répertoires.
    #[test]
    #[cfg(unix)]
    fn un_repertoire_existant_garde_ses_droits() {
        use std::os::unix::fs::PermissionsExt;
        let home = temp_home();
        let partage = home.dir().join("partage");
        std::fs::create_dir(&partage).unwrap();
        std::fs::set_permissions(&partage, std::fs::Permissions::from_mode(0o755)).unwrap();
        ecrire_atomiquement(&partage.join("export.yaml"), b"x").unwrap();
        let mode = std::fs::metadata(&partage).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "répertoire existant resserré : {mode:o}");
    }

    /// Les répertoires d'Avash, eux, sont resserrés même s'ils existaient
    /// déjà : `~/.config/avash` et `~/.ssh` naissaient avec l'umask, souvent
    /// lisibles par tous, et c'est ce que le correctif précédent ne doit pas
    /// défaire.
    #[test]
    #[cfg(unix)]
    fn les_repertoires_d_avash_sont_resserres_meme_existants() {
        use std::os::unix::fs::PermissionsExt;
        let home = temp_home();
        let ouverts = std::fs::Permissions::from_mode(0o755);
        let config = crate::repertoire_configuration().unwrap().join("avash");
        let ssh = home.dir().join(".ssh");
        for (dir, fichier) in [(&config, "folders.yaml"), (&ssh, "config")] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::set_permissions(dir, ouverts.clone()).unwrap();
            ecrire_atomiquement(&dir.join(fichier), b"x").unwrap();
            let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{} non resserré : {mode:o}", dir.display());
        }
    }

    /// Un parent qui est un fichier fait échouer `create_dir_all` : on remonte
    /// une erreur avant même de créer un temporaire.
    ///
    /// Trouvé par l'audit du 7 septembre 2026 : ce cas s'appelait
    /// `un_echec_ne_laisse_pas_de_temporaire` mais n'affirmait que `is_err()` et,
    /// échouant dans `create_dir_all` avant `options.open(&tmp)`, ne touchait
    /// jamais au nettoyage `remove_file` des branches d'écriture/renommage. Il est
    /// renommé d'après ce qu'il vérifie vraiment ; l'absence de temporaire est
    /// gardée par `un_renommage_impossible_ne_laisse_pas_de_temporaire`.
    #[test]
    fn un_parent_qui_est_un_fichier_remonte_une_erreur() {
        let home = temp_home();
        let obstacle = home.dir().join("obstacle");
        std::fs::write(&obstacle, b"je suis un fichier").unwrap();
        // « obstacle » est un fichier : on ne peut pas en faire un répertoire.
        let cible = obstacle.join("dedans.yaml");
        assert!(ecrire_atomiquement(&cible, b"x").is_err());
    }

    /// Le renommage sur un répertoire existant échoue APRÈS création du
    /// temporaire : c'est la seule branche où `remove_file` (le nettoyage sur
    /// échec de renommage) compte, et le test précédent ne l'atteignait pas.
    ///
    /// Trouvé par l'audit du 7 septembre 2026 : aucun test ne gardait ce
    /// nettoyage ; le retirer laissait un `<nom>.tmp<pid>.<n>` orphelin (fichier
    /// 0600 abandonné dans `~/.ssh` ou `~/.config/avash`) sans qu'aucune suite ne
    /// le dise. On vérifie d'abord qu'on est bien tombé dans la branche de
    /// renommage — sans quoi une régression future qui ferait échouer plus tôt
    /// (garde lecture seule étendu aux répertoires, par exemple) rendrait ce test
    /// vert pour la mauvaise raison.
    #[test]
    fn un_renommage_impossible_ne_laisse_pas_de_temporaire() {
        let home = temp_home();
        let dir = home.dir().join("cible-est-un-dossier");
        std::fs::create_dir(&dir).unwrap();
        // Cible = un répertoire existant : le temporaire est créé, puis
        // `rename(tmp, dir)` échoue (EISDIR) — la branche à garder.
        let e = ecrire_atomiquement(&dir, b"x").unwrap_err().to_string();
        assert!(e.contains("Renommage vers"), "échec trop tôt : {e}");
        // Le nom du temporaire suit `with_extension` : pour une cible sans
        // extension, `cible-est-un-dossier.tmp<pid>.<n>`, d'où le `contains`.
        let restants: Vec<_> = std::fs::read_dir(home.dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            restants.iter().all(|n| !n.contains(".tmp")),
            "temporaire orphelin : {restants:?}"
        );
    }
}

#[cfg(test)]
mod save_tests {
    use super::*;

    /// Le commentaire de dossier se pose juste après la dernière directive du
    /// bloc, avant ses lignes vides de fin (mutants survivants : `!` de la
    /// recherche de la dernière ligne pleine, et `i + 1` devenu `i * 1`).
    #[test]
    fn set_host_folder_se_pose_apres_la_derniere_directive() {
        let dir = std::env::temp_dir().join(format!("avash-sf2-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        std::fs::write(
            &path,
            "Host prod\n    HostName 10.0.0.1\n\n\nHost autre\n    HostName 10.0.0.2\n",
        )
        .unwrap();
        set_host_folder_at(&path, "prod", "x").unwrap();
        let t = std::fs::read_to_string(&path).unwrap();
        assert!(
            t.contains("    HostName 10.0.0.1\n    #Folder: x\n\n"),
            "commentaire mal placé : {t:?}"
        );
        assert!(t.starts_with("Host prod\n    HostName 10.0.0.1\n"), "{t:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_host_folder_preserve_les_autres_directives() {
        let dir = std::env::temp_dir().join(format!("avash-sf-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        std::fs::write(
            &path,
            "Host prod
    HostName 10.0.0.1
    ForwardAgent yes

Host autre
    HostName 10.0.0.2
",
        )
        .unwrap();
        // Ranger « prod » dans prod/web : la directive custom reste, le folder est posé.
        set_host_folder_at(&path, "prod", "prod/web").unwrap();
        let t = std::fs::read_to_string(&path).unwrap();
        assert!(t.contains("ForwardAgent yes"), "directive perdue : {t}");
        assert!(t.contains("#Folder: prod/web"), "folder absent : {t}");
        // Le bloc « autre » n'est pas touché.
        assert!(
            !t.contains(
                "Host autre
    HostName 10.0.0.2
    #Folder"
            ),
            "{t}"
        );
        // Re-déplacer remplace (pas de doublon), et vider retire la ligne.
        set_host_folder_at(&path, "prod", "").unwrap();
        let t2 = std::fs::read_to_string(&path).unwrap();
        assert!(!t2.contains("#Folder"), "folder non retiré : {t2}");
        assert!(t2.contains("ForwardAgent yes"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_host_folder_refuse_une_injection_par_saut_de_ligne() {
        let dir = std::env::temp_dir().join(format!("avash-inj-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        std::fs::write(&path, "Host prod\n    HostName 10.0.0.1\n").unwrap();
        // Un dossier contenant un saut de ligne tenterait d'injecter une directive.
        let r = set_host_folder_at(&path, "prod", "web\n    ProxyCommand nc evil 22");
        assert!(r.is_err(), "l'injection aurait dû être refusée");
        let t = std::fs::read_to_string(&path).unwrap();
        assert!(!t.contains("ProxyCommand"), "directive injectée : {t}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn host(alias: &str) -> SshHost {
        SshHost {
            alias: alias.into(),
            hostname: Some("10.0.0.7".into()),
            user: Some("adrien".into()),
            port: Some(2222),
            identity_file: Some("/home/a/.ssh/id_ed25519".into()),
            ..Default::default()
        }
    }

    #[test]
    fn le_bloc_rendu_est_relu_a_l_identique() {
        // Boucle complete : ce qu'on ecrit doit etre relisible par le parseur.
        let h = host("prod");
        let relu = parse_config_str(&render_host_block(&h));
        assert_eq!(relu.len(), 1);
        assert_eq!(relu[0].alias, "prod");
        assert_eq!(relu[0].hostname.as_deref(), Some("10.0.0.7"));
        assert_eq!(relu[0].user.as_deref(), Some("adrien"));
        assert_eq!(relu[0].port, Some(2222));
        assert_eq!(
            relu[0].identity_file.as_deref(),
            Some("/home/a/.ssh/id_ed25519")
        );
    }

    #[test]
    fn le_port_par_defaut_n_est_pas_ecrit() {
        // Ecrire « Port 22 » partout alourdit le fichier pour rien.
        let mut h = host("simple");
        h.port = Some(22);
        assert!(
            !render_host_block(&h).contains("Port"),
            "{}",
            render_host_block(&h)
        );
    }

    #[test]
    fn les_champs_vides_sont_omis() {
        let h = SshHost {
            alias: "minimal".into(),
            hostname: Some("  ".into()),
            user: None,
            ..Default::default()
        };
        let bloc = render_host_block(&h);
        assert_eq!(bloc.trim(), "Host minimal", "bloc : {bloc:?}");
        // Une clé ou un rebond faits d'espaces ne donnent pas de directive vide
        // (mutant survivant : le filtre `!v.trim().is_empty()` du ProxyJump).
        let h = SshHost {
            alias: "blancs".into(),
            identity_file: Some("   ".into()),
            proxy_jump: Some(" \t".into()),
            ..Default::default()
        };
        let bloc = render_host_block(&h);
        assert_eq!(bloc.trim(), "Host blancs", "bloc : {bloc:?}");
    }

    #[test]
    fn un_alias_avec_saut_de_ligne_est_refuse() {
        // Sans ce garde-fou on injecte n'importe quelle directive dans la
        // configuration SSH — ProxyCommand comprise.
        for mechant in [
            "prod\n    ProxyCommand nc evil.example 22",
            "prod\rHost *",
            "prod\0",
        ] {
            assert!(
                validate_alias(mechant).is_err(),
                "devrait etre refuse : {mechant:?}"
            );
        }
    }

    #[test]
    fn append_host_refuse_une_injection_de_directive_dans_les_champs() {
        // Regression securite : un saut de ligne dans HostName/User/
        // IdentityFile injecterait une directive arbitraire (ex. ProxyCommand,
        // execute par ssh a la connexion). Seul l'alias etait protege.
        let _g = crate::testutil::temp_home();
        for bad in [
            SshHost {
                alias: "srv".into(),
                hostname: Some("1.2.3.4\n    ProxyCommand evil".into()),
                ..Default::default()
            },
            SshHost {
                alias: "srv".into(),
                user: Some("root\nProxyCommand evil".into()),
                ..Default::default()
            },
            SshHost {
                alias: "srv".into(),
                identity_file: Some("/k\r  ProxyCommand evil".into()),
                ..Default::default()
            },
        ] {
            assert!(append_host(&bad).is_err(), "doit refuser : {bad:?}");
        }
        // Un hote propre passe toujours.
        assert!(append_host(&SshHost {
            alias: "ok".into(),
            hostname: Some("10.0.0.1".into()),
            ..Default::default()
        })
        .is_ok());
    }

    #[test]
    fn un_alias_joker_est_refuse() {
        // « Host * » s'appliquerait a TOUTES les connexions de la machine.
        for mechant in ["*", "prod*", "?", "!prod"] {
            assert!(
                validate_alias(mechant).is_err(),
                "devrait etre refuse : {mechant}"
            );
        }
    }

    #[test]
    fn un_alias_avec_espace_ou_vide_est_refuse() {
        assert!(validate_alias("").is_err());
        assert!(validate_alias("mon serveur").is_err());
    }

    #[test]
    fn un_alias_normal_passe() {
        for bon in ["prod", "prod-web", "serveur_1", "10.0.0.5"] {
            assert!(validate_alias(bon).is_ok(), "devrait passer : {bon}");
        }
    }
    // ---------- Ecriture reelle dans ~/.ssh/config ----------

    use crate::testutil::temp_home;

    #[test]
    fn append_host_cree_le_fichier_et_le_relit() {
        let _h = temp_home();
        append_host(&host("neuf")).unwrap();
        let relu = parse_ssh_config().unwrap();
        assert_eq!(relu.len(), 1);
        assert_eq!(relu[0].alias, "neuf");
    }

    #[test]
    fn append_host_enregistre_un_rebond_a_plusieurs_sauts() {
        // Trouvé par l'audit du 9 septembre 2026 : le scénario complet, celui
        // que l'interface propose dans son propre exemple de champ ProxyJump.
        // L'enregistrement échouait avant même d'écrire quoi que ce soit.
        let _h = temp_home();
        let mut h = host("prod");
        h.proxy_jump = Some("bastion, relais:2200".into());
        append_host(&h).unwrap();
        let relu = parse_ssh_config().unwrap();
        assert_eq!(
            relu[0].proxy_jump.as_deref(),
            Some("bastion, relais:2200"),
            "le rebond doit se relire tel quel"
        );
    }

    #[test]
    fn append_host_preserve_le_contenu_existant() {
        // Le risque majeur : abimer une configuration que l'utilisateur a
        // ecrite a la main, commentaires compris.
        let _h = temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let avant = "# Ma config perso\nHost ancien\n    HostName 1.2.3.4\n";
        std::fs::write(&path, avant).unwrap();

        append_host(&host("nouveau")).unwrap();

        let apres = std::fs::read_to_string(&path).unwrap();
        assert!(
            apres.starts_with(avant),
            "le contenu d'origine doit rester intact :\n{apres}"
        );
        assert!(apres.contains("# Ma config perso"), "commentaire perdu");

        let relu = parse_ssh_config().unwrap();
        let noms: Vec<_> = relu.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(noms, vec!["ancien", "nouveau"]);
    }

    #[test]
    fn append_host_separe_les_blocs_par_une_ligne_vide() {
        // Sans separation, `Host` se colle a la directive precedente et en
        // devient une sous-directive : le nouvel hote serait invisible.
        let _h = temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "Host a\n    HostName 1.1.1.1").unwrap(); // sans \n final
        append_host(&host("b")).unwrap();
        assert_eq!(parse_ssh_config().unwrap().len(), 2);
    }

    /// Un alias déclaré dans un fichier inclus doit être refusé lui aussi.
    ///
    /// Sans cela on ajoutait un second bloc pour le même alias : OpenSSH retenant
    /// la première occurrence, l'hôte semblait ne plus répondre aux
    /// modifications, et la liste affichait deux entrées identiques.
    /// La ligne vide avant un nouveau bloc n'est mise que s'il en faut une :
    /// aucune sur un fichier vide, une seule après un bloc, aucune de plus
    /// après une ligne vide déjà là. Mutants survivants : `&&` devenu `||`
    /// (ligne vide en tête d'un fichier vide) et le `!` de `ends_with('\n')`
    /// (un bloc collé au précédent, ou deux lignes vides).
    #[test]
    fn append_host_ne_met_de_ligne_vide_que_s_il_en_faut() {
        let _h = temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        for (avant, attendu) in [
            ("", "Host b\n"),
            (
                "Host a\n    HostName 1\n",
                "Host a\n    HostName 1\n\nHost b\n",
            ),
            (
                "Host a\n    HostName 1",
                "Host a\n    HostName 1\n\nHost b\n",
            ),
            (
                "Host a\n    HostName 1\n\n",
                "Host a\n    HostName 1\n\nHost b\n",
            ),
        ] {
            std::fs::write(&path, avant).unwrap();
            append_host(&host("b")).unwrap();
            let apres = std::fs::read_to_string(&path).unwrap();
            assert!(
                apres.starts_with(attendu),
                "avant {avant:?} : attendu {attendu:?}, obtenu {apres:?}"
            );
        }
    }

    #[test]
    fn append_host_refuse_une_config_non_lisible_au_lieu_de_l_abimer() {
        // Trouvé par l'audit du 7 septembre 2026 : un `~/.ssh/config` non UTF-8
        // (commentaire Latin-1 `# R\xe9seau` d'un vieil éditeur) et sans saut de
        // ligne final. Avec l'ancien `unwrap_or_default()`, `existing` devenait
        // vide : le contrôle d'unicité ne voyait plus l'alias `a` (doublon
        // possible) et le bloc `Host b` se soudait à `IdentityFile ~/.ssh/k`,
        // cassant la directive pour OpenSSH. `append_host` doit refuser et
        // laisser le fichier strictement intact.
        let _h = temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let octets = b"# R\xe9seau\nHost a\n    IdentityFile ~/.ssh/k";
        std::fs::write(&path, octets).unwrap();
        let e = append_host(&host("b")).unwrap_err().to_string();
        assert!(e.contains("Lecture de"), "message inattendu : {e}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            octets,
            "le fichier illisible a été modifié"
        );
    }

    #[test]
    fn append_host_cree_le_fichier_absent_sans_erreur() {
        // Garde-fou du correctif ci-dessus : seul `NotFound` vaut `""`. Sur un
        // fichier absent (cas normal du premier hôte), l'écriture doit réussir.
        let _h = temp_home();
        let path = ssh_config_path();
        assert!(!path.exists());
        append_host(&host("premier")).unwrap();
        assert_eq!(parse_ssh_config().unwrap().len(), 1);
    }

    #[test]
    fn append_host_voit_les_alias_declares_dans_un_include() {
        let _h = temp_home();
        let ssh = repertoire_personnel().unwrap().join(".ssh");
        std::fs::create_dir_all(ssh.join("config.d")).unwrap();
        std::fs::write(
            ssh.join("config.d").join("10-prod"),
            "Host venu-d-un-include\n    HostName 10.0.0.9\n",
        )
        .unwrap();
        std::fs::write(ssh.join("config"), "Include config.d/*\n").unwrap();

        let e = append_host(&host("venu-d-un-include"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("déjà déclaré"), "{e}");
    }

    #[test]
    fn append_host_refuse_un_alias_deja_present() {
        let _h = temp_home();
        append_host(&host("double")).unwrap();
        let e = append_host(&host("double")).unwrap_err().to_string();
        assert!(e.contains("déjà déclaré"), "{e}");
        // Insensible a la casse : OpenSSH l'est aussi.
        let mut autre = host("DOUBLE");
        autre.alias = "DOUBLE".into();
        assert!(append_host(&autre).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn append_host_pose_les_droits_attendus() {
        use std::os::unix::fs::PermissionsExt;
        let _h = temp_home();
        append_host(&host("droits")).unwrap();
        let path = ssh_config_path();
        let m = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o600, "config SSH lisible par d'autres");
        let d = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(d, 0o700, "~/.ssh trop ouvert");
    }

    // ---------- Match ----------

    #[test]
    fn un_bloc_match_ne_contamine_pas_l_hote_precedent() {
        // Regression : `Match` n'etait pas reconnu comme delimiteur, donc ses
        // directives etaient appliquees au dernier Host. Un `Match exec`
        // jamais satisfait pouvait ainsi changer l'utilisateur et le port
        // d'un hote reel — sans le moindre avertissement.
        let cfg = "Host prod\n  HostName 10.0.0.1\n  User root\n\n                   Match exec \"test -f /tmp/jamais\"\n  User compromis\n  Port 9999\n";
        let hosts = parse_config_str(cfg);
        assert_eq!(hosts.len(), 1, "seul `prod` est un hote : {hosts:?}");
        assert_eq!(
            hosts[0].user.as_deref(),
            Some("root"),
            "utilisateur contamine"
        );
        assert_eq!(hosts[0].port, None, "port contamine");
    }

    #[test]
    fn un_host_apres_un_match_est_bien_lu() {
        let cfg = "Match user root\n  ForwardAgent yes\n\nHost apres\n  HostName 1.2.3.4\n";
        let hosts = parse_config_str(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "apres");
        assert_eq!(hosts[0].hostname.as_deref(), Some("1.2.3.4"));
    }

    // ---------- Include ----------

    #[test]
    fn include_absolu_est_resolu() {
        let _h = crate::testutil::temp_home();
        let dir = ssh_config_path().parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let inc = dir.join("perso");
        std::fs::write(&inc, "Host inclus\n  HostName 5.5.5.5\n").unwrap();
        std::fs::write(
            ssh_config_path(),
            format!(
                "Host principal\n  HostName 1.1.1.1\n\nInclude {}\n",
                inc.display()
            ),
        )
        .unwrap();

        let noms: Vec<_> = parse_ssh_config()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert!(noms.contains(&"principal".to_string()), "{noms:?}");
        assert!(
            noms.contains(&"inclus".to_string()),
            "l'hote inclus doit apparaitre : {noms:?}"
        );
    }

    #[test]
    fn include_relatif_part_de_ssh() {
        // OpenSSH resout les chemins relatifs depuis ~/.ssh.
        let _h = crate::testutil::temp_home();
        let dir = ssh_config_path().parent().unwrap().to_path_buf();
        std::fs::create_dir_all(dir.join("config.d")).unwrap();
        std::fs::write(
            dir.join("config.d/dix"),
            "Host relatif\n  HostName 9.9.9.9\n",
        )
        .unwrap();
        std::fs::write(ssh_config_path(), "Include config.d/dix\n").unwrap();

        let noms: Vec<_> = parse_ssh_config()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert_eq!(noms, vec!["relatif"], "{noms:?}");
    }

    #[test]
    fn include_avec_motif_prend_tous_les_fichiers_en_ordre() {
        let _h = crate::testutil::temp_home();
        let dir = ssh_config_path().parent().unwrap().to_path_buf();
        std::fs::create_dir_all(dir.join("config.d")).unwrap();
        std::fs::write(dir.join("config.d/10-a"), "Host aaa\n  HostName 1.1.1.1\n").unwrap();
        std::fs::write(dir.join("config.d/20-b"), "Host bbb\n  HostName 2.2.2.2\n").unwrap();
        std::fs::write(ssh_config_path(), "Include config.d/*\n").unwrap();

        let noms: Vec<_> = parse_ssh_config()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert_eq!(
            noms,
            vec!["aaa", "bbb"],
            "ordre lexicographique attendu : {noms:?}"
        );
    }

    #[test]
    fn include_manquant_est_ignore_sans_planter() {
        // OpenSSH tolere un Include qui ne correspond a rien ; une config
        // partielle vaut mieux qu'aucune.
        let _h = crate::testutil::temp_home();
        let dir = ssh_config_path().parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            ssh_config_path(),
            "Include /rien/du/tout\nHost seul\n  HostName 1.1.1.1\n",
        )
        .unwrap();
        let noms: Vec<_> = parse_ssh_config()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert_eq!(noms, vec!["seul"], "{noms:?}");
    }

    #[test]
    fn include_circulaire_ne_boucle_pas() {
        // Deux fichiers qui s'incluent mutuellement : borne a 16 niveaux,
        // comme OpenSSH. Sans borne, le parseur ne rendrait jamais la main.
        let _h = crate::testutil::temp_home();
        let dir = ssh_config_path().parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("boucle"),
            "Include config\nHost cycle\n  HostName 3.3.3.3\n",
        )
        .unwrap();
        std::fs::write(ssh_config_path(), "Include boucle\n").unwrap();
        let hosts = parse_ssh_config().unwrap();
        assert!(hosts.iter().any(|h| h.alias == "cycle"), "{hosts:?}");
    }

    #[test]
    fn glob_match_gere_etoile_et_point_interrogation() {
        assert!(glob_match("*", "quoi-que-ce-soit"));
        assert!(glob_match("10-*", "10-web"));
        assert!(glob_match("*.conf", "prod.conf"));
        assert!(glob_match("config?", "config1"));
        assert!(!glob_match("config?", "config12"));
        assert!(!glob_match("10-*", "20-web"));
        assert!(glob_match("a*b*c", "axxbyyc"));
        // Une étoile en fin de nom : rien à consommer, et rien ne déborde
        // (mutant survivant : `&&` devenu `||` faisait indexer un nom vide).
        assert!(glob_match("a*", "a"));
        assert!(!glob_match("a*b", "a"));
    }

    /// Trouvé par l'audit du 7 septembre 2026 : `glob_match` faisait le même
    /// retour arrière exponentiel que les moteurs de motif naïfs. Un motif
    /// d'`Include` à plusieurs étoiles (`conf.d/*a*a*a…*b`) confronté à un long
    /// nom de fichier sans correspondance (`aaaa…a` dans `~/.ssh` ou un `conf.d`)
    /// faisait exploser le temps et figeait `parse_ssh_config` à chaque
    /// rafraîchissement de la liste d'hôtes. Le balayage itératif à un seul point
    /// de retour rend en O(n*m) : on borne ici à une seconde, alors que la
    /// version récursive n'en finissait pas.
    #[test]
    fn glob_match_ne_part_pas_en_retour_arriere_exponentiel() {
        let motif = format!("{}b", "*a".repeat(30));
        let nom = "a".repeat(60);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(glob_match(&motif, &nom));
        });
        match rx.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(r) => assert!(!r, "le motif ne doit pas correspondre au nom"),
            Err(e) => {
                panic!("glob_match n'a pas rendu la main en une seconde ({e}) : retour arrière exponentiel")
            }
        }
    }

    // ---------- remove_host ----------

    /// Un bloc `Match` qui suit l'hôte retiré termine le bloc à retirer et
    /// reste entier ; les directives de l'hôte retiré, elles, partent toutes
    /// (mutant survivant : `key == "match"` devenu `!=`, qui gardait les
    /// directives dès la première ligne).
    #[test]
    fn remove_host_s_arrete_au_bloc_match_qui_suit() {
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "Host prod\n    HostName 1.1.1.1\n    User root\n\nMatch host x\n    User u\n\nHost b\n    HostName 2.2.2.2\n",
        )
        .unwrap();
        remove_host("prod").unwrap();
        let apres = std::fs::read_to_string(&path).unwrap();
        assert!(
            !apres.contains("1.1.1.1") && !apres.contains("User root"),
            "{apres}"
        );
        assert!(apres.contains("Match host x\n    User u"), "{apres}");
        assert!(apres.contains("Host b\n    HostName 2.2.2.2"), "{apres}");
    }

    #[test]
    fn remove_host_supprime_le_bon_bloc_et_garde_le_reste() {
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "# entete perso\nHost prod\n  HostName 1.1.1.1\n\nHost staging\n  HostName 2.2.2.2\n",
        )
        .unwrap();

        remove_host("prod").unwrap();

        let apres = std::fs::read_to_string(&path).unwrap();
        assert!(
            apres.contains("# entete perso"),
            "commentaire perdu : {apres}"
        );
        assert!(
            !apres.contains("prod"),
            "prod aurait du disparaitre : {apres}"
        );
        assert!(apres.contains("staging"), "staging efface a tort : {apres}");
        let noms: Vec<_> = parse_ssh_config()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert_eq!(noms, vec!["staging"]);
    }

    #[test]
    fn remove_host_ajoute_puis_retire_revient_a_l_etat_initial() {
        let _h = crate::testutil::temp_home();
        append_host(&host("temporaire")).unwrap();
        assert_eq!(parse_ssh_config().unwrap().len(), 1);
        remove_host("temporaire").unwrap();
        assert_eq!(parse_ssh_config().unwrap().len(), 0);
    }

    #[test]
    fn remove_host_signale_un_alias_absent() {
        let _h = crate::testutil::temp_home();
        append_host(&host("existe")).unwrap();
        let e = remove_host("absent").unwrap_err().to_string();
        assert!(e.contains("introuvable"), "{e}");
    }

    #[test]
    fn remove_host_ne_touche_pas_un_bloc_a_alias_multiples() {
        // `Host prod backup` partage des directives : retirer « prod » ne doit
        // pas casser « backup ». On laisse le bloc entier plutot que d'abimer
        // l'autre alias.
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "Host prod backup\n  User root\n").unwrap();
        let e = remove_host("prod").unwrap_err().to_string();
        // Trouvé par l'audit du 7 septembre 2026 : le refus est délibéré, mais le
        // message disait « introuvable » alors que « prod » est bien listé. Il
        // doit maintenant dire « plusieurs alias » et nommer le bloc.
        assert!(e.contains("plusieurs alias"), "{e}");
        assert!(e.contains("Host prod backup"), "{e}");
        assert!(!e.contains("introuvable"), "{e}");
    }

    #[test]
    fn update_host_dit_plusieurs_alias_sur_un_bloc_a_noms_multiples() {
        // Même contre-vérité qu'au glisser-déposer : éditer « prod » depuis le
        // menu contextuel donnait « introuvable » pour un bloc `Host prod backup`.
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "Host prod backup\n  User root\n").unwrap();
        let e = update_host("prod", &host("prod")).unwrap_err().to_string();
        assert!(e.contains("plusieurs alias"), "{e}");
        assert!(e.contains("Host prod backup"), "{e}");
        assert!(!e.contains("introuvable"), "{e}");
    }

    // Vrai si aucun `\n` du texte n'est « nu » (non précédé de `\r`) : la marque
    // qu'un fichier CRLF n'a pas été partiellement converti en LF.
    fn aucun_lf_nu(s: &str) -> bool {
        let b = s.as_bytes();
        b.iter()
            .enumerate()
            .all(|(i, &c)| c != b'\n' || (i > 0 && b[i - 1] == b'\r'))
    }

    #[test]
    fn un_fichier_crlf_reste_en_crlf_apres_suppression() {
        // Trouvé par l'audit du 7 septembre 2026 : `content.lines()` retire les
        // `\r\n` et `remove_host` réémettait en `\n`, convertissant tout un
        // `~/.ssh/config` CRLF (Bloc-notes, dotfiles versionnés sous Windows) en
        // LF au premier retrait — `git diff` de toutes les lignes au lieu d'une.
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "Host a\r\n  HostName 1\r\n\r\nHost b\r\n  HostName 2\r\n",
        )
        .unwrap();
        remove_host("a").unwrap();
        let apres = std::fs::read_to_string(&path).unwrap();
        assert!(aucun_lf_nu(&apres), "LF nu introduit : {apres:?}");
        assert!(apres.contains("Host b\r\n  HostName 2\r\n"), "{apres:?}");
        assert!(!apres.contains("Host a"), "{apres:?}");
    }

    #[test]
    fn un_fichier_crlf_reste_en_crlf_apres_edition() {
        // Même conversion silencieuse par `update_host` : éditer un hôte d'un
        // fichier CRLF ne doit toucher que son bloc, pas les fins de ligne du reste.
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "Host a\r\n  HostName 1\r\n\r\nHost b\r\n  HostName 2\r\n",
        )
        .unwrap();
        update_host("a", &host("a")).unwrap();
        let apres = std::fs::read_to_string(&path).unwrap();
        assert!(aucun_lf_nu(&apres), "LF nu introduit : {apres:?}");
        assert!(apres.contains("Host b\r\n  HostName 2\r\n"), "{apres:?}");
    }

    #[test]
    fn un_fichier_crlf_reste_en_crlf_apres_rangement() {
        // `set_host_folder_at` ne pose qu'une ligne `#Folder:` : le reste du
        // fichier CRLF doit rester octet pour octet identique, fins de ligne
        // comprises. Chemin emprunté aussi par `folders::rename_core`.
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let origine = "Host a\r\n  HostName x\r\n";
        std::fs::write(&path, origine).unwrap();
        set_host_folder_at(&path, "a", "prod").unwrap();
        let apres = std::fs::read_to_string(&path).unwrap();
        assert!(aucun_lf_nu(&apres), "LF nu introduit : {apres:?}");
        assert!(apres.contains("    #Folder: prod\r\n"), "{apres:?}");
        // Hors la ligne #Folder, le fichier est inchangé octet pour octet.
        let mut sans_folder = String::new();
        for l in apres.lines().filter(|l| !l.contains("#Folder:")) {
            sans_folder.push_str(l);
            sans_folder.push_str("\r\n");
        }
        assert_eq!(sans_folder, origine, "contenu altéré hors #Folder");
    }

    #[test]
    fn append_host_conserve_le_crlf_du_fichier() {
        // `append_host` ne passe pas par `lines()` mais collait `render_host_block`
        // (LF) à la fin d'un fichier CRLF : fichier mixte. Le bloc ajouté suit
        // désormais la fin de ligne du fichier.
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "Host a\r\n  HostName 1\r\n").unwrap();
        append_host(&host("b")).unwrap();
        let apres = std::fs::read_to_string(&path).unwrap();
        assert!(
            aucun_lf_nu(&apres),
            "bloc ajouté en LF dans un fichier CRLF : {apres:?}"
        );
        assert!(apres.contains("Host b\r\n"), "{apres:?}");
    }

    #[test]
    fn set_host_folder_dit_plusieurs_alias_sur_un_bloc_a_noms_multiples() {
        // Constat de l'audit : glisser « a » de `Host a b` dans un dossier rendait
        // « Hôte « a » introuvable », alors qu'il est sous les yeux de l'utilisateur.
        let dir = std::env::temp_dir().join(format!("avash-multi-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        std::fs::write(&path, "Host a b\n  HostName x\n").unwrap();
        let e = set_host_folder_at(&path, "a", "prod")
            .unwrap_err()
            .to_string();
        assert!(e.contains("plusieurs alias"), "{e}");
        assert!(e.contains("Host a b"), "{e}");
        assert!(!e.contains("introuvable"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_host_folder_detecte_le_bloc_multi_alias_en_casse_mixte() {
        // `parse_config_str` accepte `HoSt` et la tabulation comme séparateur :
        // la détection du bloc à plusieurs alias doit les reconnaître aussi, sinon
        // `HoSt\ta b` retomberait sur « introuvable ».
        let dir = std::env::temp_dir().join(format!("avash-multi-cx-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        std::fs::write(&path, "HoSt\ta b\n  HostName x\n").unwrap();
        let e = set_host_folder_at(&path, "a", "prod")
            .unwrap_err()
            .to_string();
        assert!(e.contains("plusieurs alias"), "{e}");
        assert!(!e.contains("introuvable"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_host_folder_garde_introuvable_pour_un_vrai_absent() {
        // Aucun bloc ne cite « fantome » : le message « introuvable » reste juste.
        let dir = std::env::temp_dir().join(format!("avash-absent-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        std::fs::write(&path, "Host a b\n  HostName x\n").unwrap();
        let e = set_host_folder_at(&path, "fantome", "prod")
            .unwrap_err()
            .to_string();
        assert!(e.contains("introuvable"), "{e}");
        assert!(!e.contains("plusieurs alias"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_host_garde_le_commentaire_qui_annonce_le_bloc_suivant() {
        // Trouvé par l'audit du 7 septembre 2026 : le saut du bloc retiré
        // emportait la ligne vide et le commentaire qui SUIT le bloc, alors
        // qu'ils annoncent le bloc suivant (« # Staging » avant `Host staging`).
        // La note se perdait et « # Prod » coiffait alors staging.
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "# Prod\nHost prod\n  HostName 1\n\n# Staging — accès via Jean, clé chez ops\nHost staging\n  HostName 2\n",
        )
        .unwrap();

        remove_host("prod").unwrap();

        let apres = std::fs::read_to_string(&path).unwrap();
        assert!(
            apres.contains("# Staging — accès via Jean, clé chez ops\nHost staging"),
            "la note sur staging doit rester devant son bloc :\n{apres}"
        );
        assert!(
            !apres.contains("HostName 1"),
            "prod aurait dû partir :\n{apres}"
        );
    }

    #[test]
    fn remove_host_emporte_les_marqueurs_avash_du_bloc() {
        // Complément du cas précédent : un `#Tags:` non indenté que le parseur
        // rattache à prod (jusqu'au prochain Host/Match) doit partir AVEC prod,
        // sans que le tampon ne le prenne pour l'annonce du bloc suivant.
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "Host prod\n  HostName 1\n#Tags: x\n\n# Staging note\nHost staging\n  HostName 2\n",
        )
        .unwrap();

        remove_host("prod").unwrap();

        let apres = std::fs::read_to_string(&path).unwrap();
        assert!(
            !apres.contains("#Tags: x"),
            "le marqueur Avash aurait dû partir :\n{apres}"
        );
        assert!(
            apres.contains("# Staging note\nHost staging"),
            "l'annonce de staging doit rester :\n{apres}"
        );
    }

    #[test]
    fn remove_host_garde_un_commentaire_de_fin_de_fichier() {
        // Bloc en fin de fichier suivi d'un commentaire : le tampon est réémis à
        // la fin, le commentaire survit à la suppression du dernier bloc.
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "Host last\n  HostName x\n\n# fin\n").unwrap();

        remove_host("last").unwrap();

        let apres = std::fs::read_to_string(&path).unwrap();
        assert!(
            apres.contains("# fin"),
            "commentaire de fin perdu :\n{apres}"
        );
        assert!(
            !apres.contains("HostName x"),
            "last aurait dû partir :\n{apres}"
        );
    }

    // ---------- update_host ----------

    #[test]
    fn update_host_remplace_le_bloc_sur_place() {
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "Host un\n  HostName 1.1.1.1\n\nHost prod\n  HostName 2.2.2.2\n  User old\n\nHost deux\n  HostName 3.3.3.3\n",
        )
        .unwrap();

        let mut modifie = host("prod");
        modifie.hostname = Some("9.9.9.9".into());
        modifie.user = Some("nouveau".into());
        update_host("prod", &modifie).unwrap();

        let hosts = parse_ssh_config().unwrap();
        // Ordre preserve : un, prod, deux.
        let noms: Vec<_> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(noms, vec!["un", "prod", "deux"], "ordre casse");
        let p = hosts.iter().find(|h| h.alias == "prod").unwrap();
        assert_eq!(p.hostname.as_deref(), Some("9.9.9.9"));
        assert_eq!(p.user.as_deref(), Some("nouveau"));
    }

    #[test]
    fn update_host_preserve_les_directives_non_gerees() {
        // Trouvé par l'audit du 7 septembre 2026 : éditer un hôte depuis
        // l'interface réécrivait le bloc et perdait en silence toute directive
        // qu'Avash ne gère pas (ForwardAgent, LocalForward, IdentitiesOnly…).
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "Host prod\n  HostName 2.2.2.2\n  User old\n  ForwardAgent yes\n  \
             LocalForward 8080 127.0.0.1:80\n  IdentitiesOnly yes\n  # note perso\n\nHost autre\n  HostName 3.3.3.3\n",
        )
        .unwrap();

        let mut modifie = host("prod");
        modifie.hostname = Some("9.9.9.9".into());
        modifie.user = Some("nouveau".into());
        update_host("prod", &modifie).unwrap();

        let texte = std::fs::read_to_string(&path).unwrap();
        for attendu in [
            "ForwardAgent yes",
            "LocalForward 8080 127.0.0.1:80",
            "IdentitiesOnly yes",
            "# note perso",
            "HostName 9.9.9.9",
            "User nouveau",
        ] {
            assert!(texte.contains(attendu), "« {attendu} » perdu :\n{texte}");
        }
        // L'ancienne valeur régénérée ne subsiste pas en double.
        assert!(
            !texte.contains("2.2.2.2"),
            "ancienne HostName restée :\n{texte}"
        );
        assert!(!texte.contains("User old"), "ancien User resté :\n{texte}");
        // L'hôte voisin et l'ordre sont intacts.
        let noms: Vec<_> = parse_ssh_config()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert_eq!(noms, vec!["prod", "autre"]);
    }

    #[test]
    fn update_host_gere_le_renommage() {
        let _h = crate::testutil::temp_home();
        append_host(&host("ancien")).unwrap();
        let mut renomme = host("ancien");
        renomme.alias = "nouveau".into();
        update_host("ancien", &renomme).unwrap();
        let noms: Vec<_> = parse_ssh_config()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert_eq!(noms, vec!["nouveau"]);
    }

    #[test]
    fn update_host_refuse_de_renommer_vers_un_alias_existant() {
        let _h = crate::testutil::temp_home();
        append_host(&host("a")).unwrap();
        append_host(&host("b")).unwrap();
        let mut collision = host("a");
        collision.alias = "b".into();
        let e = update_host("a", &collision).unwrap_err().to_string();
        assert!(e.contains("existe déjà"), "{e}");
    }

    #[test]
    fn update_host_voit_les_alias_declares_dans_un_include_lors_d_un_renommage() {
        // Trouvé par l'audit du 9 septembre 2026 : `append_host` vérifiait déjà
        // l'unicité sur la configuration COMPLÈTE (Include résolus), mais
        // `update_host` n'avait jamais reçu le même traitement : il lisait le
        // fichier principal brut. Renommer « ancien » en « backup » alors qu'un
        // fichier inclus déclarait déjà « backup » passait sans erreur, et
        // OpenSSH, qui retient la PREMIÈRE occurrence, continuait de joindre la
        // machine du fichier inclus : la connexion partait vers le mauvais hôte.
        let _h = crate::testutil::temp_home();
        let ssh = repertoire_personnel().unwrap().join(".ssh");
        std::fs::create_dir_all(ssh.join("conf.d")).unwrap();
        std::fs::write(
            ssh.join("conf.d").join("prod.conf"),
            "Host backup\n    HostName 10.0.0.99\n",
        )
        .unwrap();
        std::fs::write(
            ssh.join("config"),
            "Include conf.d/*.conf\n\nHost ancien\n    HostName 10.0.0.1\n",
        )
        .unwrap();

        let mut collision = host("ancien");
        collision.alias = "backup".into();
        let e = update_host("ancien", &collision).unwrap_err().to_string();
        assert!(e.contains("existe déjà"), "{e}");
        let principal = std::fs::read_to_string(ssh.join("config")).unwrap();
        assert!(
            principal.contains("Host ancien"),
            "le bloc a été renommé malgré la collision : {principal}"
        );
        assert!(
            !principal.contains("Host backup"),
            "un second « backup » a été écrit dans le fichier principal : {principal}"
        );
    }

    #[test]
    fn update_host_meme_alias_ne_declenche_pas_la_collision() {
        // Modifier sans renommer ne doit pas se heurter a « existe deja ».
        let _h = crate::testutil::temp_home();
        append_host(&host("stable")).unwrap();
        let mut m = host("stable");
        m.user = Some("change".into());
        assert!(update_host("stable", &m).is_ok());
        assert_eq!(
            parse_ssh_config().unwrap()[0].user.as_deref(),
            Some("change")
        );
    }

    #[test]
    fn update_host_garde_le_commentaire_qui_annonce_le_bloc_suivant() {
        // Trouvé par l'audit du 7 septembre 2026 : comme `remove_host`,
        // `update_host` avalait la ligne vide séparant le bloc réécrit du suivant
        // (le bloc rendu se collait à `Host staging`) et déplaçait la note qui
        // annonce staging. Le tampon de fin la garde devant son bloc.
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "# Prod\nHost prod\n  HostName 1\n\n# Staging — accès via Jean, clé chez ops\nHost staging\n  HostName 2\n",
        )
        .unwrap();

        let mut modifie = host("prod");
        modifie.hostname = Some("9.9.9.9".into());
        update_host("prod", &modifie).unwrap();

        let apres = std::fs::read_to_string(&path).unwrap();
        assert!(
            apres.contains("# Staging — accès via Jean, clé chez ops\nHost staging"),
            "la note sur staging doit rester devant son bloc :\n{apres}"
        );
        // Le bloc réécrit ne se colle plus à staging : une ligne vide sépare
        // encore les deux blocs.
        assert!(
            apres.contains("\n\n# Staging"),
            "le séparateur entre les deux blocs a été avalé :\n{apres}"
        );
        assert_eq!(
            parse_ssh_config().unwrap()[0].hostname.as_deref(),
            Some("9.9.9.9")
        );
    }
}

/// Fuzzing par mutation du parseur `~/.ssh/config`.
///
/// C'est la surface d'entrée la plus exposée du cœur : un fichier que
/// l'utilisateur édite à la main, qu'un outil tiers réécrit, ou qu'un dépôt de
/// dotfiles fournit. Le même principe que pour le processus RDP : muter un
/// contenu authentique atteint des chemins que des octets aléatoires ne
/// touchent jamais, parce que le tout premier `Host` filtre déjà tout ce qui ne
/// ressemble pas à une configuration.
#[cfg(test)]
mod tests_mutation {
    use super::{parse_config_str, split_proxy_jump};

    /// Générateur déterministe (LCG) : une suite rejouable, aucun crate de plus.
    struct Graine(u64);
    impl Graine {
        fn suivant(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }
        fn entre(&mut self, borne: usize) -> usize {
            (self.suivant() as usize) % borne.max(1)
        }
    }

    const SOUCHE: &str = "\
# Configuration réaliste, avec ce que le parseur sait lire.
Include ~/.ssh/conf.d/*.conf
Host prod
  HostName 10.0.0.7
  User adrien
  Port 2222
  IdentityFile ~/.ssh/id_ed25519
  ProxyJump bastion, relais:2200
  #Tags: prod, web
  #Folder: clients/acme
Host bastion
  HostName bastion.exemple.net
  User rebond
Match host *.interne
  ProxyJump none
Host *
  ServerAliveInterval 30
";

    /// Fragments qui visent les chemins du parseur : mots-clés, séparateurs,
    /// commentaires porteurs de sens, valeurs vides, octets hostiles.
    const FRAGMENTS: &[&str] = &[
        "Host ",
        "Host\t",
        "Match ",
        "Include ",
        "ProxyJump ",
        "#Tags: ",
        "#Folder: ",
        "Port ",
        "Port 99999",
        "Port 0",
        ":0",
        "\n",
        "\r\n",
        "\0",
        "  ",
        "=",
        "\"",
        "*",
        "?",
        ",",
        "../",
        "é",
        "\u{feff}",
        "\u{2028}",
        "none",
    ];

    fn muter(graine: &mut Graine, base: &str) -> String {
        let mut octets: Vec<u8> = base.as_bytes().to_vec();
        for _ in 0..=graine.entre(6) {
            match graine.entre(6) {
                0 if !octets.is_empty() => {
                    let i = graine.entre(octets.len());
                    octets[i] = graine.suivant() as u8;
                }
                1 if !octets.is_empty() => {
                    let i = graine.entre(octets.len());
                    octets.truncate(i);
                }
                2 => {
                    let i = graine.entre(octets.len() + 1);
                    let f = FRAGMENTS[graine.entre(FRAGMENTS.len())];
                    octets.splice(i..i, f.bytes());
                }
                3 if octets.len() > 2 => {
                    let a = graine.entre(octets.len());
                    let b = a + graine.entre(octets.len() - a);
                    let bloc: Vec<u8> = octets[a..b].to_vec();
                    let i = graine.entre(octets.len());
                    octets.splice(i..i, bloc);
                }
                4 if octets.len() > 2 => {
                    let a = graine.entre(octets.len());
                    let b = a + graine.entre(octets.len() - a);
                    octets.drain(a..b);
                }
                _ => {
                    let i = graine.entre(octets.len() + 1);
                    let n = 1 + graine.entre(64);
                    octets.splice(i..i, std::iter::repeat_n(b'A', n));
                }
            }
        }
        // Le parseur reçoit une `&str` (conversion lossy des octets mutés). À ne
        // pas confondre avec `read_to_string`, qui n'est PAS lossy : il rend
        // `Err(InvalidData)` sur un octet non UTF-8. Ce banc n'exerce donc que
        // `parse_config_str`, pas le vrai comportement de lecture ; le refus
        // d'un `config` non UTF-8 est couvert par les tests de `save_tests`.
        String::from_utf8_lossy(&octets).into_owned()
    }

    /// Aucune mutation ne fait paniquer le parseur, et ce qu'il rend reste
    /// cohérent : un alias jamais vide, un port jamais nul, des rebonds sans
    /// espace autour.
    #[test]
    fn aucun_fichier_mute_ne_fait_paniquer_le_parseur() {
        let mut graine = Graine(0x5eed_0002_0926);
        let mut hotes_vus = 0usize;
        for _ in 0..2_000 {
            let contenu = muter(&mut graine, SOUCHE);
            let hotes = parse_config_str(&contenu);
            for h in &hotes {
                assert!(!h.alias.is_empty(), "alias vide pour :\n{contenu}");
                assert!(!h.alias.contains(['\n', '\r']), "alias multiligne");
                assert_ne!(h.port, Some(0), "port nul accepté pour :\n{contenu}");
                if let Some(pj) = &h.proxy_jump {
                    for hop in split_proxy_jump(pj) {
                        assert_eq!(hop.host.trim(), hop.host, "rebond non rogné : {hop:?}");
                        assert!(
                            !hop.host.starts_with('['),
                            "crochet IPv6 gardé dans l'hôte : {hop:?}"
                        );
                    }
                }
            }
            hotes_vus += hotes.len();
        }
        assert!(
            hotes_vus > 0,
            "les mutations ont tué toutes les entrées : test sans portée"
        );
    }

    /// Trouvé par cargo-fuzz en quelques secondes, là où 2 000 mutations
    /// n'avaient jamais produit « Port 0 » : OpenSSH le refuse, nous le
    /// lisions. Un port nul n'est pas un port, ni pour l'hôte ni pour un
    /// rebond.
    #[test]
    fn le_port_zero_n_est_pas_un_port() {
        let hotes = parse_config_str("Host a\n  HostName a.local\n  Port 0\n");
        assert_eq!(hotes.len(), 1);
        assert_eq!(hotes[0].port, None);
        let hops = split_proxy_jump("relais:0, autre:2200");
        assert_eq!(hops.len(), 2);
        assert_eq!(hops[0].host, "relais:0");
        assert_eq!(hops[0].port, None);
        assert_eq!(hops[1].port, Some(2200));
    }

    /// Les mutations sont rejouables : deux exécutions rendent la même suite.
    #[test]
    fn la_suite_de_mutations_est_deterministe() {
        let a: Vec<String> = {
            let mut g = Graine(42);
            (0..20).map(|_| muter(&mut g, SOUCHE)).collect()
        };
        let b: Vec<String> = {
            let mut g = Graine(42);
            (0..20).map(|_| muter(&mut g, SOUCHE)).collect()
        };
        assert_eq!(a, b);
    }
}
