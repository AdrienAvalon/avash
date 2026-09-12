//! Port série : la console d'un commutateur, d'un routeur, d'une carte, dans un
//! onglet comme les autres.
//!
//! Le port est lu et écrit par deux fils d'exécution bloquants (la
//! bibliothèque ne connaît pas l'asynchrone) qui parlent au reste de
//! l'application par les mêmes canaux qu'une session SSH : le clavier entre
//! par `in_tx`, la sortie ressort par `out_rx`. Lâcher `in_tx` ferme le port :
//! le fil d'écriture s'arrête, lève le drapeau, le fil de lecture le voit à sa
//! prochaine échéance et s'en va, ce qui ferme `out_rx` et termine le pump.

use anyhow::{anyhow, Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Un port présent sur le poste.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PortSerie {
    pub chemin: String,
    /// Ce que le système en dit (fabricant, produit USB), ou rien.
    pub description: String,
}

/// Les vitesses proposées, celles que l'on rencontre en pratique ; une autre
/// valeur reste acceptée par `ouvrir`.
pub const VITESSES: &[u32] = &[
    9600, 19_200, 38_400, 57_600, 115_200, 230_400, 460_800, 921_600,
];

/// Les ports du poste, triés par chemin.
///
/// Contrat K2 de l'audit du 12 septembre 2026 (C-SIL-12) : une énumération qui
/// échoue (droits sur `/dev`, erreur de `serialport`) rend une erreur, et non
/// une liste vide que le formulaire affichait comme « aucun port série ».
pub fn lister_ports() -> Result<Vec<PortSerie>> {
    ports_depuis(serialport::available_ports())
}

/// Le cœur de [`lister_ports`], sur le résultat d'une énumération : de quoi
/// éprouver le cas d'échec sans casser `/dev`.
fn ports_depuis(
    enumeration: serialport::Result<Vec<serialport::SerialPortInfo>>,
) -> Result<Vec<PortSerie>> {
    let mut ports: Vec<PortSerie> = enumeration
        .map_err(|e| anyhow!("Énumération des ports série impossible : {e}"))?
        .into_iter()
        .map(|p| PortSerie {
            description: match &p.port_type {
                serialport::SerialPortType::UsbPort(u) => {
                    [u.manufacturer.as_deref(), u.product.as_deref()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" ")
                }
                serialport::SerialPortType::BluetoothPort => "Bluetooth".to_owned(),
                serialport::SerialPortType::PciPort => "PCI".to_owned(),
                serialport::SerialPortType::Unknown => String::new(),
            },
            chemin: p.port_name,
        })
        .collect();
    ports.sort_by(|a, b| a.chemin.cmp(&b.chemin));
    ports.dedup_by(|a, b| a.chemin == b.chemin);
    Ok(ports)
}

/// Ce que l'on vérifie d'un chemin avant d'ouvrir : quelque chose qui
/// ressemble à un port, pas un fichier quelconque du disque.
fn chemin_plausible(chemin: &str) -> bool {
    if chemin.is_empty() || chemin.contains('\0') {
        return false;
    }
    #[cfg(windows)]
    {
        let c = chemin.trim_start_matches(r"\\.\");
        // Sur les octets, pas sur des tranches d'index str : le chemin vient du
        // champ libre « port série » du formulaire, et `c[..3]` paniquait
        // (« byte index 3 is not a char boundary ») quand l'octet 3 tombe au
        // milieu d'un caractère (« COé », « abé »). Trouvé par l'audit du
        // 7 septembre 2026 : la panique tuait la tâche tokio de `serie_open`,
        // laissant l'onglet figé sur « connexion à … ».
        let b = c.as_bytes();
        b.len() > 3 && b[..3].eq_ignore_ascii_case(b"com") && b[3..].iter().all(u8::is_ascii_digit)
    }
    #[cfg(not(windows))]
    {
        // Par sa cible réelle : un lien vers /dev/pts/N (socat, udev) est un
        // port, un fichier ordinaire n'en est pas un.
        std::fs::canonicalize(chemin).is_ok_and(|reel| reel.starts_with("/dev/"))
    }
}

/// Une session ouverte : ses canaux et de quoi la décrire.
pub struct SessionSerie {
    pub in_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    pub out_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    pub label: String,
}

/// Ouvre `chemin` à `vitesse` bauds, 8 bits, sans parité, un bit d'arrêt, sans
/// contrôle de flux (le réglage des consoles série), et lance les deux fils.
pub fn ouvrir(chemin: &str, vitesse: u32) -> Result<SessionSerie> {
    if !chemin_plausible(chemin) {
        return Err(anyhow!("« {chemin} » n'est pas un port série."));
    }
    if vitesse == 0 {
        return Err(anyhow!("La vitesse doit être un nombre de bauds non nul."));
    }
    let port = serialport::new(chemin, vitesse)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        // Une lecture rend la main régulièrement : c'est là que le fil de
        // lecture regarde si on lui a demandé de partir.
        .timeout(Duration::from_millis(50))
        .open()
        .with_context(|| format!("Ouverture de {chemin} impossible"))?;
    let mut ecriture = port
        .try_clone()
        .with_context(|| format!("Port {chemin} : second descripteur impossible"))?;
    let mut lecture = port;
    let arret = Arc::new(AtomicBool::new(false));
    let (in_tx, mut in_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);

    let arret_ecriture = arret.clone();
    std::thread::Builder::new()
        .name("serie-ecriture".into())
        .spawn(move || {
            use std::io::Write as _;
            while let Some(octets) = in_rx.blocking_recv() {
                if ecriture
                    .write_all(&octets)
                    .and_then(|()| ecriture.flush())
                    .is_err()
                {
                    break;
                }
            }
            arret_ecriture.store(true, Ordering::Relaxed);
        })
        .context("fil d'écriture série")?;

    std::thread::Builder::new()
        .name("serie-lecture".into())
        .spawn(move || {
            use std::io::Read as _;
            let mut tampon = [0u8; 4096];
            while !arret.load(Ordering::Relaxed) {
                match lecture.read(&mut tampon) {
                    Ok(0) => break,
                    Ok(n) => {
                        if out_tx.blocking_send(tampon[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        })
        .context("fil de lecture série")?;

    Ok(SessionSerie {
        in_tx,
        out_rx,
        label: format!("{chemin} @ {vitesse}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_chemin_hors_de_dev_n_est_pas_un_port() {
        #[cfg(not(windows))]
        {
            assert!(chemin_plausible("/dev/null"), "un nœud de /dev");
            assert!(!chemin_plausible("/etc/passwd"));
            assert!(!chemin_plausible("/dev/inexistant-avash"));
            assert!(!chemin_plausible(""));
            assert!(ouvrir("/etc/passwd", 9600).is_err());
            // Un lien vers /dev vaut sa cible : c'est ainsi que socat et udev
            // nomment un port.
            let lien =
                std::env::temp_dir().join(format!("avash-serie-lien-{}", std::process::id()));
            let _ = std::fs::remove_file(&lien);
            std::os::unix::fs::symlink("/dev/null", &lien).unwrap();
            assert!(chemin_plausible(lien.to_str().unwrap()));
            let _ = std::fs::remove_file(&lien);
        }
        #[cfg(windows)]
        {
            assert!(chemin_plausible("COM3"));
            assert!(chemin_plausible(r"\\.\COM12"));
            assert!(!chemin_plausible(r"C:\Windows\notepad.exe"));
            // Un chemin non ASCII dont l'octet 3 tombe au milieu d'un caractère
            // ne doit plus paniquer, juste être refusé (audit du 7 sept. 2026).
            assert!(!chemin_plausible("COé"));
            assert!(!chemin_plausible("abé"));
            assert!(!chemin_plausible("COMx"));
        }
        assert!(ouvrir("/dev/null", 0).is_err(), "vitesse nulle refusée");
    }

    /// Contrat K2 de l'audit du 12 septembre 2026 (C-SIL-12) : une
    /// énumération qui échoue (droits sur `/dev`, erreur de `serialport`)
    /// n'est pas un poste sans port. `unwrap_or_default` la rendait
    /// indistinguable, et le formulaire disait « aucun port série ».
    #[test]
    fn une_enumeration_en_erreur_ne_vaut_pas_zero_port() {
        let e = ports_depuis(Err(serialport::Error::new(
            serialport::ErrorKind::Unknown,
            "lecture de /dev refusée",
        )))
        .unwrap_err();
        assert!(e.to_string().contains("lecture de /dev refusée"), "{e}");
        let port = |nom: &str| serialport::SerialPortInfo {
            port_name: nom.into(),
            port_type: serialport::SerialPortType::Unknown,
        };
        let ports = ports_depuis(Ok(vec![
            port("/dev/ttyUSB1"),
            port("/dev/ttyUSB0"),
            port("/dev/ttyUSB1"),
        ]))
        .unwrap();
        let chemins: Vec<&str> = ports.iter().map(|p| p.chemin.as_str()).collect();
        assert_eq!(
            chemins,
            ["/dev/ttyUSB0", "/dev/ttyUSB1"],
            "triés, sans doublon"
        );
        assert!(ports_depuis(Ok(Vec::new())).unwrap().is_empty());
    }

    /// Un pseudo-terminal : le maître, possédé (fermé à sa chute), et
    /// l'esclave par son chemin, que la session rouvre elle-même.
    ///
    /// Audit du 12 septembre 2026 (C-unsafe-1) : quatre blocs `unsafe` de FFI
    /// libc faisaient ce travail, dont un `CStr::from_ptr(ptsname(m))` sans
    /// test du pointeur nul, sur le tampon statique non réentrant de
    /// `ptsname(3)`. `nix` (déjà compilé pour `serialport`) donne des types
    /// possédants et `ptsname_r`, réentrant, qui rend une erreur au lieu d'un
    /// pointeur nul : plus un seul `unsafe`.
    #[cfg(target_os = "linux")]
    fn pty() -> (nix::pty::PtyMaster, String) {
        use nix::fcntl::OFlag;
        let maitre = nix::pty::posix_openpt(OFlag::O_RDWR | OFlag::O_NOCTTY).expect("posix_openpt");
        nix::pty::grantpt(&maitre).expect("grantpt");
        nix::pty::unlockpt(&maitre).expect("unlockpt");
        let nom = nix::pty::ptsname_r(&maitre).expect("ptsname_r");
        (maitre, nom)
    }

    /// Ce qui est écrit ressort par le maître du pseudo-terminal, et ce que
    /// le maître écrit arrive par `out_rx` ; fermer `in_tx` termine tout.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn une_session_lit_et_ecrit_sur_un_pseudo_terminal() {
        use std::io::{Read as _, Write as _};
        let (mut maitre, esclave) = pty();
        let SessionSerie {
            in_tx,
            mut out_rx,
            label,
        } = ouvrir(&esclave, 115_200).unwrap();
        assert_eq!(label, format!("{esclave} @ 115200"));

        in_tx.send(b"show version\r".to_vec()).await.unwrap();
        let mut recu = Vec::new();
        let mut tampon = [0u8; 64];
        while !recu.ends_with(b"\r") {
            let n = maitre.read(&mut tampon).unwrap();
            assert!(n > 0);
            recu.extend_from_slice(&tampon[..n]);
        }
        assert_eq!(recu, b"show version\r");

        maitre.write_all(b"Cisco IOS\r\n").unwrap();
        let mut sortie = Vec::new();
        while !sortie.ends_with(b"\r\n") {
            let bloc = tokio::time::timeout(Duration::from_secs(5), out_rx.recv())
                .await
                .expect("sortie attendue")
                .expect("canal vivant");
            sortie.extend_from_slice(&bloc);
        }
        assert_eq!(sortie, b"Cisco IOS\r\n");

        drop(in_tx);
        // Le fil de lecture part à sa prochaine échéance : le canal se ferme.
        let fin = tokio::time::timeout(Duration::from_secs(5), async {
            while out_rx.recv().await.is_some() {}
        })
        .await;
        assert!(
            fin.is_ok(),
            "la session ne s'est pas arrêtée après la fermeture du clavier"
        );
        drop(maitre);
    }
}
