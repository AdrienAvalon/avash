//! Intégration RDP côté Tauri : lance le sidecar `avash-rdp` (isolé de russh),
//! qui sert lui-même le bureau à la webview via un WebSocket local **binaire**
//! (vrai `ArrayBuffer` : pas de base64, pas de JSON — débit maximal). Avash ne
//! fait que gérer le cycle de vie du sidecar et transmettre le point de
//! connexion (port + jeton) au front.

use std::collections::HashMap;
use std::sync::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Child;

/// Sessions RDP vivantes, par id d'onglet.
#[derive(Default)]
pub struct RdpStore {
    pub inner: Mutex<HashMap<u64, Child>>,
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
fn sidecar_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("AVASH_RDP_BIN") {
        let p = std::path::PathBuf::from(p);
        // Même une variable d'environnement doit désigner un chemin absolu :
        // relative, elle rouvrirait exactement la porte qu'on vient de fermer.
        return p.is_absolute().then_some(p);
    }
    // Sous Windows l'exécutable porte une extension : sans elle, le fichier
    // posé à côté de l'application (avash-rdp.exe) n'était jamais trouvé et
    // toute connexion RDP échouait.
    let nom = format!("avash-rdp{}", std::env::consts::EXE_SUFFIX);
    if let Ok(exe) = std::env::current_exe() {
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
                if devside.exists() {
                    return Some(devside);
                }
            }
        }
    }
    None
}

/// Lance le sidecar et renvoie le WebSocket (port + jeton) qu'il annonce.
#[allow(clippy::too_many_arguments)]
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
#[tauri::command]
pub async fn rdp_open(
    state: tauri::State<'_, RdpStore>,
    id: u64,
    host: String,
    port: Option<u16>,
    user: String,
    password: String,
    width: u16,
    height: u16,
    sans_nla: bool,
) -> Result<RdpConn, String> {
    use std::process::Stdio;
    let password = if password.is_empty() {
        avash::secrets::load(&rdphost::keyring_account(
            &user,
            &host,
            port.unwrap_or(3389),
        ))
        .unwrap_or_default()
    } else {
        password
    };
    // L'adresse vient du front et arrive telle quelle jusqu'à la clé du fichier
    // d'empreintes. `RdpHost::validate` ne la voyait qu'à l'enregistrement : une
    // connexion manuelle, ou un rdp.yaml écrit par une version antérieure, la
    // contournait entièrement — et une espace dans l'adresse suffit à casser
    // `rdp_known_hosts`, donc à désarmer le TOFU en silence.
    avash::rdphost::RdpHost::new("", &host, port.unwrap_or(3389), &user, width, height)
        .validate()
        .map_err(|e| format!("{e:#}"))?;
    let Some(bin) = sidecar_path() else {
        return Err(
            "Le processus RDP (avash-rdp) est introuvable à côté de l'application. \
             Réinstalle Avash, ou indique son chemin absolu dans AVASH_RDP_BIN."
                .to_owned(),
        );
    };
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args([
        "--host",
        &host,
        "--port",
        &port.unwrap_or(3389).to_string(),
        "-u",
        &user,
        "--width",
        &width.to_string(),
        "--height",
        &height.to_string(),
    ])
    .args(if sans_nla {
        &["--sans-nla"][..]
    } else {
        &[][..]
    })
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    // Sous Windows, lancer un programme console ouvre une fenêtre noire à chaque
    // connexion RDP. CREATE_NO_WINDOW l'en empêche : le sidecar reste invisible,
    // ses flux restant redirigés vers nous.
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Lancement du sidecar RDP impossible : {e}"))?;

    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Enregistré AVANT tout point de sortie. La connexion RDP (TLS + NLA) peut
    // durer plusieurs secondes ; si l'utilisateur ferme l'onglet pendant ce
    // temps, `rdp_close` doit trouver l'enfant pour le tuer. Enregistré à la fin
    // comme auparavant, le sidecar survivait à la fermeture — session
    // authentifiée ouverte, socket en écoute, invisible de l'interface.
    if let Some(mut old) = state.inner.lock().unwrap().insert(id, child) {
        let _ = old.start_kill();
    }

    // À partir d'ici, toute sortie en erreur doit emporter l'enfant enregistré.
    let (Some(mut stdin), Some(stdout), Some(mut stderr)) = (stdin, stdout, stderr) else {
        return Err(tuer(
            &state,
            id,
            "Flux du sidecar RDP indisponibles.".into(),
        ));
    };

    // Mot de passe transmis par stdin plutôt qu'en argument : évite sa fuite via
    // /proc/<pid>/cmdline, lisible par les autres utilisateurs locaux.
    {
        use tokio::io::AsyncWriteExt as _;
        if let Err(e) = stdin.write_all(format!("{password}\n").as_bytes()).await {
            return Err(tuer(
                &state,
                id,
                format!("Envoi du mot de passe au sidecar : {e}"),
            ));
        }
    }

    let (port, token) = match lire_annonce(stdout, &mut stderr).await {
        Ok(v) => v,
        Err(e) => return Err(tuer(&state, id, e)),
    };

    // L'utilisateur a-t-il fermé l'onglet pendant la connexion ? `rdp_close` a
    // alors retiré et tué notre enfant : on ne prétend pas avoir ouvert une
    // session, et le front n'affiche pas d'erreur trompeuse.
    if !state.inner.lock().unwrap().contains_key(&id) {
        return Err(CONNEXION_ANNULEE.into());
    }

    suivre_diagnostic(&state, id, stderr);
    Ok(RdpConn { port, token })
}

/// Garde les dernières lignes que le sidecar écrit sur son erreur standard.
///
/// Passé l'ouverture, `stderr` n'était plus lu : il était libéré à la sortie de
/// `rdp_open`. Une panique ou une erreur du sidecar **en cours de session**
/// écrivait alors dans un tube fermé — le message était perdu et l'onglet RDP
/// mourait sans motif.
fn suivre_diagnostic(
    state: &tauri::State<'_, RdpStore>,
    id: u64,
    flux: tokio::process::ChildStderr,
) {
    let journal = state
        .journaux
        .lock()
        .unwrap()
        .entry(id)
        .or_default()
        .clone();
    tokio::spawn(async move {
        let mut sorties = BufReader::new(flux).lines();
        while let Ok(Some(l)) = sorties.next_line().await {
            let mut g = journal.lock().unwrap();
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
    let msg = diag.trim().lines().last().unwrap_or("").trim();
    if msg.is_empty() {
        "Le sidecar RDP s'est arrêté sans se connecter.".to_owned()
    } else {
        msg.to_owned()
    }
}

/// Nombre de lignes de diagnostic gardées par session. De quoi expliquer une
/// fin de session sans laisser un serveur bavard remplir la mémoire.
const JOURNAL_MAX: usize = 32;

/// Dernières lignes écrites par le sidecar d'une session.
///
/// L'interface les joint au message de fermeture : sans elles, un onglet RDP
/// qui meurt en cours de route ne dit rien de la raison.
#[tauri::command]
#[must_use]
pub fn rdp_diagnostic(state: tauri::State<'_, RdpStore>, id: u64) -> String {
    state
        .journaux
        .lock()
        .unwrap()
        .get(&id)
        .map(|j| {
            j.lock()
                .unwrap()
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        })
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
fn tuer(state: &tauri::State<'_, RdpStore>, id: u64, msg: String) -> String {
    if let Some(mut child) = state.inner.lock().unwrap().remove(&id) {
        let _ = child.start_kill();
    }
    state.journaux.lock().unwrap().remove(&id);
    msg
}

#[tauri::command]
pub fn rdp_close(state: tauri::State<'_, RdpStore>, id: u64) -> Result<(), String> {
    if let Some(mut child) = state.inner.lock().unwrap().remove(&id) {
        let _ = child.start_kill();
    }
    state.journaux.lock().unwrap().remove(&id);
    Ok(())
}

// ---------- Connexions RDP enregistrées ----------

use avash::rdphost::{self, RdpHost};

#[tauri::command]
pub fn rdp_hosts() -> Result<Vec<RdpHost>, String> {
    rdphost::load_hosts().map_err(|e| e.to_string())
}

/// Cree (`id` absent) ou modifie une connexion RDP enregistree.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn rdp_host_save(
    id: Option<String>,
    name: String,
    host: String,
    port: u16,
    user: String,
    width: u16,
    height: u16,
    folder: Option<String>,
) -> Result<RdpHost, String> {
    let mut h = RdpHost::new(&name, &host, port, &user, width, height);
    if let Some(id) = id.filter(|i| !i.is_empty()) {
        h.id = id;
    }
    h.folder = avash::folders::normalize(&folder.unwrap_or_default());
    rdphost::upsert_host_in(&rdphost::hosts_path(), h.clone()).map_err(|e| e.to_string())?;
    Ok(h)
}

/// Range un bureau RDP dans un dossier (déplacement).
#[tauri::command]
pub fn rdp_host_set_folder(id: String, folder: String) -> Result<(), String> {
    let mut all = rdphost::load_hosts().map_err(|e| e.to_string())?;
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
pub fn rdp_host_delete(id: String) -> Result<(), String> {
    // Retrouver l'hote pour oublier son mot de passe avant suppression.
    if let Ok(hosts) = rdphost::load_hosts() {
        if let Some(h) = hosts.iter().find(|h| h.id == id) {
            let _ = avash::secrets::forget(&rdphost::keyring_account(&h.user, &h.host, h.port));
        }
    }
    rdphost::remove_host_in(&rdphost::hosts_path(), &id).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn rdp_password_save(
    host: String,
    port: u16,
    user: String,
    password: String,
) -> Result<(), String> {
    let id = rdphost::keyring_account(&user, &host, port);
    avash::secrets::save(&id, &password).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
#[must_use]
/// Un mot de passe est-il mémorisé pour ce bureau ?
///
/// Ne renvoie **que** l'existence, jamais le secret : celui-ci reste côté natif
/// et n'entre pas dans le tas de la webview, où il survivait jusque-là toute la
/// durée de l'onglet (conservé pour la reconnexion). Le volet SSH procède ainsi
/// depuis toujours (`password_known`).
pub fn rdp_password_known(host: String, port: u16, user: String) -> bool {
    avash::secrets::load(&rdphost::keyring_account(&user, &host, port)).is_some()
}

/// Déplace le secret d'un compte vers un autre lors d'une modification de bureau.
///
/// La migration se faisait côté interface, en relisant le mot de passe pour le
/// réécrire : le secret traversait l'IPC deux fois de plus, à la seule fin de
/// changer de clé. Le trousseau est ici manipulé sans que le secret ne quitte
/// le processus natif.
#[tauri::command]
pub fn rdp_password_move(
    old_host: String,
    old_port: u16,
    old_user: String,
    host: String,
    port: u16,
    user: String,
) -> Result<(), String> {
    let ancien = rdphost::keyring_account(&old_user, &old_host, old_port);
    let Some(secret) = avash::secrets::load(&ancien) else {
        return Ok(()); // rien à déplacer
    };
    let nouveau = rdphost::keyring_account(&user, &host, port);
    avash::secrets::save(&nouveau, &secret).map_err(|e| format!("{e:#}"))?;
    // L'oubli n'a lieu qu'après une écriture réussie : l'inverse perdrait le
    // secret si le trousseau refusait la nouvelle entrée.
    avash::secrets::forget(&ancien).map_err(|e| format!("{e:#}"))
}

/// Retient qu'un serveur ne sait pas faire de NLA, après accord de l'utilisateur.
///
/// Sans cela il faudrait redonner cet accord à chaque connexion. Le choix est
/// par serveur, jamais global : accepter pour un xrdp mal configuré ne doit pas
/// relâcher la garde pour les autres.
#[tauri::command]
pub fn rdp_host_set_sans_nla(id: String, valeur: bool) -> Result<(), String> {
    let chemin = avash::rdphost::hosts_path();
    let mut tous = avash::rdphost::load_hosts_from(&chemin).map_err(|e| format!("{e:#}"))?;
    let Some(h) = tous.iter_mut().find(|h| h.id == id) else {
        return Err(format!("Bureau RDP inconnu : {id}"));
    };
    h.sans_nla = valeur;
    avash::rdphost::save_hosts_to(&chemin, &tous).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn rdp_password_forget(host: String, port: u16, user: String) -> Result<(), String> {
    let id = rdphost::keyring_account(&user, &host, port);
    avash::secrets::forget(&id).map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests_chemin_sidecar {
    use super::sidecar_path;

    /// Le repli relatif `rdp-sidecar/target/release/avash-rdp` était résolu
    /// depuis le répertoire courant : lancée depuis un répertoire où un autre
    /// compte peut écrire, l'application y exécutait le binaire déposé et lui
    /// confiait le mot de passe RDP sur l'entrée standard. Quoi qu'elle rende,
    /// cette fonction doit rendre un chemin absolu — ou rien.
    ///
    /// Un seul test : `set_var`/`remove_var` portent sur tout le processus, et
    /// deux tests concurrents se marcheraient dessus.
    #[test]
    fn le_chemin_rendu_n_est_jamais_relatif() {
        // SAFETY: le test possède la variable et s'exécute d'un seul tenant.
        unsafe { std::env::remove_var("AVASH_RDP_BIN") };
        assert!(
            sidecar_path().is_none_or(|p| p.is_absolute()),
            "le repli ne doit pas dépendre du répertoire courant"
        );

        // Une variable d'environnement relative rouvrirait la même porte.
        unsafe { std::env::set_var("AVASH_RDP_BIN", "avash-rdp") };
        let rendu = sidecar_path();
        unsafe { std::env::remove_var("AVASH_RDP_BIN") };
        assert!(
            rendu.is_none_or(|p| p.is_absolute()),
            "AVASH_RDP_BIN relative doit être refusée"
        );
    }
}

#[cfg(test)]
mod tests_ouverture {
    use super::{rdp_close, rdp_diagnostic, rdp_open, RdpStore};
    use tauri::Manager as _;

    fn app_de_test() -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .manage(RdpStore::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("application factice")
    }

    /// L'adresse arrive du front telle quelle jusqu'à la clé du fichier
    /// d'empreintes : une espace suffit à désarmer le TOFU en silence. Elle
    /// doit être refusée AVANT tout lancement de processus.
    #[tokio::test]
    async fn une_adresse_a_espace_est_refusee_avant_de_lancer_quoi_que_ce_soit() {
        let app = app_de_test();
        let issue = rdp_open(
            app.state::<RdpStore>(),
            1,
            "hote avec espace".into(),
            None,
            "u".into(),
            "p".into(),
            800,
            600,
            false,
        )
        .await;
        let Err(e) = issue else {
            panic!("une adresse à espace a été acceptée")
        };
        assert!(e.contains("caractère interdit"), "{e}");
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
    }
}
