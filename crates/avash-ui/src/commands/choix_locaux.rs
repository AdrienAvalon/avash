//! Les chemins locaux que l'utilisateur a désignés, et rien d'autre.
//!
//! Trouvé par l'audit du 9 septembre 2026, laissé en réserve par sa relecture :
//! exiger d'un envoi SFTP un chemin absolu et existant ne fermait rien,
//! `~/.ssh/id_ed25519` étant absolu et existant. Un script hostile dans la
//! webview (dépendance front compromise, outils de développement) pouvait donc
//! envoyer n'importe quel fichier lisible vers un serveur de son choix, par
//! `sftp_upload` comme par l'offre de fichiers au bureau distant, que le front
//! passe au processus RDP sur son WebSocket.
//!
//! Ce que le front sait d'un fichier à envoyer, il le tient de deux gestes de
//! l'utilisateur que le natif voit passer : la boîte de sélection, ouverte ici
//! plutôt que par le greffon JavaScript, et le dépôt d'un fichier sur la
//! fenêtre, que tao signale au natif avant la webview. Les deux retiennent le
//! chemin désigné ; un envoi n'accepte ensuite que ce qui a été retenu. Un
//! script qui invente un chemin n'a jamais été vu le désigner : refus.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Les chemins désignés depuis le lancement de l'application. On ne les oublie
/// pas : l'utilisateur renvoie volontiers le même fichier deux fois, et un
/// chemin qu'il a choisi une fois ne redevient pas secret.
#[derive(Default)]
pub struct ChoixLocaux {
    inner: Mutex<HashSet<PathBuf>>,
}

impl ChoixLocaux {
    /// Retient des chemins désignés par l'utilisateur.
    pub fn retenir(&self, chemins: impl IntoIterator<Item = PathBuf>) {
        self.inner.lock().unwrap().extend(chemins);
    }

    /// Ce chemin a-t-il été désigné, tel quel, par l'utilisateur ? L'égalité
    /// est stricte : la boîte de sélection et le dépôt rendent des chemins
    /// absolus et canoniques, que le front repasse sans les toucher.
    #[must_use]
    pub fn designe(&self, chemin: &Path) -> bool {
        self.inner.lock().unwrap().contains(chemin)
    }
}

/// Le fichier ou dossier local qu'un envoi peut lire : désigné par
/// l'utilisateur, absolu et existant, dans cet ordre de vérification. La garde
/// d'existence reste celle de `local_source` : un chemin choisi puis supprimé
/// avant l'envoi donne la même erreur nette qu'avant.
pub(crate) fn source_autorisee(choix: &ChoixLocaux, local: &str) -> Result<PathBuf, String> {
    let chemin = super::sftp::local_source(local)?;
    if !choix.designe(&chemin) {
        return Err(format!(
            "Le fichier « {local} » n'a pas été désigné par la boîte de sélection ni déposé sur la fenêtre : envoi refusé."
        ));
    }
    Ok(chemin)
}

/// Ouvre la boîte de sélection native et rend les chemins choisis, après les
/// avoir retenus ici et annoncés aux processus de bureau distant en cours, qui
/// appliquent la même règle à leurs offres de fichiers.
///
/// Le dialogue est lancé par son rappel (pas sa variante bloquante) : la
/// commande est asynchrone et n'a pas à immobiliser un fil du runtime pendant
/// que l'utilisateur parcourt ses dossiers.
#[tauri::command]
pub async fn choisir_fichiers_locaux(
    app: tauri::AppHandle,
    choix: tauri::State<'_, ChoixLocaux>,
    rdp: tauri::State<'_, crate::rdp::RdpStore>,
    titre: String,
    dossiers: bool,
) -> Result<Vec<String>, String> {
    use tauri_plugin_dialog::DialogExt as _;
    let boite = app.dialog().file().set_title(titre);
    let (tx, rx) = tokio::sync::oneshot::channel();
    if dossiers {
        boite.pick_folders(move |c| {
            let _ = tx.send(c);
        });
    } else {
        boite.pick_files(move |c| {
            let _ = tx.send(c);
        });
    }
    let choisis = rx
        .await
        .map_err(|_| "La boîte de sélection s'est fermée sans répondre.".to_owned())?;
    let chemins: Vec<PathBuf> = choisis
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| f.into_path().ok())
        .collect();
    designer(&choix, &rdp, chemins.clone()).await;
    Ok(chemins
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect())
}

/// Un geste de désignation, d'où qu'il vienne : retenu ici, annoncé aux
/// processus de bureau distant en cours.
pub(crate) async fn designer(
    choix: &ChoixLocaux,
    rdp: &crate::rdp::RdpStore,
    chemins: Vec<PathBuf>,
) {
    choix.retenir(chemins.iter().cloned());
    crate::rdp::annoncer_designations(rdp, &chemins).await;
}
