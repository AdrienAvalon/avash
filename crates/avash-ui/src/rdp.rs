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
            // En dev : le sidecar est un projet séparé. Depuis target/release/,
            // remonter jusqu'à la racine du dépôt.
            if let Some(root) = dir.parent().and_then(std::path::Path::parent) {
                // En dev seulement : le sidecar est un projet séparé. `root`
                // vient de current_exe(), donc absolu. En release ce chemin
                // n'a rien à faire là : pour une installation dans
                // ~/.local/bin il désignerait ~/rdp-sidecar/target/release/,
                // un emplacement imprévu à qui l'on écrit le mot de passe.
                #[cfg(debug_assertions)]
                {
                    let devside = root.join("rdp-sidecar/target/release").join(&nom);
                    if devside.exists() {
                        return Some(devside);
                    }
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

    // Le sidecar imprime « PORT TOKEN » quand son WebSocket est prêt. S'il
    // échoue avant (auth, TLS, NLA…), on remonte son diagnostic (stderr).
    let mut lines = BufReader::new(stdout).lines();
    let first = match lines.next_line().await {
        Ok(l) => l,
        Err(e) => return Err(tuer(&state, id, e.to_string())),
    };
    let Some(line) = first else {
        let mut diag = String::new();
        let _ = stderr.read_to_string(&mut diag).await;
        let msg = diag.trim().lines().last().unwrap_or("").to_string();
        return Err(tuer(
            &state,
            id,
            if msg.is_empty() {
                "Le sidecar RDP s'est arrêté sans se connecter.".into()
            } else {
                msg
            },
        ));
    };
    let mut it = line.split_whitespace();
    let Some(port) = it.next().and_then(|s| s.parse::<u16>().ok()) else {
        return Err(tuer(&state, id, "Port WebSocket illisible.".into()));
    };
    let Some(token) = it.next().map(str::to_owned) else {
        return Err(tuer(&state, id, "Jeton WebSocket manquant.".into()));
    };

    // L'utilisateur a-t-il fermé l'onglet pendant la connexion ? `rdp_close` a
    // alors retiré et tué notre enfant : on ne prétend pas avoir ouvert une
    // session, et le front n'affiche pas d'erreur trompeuse.
    if !state.inner.lock().unwrap().contains_key(&id) {
        return Err(CONNEXION_ANNULEE.into());
    }
    Ok(RdpConn { port, token })
}

/// Message d'annulation volontaire : le front le reconnaît pour ne pas
/// présenter une fermeture d'onglet comme un échec de connexion.
pub const CONNEXION_ANNULEE: &str = "[AVASH_RDP_ANNULE]";

/// Tue le sidecar enregistré sous `id` et rend le message d'erreur tel quel.
///
/// Sans cela, chaque sortie en erreur après le `spawn` abandonnait un processus
/// vivant : `tokio::process::Command` ne tue pas l'enfant à la libération.
fn tuer(state: &tauri::State<'_, RdpStore>, id: u64, msg: String) -> String {
    if let Some(mut child) = state.inner.lock().unwrap().remove(&id) {
        let _ = child.start_kill();
    }
    msg
}

#[tauri::command]
pub fn rdp_close(state: tauri::State<'_, RdpStore>, id: u64) -> Result<(), String> {
    if let Some(mut child) = state.inner.lock().unwrap().remove(&id) {
        let _ = child.start_kill();
    }
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
