//! Intégration RDP côté Tauri : lance le sidecar `avash-rdp` (isolé de russh),
//! qui sert lui-même le bureau à la webview via un WebSocket local **binaire**
//! (vrai `ArrayBuffer` : pas de base64, pas de JSON — débit maximal). Avash ne
//! fait que gérer le cycle de vie du sidecar et transmettre le point de
//! connexion (port + jeton) au front.

use avash::Verrou as _;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Child;

/// Sessions RDP vivantes, par id d'onglet.
#[derive(Default)]
pub struct RdpStore {
    pub inner: Mutex<HashMap<u64, Child>>,
    /// L'entrée standard de chaque sidecar, gardée ouverte après le mot de
    /// passe : c'est par là que lui sont annoncés les chemins que l'utilisateur
    /// désigne (voir `commands::choix_locaux`), seuls chemins qu'il acceptera
    /// d'offrir au distant. Verrou tokio : on écrit dedans en asynchrone.
    pub stdins: tokio::sync::Mutex<HashMap<u64, tokio::process::ChildStdin>>,
    /// Dernières lignes de diagnostic du sidecar, par session.
    pub journaux: Mutex<HashMap<u64, std::sync::Arc<Mutex<std::collections::VecDeque<String>>>>>,
}

/// Point de connexion WebSocket renvoyé au front.
#[derive(serde::Serialize)]
pub struct RdpConn {
    pub port: u16,
    pub token: String,
}

/// Chemin du processus RDP, ou `None` s'il est introuvable.
///
/// **Aucun repli relatif.** Le dernier recours était
/// `rdp-sidecar/target/release/avash-rdp`, résolu depuis le répertoire courant :
/// lancée depuis `/tmp`, un partage ou `~/Téléchargements`, l'application y
/// exécutait le binaire qu'un autre compte avait pu y déposer — et lui écrivait
/// le mot de passe RDP sur son entrée standard, celui du trousseau compris.
/// Mieux vaut une erreur nommée qu'un chemin deviné.
pub(crate) fn sidecar_path() -> Option<std::path::PathBuf> {
    sidecar_path_depuis(
        std::env::var_os("AVASH_RDP_BIN"),
        std::env::current_exe().ok(),
    )
}

/// La décision de `sidecar_path`, sans lire l'environnement : `var` est la
/// valeur d'`AVASH_RDP_BIN`, `exe` le chemin de l'application.
///
/// Pure depuis l'audit du 12 septembre 2026 (C-unsafe-3) : son test posait
/// `AVASH_RDP_BIN` par `set_var` pendant que d'autres tests du même binaire
/// lisaient l'environnement en parallèle. Les tests passent désormais les
/// valeurs en argument, sans toucher au processus.
pub(crate) fn sidecar_path_depuis(
    var: Option<std::ffi::OsString>,
    exe: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    if let Some(p) = var {
        let p = std::path::PathBuf::from(p);
        // Même une variable d'environnement doit désigner un chemin absolu :
        // relative, elle rouvrirait exactement la porte qu'on vient de fermer.
        return p.is_absolute().then_some(p);
    }
    // Sous Windows l'exécutable porte une extension : sans elle, le fichier
    // posé à côté de l'application (avash-rdp.exe) n'était jamais trouvé et
    // toute connexion RDP échouait.
    let nom = format!("avash-rdp{}", std::env::consts::EXE_SUFFIX);
    if let Some(exe) = exe {
        if let Some(dir) = exe.parent() {
            // À côté de l'exe (installation / bundle / version portable).
            let side = dir.join(&nom);
            if side.exists() {
                return Some(side);
            }
            // En développement seulement : le sidecar est un projet séparé, et
            // l'on remonte de target/debug/ jusqu'à la racine du dépôt. `root`
            // vient de `current_exe()`, donc absolu. En release ce chemin n'a
            // rien à faire là : pour une installation dans ~/.local/bin il
            // désignerait ~/rdp-sidecar/target/release/, un emplacement
            // imprévu à qui l'on écrit le mot de passe.
            //
            // Le `cfg` porte sur tout le bloc, `root` compris : placé plus bas,
            // il laissait une variable inutilisée en release — invisible à
            // clippy, qui ne compile qu'en debug.
            #[cfg(debug_assertions)]
            if let Some(root) = dir.parent().and_then(std::path::Path::parent) {
                let devside = root.join("rdp-sidecar/target/release").join(&nom);
                if devside.exists() && devside.is_absolute() {
                    return Some(devside);
                }
            }
        }
    }
    None
}

/// Ce que le front demande pour ouvrir un bureau, regroupé : `ouvrir_avec`
/// reçoit ainsi un seul objet, que les tests construisent sans IPC.
pub(crate) struct Demande {
    pub id: u64,
    pub host: String,
    pub port: Option<u16>,
    pub user: String,
    /// Mot de passe saisi à l'instant, ou vide pour le lire au trousseau.
    /// Effacé de la mémoire à la libération (audit du 12 septembre 2026,
    /// C-secrets-2) : il séjournait sinon dans le tas jusqu'à réutilisation.
    pub password: zeroize::Zeroizing<String>,
    pub width: u16,
    pub height: u16,
    pub echelle: u32,
    pub options: Options,
    pub partage: Option<String>,
}

/// Délai de garde de l'annonce « PORT JETON » du sidecar.
///
/// Trouvé par l'audit du 12 septembre 2026 (C-SIL-13) : l'attente de
/// l'annonce n'avait pas de borne propre. Le sidecar borne sa connexion (10 s
/// puis 25 s), mais pas ce qui la précède : un dossier partagé posé sur un
/// montage réseau lent, ou `localectl status` qui ne répond pas, laissaient
/// l'onglet sur « connexion » sans fin. Soixante secondes couvrent largement
/// ses propres délais, reprise comprise.
const DELAI_ANNONCE: std::time::Duration = std::time::Duration::from_secs(60);

/// Un mot de passe qui peut partir sur l'entrée standard du sidecar.
///
/// Trouvé par l'audit de sécurité du 12 septembre 2026 (C-secrets-1) : ce
/// canal est lu ligne à ligne, le mot de passe d'abord, puis des lignes
/// `AUTORISE <chemin>` qui désignent les seuls fichiers que le bureau distant
/// peut recevoir. Un saut de ligne dans le mot de passe faisait de la suite une
/// désignation : un script de la webview offrait `~/.ssh/id_ed25519` au serveur
/// de son choix. Un caractère nul n'a rien à faire dans un mot de passe non
/// plus. Le refus tombe avant tout lancement et avant tout enregistrement.
fn mot_de_passe_transmissible(password: &str) -> Result<(), String> {
    if password.contains(['\n', '\r', '\0']) {
        return Err(
            "Le mot de passe ne peut contenir ni saut de ligne ni caractère nul.".to_owned(),
        );
    }
    Ok(())
}

/// Le dossier partagé demandé est-il un choix de l'utilisateur ?
///
/// Trouvé par l'audit de sécurité du 12 septembre 2026 (C-ipc-1) : le dossier
/// servi au bureau distant (lecture, écriture, suppression) était une chaîne
/// libre venue de la page, contrôlée seulement « absolue et existante ». Un
/// script de la webview partageait ainsi tout `~` avec le serveur de son
/// choix. Deux sources font foi : la boîte de sélection native
/// (`choisir_fichiers_locaux` en mode dossier, retenue dans `ChoixLocaux`),
/// et le dossier déjà enregistré dans `rdp.yaml` pour ce même bureau
/// (adresse, port et utilisateur), qui avait lui-même été désigné ainsi.
fn partage_designe(
    choix: &crate::commands::ChoixLocaux,
    enregistres: &[RdpHost],
    host: &str,
    port: u16,
    user: &str,
    partage: &str,
) -> bool {
    choix.designe(std::path::Path::new(partage))
        || enregistres.iter().any(|h| {
            h.host == host
                && h.port == port
                && h.user == user
                && h.partage.as_deref() == Some(partage)
        })
}

/// Lance le sidecar et renvoie le WebSocket (port + jeton) qu'il annonce.
/// Ouvre un bureau distant.
///
/// `password` est le mot de passe saisi à l'instant, s'il y en a un. Vide pour
/// un bureau enregistré : le secret est alors lu **ici**, côté natif, et ne
/// traverse jamais l'IPC — comme le fait déjà le volet SSH. Il séjournait
/// sinon dans le tas de la webview toute la durée de l'onglet.
///
/// `sans_nla` : l'utilisateur a accepté de se passer d'authentification réseau
/// pour ce serveur. Refusé par défaut — c'est une décision qui lui appartient,
/// pas un repli silencieux.
///
/// `tls_herite` : l'utilisateur a accepté les suites TLS héritées du système
/// pour ce serveur (Windows Server 2012 R2 et antérieurs, voir le module
/// `tls_herite` du processus RDP). Même règle : refusé par défaut, décision
/// explicite, retenue par serveur.
///
/// `vnc` : le serveur parle RFB. Même processus, même canal local ; le port
/// par défaut devient 5900 et l'utilisateur peut être vide.
///
/// `partage` (contrat K12, audit du 12 septembre 2026) : le dossier servi au
/// bureau distant n'est accepté que s'il a été désigné par la boîte native
/// (`invoke("choisir_fichiers_locaux", { titre, dossiers: true })`, qui le
/// retient) ou s'il est celui que `rdp.yaml` enregistre déjà pour ce bureau.
/// Signature vue du front, inchangée :
/// `invoke("rdp_open", { id, host, port, user, password, width, height,
/// desktopScaleFactor, options: { sansNla, tlsHerite, vnc, sansSon }, partage })`.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn rdp_open<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    id: u64,
    host: String,
    port: Option<u16>,
    user: String,
    password: String,
    width: u16,
    height: u16,
    // Échelle DPI (`devicePixelRatio` × 100) annoncée au serveur RDP quand la
    // définition est négociée en pixels physiques (HiDPI). Transmise telle
    // quelle au sidecar par `--scale`. Ajouté par l'audit du 7 septembre 2026.
    desktop_scale_factor: u32,
    options: Options,
    partage: Option<String>,
) -> Result<RdpConn, String> {
    let demande = Demande {
        id,
        host,
        port,
        user,
        password: zeroize::Zeroizing::new(password),
        width,
        height,
        echelle: desktop_scale_factor,
        options,
        partage,
    };
    ouvrir_avec(&app, sidecar_path(), demande).await
}

/// Tout `rdp_open` après la résolution du binaire : les tests y passent un
/// sidecar factice (un script) sans toucher à `AVASH_RDP_BIN`. Audit du
/// 12 septembre 2026 (C-couv-2) : rien au-delà de la validation n'était
/// exercé hors de la suite bout en bout, contrat « mot de passe par l'entrée
/// standard » compris.
pub(crate) async fn ouvrir_avec<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    bin: Option<std::path::PathBuf>,
    d: Demande,
) -> Result<RdpConn, String> {
    use tauri::Manager as _;
    let state = app.state::<RdpStore>();
    let id = d.id;
    let (protocole, port, partage) = controler(app, &d).await?;
    let Some(bin) = bin else {
        return Err(
            "Le processus RDP (avash-rdp) est introuvable à côté de l'application. \
             Réinstalle Avash, ou indique son chemin absolu dans AVASH_RDP_BIN."
                .to_owned(),
        );
    };
    let mut cmd = commande_sidecar(&bin, &d, port, partage.as_deref());
    let compte = rdphost::keyring_account_pour(protocole, &d.user, &d.host, port);
    let password = mot_de_passe_a_transmettre(app, d.password, compte).await?;

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Lancement du sidecar RDP impossible : {e}"))?;
    let pid = child.id();

    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Enregistré AVANT tout point de sortie. La connexion RDP (TLS + NLA) peut
    // durer plusieurs secondes ; si l'utilisateur ferme l'onglet pendant ce
    // temps, `rdp_close` doit trouver l'enfant pour le tuer. Enregistré à la fin
    // comme auparavant, le sidecar survivait à la fermeture — session
    // authentifiée ouverte, socket en écoute, invisible de l'interface.
    if let Some(mut old) = state.inner.verrou().insert(id, child) {
        let _ = old.start_kill();
    }

    // À partir d'ici, toute sortie en erreur doit emporter l'enfant enregistré.
    let (Some(mut stdin), Some(stdout), Some(mut stderr)) = (stdin, stdout, stderr) else {
        return Err(echec(&state, id, pid, "Flux du sidecar RDP indisponibles.".into()).await);
    };

    // Mot de passe transmis par stdin plutôt qu'en argument : évite sa fuite via
    // /proc/<pid>/cmdline, lisible par les autres utilisateurs locaux. Deux
    // écritures plutôt qu'un `format!` : pas de copie du secret hors de son
    // `Zeroizing`.
    {
        use tokio::io::AsyncWriteExt as _;
        let envoi = async {
            stdin.write_all(password.as_bytes()).await?;
            stdin.write_all(b"\n").await
        };
        if let Err(e) = envoi.await {
            return Err(echec(
                &state,
                id,
                pid,
                format!("Envoi du mot de passe au sidecar : {e}"),
            )
            .await);
        }
    }
    drop(password);
    // Gardée ouverte : les désignations de fichiers passeront par là. Fermée
    // ici, c'était interdire au sidecar toute offre au distant.
    state.stdins.lock().await.insert(id, stdin);

    let annonce = match tokio::time::timeout(DELAI_ANNONCE, lire_annonce(stdout, &mut stderr)).await
    {
        Ok(r) => r,
        Err(_) => Err(format!(
            "Le processus de bureau distant n'a pas annoncé sa connexion en {} s : abandon.",
            DELAI_ANNONCE.as_secs()
        )),
    };
    let (port, token) = match annonce {
        Ok(v) => v,
        Err(e) => return Err(echec(&state, id, pid, e).await),
    };

    // L'utilisateur a-t-il fermé l'onglet pendant la connexion ? `rdp_close` a
    // alors retiré et tué notre enfant : on ne prétend pas avoir ouvert une
    // session, et le front n'affiche pas d'erreur trompeuse.
    if !est_le_notre(&state, id, pid) {
        return Err(CONNEXION_ANNULEE.into());
    }

    suivre_diagnostic(&state, id, stderr);
    Ok(RdpConn { port, token })
}

/// Les contrôles d'avant lancement : mot de passe transmissible, dossier
/// partagé désigné, adresse valide. Rend le protocole, le port et le dossier.
async fn controler<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    d: &Demande,
) -> Result<(rdphost::Protocole, u16, Option<String>), String> {
    use tauri::Manager as _;
    mot_de_passe_transmissible(&d.password)?;
    // Seul le protocole se décide ici ; les autres choix partent tels quels au
    // processus (`drapeaux`).
    let protocole = if d.options.vnc {
        rdphost::Protocole::Vnc
    } else {
        rdphost::Protocole::Rdp
    };
    let port = d.port.unwrap_or(protocole.port_par_defaut());
    // Le dossier partagé doit exister ici, avant de lancer quoi que ce soit :
    // le sidecar le refuserait aussi, mais après la connexion, et l'utilisateur
    // verrait un bureau qui se ferme au lieu d'un message.
    let partage = dossier_partage(d.partage.clone())?;
    if let Some(p) = &partage {
        let enregistres = lire_bureaux_bruts().await;
        if !partage_designe(
            &app.state::<crate::commands::ChoixLocaux>(),
            &enregistres,
            &d.host,
            port,
            &d.user,
            p,
        ) {
            return Err(format!(
                "Le dossier à partager « {p} » n'a pas été choisi par la boîte de sélection : \
                 choisis-le avec le bouton « Choisir… » de la fiche du bureau."
            ));
        }
    }
    // L'adresse vient du front et arrive telle quelle jusqu'à la clé du fichier
    // d'empreintes. `RdpHost::validate` ne la voyait qu'à l'enregistrement : une
    // connexion manuelle, ou un rdp.yaml écrit par une version antérieure, la
    // contournait entièrement — et une espace dans l'adresse suffit à casser
    // `rdp_known_hosts`, donc à désarmer le TOFU en silence.
    avash::rdphost::RdpHost::new("", &d.host, port, &d.user, d.width, d.height)
        .en(protocole)
        .validate()
        .map_err(|e| format!("{e:#}"))?;
    Ok((protocole, port, partage))
}

/// Le mot de passe à écrire sur l'entrée standard : la saisie, sinon celui du
/// trousseau. Un trousseau en panne rend une erreur qui demande la saisie
/// (contrat K1) : partir avec un mot de passe vide faisait refuser la
/// connexion comme un mauvais mot de passe.
async fn mot_de_passe_a_transmettre<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    saisi: zeroize::Zeroizing<String>,
    compte: String,
) -> Result<zeroize::Zeroizing<String>, String> {
    if !saisi.is_empty() {
        return Ok(saisi);
    }
    match crate::commands::charger_secret(app, compte).await? {
        Some(p) => {
            // Un secret mémorisé passe par la même garde que la saisie : il a
            // pu être écrit par une version qui ne refusait rien.
            mot_de_passe_transmissible(&p)?;
            Ok(p)
        }
        None => Ok(saisi),
    }
}

/// La ligne de commande du sidecar. Jamais le mot de passe : il part par
/// l'entrée standard.
fn commande_sidecar(
    bin: &std::path::Path,
    d: &Demande,
    port: u16,
    partage: Option<&str>,
) -> tokio::process::Command {
    use std::process::Stdio;
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args([
        "--host",
        &d.host,
        "--port",
        &port.to_string(),
        "-u",
        &d.user,
        "--width",
        &d.width.to_string(),
        "--height",
        &d.height.to_string(),
        "--scale",
        &d.echelle.to_string(),
    ])
    // Le son du bureau distant se coupe dans la palette : le processus
    // n'annonce alors pas le canal, plutôt que de recevoir pour rien.
    .args(drapeaux(d.options, partage))
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    // Trouvé par l'audit de sécurité du 12 septembre 2026 (C-sidecar-1) :
    // `tokio::process::Command` ne tue pas l'enfant à la libération. Une
    // application qui se ferme sans passer par `rdp_close` (sortie de la boucle
    // d'événements, magasin libéré) laissait une session RDP authentifiée
    // ouverte, invisible, jusqu'à l'expiration côté serveur.
    .kill_on_drop(true);

    // Sous Windows, lancer un programme console ouvre une fenêtre noire à chaque
    // connexion RDP. CREATE_NO_WINDOW l'en empêche : le sidecar reste invisible,
    // ses flux restant redirigés vers nous.
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Les bureaux de `rdp.yaml`, lus hors des fils du runtime (disque, parfois
/// un profil réseau) ; un fichier illisible vaut « aucun ».
async fn lire_bureaux_bruts() -> Vec<RdpHost> {
    tokio::task::spawn_blocking(|| {
        rdphost::load_hosts_brut_from(&rdphost::hosts_path()).unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

/// L'enfant enregistré sous `id` est-il encore celui qu'on a lancé ?
fn est_le_notre(state: &RdpStore, id: u64, pid: Option<u32>) -> bool {
    state.inner.verrou().get(&id).is_some_and(|c| c.id() == pid)
}

/// Sortie en erreur d'une ouverture : tue NOTRE sidecar et rend le message, ou
/// rend `CONNEXION_ANNULEE` si l'onglet a été fermé entre-temps.
///
/// Trouvé par l'audit du 12 septembre 2026 (C-couv-2) : dans le cas réel
/// (onglet fermé pendant TLS + NLA), `rdp_close` tue l'enfant, l'annonce
/// reçoit une fin de flux, et l'on rendait « Le sidecar RDP s'est arrêté sans
/// se connecter » au lieu du marqueur documenté. Et `tuer` retirait l'enfant
/// enregistré sous `id` sans regarder si c'était le nôtre : une seconde
/// ouverture sous le même identifiant pouvait perdre le sien.
async fn echec(state: &RdpStore, id: u64, pid: Option<u32>, msg: String) -> String {
    let (present, notre) = {
        let inner = state.inner.verrou();
        let c = inner.get(&id);
        (c.is_some(), c.is_some_and(|c| c.id() == pid))
    };
    if !notre {
        // Fermé par l'utilisateur, ou évincé par une ouverture plus récente
        // sous le même onglet : ce n'est pas un échec à montrer.
        let _ = present;
        return CONNEXION_ANNULEE.to_owned();
    }
    tuer(state, id, msg).await
}

/// Garde les dernières lignes que le sidecar écrit sur son erreur standard.
///
/// Passé l'ouverture, `stderr` n'était plus lu : il était libéré à la sortie de
/// `rdp_open`. Une panique ou une erreur du sidecar **en cours de session**
/// écrivait alors dans un tube fermé — le message était perdu et l'onglet RDP
/// mourait sans motif.
fn suivre_diagnostic(state: &RdpStore, id: u64, flux: tokio::process::ChildStderr) {
    let journal = state.journaux.verrou().entry(id).or_default().clone();
    tokio::spawn(async move {
        let mut sorties = BufReader::new(flux).lines();
        while let Ok(Some(l)) = sorties.next_line().await {
            let mut g = journal.verrou();
            if g.len() == JOURNAL_MAX {
                g.pop_front();
            }
            g.push_back(l);
        }
    });
}

/// Lit l'annonce « PORT JETON » que le sidecar imprime quand il est prêt.
///
/// S'il s'arrête avant (authentification, TLS, NLA…), on remonte la dernière
/// ligne de son diagnostic plutôt qu'un message générique.
async fn lire_annonce(
    stdout: tokio::process::ChildStdout,
    stderr: &mut tokio::process::ChildStderr,
) -> Result<(u16, String), String> {
    let mut lines = BufReader::new(stdout).lines();
    let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? else {
        let mut diag = String::new();
        let _ = stderr.read_to_string(&mut diag).await;
        return Err(message_arret(&diag));
    };
    analyser_annonce(&line)
}

/// Analyse la ligne « PORT JETON » émise par le sidecar. Pure et testée : c'est
/// par elle que passe l'ouverture d'une session — un port hors `u16` ou un jeton
/// manquant doivent être rejetés clairement, pas produire une connexion sans
/// authentification.
fn analyser_annonce(ligne: &str) -> Result<(u16, String), String> {
    let mut it = ligne.split_whitespace();
    let port = it
        .next()
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| "Port WebSocket illisible.".to_owned())?;
    let token = it
        .next()
        .map(str::to_owned)
        .ok_or_else(|| "Jeton WebSocket manquant.".to_owned())?;
    Ok((port, token))
}

/// Message d'erreur quand le sidecar s'arrête sans annoncer de port : on remonte
/// la DERNIÈRE ligne de son diagnostic (« authentification refusée », « TLS »…)
/// plutôt qu'un générique, seul moyen pour l'utilisateur d'apprendre la cause.
fn message_arret(diag: &str) -> String {
    let diag = diag.trim();
    // Une erreur du processus commence par « Error: » et peut tenir sur
    // plusieurs lignes (un certificat qui change en fait cinq, empreintes
    // comprises) : on la rend entière, sinon l'utilisateur ne voyait que la
    // dernière ligne, « retirez la ligne … de rdp_known_hosts », sans le
    // pourquoi ni les empreintes.
    if let Some(debut) = diag.rfind("Error: ") {
        let bloc = diag[debut + "Error: ".len()..].trim();
        if !bloc.is_empty() {
            return bloc.to_owned();
        }
    }
    let msg = diag.lines().last().unwrap_or("").trim();
    if msg.is_empty() {
        "Le sidecar RDP s'est arrêté sans se connecter.".to_owned()
    } else {
        msg.to_owned()
    }
}

/// Nombre de lignes de diagnostic gardées par session. De quoi expliquer une
/// fin de session sans laisser un serveur bavard remplir la mémoire.
const JOURNAL_MAX: usize = 32;

/// Les journaux de toutes les sessions, pour le diagnostic exporté : l'identifiant
/// de session et ses dernières lignes, dans l'ordre des identifiants.
pub(crate) fn journaux(state: &RdpStore) -> Vec<(u64, String)> {
    let mut tous: Vec<(u64, String)> = state
        .journaux
        .verrou()
        .iter()
        .map(|(id, j)| {
            let lignes: Vec<String> = j.verrou().iter().cloned().collect();
            (*id, lignes.join("\n"))
        })
        .collect();
    tous.sort_by_key(|(id, _)| *id);
    tous
}

/// Dernières lignes écrites par le sidecar d'une session.
///
/// L'interface les joint au message de fermeture : sans elles, un onglet RDP
/// qui meurt en cours de route ne dit rien de la raison.
#[tauri::command]
#[must_use]
pub fn rdp_diagnostic(state: tauri::State<'_, RdpStore>, id: u64) -> String {
    state
        .journaux
        .verrou()
        .get(&id)
        .map(|j| j.verrou().iter().cloned().collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

/// Message d'annulation volontaire : le front le reconnaît pour ne pas
/// présenter une fermeture d'onglet comme un échec de connexion.
pub const CONNEXION_ANNULEE: &str = "[AVASH_RDP_ANNULE]";

/// Le serveur ne sait pas faire de NLA. L'interface le reconnaît pour proposer
/// de se connecter quand même, en expliquant ce que cela coûte. Doit rester
/// identique au marqueur émis par le processus RDP.
pub const NLA_INDISPONIBLE: &str = "[AVASH_RDP_SANS_NLA]";

/// Tue le sidecar enregistré sous `id` et rend le message d'erreur tel quel.
///
/// Sans cela, chaque sortie en erreur après le `spawn` abandonnait un processus
/// vivant : `tokio::process::Command` ne tue pas l'enfant à la libération.
/// L'entrée standard gardée pour les désignations part avec lui (audit du
/// 12 septembre 2026 : elle restait dans `stdins` après un échec d'annonce).
async fn tuer(state: &RdpStore, id: u64, msg: String) -> String {
    if let Some(mut child) = state.inner.verrou().remove(&id) {
        let _ = child.start_kill();
    }
    state.journaux.verrou().remove(&id);
    state.stdins.lock().await.remove(&id);
    msg
}

/// Ouvre dans le gestionnaire de fichiers le dossier où des fichiers venus
/// d'un bureau distant ont été reçus. Le chemin vient du processus RDP, par
/// l'interface : on n'ouvre qu'un dossier qui existe, jamais un fichier (un
/// fichier reçu ne doit pas s'exécuter d'un clic sur une notification).
#[tauri::command]
pub async fn rdp_ouvrir_dossier(chemin: String) -> Result<(), String> {
    crate::commands::bloquant(move || {
        let p = std::path::Path::new(&chemin);
        if !p.is_absolute() || !p.is_dir() {
            return Err(format!("{chemin} n'est pas un dossier existant."));
        }
        open::that(p).map_err(|e| format!("Ouverture impossible : {e}"))
    })
    .await
}

#[tauri::command]
pub fn rdp_close(state: tauri::State<'_, RdpStore>, id: u64) -> Result<(), String> {
    if let Some(mut child) = state.inner.verrou().remove(&id) {
        let _ = child.start_kill();
        // Commande synchrone : un « try » sur le verrou tokio suffit, l'entrée
        // standard n'est tenue que le temps d'une annonce.
        if let Ok(mut stdins) = state.stdins.try_lock() {
            stdins.remove(&id);
        }
    }
    state.journaux.verrou().remove(&id);
    Ok(())
}

// ---------- Connexions RDP enregistrées ----------

use avash::rdphost::{self, RdpHost};

#[tauri::command(async)]
pub fn rdp_hosts() -> Result<Vec<RdpHost>, String> {
    rdphost::load_hosts().map_err(|e| e.to_string())
}

/// Un bureau autre que celui d'`id_exclu` utilise-t-il encore ce compte de
/// trousseau ?
///
/// Le compte dérive de `protocole:user@host:port`, jamais de l'`id` du bureau :
/// deux bureaux vers le même serveur (que l'import repère lui-même comme
/// doublons) partagent l'entrée. Avant d'oublier ou de déplacer le secret, on
/// vérifie qu'aucun autre bureau ne le réclame. Trouvé par l'audit du
/// 7 septembre 2026 : supprimer l'un des deux effaçait le mot de passe de l'autre.
#[must_use]
fn compte_encore_utilise(hosts: &[RdpHost], id_exclu: &str, compte: &str) -> bool {
    hosts
        .iter()
        .filter(|h| h.id != id_exclu)
        .any(|h| h.compte_trousseau() == compte)
}

/// Les choix indépendants transmis au processus RDP, chacun sous décision de
/// l'utilisateur ou de la palette. Un seul objet plutôt que quatre booléens
/// positionnels : deux d'entre eux ne peuvent plus s'intervertir sans que ça
/// se voie, ni dans l'appel ni dans le JSON venu du front (`options`).
// Quatre choix binaires réellement indépendants, chacun nommé : un type par
// choix n'apporterait qu'une couche, et un jeu de drapeaux resterait des bools.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    /// L'utilisateur a accepté de se passer d'authentification réseau.
    pub sans_nla: bool,
    /// L'utilisateur a accepté les suites TLS héritées du système.
    pub tls_herite: bool,
    /// Le serveur parle RFB (VNC) plutôt que RDP.
    pub vnc: bool,
    /// Le son du bureau distant est coupé dans la palette.
    pub sans_son: bool,
}

/// Les options du sidecar, dans l'ordre : chacune n'apparaît que demandée.
fn drapeaux(options: Options, partage: Option<&str>) -> Vec<String> {
    let Options {
        sans_nla,
        tls_herite,
        vnc,
        sans_son,
    } = options;
    let mut v = Vec::new();
    if sans_nla {
        v.push("--sans-nla".to_owned());
    }
    if tls_herite {
        v.push("--tls-herite".to_owned());
    }
    if vnc {
        v.push("--vnc".to_owned());
    }
    if sans_son {
        v.push("--sans-son".to_owned());
    }
    if let Some(p) = partage {
        v.push("--lecteur".to_owned());
        v.push(p.to_owned());
    }
    v
}

/// Annonce des chemins désignés par l'utilisateur à un sidecar, une ligne
/// `AUTORISE <chemin>` par chemin. Un chemin qui porte lui-même un saut de
/// ligne ne peut pas s'écrire sur ce protocole ligne à ligne : il n'est pas
/// annoncé, donc jamais offert, ce qui est le côté sûr.
pub(crate) async fn annoncer_dans(
    puits: &mut (impl tokio::io::AsyncWrite + Unpin),
    chemins: &[std::path::PathBuf],
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    for chemin in chemins {
        let texte = chemin.to_string_lossy();
        if texte.contains(['\n', '\r']) {
            continue;
        }
        puits
            .write_all(format!("AUTORISE {texte}\n").as_bytes())
            .await?;
    }
    puits.flush().await
}

/// Annonce des chemins désignés à tous les sidecars en cours. Un sidecar dont
/// l'entrée standard ne répond plus est en train de mourir : son entrée est
/// retirée, `rdp_close` fera le reste.
pub(crate) async fn annoncer_designations(store: &RdpStore, chemins: &[std::path::PathBuf]) {
    if chemins.is_empty() {
        return;
    }
    let mut stdins = store.stdins.lock().await;
    let mut morts = Vec::new();
    for (id, stdin) in stdins.iter_mut() {
        if annoncer_dans(stdin, chemins).await.is_err() {
            morts.push(*id);
        }
    }
    for id in morts {
        stdins.remove(&id);
    }
}

#[cfg(test)]
mod tests_designations {
    use super::annoncer_dans;
    use std::path::PathBuf;

    /// Le format que lit le sidecar (`fichiers::designation_depuis_ligne`) :
    /// une ligne par chemin, préfixée. Un chemin qui contient un saut de ligne
    /// casserait le protocole : il est tu, jamais tronqué ni découpé.
    #[tokio::test]
    async fn les_designations_partent_une_par_ligne() {
        let mut puits: Vec<u8> = Vec::new();
        annoncer_dans(
            &mut puits,
            &[
                PathBuf::from("/home/a/rapport.pdf"),
                PathBuf::from("/home/a/piège\nAUTORISE /etc/shadow"),
                PathBuf::from("/home/a/photos"),
            ],
        )
        .await
        .unwrap();
        assert_eq!(
            String::from_utf8(puits).unwrap(),
            "AUTORISE /home/a/rapport.pdf\nAUTORISE /home/a/photos\n"
        );
    }
}

#[cfg(test)]
mod tests_drapeaux {
    use super::{drapeaux, Options};

    /// Rien de demandé, rien de passé ; tout demandé, tout passé, le dossier
    /// après son drapeau.
    #[test]
    fn les_drapeaux_du_sidecar_suivent_les_options() {
        assert!(drapeaux(Options::default(), None).is_empty());
        let tout = Options {
            sans_nla: true,
            tls_herite: true,
            vnc: true,
            sans_son: true,
        };
        assert_eq!(
            drapeaux(tout, Some("/srv/partage")),
            [
                "--sans-nla",
                "--tls-herite",
                "--vnc",
                "--sans-son",
                "--lecteur",
                "/srv/partage"
            ]
        );
        let sans_son = Options {
            sans_son: true,
            ..Options::default()
        };
        assert_eq!(drapeaux(sans_son, None), ["--sans-son"]);
        // Le TLS hérité seul, sans renoncer à NLA : les deux choix sont distincts.
        let herite = Options {
            tls_herite: true,
            ..Options::default()
        };
        assert_eq!(drapeaux(herite, None), ["--tls-herite"]);
    }
}

/// Le dossier partagé tel que l'interface le donne : vide vaut « rien », et
/// tout le reste doit être un dossier existant, en chemin absolu.
fn dossier_partage(partage: Option<String>) -> Result<Option<String>, String> {
    let partage = partage
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty());
    if let Some(p) = &partage {
        let chemin = std::path::Path::new(p);
        if !chemin.is_absolute() || !chemin.is_dir() {
            return Err(format!(
                "Le dossier à partager n'existe pas ou n'est pas un chemin absolu : {p}"
            ));
        }
    }
    Ok(partage)
}

/// Cree (`id` absent) ou modifie une connexion RDP enregistree.
///
/// `protocole` : « rdp » (défaut) ou « vnc ».
// Une commande Tauri reflète les champs de la fiche, un par argument.
#[allow(clippy::too_many_arguments)]
#[tauri::command(async)]
pub fn rdp_host_save(
    choix: tauri::State<'_, crate::commands::ChoixLocaux>,
    id: Option<String>,
    name: String,
    host: String,
    port: u16,
    user: String,
    width: u16,
    height: u16,
    folder: Option<String>,
    protocole: Option<String>,
    partage: Option<String>,
) -> Result<RdpHost, String> {
    let mut h = RdpHost::new(&name, &host, port, &user, width, height)
        .en(rdphost::Protocole::depuis(protocole.as_deref()));
    if let Some(id) = id.filter(|i| !i.is_empty()) {
        h.id = id;
    }
    h.folder = avash::folders::normalize(&folder.unwrap_or_default());
    // Le dossier partagé est vérifié à l'enregistrement, pas seulement à la
    // connexion : une faute de frappe se voit tout de suite, dans la fiche.
    h.partage = dossier_partage(partage)?;
    // Contrat K12 (audit de sécurité du 12 septembre 2026, C-ipc-1) : le
    // dossier servi au bureau distant vient de la boîte de sélection native,
    // ou reste celui que la fiche portait déjà. Une chaîne libre venue de la
    // page aurait partagé `~` entier avec le serveur d'un script hostile.
    if let Some(p) = &h.partage {
        let deja = rdphost::load_hosts_brut_from(&rdphost::hosts_path())
            .ok()
            .and_then(|tous| tous.into_iter().find(|x| x.id == h.id))
            .and_then(|x| x.partage);
        if !choix.designe(std::path::Path::new(p)) && deja.as_deref() != Some(p.as_str()) {
            return Err(format!(
                "Le dossier à partager « {p} » n'a pas été choisi par la boîte de sélection : \
                 utilise le bouton « Choisir… »."
            ));
        }
    }
    rdphost::upsert_host_in(&rdphost::hosts_path(), h.clone()).map_err(|e| e.to_string())?;
    Ok(h)
}

/// Range un bureau RDP dans un dossier (déplacement).
#[tauri::command(async)]
pub fn rdp_host_set_folder(id: String, folder: String) -> Result<(), String> {
    // Liste brute : ranger un bureau ne doit pas effacer du fichier les autres
    // entrées invalides (audit du 7 septembre 2026). `rdp_open` revalide, donc
    // toucher une entrée invalide existante est sans danger.
    let mut all =
        rdphost::load_hosts_brut_from(&rdphost::hosts_path()).map_err(|e| e.to_string())?;
    let norm = avash::folders::normalize(&folder);
    let h = all
        .iter_mut()
        .find(|h| h.id == id)
        .ok_or("Bureau RDP introuvable.")?;
    h.folder.clone_from(&norm);
    rdphost::save_hosts_to(&rdphost::hosts_path(), &all).map_err(|e| e.to_string())?;
    if !norm.is_empty() {
        avash::folders::create(&norm).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Supprime une connexion enregistree et oublie son mot de passe.
#[tauri::command]
pub async fn rdp_host_delete(id: String) -> Result<(), String> {
    crate::commands::bloquant(move || supprimer_bureau(&id)).await
}

fn supprimer_bureau(id: &str) -> Result<(), String> {
    // Compte du trousseau AVANT suppression : après, le bureau n'existe plus et
    // on ne saurait plus lequel oublier.
    let compte = rdphost::load_hosts().ok().and_then(|hosts| {
        hosts
            .iter()
            .find(|h| h.id == id)
            .map(RdpHost::compte_trousseau)
    });
    // On supprime d'abord, on n'oublie le secret qu'APRÈS le succès (comme côté
    // SSH depuis 664d45e : dans l'autre ordre, une suppression qui échouait
    // perdait quand même le mot de passe) et seulement si aucun autre bureau ne
    // partage ce compte. Trouvé par l'audit du 7 septembre 2026 : deux bureaux
    // vers le même user@host:port partagent l'entrée, supprimer l'un l'effaçait
    // pour l'autre.
    let restants =
        rdphost::remove_host_in(&rdphost::hosts_path(), id).map_err(|e| e.to_string())?;
    if let Some(compte) = compte {
        if !compte_encore_utilise(&restants, id, &compte) {
            let _ = avash::secrets::forget(&compte);
        }
    }
    Ok(())
}

/// Le compte du trousseau d'un bureau, d'après le protocole que le front
/// nomme (« rdp » par défaut, « vnc »).
fn compte(protocole: Option<&str>, user: &str, host: &str, port: u16) -> String {
    rdphost::keyring_account_pour(rdphost::Protocole::depuis(protocole), user, host, port)
}

#[tauri::command]
pub async fn rdp_password_save(
    host: String,
    port: u16,
    user: String,
    password: String,
    protocole: Option<String>,
) -> Result<(), String> {
    let password = zeroize::Zeroizing::new(password);
    // Même garde qu'à l'ouverture (C-secrets-1) : un mot de passe à saut de
    // ligne dormirait au trousseau, puis `rdp_open` le relirait et l'écrirait
    // sur l'entrée standard du sidecar, désignations piégées comprises.
    mot_de_passe_transmissible(&password)?;
    let id = compte(protocole.as_deref(), &user, &host, port);
    crate::commands::bloquant(move || {
        avash::secrets::save(&id, &password).map_err(|e| format!("{e:#}"))
    })
    .await
}

#[tauri::command]
#[must_use]
/// Un mot de passe est-il mémorisé pour ce bureau ?
///
/// Ne renvoie **que** l'existence, jamais le secret : celui-ci reste côté natif
/// et n'entre pas dans le tas de la webview, où il survivait jusque-là toute la
/// durée de l'onglet (conservé pour la reconnexion). Le volet SSH procède ainsi
/// depuis toujours (`password_known`).
pub async fn rdp_password_known<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    host: String,
    port: u16,
    user: String,
    protocole: Option<String>,
) -> bool {
    let compte = compte(protocole.as_deref(), &user, &host, port);
    matches!(
        crate::commands::charger_secret(&app, compte).await,
        Ok(Some(_))
    )
}

/// Déplace le secret d'un compte vers un autre lors d'une modification de bureau.
///
/// La migration se faisait côté interface, en relisant le mot de passe pour le
/// réécrire : le secret traversait l'IPC deux fois de plus, à la seule fin de
/// changer de clé. Le trousseau est ici manipulé sans que le secret ne quitte
/// le processus natif.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn rdp_password_move(
    old_host: String,
    old_port: u16,
    old_user: String,
    host: String,
    port: u16,
    user: String,
    old_protocole: Option<String>,
    protocole: Option<String>,
) -> Result<(), String> {
    let ancien = compte(old_protocole.as_deref(), &old_user, &old_host, old_port);
    let nouveau = compte(protocole.as_deref(), &user, &host, port);
    crate::commands::bloquant(move || deplacer_mot_de_passe(&ancien, &nouveau)).await
}

fn deplacer_mot_de_passe(ancien: &str, nouveau: &str) -> Result<(), String> {
    let Some(secret) = avash::secrets::load(ancien).map(zeroize::Zeroizing::new) else {
        return Ok(()); // rien à déplacer
    };
    avash::secrets::save(nouveau, &secret).map_err(|e| format!("{e:#}"))?;
    // L'oubli n'a lieu qu'après une écriture réussie (l'inverse perdrait le
    // secret si le trousseau refusait la nouvelle entrée) et seulement si aucun
    // autre bureau ne partage encore l'ancien compte. Le bureau édité a déjà été
    // enregistré avec le nouveau compte (rdp_host_save précède cet appel), donc
    // il ne compte plus pour l'ancien. Trouvé par l'audit du 7 septembre 2026 :
    // deux bureaux vers le même serveur partagent l'entrée, déplacer l'un
    // effaçait le mot de passe de l'autre.
    let partage =
        rdphost::load_hosts().is_ok_and(|hosts| compte_encore_utilise(&hosts, "", ancien));
    if partage {
        Ok(())
    } else {
        avash::secrets::forget(ancien).map_err(|e| format!("{e:#}"))
    }
}

/// Retient qu'un serveur ne sait pas faire de NLA, après accord de l'utilisateur.
///
/// Sans cela il faudrait redonner cet accord à chaque connexion. Le choix est
/// par serveur, jamais global : accepter pour un xrdp mal configuré ne doit pas
/// relâcher la garde pour les autres.
#[tauri::command(async)]
pub fn rdp_host_set_sans_nla(id: String, valeur: bool) -> Result<(), String> {
    let chemin = avash::rdphost::hosts_path();
    // Liste brute : basculer le NLA d'un bureau ne doit pas effacer du fichier
    // les autres entrées invalides (audit du 7 septembre 2026).
    let mut tous = avash::rdphost::load_hosts_brut_from(&chemin).map_err(|e| format!("{e:#}"))?;
    let Some(h) = tous.iter_mut().find(|h| h.id == id) else {
        return Err(format!("Bureau RDP inconnu : {id}"));
    };
    h.sans_nla = valeur;
    avash::rdphost::save_hosts_to(&chemin, &tous).map_err(|e| format!("{e:#}"))
}

/// Retient qu'un serveur n'a que des suites TLS héritées, après accord de
/// l'utilisateur. Même règle que `rdp_host_set_sans_nla` : par serveur, jamais
/// global, et sans effacer les autres entrées du fichier.
#[tauri::command(async)]
pub fn rdp_host_set_tls_herite(id: String, valeur: bool) -> Result<(), String> {
    let chemin = avash::rdphost::hosts_path();
    let mut tous = avash::rdphost::load_hosts_brut_from(&chemin).map_err(|e| format!("{e:#}"))?;
    let Some(h) = tous.iter_mut().find(|h| h.id == id) else {
        return Err(format!("Bureau RDP inconnu : {id}"));
    };
    h.tls_herite = valeur;
    avash::rdphost::save_hosts_to(&chemin, &tous).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn rdp_password_forget(
    host: String,
    port: u16,
    user: String,
    protocole: Option<String>,
) -> Result<(), String> {
    let id = compte(protocole.as_deref(), &user, &host, port);
    crate::commands::bloquant(move || avash::secrets::forget(&id).map_err(|e| format!("{e:#}")))
        .await
}

#[cfg(test)]
mod tests_chemin_sidecar {
    use super::sidecar_path_depuis;
    use std::ffi::OsString;
    use std::path::PathBuf;

    /// Le repli relatif `rdp-sidecar/target/release/avash-rdp` était résolu
    /// depuis le répertoire courant : lancée depuis un répertoire où un autre
    /// compte peut écrire, l'application y exécutait le binaire déposé et lui
    /// confiait le mot de passe RDP sur l'entrée standard. Quoi qu'elle rende,
    /// cette fonction doit rendre un chemin absolu — ou rien.
    ///
    /// Depuis l'audit du 12 septembre 2026 (C-unsafe-3), la décision est pure :
    /// plus de `set_var` en test, donc plus de course avec les autres tests du
    /// binaire qui lisent l'environnement.
    #[test]
    fn le_chemin_rendu_n_est_jamais_relatif() {
        let exe = std::env::current_exe().ok();
        assert!(
            sidecar_path_depuis(None, exe.clone()).is_none_or(|p| p.is_absolute()),
            "le repli ne doit pas dépendre du répertoire courant"
        );
        // Une variable d'environnement relative rouvrirait la même porte.
        assert_eq!(
            sidecar_path_depuis(Some(OsString::from("avash-rdp")), exe),
            None,
            "AVASH_RDP_BIN relative doit être refusée"
        );
        // Sans exécutable connu, rien n'est deviné.
        assert_eq!(sidecar_path_depuis(None, None), None);
    }

    /// Une variable absolue est prise telle quelle ; sinon, le binaire posé à
    /// côté de l'application est trouvé, extension comprise.
    #[test]
    fn la_variable_absolue_puis_le_voisin_de_l_executable() {
        let absolu = std::env::temp_dir().join("ailleurs").join("avash-rdp");
        assert_eq!(
            sidecar_path_depuis(Some(absolu.clone().into_os_string()), None),
            Some(absolu)
        );
        let dir = std::env::temp_dir().join(format!(
            "avash-sidecar-voisin-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let nom = format!("avash-rdp{}", std::env::consts::EXE_SUFFIX);
        let exe: PathBuf = dir.join("avash-ui");
        assert_eq!(sidecar_path_depuis(None, Some(exe.clone())), None);
        std::fs::write(dir.join(&nom), b"").unwrap();
        assert_eq!(sidecar_path_depuis(None, Some(exe)), Some(dir.join(&nom)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests_ouverture {
    use super::{rdp_close, rdp_diagnostic, rdp_open, RdpStore};
    use crate::commands::tests::app_de_test;
    use tauri::Manager as _;

    /// L'adresse arrive du front telle quelle jusqu'à la clé du fichier
    /// d'empreintes : une espace suffit à désarmer le TOFU en silence. Elle
    /// doit être refusée AVANT tout lancement de processus.
    #[tokio::test]
    async fn une_adresse_a_espace_est_refusee_avant_de_lancer_quoi_que_ce_soit() {
        let app = app_de_test();
        let issue = rdp_open(
            app.handle().clone(),
            1,
            "hote avec espace".into(),
            None,
            "u".into(),
            "p".into(),
            800,
            600,
            100,
            super::Options::default(),
            None,
        )
        .await;
        let Err(e) = issue else {
            panic!("une adresse à espace a été acceptée")
        };
        assert!(e.contains("caractère interdit"), "{e}");
        assert!(app.state::<RdpStore>().inner.lock().unwrap().is_empty());
    }

    /// Un dossier partagé qui n'existe pas, ou un chemin relatif, est refusé
    /// avant de lancer le sidecar : l'utilisateur lit la raison dans la
    /// fiche, pas un bureau qui se ferme.
    #[tokio::test]
    async fn un_dossier_partage_absent_est_refuse_avant_de_lancer_quoi_que_ce_soit() {
        let app = app_de_test();
        for dossier in ["/ce/dossier/n/existe/pas", "relatif/partage"] {
            let issue = rdp_open(
                app.handle().clone(),
                3,
                "hote".into(),
                None,
                "u".into(),
                "p".into(),
                800,
                600,
                100,
                super::Options::default(),
                Some(dossier.to_owned()),
            )
            .await;
            let Err(e) = issue else {
                panic!("un dossier absent a été accepté : {dossier}")
            };
            assert!(e.contains("dossier à partager"), "{e}");
        }
        assert!(app.state::<RdpStore>().inner.lock().unwrap().is_empty());
    }

    /// En VNC l'utilisateur est facultatif : ce n'est pas lui qui doit
    /// arrêter la connexion. L'adresse, elle, reste contrôlée.
    #[tokio::test]
    async fn en_vnc_un_utilisateur_vide_ne_bloque_pas_mais_l_adresse_reste_controlee() {
        let app = app_de_test();
        let issue = rdp_open(
            app.handle().clone(),
            2,
            "hote avec espace".into(),
            None,
            String::new(),
            "p".into(),
            800,
            600,
            100,
            super::Options {
                vnc: true,
                ..super::Options::default()
            },
            None,
        )
        .await;
        let Err(e) = issue else {
            panic!("une adresse à espace a été acceptée en VNC")
        };
        assert!(
            e.contains("caractère interdit"),
            "utilisateur vide refusé avant l'adresse : {e}"
        );
    }

    /// Trouvé par l'audit de sécurité du 12 septembre 2026 (C-secrets-1) : le
    /// protocole de l'entrée standard du sidecar est « une ligne, un message »,
    /// le mot de passe d'abord, puis des lignes `AUTORISE <chemin>`. Un mot de
    /// passe qui porte un saut de ligne faisait de la suite une désignation :
    /// un script de la webview offrait ainsi `~/.ssh/id_ed25519` au serveur.
    /// Le refus doit tomber avant tout lancement de processus.
    #[tokio::test]
    async fn un_mot_de_passe_a_saut_de_ligne_est_refuse_avant_de_lancer_le_sidecar() {
        let app = app_de_test();
        for piege in ["x\nAUTORISE /etc/passwd", "x\rAUTORISE /etc/passwd", "x\0y"] {
            let issue = rdp_open(
                app.handle().clone(),
                1,
                "hote".into(),
                None,
                "u".into(),
                piege.into(),
                800,
                600,
                100,
                super::Options::default(),
                None,
            )
            .await;
            let Err(e) = issue else {
                panic!("un mot de passe piégé a été accepté : {piege:?}")
            };
            assert!(e.contains("saut de ligne"), "{e}");
        }
        assert!(app.state::<RdpStore>().inner.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn fermer_ou_diagnostiquer_une_session_inconnue_ne_casse_rien() {
        let app = app_de_test();
        assert!(rdp_close(app.state::<RdpStore>(), 42).is_ok());
        assert_eq!(rdp_diagnostic(app.state::<RdpStore>(), 42), "");
    }
}

#[cfg(test)]
mod tests_annonce {
    use super::{analyser_annonce, message_arret};

    #[test]
    fn une_annonce_valide_donne_port_et_jeton() {
        assert_eq!(
            analyser_annonce("5000 abcdef0123456789"),
            Ok((5000, "abcdef0123456789".to_owned()))
        );
        // Espaces multiples tolérés (split_whitespace).
        assert_eq!(
            analyser_annonce("  42   jeton  "),
            Ok((42, "jeton".to_owned()))
        );
    }

    #[test]
    fn un_port_illisible_est_refuse() {
        // Hors u16, non numérique, ou vide : jamais une connexion silencieuse.
        assert!(analyser_annonce("70000 jeton").is_err()); // > 65535
        assert!(analyser_annonce("pas-un-port jeton").is_err());
        assert!(analyser_annonce("").is_err());
    }

    #[test]
    fn un_jeton_manquant_est_refuse() {
        // Un port seul, sans jeton, ne doit pas ouvrir de session non authentifiée.
        let e = analyser_annonce("5000").unwrap_err();
        assert!(e.contains("Jeton"), "message inattendu : {e}");
    }

    #[test]
    fn l_arret_remonte_la_derniere_ligne_du_diagnostic() {
        // C'est le seul chemin par lequel l'utilisateur apprend la vraie cause.
        assert_eq!(
            message_arret("connexion…\nauthentification refusée"),
            "authentification refusée"
        );
        // Diagnostic vide : message générique plutôt qu'une chaîne vide.
        assert_eq!(
            message_arret("   \n  "),
            "Le sidecar RDP s'est arrêté sans se connecter."
        );
        // Une erreur sur plusieurs lignes (certificat changé) revient entière,
        // pas réduite à sa dernière ligne (régression vue avec VeNCrypt).
        assert_eq!(
            message_arret("connexion…\nError: Le certificat a changé.\n\nEmpreinte : abc\nRetirez la ligne.\n"),
            "Le certificat a changé.\n\nEmpreinte : abc\nRetirez la ligne."
        );
    }
}

#[cfg(test)]
mod tests_compte_partage {
    use super::*;

    fn bureau(id: &str, host: &str, port: u16, user: &str) -> RdpHost {
        let mut h = RdpHost::new("", host, port, user, 0, 0);
        h.id = id.to_string();
        h
    }

    #[test]
    fn deux_bureaux_vers_le_meme_serveur_partagent_le_compte() {
        // Trouvé par l'audit du 7 septembre 2026 : le compte du trousseau dérive
        // de `rdp:user@host:port`, jamais de l'`id` du bureau. Deux bureaux vers
        // le même serveur partagent l'entrée ; supprimer l'un ne doit pas oublier
        // le mot de passe tant que l'autre le réclame.
        let a = bureau("a", "srv", 3389, "admin");
        let b = bureau("b", "srv", 3389, "admin");
        let compte = a.compte_trousseau();
        let restants = vec![b];
        assert!(
            compte_encore_utilise(&restants, "a", &compte),
            "le bureau b réclame encore le compte : ne pas l'oublier"
        );
    }

    #[test]
    fn un_seul_bureau_vers_le_serveur_laisse_oublier_le_compte() {
        let a = bureau("a", "srv", 3389, "admin");
        let compte = a.compte_trousseau();
        let restants = vec![a];
        assert!(
            !compte_encore_utilise(&restants, "a", &compte),
            "plus aucun autre bureau : le compte est orphelin"
        );
    }

    #[test]
    fn un_port_different_ne_partage_pas_le_compte() {
        let a = bureau("a", "srv", 3389, "admin");
        let b = bureau("b", "srv", 3390, "admin");
        let compte = a.compte_trousseau();
        assert!(
            !compte_encore_utilise(&[b], "a", &compte),
            "port 3390 distinct : compte différent"
        );
    }
}

#[cfg(test)]
mod tests_placement_attributs {
    /// Trouvé par l'audit du 7 septembre 2026 : un attribut (`#[allow(...)]`,
    /// `#[tauri::command]`…) glissé ENTRE deux lignes `///` sépare un bloc de
    /// doc de son item. Rustdoc rattache alors ce premier `///` à l'item qui
    /// suit : ici la doc de `rdp_host_save` (« Cree… / `protocole` : … ») s'était
    /// collée à la fonction privée `drapeaux`, qui n'a ni création ni protocole,
    /// et `rdp_host_save` se retrouvait sans doc. Vestige d'un déplacement de
    /// code. La règle : l'attribut précède TOUS les `///` de l'item. Ce garde
    /// relit le source et refuse le motif ; aucun test de comportement ne le voit.
    #[test]
    fn aucun_attribut_intercale_entre_deux_blocs_de_doc() {
        let lignes: Vec<&str> = include_str!("rdp.rs").lines().collect();
        let precedent_non_vide = |i: usize| (0..i).rev().find(|&j| !lignes[j].trim().is_empty());
        let suivant_non_vide =
            |i: usize| (i + 1..lignes.len()).find(|&j| !lignes[j].trim().is_empty());

        let mut fautes = Vec::new();
        for (i, ligne) in lignes.iter().enumerate() {
            if !ligne.trim().starts_with("#[") {
                continue;
            }
            let avant_est_doc =
                precedent_non_vide(i).is_some_and(|j| lignes[j].trim().starts_with("///"));
            let apres_est_doc =
                suivant_non_vide(i).is_some_and(|j| lignes[j].trim().starts_with("///"));
            if avant_est_doc && apres_est_doc {
                fautes.push(i + 1); // ligne 1-indexée, comme un éditeur l'affiche
            }
        }
        assert!(
            fautes.is_empty(),
            "attribut(s) intercalé(s) entre deux blocs `///`, ligne(s) {fautes:?} : \
             l'attribut doit précéder tous les `///` de l'item"
        );
    }
}

/// Tests de `ouvrir_avec` avec un sidecar factice (`tests/fixtures/sidecar-factice.sh`).
///
/// Audit du 12 septembre 2026 (C-couv-2) : au-delà de la validation, rien de
/// l'ouverture n'était exercé hors de la suite bout en bout, ni le contrat
/// « mot de passe par l'entrée standard » (SECURITY.md), ni les chemins
/// d'échec du sidecar (mort avant l'annonce, annonce illisible, onglet fermé
/// pendant la connexion). Le script factice rejoue chacun d'eux.
// `cfg(test)` seul d'abord, la plateforme ensuite : clippy ne reconnaît pas
// `cfg(all(test, …))` comme un module de test (`allow-unwrap-in-tests`).
#[cfg(test)]
#[cfg(unix)]
mod tests_sidecar_factice {
    use super::{
        ouvrir_avec, rdp_close, rdp_diagnostic, rdp_host_delete, rdp_host_save, rdp_ouvrir_dossier,
        rdp_password_forget, rdp_password_known, rdp_password_move, rdp_password_save, Demande,
        Options, RdpStore, CONNEXION_ANNULEE,
    };
    use crate::commands::tests::{app_de_test, with_ssh_config};
    use crate::commands::ChoixLocaux;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tauri::Manager as _;

    static SUITE: AtomicU64 = AtomicU64::new(0);

    /// Annonce un port et un jeton après avoir lu le mot de passe, écrit une
    /// erreur, puis attend (remplacé par `sleep` : tuer le shell tue l'attente).
    const ANNONCE_PUIS_ATTENTE: &str = "IFS= read -r mdp\n\
        printf '%s' \"$mdp\" > \"$D/stdin\"\n\
        echo '5000 jeton-factice'\n\
        echo 'Error: coupure simulée' >&2\n\
        exec sleep 30\n";

    /// Le dossier d'un scénario : son nom sert d'adresse au sidecar factice.
    struct Factice {
        hote: String,
        dir: PathBuf,
    }

    impl Factice {
        fn nouveau(scenario: &str) -> Self {
            let n = SUITE.fetch_add(1, Ordering::Relaxed);
            let hote = format!("factice-{}-{n}", std::process::id());
            let dir = std::env::temp_dir().join(&hote);
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("scenario.sh"), scenario).unwrap();
            Self { hote, dir }
        }

        fn bin() -> PathBuf {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sidecar-factice.sh")
        }

        fn lire(&self, nom: &str) -> Option<String> {
            std::fs::read_to_string(self.dir.join(nom)).ok()
        }

        fn demande(&self, id: u64, password: &str) -> Demande {
            Demande {
                id,
                host: self.hote.clone(),
                port: Some(3389),
                user: "admin".into(),
                password: zeroize::Zeroizing::new(password.to_owned()),
                width: 800,
                height: 600,
                echelle: 100,
                options: Options::default(),
                partage: None,
            }
        }
    }

    impl Drop for Factice {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Le processus `pid` est-il encore vivant (ni disparu, ni zombie) ?
    #[cfg(target_os = "linux")]
    fn vivant(pid: u32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
            let etat = s
                .rsplit_once(')')
                .and_then(|(_, reste)| reste.trim_start().chars().next());
            !matches!(etat, Some('Z' | 'X'))
        })
    }

    /// Attend (au plus deux secondes) que `pid` meure.
    #[cfg(target_os = "linux")]
    async fn mort_sous_deux_secondes(pid: u32) -> bool {
        for _ in 0..40 {
            if !vivant(pid) {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        false
    }

    fn vide(app: &tauri::App<tauri::test::MockRuntime>) -> bool {
        let s = app.state::<RdpStore>();
        let vide = s.inner.lock().unwrap().is_empty() && s.journaux.lock().unwrap().is_empty();
        vide && s.stdins.try_lock().is_ok_and(|g| g.is_empty())
    }

    /// Le contrat de SECURITY.md : un bureau enregistré ouvre sans que son mot
    /// de passe ne traverse l'IPC (champ vide, relu au trousseau côté natif),
    /// et le secret part par l'entrée standard, jamais sur la ligne de
    /// commande, lisible dans `/proc/<pid>/cmdline` par les autres comptes.
    #[tokio::test]
    async fn le_mot_de_passe_part_par_l_entree_standard_jamais_en_argument() {
        let _g = with_ssh_config("");
        let f = Factice::nouveau(ANNONCE_PUIS_ATTENTE);
        let app = app_de_test();
        rdp_password_save(f.hote.clone(), 3389, "admin".into(), "s3cr3t".into(), None)
            .await
            .unwrap();
        let conn = ouvrir_avec(app.handle(), Some(Factice::bin()), f.demande(1, ""))
            .await
            .unwrap();
        assert_eq!((conn.port, conn.token.as_str()), (5000, "jeton-factice"));
        assert_eq!(f.lire("stdin").as_deref(), Some("s3cr3t"));
        let argv = f.lire("argv").unwrap();
        assert!(!argv.contains("s3cr3t"), "{argv}");
        let attendu = format!(
            "--host\n{}\n--port\n3389\n-u\nadmin\n--width\n800\n--height\n600\n--scale\n100\n",
            f.hote
        );
        assert_eq!(argv, attendu);
        rdp_close(app.state::<RdpStore>(), 1).unwrap();
        assert!(vide(&app));
    }

    /// Une saisie à l'instant prime sur ce que le trousseau garde.
    #[tokio::test]
    async fn un_mot_de_passe_saisi_prime_sur_le_trousseau() {
        let _g = with_ssh_config("");
        let f = Factice::nouveau(ANNONCE_PUIS_ATTENTE);
        let app = app_de_test();
        rdp_password_save(f.hote.clone(), 3389, "admin".into(), "ancien".into(), None)
            .await
            .unwrap();
        ouvrir_avec(app.handle(), Some(Factice::bin()), f.demande(1, "tape"))
            .await
            .unwrap();
        assert_eq!(f.lire("stdin").as_deref(), Some("tape"));
        rdp_close(app.state::<RdpStore>(), 1).unwrap();
    }

    /// Les choix de la palette et le dossier partagé (désigné) arrivent au
    /// processus, dans l'ordre : le test de `drapeaux` prouve l'ordre, celui-ci
    /// prouve qu'ils sont passés.
    #[tokio::test]
    async fn les_options_arrivent_au_sidecar_dans_l_ordre() {
        let _g = with_ssh_config("");
        let f = Factice::nouveau(ANNONCE_PUIS_ATTENTE);
        let app = app_de_test();
        let partage = f.dir.join("partage");
        std::fs::create_dir_all(&partage).unwrap();
        app.state::<ChoixLocaux>().retenir([partage.clone()]);
        let mut d = f.demande(1, "p");
        d.options = Options {
            sans_nla: true,
            tls_herite: true,
            vnc: true,
            sans_son: true,
        };
        d.partage = Some(partage.display().to_string());
        ouvrir_avec(app.handle(), Some(Factice::bin()), d)
            .await
            .unwrap();
        let argv = f.lire("argv").unwrap();
        let fin = format!(
            "--sans-nla\n--tls-herite\n--vnc\n--sans-son\n--lecteur\n{}\n",
            partage.display()
        );
        assert!(argv.ends_with(&fin), "{argv}");
        rdp_close(app.state::<RdpStore>(), 1).unwrap();
    }

    /// Un sidecar qui meurt avant son annonce : l'utilisateur lit SA dernière
    /// erreur, et rien ne reste dans le magasin (enfant, journal, entrée
    /// standard).
    #[tokio::test]
    async fn un_sidecar_mort_avant_l_annonce_remonte_sa_derniere_erreur_et_ne_laisse_rien() {
        let f = Factice::nouveau(
            "IFS= read -r mdp\necho 'Error: authentification refusée' >&2\nexit 1\n",
        );
        let app = app_de_test();
        let e = ouvrir_avec(app.handle(), Some(Factice::bin()), f.demande(1, "p"))
            .await
            .err()
            .unwrap();
        assert!(e.contains("authentification refusée"), "{e}");
        assert!(vide(&app));
    }

    /// Une annonce qui n'est pas « PORT JETON » : refus nommé, enfant tué.
    #[tokio::test]
    async fn une_annonce_illisible_tue_le_sidecar() {
        let f = Factice::nouveau("IFS= read -r mdp\necho bidule\nexec sleep 30\n");
        let app = app_de_test();
        let e = ouvrir_avec(app.handle(), Some(Factice::bin()), f.demande(1, "p"))
            .await
            .err()
            .unwrap();
        assert_eq!(e, "Port WebSocket illisible.");
        assert!(vide(&app));
    }

    /// Ce que le sidecar écrit après l'ouverture reste lisible pour le message
    /// de fermeture, même une fois mort ; fermer l'onglet l'efface.
    #[tokio::test]
    async fn les_dernieres_lignes_du_sidecar_sont_lisibles_apres_sa_mort() {
        let f = Factice::nouveau(
            "IFS= read -r mdp\necho '5000 j'\necho 'Error: coupure simulée' >&2\nexit 0\n",
        );
        let app = app_de_test();
        ouvrir_avec(app.handle(), Some(Factice::bin()), f.demande(1, "p"))
            .await
            .unwrap();
        let mut lu = String::new();
        for _ in 0..40 {
            lu = rdp_diagnostic(app.state::<RdpStore>(), 1);
            if lu.contains("coupure simulée") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(lu.contains("coupure simulée"), "{lu:?}");
        rdp_close(app.state::<RdpStore>(), 1).unwrap();
        assert_eq!(rdp_diagnostic(app.state::<RdpStore>(), 1), "");
    }

    /// Un serveur bavard ne remplit pas la mémoire : trente-deux lignes, les
    /// plus récentes.
    #[tokio::test]
    async fn le_journal_du_sidecar_est_borne_a_trente_deux_lignes() {
        let f = Factice::nouveau(
            "IFS= read -r mdp\necho '5000 j'\n\
             i=1; while [ $i -le 100 ]; do echo \"ligne $i\" >&2; i=$((i+1)); done\n\
             exec sleep 30\n",
        );
        let app = app_de_test();
        ouvrir_avec(app.handle(), Some(Factice::bin()), f.demande(1, "p"))
            .await
            .unwrap();
        let mut lu = String::new();
        for _ in 0..40 {
            lu = rdp_diagnostic(app.state::<RdpStore>(), 1);
            if lu.ends_with("ligne 100") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let lignes: Vec<&str> = lu.lines().collect();
        assert_eq!(lignes.len(), 32, "{lu}");
        assert_eq!(lignes.first(), Some(&"ligne 69"));
        rdp_close(app.state::<RdpStore>(), 1).unwrap();
    }

    /// Onglet fermé pendant la connexion : le front doit recevoir le marqueur
    /// d'annulation, pas « le sidecar s'est arrêté sans se connecter ».
    #[tokio::test]
    async fn fermer_l_onglet_pendant_la_connexion_rend_le_marqueur_d_annulation() {
        let f = Factice::nouveau("IFS= read -r mdp\n: > \"$D/lu\"\nexec sleep 30\n");
        let app = app_de_test();
        let h = app.handle().clone();
        let d = f.demande(1, "p");
        let tache = tokio::spawn(async move { ouvrir_avec(&h, Some(Factice::bin()), d).await });
        for _ in 0..100 {
            if f.lire("lu").is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            f.lire("lu").is_some(),
            "le sidecar factice n'a pas lu le mot de passe"
        );
        rdp_close(app.state::<RdpStore>(), 1).unwrap();
        let issue = tache.await.unwrap();
        assert_eq!(issue.err().as_deref(), Some(CONNEXION_ANNULEE));
        assert!(vide(&app));
    }

    /// Deux ouvertures sous le même onglet : la seconde remplace la première,
    /// qui est tuée, pas abandonnée.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn deux_ouvertures_sous_le_meme_id_tuent_la_premiere() {
        let f = Factice::nouveau(ANNONCE_PUIS_ATTENTE);
        let app = app_de_test();
        ouvrir_avec(app.handle(), Some(Factice::bin()), f.demande(1, "p"))
            .await
            .unwrap();
        let premier = app.state::<RdpStore>().inner.lock().unwrap()[&1]
            .id()
            .unwrap();
        ouvrir_avec(app.handle(), Some(Factice::bin()), f.demande(1, "p"))
            .await
            .unwrap();
        let second = app.state::<RdpStore>().inner.lock().unwrap()[&1]
            .id()
            .unwrap();
        assert_ne!(premier, second);
        assert_eq!(app.state::<RdpStore>().inner.lock().unwrap().len(), 1);
        assert!(
            mort_sous_deux_secondes(premier).await,
            "le premier sidecar survit"
        );
        rdp_close(app.state::<RdpStore>(), 1).unwrap();
        assert!(mort_sous_deux_secondes(second).await);
    }

    /// Audit de sécurité du 12 septembre 2026 (C-sidecar-1) : un enfant lâché
    /// sans `rdp_close` (magasin libéré à la sortie de l'application) doit
    /// mourir avec sa poignée, sinon la session RDP authentifiée survit.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn un_sidecar_lache_sans_rdp_close_est_tue() {
        let f = Factice::nouveau(ANNONCE_PUIS_ATTENTE);
        let app = app_de_test();
        ouvrir_avec(app.handle(), Some(Factice::bin()), f.demande(1, "p"))
            .await
            .unwrap();
        let enfant = app
            .state::<RdpStore>()
            .inner
            .lock()
            .unwrap()
            .remove(&1)
            .unwrap();
        let pid = enfant.id().unwrap();
        drop(enfant);
        assert!(
            mort_sous_deux_secondes(pid).await,
            "le sidecar lâché survit"
        );
        rdp_close(app.state::<RdpStore>(), 1).unwrap();
    }

    /// Audit du 12 septembre 2026 (C-SIL-13) : un sidecar qui n'annonce jamais
    /// rien (montage réseau lent, `localectl` bloqué) laissait l'onglet sur
    /// « connexion » sans fin. Temps figé : les soixante secondes passent d'un
    /// coup, le processus réel dort toujours.
    #[tokio::test(start_paused = true)]
    async fn l_annonce_du_sidecar_est_bornee() {
        let f = Factice::nouveau("IFS= read -r mdp\nexec sleep 30\n");
        let app = app_de_test();
        let e = ouvrir_avec(app.handle(), Some(Factice::bin()), f.demande(1, "p"))
            .await
            .err()
            .unwrap();
        assert!(e.contains("60 s"), "{e}");
        assert!(vide(&app));
    }

    /// Audit de sécurité du 12 septembre 2026 (C-ipc-1, contrat K12) : un
    /// dossier existant mais jamais désigné par la boîte native est refusé
    /// avant tout lancement ; celui que la fiche du bureau porte déjà passe.
    #[tokio::test]
    async fn un_dossier_partage_non_designe_est_refuse_avant_de_lancer_quoi_que_ce_soit() {
        let _g = with_ssh_config("");
        let f = Factice::nouveau(ANNONCE_PUIS_ATTENTE);
        let app = app_de_test();
        let partage = f.dir.join("maison");
        std::fs::create_dir_all(&partage).unwrap();
        let mut d = f.demande(1, "p");
        d.partage = Some(partage.display().to_string());
        let e = ouvrir_avec(app.handle(), Some(Factice::bin()), d)
            .await
            .err()
            .unwrap();
        assert!(e.contains("boîte de sélection"), "{e}");
        assert!(f.lire("argv").is_none(), "le sidecar a été lancé");
        assert!(vide(&app));
        // Une fiche enregistrée pour ce bureau avec ce dossier fait foi.
        let mut fiche = avash::rdphost::RdpHost::new("b", &f.hote, 3389, "admin", 800, 600);
        fiche.partage = Some(partage.display().to_string());
        avash::rdphost::upsert_host_in(&avash::rdphost::hosts_path(), fiche).unwrap();
        let mut d = f.demande(1, "p");
        d.partage = Some(partage.display().to_string());
        ouvrir_avec(app.handle(), Some(Factice::bin()), d)
            .await
            .unwrap();
        rdp_close(app.state::<RdpStore>(), 1).unwrap();
    }

    /// Contrat K1 (audit du 12 septembre 2026, C-SIL-8) : un trousseau en
    /// panne ne fait plus partir le sidecar avec un mot de passe vide, que le
    /// serveur refusait comme un mauvais mot de passe (« `LOGON_FAILURE` ») ;
    /// la commande demande la saisie, et le front est prévenu une fois.
    #[tokio::test]
    async fn rdp_open_ne_part_pas_avec_un_mot_de_passe_vide_faute_de_trousseau() {
        let _g = crate::commands::tests::with_trousseau_en_panne("");
        let f = Factice::nouveau(ANNONCE_PUIS_ATTENTE);
        let app = app_de_test();
        let signales = crate::commands::tests::ecouter(&app, "trousseau-indisponible");
        for _ in 0..2 {
            let e = ouvrir_avec(app.handle(), Some(Factice::bin()), f.demande(1, ""))
                .await
                .err()
                .unwrap();
            assert!(e.contains("trousseau"), "{e}");
            assert!(e.contains("saisis"), "{e}");
        }
        assert!(f.lire("argv").is_none(), "le sidecar a été lancé");
        assert!(vide(&app));
        assert_eq!(signales.lock().unwrap().len(), 1);
    }

    /// Même règle à l'enregistrement de la fiche : désigné, ou inchangé.
    #[test]
    fn le_dossier_partage_d_une_fiche_doit_etre_designe_ou_inchange() {
        let _g = with_ssh_config("");
        let app = app_de_test();
        let base = avash::repertoire_personnel().unwrap();
        let (choisi, autre) = (base.join("choisi"), base.join("autre"));
        std::fs::create_dir_all(&choisi).unwrap();
        std::fs::create_dir_all(&autre).unwrap();
        let enregistrer = |partage: &PathBuf| {
            rdp_host_save(
                app.state::<ChoixLocaux>(),
                Some("b1".into()),
                "b".into(),
                "srv".into(),
                3389,
                "admin".into(),
                800,
                600,
                None,
                None,
                Some(partage.display().to_string()),
            )
        };
        let e = enregistrer(&choisi).unwrap_err();
        assert!(e.contains("boîte de sélection"), "{e}");
        app.state::<ChoixLocaux>().retenir([choisi.clone()]);
        enregistrer(&choisi).unwrap();
        // Une autre application (état neuf) : le dossier inchangé passe, un
        // autre dossier jamais désigné non.
        let neuve = app_de_test();
        rdp_host_save(
            neuve.state::<ChoixLocaux>(),
            Some("b1".into()),
            "b".into(),
            "srv".into(),
            3389,
            "admin".into(),
            800,
            600,
            None,
            None,
            Some(choisi.display().to_string()),
        )
        .unwrap();
        assert!(rdp_host_save(
            neuve.state::<ChoixLocaux>(),
            Some("b1".into()),
            "b".into(),
            "srv".into(),
            3389,
            "admin".into(),
            800,
            600,
            None,
            None,
            Some(autre.display().to_string()),
        )
        .is_err());
    }

    /// C-secrets-1 : le trousseau ne garde pas un mot de passe qui ouvrirait
    /// le protocole de l'entrée standard.
    #[tokio::test]
    async fn un_mot_de_passe_a_saut_de_ligne_ne_se_memorise_pas() {
        let _g = with_ssh_config("");
        let app = app_de_test();
        let e = rdp_password_save(
            "srv".into(),
            3389,
            "admin".into(),
            "x\nAUTORISE /etc/passwd".into(),
            None,
        )
        .await
        .unwrap_err();
        assert!(e.contains("saut de ligne"), "{e}");
        assert!(
            !rdp_password_known(
                app.handle().clone(),
                "srv".into(),
                3389,
                "admin".into(),
                None
            )
            .await
        );
    }

    /// RDP et VNC vers le même serveur ont deux comptes distincts.
    #[tokio::test]
    async fn le_mot_de_passe_rdp_se_memorise_se_relit_et_s_oublie_par_protocole() {
        let _g = with_ssh_config("");
        let app = app_de_test();
        let connu = |proto: &'static str| {
            rdp_password_known(
                app.handle().clone(),
                "srv".into(),
                3389,
                "admin".into(),
                Some(proto.into()),
            )
        };
        rdp_password_save(
            "srv".into(),
            3389,
            "admin".into(),
            "a".into(),
            Some("rdp".into()),
        )
        .await
        .unwrap();
        assert!(connu("rdp").await);
        assert!(!connu("vnc").await);
        rdp_password_forget("srv".into(), 3389, "admin".into(), Some("rdp".into()))
            .await
            .unwrap();
        assert!(!connu("rdp").await);
    }

    fn fiche(app: &tauri::App<tauri::test::MockRuntime>, id: &str, host: &str) {
        rdp_host_save(
            app.state::<ChoixLocaux>(),
            Some(id.into()),
            id.into(),
            host.into(),
            3389,
            "admin".into(),
            800,
            600,
            None,
            None,
            None,
        )
        .unwrap();
    }

    async fn connu(app: &tauri::App<tauri::test::MockRuntime>, host: &str) -> bool {
        rdp_password_known(
            app.handle().clone(),
            host.into(),
            3389,
            "admin".into(),
            None,
        )
        .await
    }

    /// Déplacer le secret d'un bureau modifié : copié vers le nouveau compte,
    /// l'ancien n'est oublié que si plus aucun bureau ne le partage.
    #[tokio::test]
    async fn deplacer_le_mot_de_passe_d_un_bureau_n_oublie_l_ancien_que_s_il_est_orphelin() {
        let _g = with_ssh_config("");
        let app = app_de_test();
        fiche(&app, "a", "srv");
        fiche(&app, "b", "srv");
        rdp_password_save("srv".into(), 3389, "admin".into(), "s".into(), None)
            .await
            .unwrap();
        // Le front enregistre la fiche modifiée, puis déplace le secret.
        fiche(&app, "a", "srv2");
        let deplacer = |de: &str, vers: &str| {
            rdp_password_move(
                de.into(),
                3389,
                "admin".into(),
                vers.into(),
                3389,
                "admin".into(),
                None,
                None,
            )
        };
        deplacer("srv", "srv2").await.unwrap();
        assert!(connu(&app, "srv2").await);
        assert!(
            connu(&app, "srv").await,
            "« b » partage encore l'ancien compte"
        );
        // Seul bureau vers srv3 : déplacé vers srv4, l'ancien est oublié.
        fiche(&app, "c", "srv3");
        rdp_password_save("srv3".into(), 3389, "admin".into(), "t".into(), None)
            .await
            .unwrap();
        fiche(&app, "c", "srv4");
        deplacer("srv3", "srv4").await.unwrap();
        assert!(connu(&app, "srv4").await);
        assert!(!connu(&app, "srv3").await);
    }

    /// Supprimer un bureau n'oublie son secret que s'il est le dernier à s'en
    /// servir.
    #[tokio::test]
    async fn supprimer_un_bureau_n_oublie_son_secret_que_s_il_est_orphelin() {
        let _g = with_ssh_config("");
        let app = app_de_test();
        fiche(&app, "a", "srv");
        fiche(&app, "b", "srv");
        rdp_password_save("srv".into(), 3389, "admin".into(), "s".into(), None)
            .await
            .unwrap();
        rdp_host_delete("a".into()).await.unwrap();
        assert!(connu(&app, "srv").await);
        rdp_host_delete("b".into()).await.unwrap();
        assert!(!connu(&app, "srv").await);
    }

    /// Un fichier reçu ne s'ouvre pas d'un clic : seul un dossier existant, en
    /// chemin absolu, est confié au gestionnaire de fichiers.
    #[tokio::test]
    async fn rdp_ouvrir_dossier_refuse_un_fichier_et_un_chemin_relatif() {
        for chemin in ["/etc/passwd", "relatif"] {
            let e = rdp_ouvrir_dossier(chemin.into()).await.unwrap_err();
            assert!(e.contains("dossier existant"), "{e}");
        }
    }
}
