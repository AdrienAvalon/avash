//! Panneau SFTP : canal sur la session de l'onglet, listing, transferts, dossiers.

use super::SessionStore;
use avash::sftp::SftpHandle;
use tauri::{AppHandle, Emitter};

/// Ouvre (ou réutilise) le canal SFTP d'un onglet, sur la session SSH du
/// terminal. Retourne un Arc partagé avec le store.
pub(crate) async fn sftp_of(
    state: &tauri::State<'_, SessionStore>,
    id: u64,
) -> Result<std::sync::Arc<SftpHandle>, String> {
    // Rapide : déjà ouverte ?
    {
        let store = state.inner.lock().unwrap();
        if let Some(h) = store.get(&id) {
            if let Some(s) = h.sftp.lock().unwrap().as_ref() {
                return Ok(s.clone());
            }
        }
    }
    // Sinon : un canal de plus sur la session du terminal, puis stockage. Pas
    // de seconde connexion, donc pas de seconde authentification ni de cible à
    // rejouer — le mot de passe n'a pas été gardé.
    let ouvrir = {
        let store = state.inner.lock().unwrap();
        store
            .get(&id)
            .map(|h| h.ouvrir_sftp.clone())
            .ok_or_else(|| format!("Session {id} inconnue"))?
    };
    let mut fresh = Some(ouvrir().await?);

    // Course : deux commandes SFTP concurrentes sur le meme onglet ont pu, le
    // temps de cette ouverture, en obtenir chacune un. On re-verifie et on
    // stocke atomiquement sous le verrou ; aucun `await` n'y a lieu (les gardes
    // de Mutex ne sont pas Send). Le canal perdant est ferme ensuite, hors du
    // verrou, sinon il resterait ouvert sur le serveur.
    let mut to_close: Option<SftpHandle> = None;
    let chosen: Option<std::sync::Arc<SftpHandle>> = {
        let store = state.inner.lock().unwrap();
        match store.get(&id) {
            None => None,
            Some(h) => {
                let mut slot = h.sftp.lock().unwrap();
                if let Some(existing) = slot.as_ref() {
                    // Un autre appel a gagne : on fermera notre connexion.
                    to_close = fresh.take();
                    Some(existing.clone())
                } else {
                    let arc = std::sync::Arc::new(fresh.take().unwrap());
                    *slot = Some(arc.clone());
                    Some(arc)
                }
            }
        }
    };
    // Handle en trop (course perdue) ou session disparue : on ferme proprement.
    if let Some(f) = to_close.or(fresh) {
        let _ = f.close().await;
    }
    chosen.ok_or_else(|| format!("Session {id} inconnue"))
}

/// Determine le chemin local d'un telechargement.
///
/// Si l'appelant n'impose rien, on derive le nom depuis le chemin distant.
/// Un chemin distant sans nom de fichier exploitable (`/`, `.`, `..`) ne doit
/// PAS retomber silencieusement sur le dossier de telechargement lui-meme :
/// l'ecriture echouerait ensuite avec une erreur obscure.
///
/// Trouvé par l'audit du 7 septembre 2026 : le front ne passe jamais `local`
/// (`web/sftp.ts`), et l'on rendait `~/Téléchargements/<nom>` sans regarder si
/// le fichier existait. Télécharger un second `backup.sql` (autre hôte, autre
/// jour) écrasait le premier sans un mot ; télécharger un dossier distant sur un
/// dossier local homonyme les fusionnait et remplaçait d'un coup tous les
/// fichiers de même nom. On choisit donc un nom local libre (`nom (2).ext`),
/// comme la réception RDP ne remplace jamais un fichier existant (SECURITY.md).
/// C'est ici, à la racine du transfert, que le nom se décide : le cas dossier
/// est couvert comme le cas fichier. La reprise n'en pâtit pas : elle s'appuie
/// sur le `.part` dérivé de ce chemin, jamais sur la cible finale, et le nom est
/// choisi une seule fois par téléchargement, ici, puis réutilisé.
///
/// Trouvé par l'audit du 9 septembre 2026 : un `local` fourni par l'appelant
/// court-circuitait tout ce qui précède. Il était rendu tel quel, sans être
/// absolu ni libre, alors que `download_reprise` termine par un `rename()`
/// POSIX qui remplace sa cible sans un mot. Depuis la webview (dépendance front
/// compromise, outils de développement) un `invoke("sftp_download", { local: "…/.bashrc" })`
/// écrivait donc le contenu d'un serveur choisi par l'appelant par-dessus un
/// fichier de l'utilisateur. Le chemin imposé passe désormais par les mêmes
/// gardes que le chemin dérivé, comme toutes les commandes voisines qui
/// touchent au disque local (`diagnostic_exporter`, `dossier_partage`,
/// `rdp_ouvrir_dossier`).
pub(crate) fn local_target(remote: &str, local: Option<String>) -> Result<String, String> {
    if let Some(l) = local {
        let vise = std::path::Path::new(&l);
        if !vise.is_absolute() {
            return Err(format!("Le chemin local doit être absolu : {l}"));
        }
        return Ok(chemin_local_libre(vise).to_string_lossy().into_owned());
    }
    let name = std::path::Path::new(remote)
        .file_name()
        .ok_or_else(|| format!("Chemin distant sans nom de fichier : {remote}"))?;
    let base = avash::sftp::default_local_dir().join(name);
    Ok(chemin_local_libre(&base).to_string_lossy().into_owned())
}

/// Premier chemin libre à partir de `base` : `base`, puis `base (2)`,
/// `base (3)`… en glissant le « (n) » avant l'extension.
///
/// Seule l'existence de la cible FINALE compte, jamais celle d'un `.part` : un
/// téléchargement interrompu puis relancé doit retomber sur le même nom pour
/// reprendre son `.part`, au lieu de glisser au nom suivant.
fn chemin_local_libre(base: &std::path::Path) -> std::path::PathBuf {
    if !base.exists() {
        return base.to_path_buf();
    }
    let dir = base.parent().unwrap_or_else(|| std::path::Path::new("."));
    let tige = base
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = base.extension().map(|e| e.to_string_lossy().into_owned());
    for n in 2..=u32::MAX {
        let nom = match &ext {
            Some(e) => format!("{tige} ({n}).{e}"),
            None => format!("{tige} ({n})"),
        };
        let candidat = dir.join(nom);
        if !candidat.exists() {
            return candidat;
        }
    }
    base.to_path_buf() // inatteignable : 2^32 homonymes déjà présents
}

/// Le fichier ou dossier local qu'un téléversement doit lire.
///
/// Trouvé par l'audit du 9 septembre 2026 : `sftp_upload` prenait son `local`
/// sans la moindre garde, seule commande du fichier à toucher au disque local
/// sans en poser. Un appel direct depuis la webview envoyait ainsi n'importe
/// quel fichier lisible (`~/.ssh/id_ed25519`) vers un serveur choisi par
/// l'appelant. On exige au minimum ce qu'exigent les voisines : un chemin
/// absolu (un relatif viserait le répertoire courant de l'application, que
/// l'utilisateur ne voit nulle part) et une cible qui existe, pour une erreur
/// nette au lieu d'un échec de lecture au milieu du transfert. Le front n'envoie
/// que des chemins venus de la boîte de sélection ou d'un glisser-déposer, donc
/// déjà absolus et existants : la voie normale ne change pas.
pub(crate) fn local_source(local: &str) -> Result<std::path::PathBuf, String> {
    let chemin = std::path::Path::new(local);
    if !chemin.is_absolute() || !chemin.exists() {
        return Err(format!(
            "Le fichier à envoyer n'existe pas ou n'est pas un chemin absolu : {local}"
        ));
    }
    Ok(chemin.to_path_buf())
}

/// Résout un chemin distant en absolu (`.` → home). Certains serveurs SFTP
/// refusent `read_dir(".")` : on canonicalise d'abord, et le front affiche
/// alors un vrai chemin dans sa barre plutôt qu'un `.` opaque.
#[tauri::command]
pub async fn sftp_realpath(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    path: String,
) -> Result<String, String> {
    let sftp = sftp_of(&state, id).await?;
    Ok(sftp.realpath(&path).await)
}

/// Liste un répertoire distant via SFTP.
#[tauri::command]
pub async fn sftp_list(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    path: String,
) -> Result<Vec<avash::sftp::SftpEntry>, String> {
    let sftp = sftp_of(&state, id).await?;
    sftp.list(&path).await.map_err(|e| e.to_string())
}

/// Les transferts en cours, par identifiant choisi par le front, avec leur
/// drapeau d'annulation. Le front en lance plusieurs à la fois et en annule
/// un sans toucher aux autres.
#[derive(Default)]
pub struct TransfertsStore {
    pub inner: std::sync::Mutex<std::collections::HashMap<u64, avash::sftp::Annulation>>,
}

/// Inscrit un transfert et rend son drapeau d'annulation ; `retirer` l'ôte à
/// la fin de la commande, en succès comme en erreur.
fn inscrire(store: &tauri::State<'_, TransfertsStore>, transfert: u64) -> avash::sftp::Annulation {
    let drapeau: avash::sftp::Annulation = std::sync::Arc::default();
    store
        .inner
        .lock()
        .unwrap()
        .insert(transfert, drapeau.clone());
    drapeau
}

/// Annule un transfert en cours : ses boucles s'arrêtent entre deux blocs,
/// et ce qui est écrit reste en place avec sa carte de reprise.
#[tauri::command]
#[must_use]
pub fn sftp_annuler(store: tauri::State<'_, TransfertsStore>, transfert: u64) -> bool {
    match store.inner.lock().unwrap().get(&transfert) {
        Some(d) => {
            d.store(true, std::sync::atomic::Ordering::Relaxed);
            true
        }
        None => false,
    }
}

fn retirer(store: &tauri::State<'_, TransfertsStore>, transfert: u64) {
    store.inner.lock().unwrap().remove(&transfert);
}

/// Rapporte la progression d'un transfert au front, sans le noyer : au plus
/// un evenement toutes les 80 ms, plus le dernier.
///
/// `transfert` identifie la ligne dans la file du panneau ; `termines` et
/// `nombre` ne servent qu'aux dossiers.
fn progress_reporter<R: tauri::Runtime>(
    app: &AppHandle<R>,
    id: u64,
    transfert: u64,
    name: &str,
    kind: &str,
) -> impl FnMut(&str, u64, u64, usize, usize) {
    let app = app.clone();
    let name = name.to_string();
    let kind = kind.to_string();
    let mut last: Option<std::time::Instant> = None;
    move |fichier: &str, done, total, termines, nombre| {
        let due = last.is_none_or(|t| t.elapsed() >= std::time::Duration::from_millis(80));
        if due || done == total {
            last = Some(std::time::Instant::now());
            let _ = app.emit(
                "sftp-progress",
                serde_json::json!({
                    "id": id, "transfert": transfert, "name": name, "kind": kind,
                    "fichier": fichier, "done": done, "total": total,
                    "termines": termines, "nombre": nombre,
                }),
            );
        }
    }
}

/// Télécharge un fichier ou un dossier distant → local (dossier
/// Téléchargements par défaut). Un fichier à moitié reçu reprend là où il
/// s'était arrêté.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn sftp_download(
    app: AppHandle,
    state: tauri::State<'_, SessionStore>,
    transferts: tauri::State<'_, TransfertsStore>,
    id: u64,
    transfert: u64,
    remote: String,
    local: Option<String>,
    is_dir: Option<bool>,
) -> Result<String, String> {
    let sftp = sftp_of(&state, id).await?;
    let local = local_target(&remote, local)?;
    let name = std::path::Path::new(&remote)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut report = progress_reporter(&app, id, transfert, &name, "download");
    let annulation = inscrire(&transferts, transfert);
    let issue = if is_dir.unwrap_or(false) {
        sftp.download_dir_with(
            &remote,
            std::path::Path::new(&local),
            Some(&annulation),
            |a| {
                report(&a.fichier, a.fait, a.total, a.termines, a.nombre);
            },
        )
        .await
    } else {
        sftp.download_reprise(
            &remote,
            std::path::Path::new(&local),
            Some(&annulation),
            |f, t| {
                report(&name, f, t, 0, 1);
            },
        )
        .await
    };
    retirer(&transferts, transfert);
    let n = issue.map_err(|e| format!("{e:#}"))?;
    Ok(format!("{local} ({n} octets)"))
}

/// Téléverse un fichier ou un dossier local dans un dossier distant, sous son
/// propre nom. Rend le chemin distant créé. Un fichier à moitié envoyé
/// reprend là où il s'était arrêté.
#[tauri::command]
pub async fn sftp_upload(
    app: AppHandle,
    state: tauri::State<'_, SessionStore>,
    transferts: tauri::State<'_, TransfertsStore>,
    id: u64,
    transfert: u64,
    local: String,
    remote_dir: String,
) -> Result<String, String> {
    // Désigné par l'utilisateur, absolu, existant : voir `choix_locaux`. Le
    // registre est pris sur l'application plutôt qu'en paramètre : clippy borne
    // les commandes à sept arguments, et celle-ci les a déjà.
    let local_path = {
        use tauri::Manager as _;
        super::source_autorisee(&app.state::<super::ChoixLocaux>(), &local)?
    };
    let name = local_path
        .file_name()
        .ok_or_else(|| format!("Nom de fichier illisible : {local}"))?
        .to_string_lossy()
        .into_owned();
    let remote = remote_join(&remote_dir, &name);
    let sftp = sftp_of(&state, id).await?;
    let mut report = progress_reporter(&app, id, transfert, &name, "upload");
    let annulation = inscrire(&transferts, transfert);
    let issue = if local_path.is_dir() {
        sftp.upload_dir_with(&local_path, &remote, Some(&annulation), |a| {
            report(&a.fichier, a.fait, a.total, a.termines, a.nombre);
        })
        .await
    } else {
        // Envoi de fichier unitaire : on refuse d'écraser une cible du même nom
        // (audit du 7 septembre 2026), sauf reprise d'un envoi interrompu.
        sftp.upload_reprise(&local_path, &remote, true, Some(&annulation), |f, t| {
            report(&name, f, t, 0, 1);
        })
        .await
    };
    retirer(&transferts, transfert);
    issue.map_err(|e| format!("{e:#}"))?;
    Ok(remote)
}

/// Copie un fichier ou un dossier d'une session vers une autre.
///
/// Par défaut, les octets traversent le poste sans y être écrits (relais par
/// bandes, un descripteur de lecture ici, un d'écriture là-bas). En mode
/// `direct`, c'est l'hôte source qui envoie lui-même, par `scp` lancé chez lui
/// avec l'agent SSH du poste redirigé le temps de la commande : rien ne passe
/// par le poste, mais l'hôte source doit joindre l'hôte cible, et le poste lui
/// prête ses clés ce temps-là. La clé de l'hôte cible y est acceptée au premier
/// contact et refusée si elle change, comme ici.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn sftp_copier_vers<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, SessionStore>,
    transferts: tauri::State<'_, TransfertsStore>,
    id: u64,
    transfert: u64,
    remote: String,
    is_dir: bool,
    id_cible: u64,
    remote_dir_cible: String,
    direct: bool,
) -> Result<String, String> {
    let name = std::path::Path::new(&remote)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| format!("Chemin distant sans nom : {remote}"))?;
    let chez_cible = remote_join(&remote_dir_cible, &name);
    if direct {
        // Note (audit du 7 septembre 2026) : la copie DIRECTE laisse `scp -rpq`
        // écrire chez la cible, y compris par-dessus un fichier du même nom. On
        // ne sonde pas la cible pour l'en empêcher : le mode direct a justement
        // pour contrat que le poste ne médie pas le transfert (aucun canal SFTP
        // ouvert vers la cible), et ce mode est un choix avancé, décoché par
        // défaut. La protection contre l'écrasement porte sur le mode relais (par
        // défaut) et sur l'envoi/le téléchargement.
        // scp chez la source, vers la cible, avec l'agent du poste prêté.
        let (executer, cible) = {
            let store = state.inner.lock().unwrap();
            let src = store
                .get(&id)
                .ok_or_else(|| format!("Session {id} inconnue"))?;
            let dst = store
                .get(&id_cible)
                .ok_or_else(|| format!("Session {id_cible} inconnue"))?;
            (src.executer.clone(), dst.cible.clone())
        };
        let (hote, port, user) = cible;
        // Les chemins et l'adresse deviennent des arguments de scp chez la
        // source : un chemin qui commence par « - » y serait lu comme une
        // option, quelle que soit la mise entre apostrophes (qui ne parle
        // qu'au shell). Le marqueur « -- » clôt les options, et l'on refuse
        // en plus ce qui y ressemble ; l'utilisateur et l'hôte restent dans
        // l'alphabet d'un nom de compte et d'une adresse.
        if remote.starts_with('-') || chez_cible.starts_with('-') {
            return Err("Un chemin ne peut pas commencer par « - » pour une copie directe.".into());
        }
        let sain = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-:[]@".contains(c))
        };
        if !sain(&user) || !sain(&hote) {
            return Err(format!(
                "Utilisateur ou adresse de la cible inattendus pour scp : {user}@{hote}"
            ));
        }
        let commande = format!(
            "scp -rpq -o BatchMode=yes -o StrictHostKeyChecking=accept-new -P {port} -- {} {}@{}:{}",
            citer(&remote),
            citer(&user),
            citer(&hote),
            citer(&chez_cible)
        );
        // Trouvé par l'audit du 7 septembre 2026 : cette branche n'inscrivait
        // aucun drapeau, si bien que `sftp_annuler` rendait `false` et que le
        // bouton « Annuler » du front (affiché sur toute ligne en cours) ne
        // faisait rien, sans le dire. On inscrit désormais le transfert et l'on
        // passe le drapeau à `run_avec_agent`, qui coupe le scp distant quand il
        // est levé ; `retirer` l'ôte en succès comme en erreur (sinon un
        // transfert terminé resterait « annulable »).
        let annulation = inscrire(&transferts, transfert);
        let issue = executer(commande, Some(annulation)).await;
        retirer(&transferts, transfert);
        let (sortie, code) = issue?;
        if code != 0 {
            let detail = sortie.trim();
            return Err(format!(
                "scp chez la source a rendu le code {code}{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(" : {detail}")
                }
            ));
        }
        return Ok(chez_cible);
    }
    let source = sftp_of(&state, id).await?;
    let cible = sftp_of(&state, id_cible).await?;
    let mut report = progress_reporter(&app, id, transfert, &name, "copie");
    let annulation = inscrire(&transferts, transfert);
    let issue = if is_dir {
        source
            .relayer_dir_vers(&remote, &cible, &chez_cible, Some(&annulation), |a| {
                report(&a.fichier, a.fait, a.total, a.termines, a.nombre);
            })
            .await
    } else {
        source
            .relayer_vers(
                &remote,
                &cible,
                &chez_cible,
                // Copie d'un fichier unitaire : ne pas écraser une cible du même
                // nom (audit du 7 septembre 2026). Le dossier, lui, fusionne.
                true,
                Some(&annulation),
                |f, t| {
                    report(&name, f, t, 0, 1);
                },
            )
            .await
    };
    retirer(&transferts, transfert);
    issue.map_err(|e| format!("{e:#}"))?;
    Ok(chez_cible)
}

/// Un argument pour le shell distant, entre apostrophes, une apostrophe
/// intérieure fermée, échappée et rouverte : `it's` → `'it'\''s'`.
pub(crate) fn citer(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Concatene un dossier distant et un nom, comme le fait le front.
pub(crate) fn remote_join(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir == "." {
        return name.to_string();
    }
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

#[tauri::command]
pub async fn sftp_mkdir(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    path: String,
) -> Result<(), String> {
    let sftp = sftp_of(&state, id).await?;
    sftp.mkdir(&path).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sftp_remove(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    path: String,
    is_dir: bool,
) -> Result<(), String> {
    let sftp = sftp_of(&state, id).await?;
    sftp.remove(&path, is_dir).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sftp_rename(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    from: String,
    to: String,
) -> Result<(), String> {
    let sftp = sftp_of(&state, id).await?;
    sftp.rename(&from, &to).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests_citer {
    use super::citer;

    /// Le chemin part dans une commande shell chez la source : une apostrophe
    /// ou un espace dans un nom de fichier ne doit ni casser la commande ni en
    /// injecter une autre.
    #[test]
    fn citer_rend_le_shell_inoffensif() {
        assert_eq!(citer("simple"), "'simple'");
        assert_eq!(citer("avec espace"), "'avec espace'");
        assert_eq!(citer("l'apostrophe"), "'l'\\''apostrophe'");
        assert_eq!(citer("$(rm -rf /)"), "'$(rm -rf /)'");
    }
}
