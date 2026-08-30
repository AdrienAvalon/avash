//! Tunnels SSH : redirections de port locales (`ssh -L`), distantes
//! (`ssh -R`) et dynamiques SOCKS5 (`ssh -D`).
//!
//! Un tunnel vit sur sa propre connexion SSH, independante des onglets de
//! terminal : fermer un onglet ne coupe pas un tunnel, et inversement.
//! Les definitions sont conservees dans `~/.config/avash/tunnels.yaml`.

use crate::ssh::{AvashSession, ForwardCounters};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;

/// Sens d'un tunnel, avec la lettre `ssh` correspondante.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelKind {
    /// `-L` : un port chez nous mene, via le serveur, a une destination.
    Local,
    /// `-R` : un port sur le serveur mene, via nous, a une destination locale.
    Remote,
    /// `-D` : un mandataire SOCKS5 chez nous, sortant par le serveur.
    Dynamic,
}

/// Definition persistante d'un tunnel, rattachee a un hote de `~/.ssh/config`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelDef {
    pub id: String,
    pub alias: String,
    pub kind: TunnelKind,
    /// Port ecoute : chez nous (`-L`, `-D`) ou sur le serveur (`-R`).
    /// 0 = choisi automatiquement.
    pub bind_port: u16,
    /// Destination : jointe par le serveur (`-L`) ou chez nous (`-R`).
    /// Sans objet en `-D`.
    #[serde(default)]
    pub target_host: String,
    #[serde(default)]
    pub target_port: u16,
    /// Libelle libre, facultatif.
    #[serde(default)]
    pub name: String,
}

impl TunnelDef {
    /// Cree une definition avec un identifiant neuf.
    #[must_use]
    pub fn new(
        alias: &str,
        kind: TunnelKind,
        bind_port: u16,
        target_host: &str,
        target_port: u16,
        name: &str,
    ) -> Self {
        Self {
            id: format!("t-{:016x}", rand::random::<u64>()),
            alias: alias.to_string(),
            kind,
            bind_port,
            target_host: target_host.trim().to_string(),
            target_port,
            name: name.trim().to_string(),
        }
    }

    /// Refuse une definition incoherente avec un message actionnable.
    pub fn validate(&self) -> Result<()> {
        if self.alias.trim().is_empty() {
            bail!("Le tunnel n'est rattaché à aucun hôte.");
        }
        match self.kind {
            TunnelKind::Dynamic => {
                if self.bind_port == 0 {
                    bail!("Un mandataire SOCKS a besoin d'un port local fixe (ex. 1080).");
                }
            }
            TunnelKind::Local | TunnelKind::Remote => {
                if self.target_host.trim().is_empty() {
                    bail!("L'hôte de destination est vide.");
                }
                if self.target_port == 0 {
                    bail!("Le port de destination est vide.");
                }
                // Un port a 0 est valide en SSH (choisi automatiquement) mais
                // inutilisable pour un humain : il ne saurait pas ou se
                // connecter.
                if self.bind_port == 0 {
                    bail!("Le port d'écoute est vide.");
                }
            }
        }
        Ok(())
    }

    /// Resume lisible, dans le sens du trafic.
    #[must_use]
    pub fn describe(&self) -> String {
        match self.kind {
            TunnelKind::Local => format!(
                "localhost:{} → {} → {}:{}",
                self.bind_port, self.alias, self.target_host, self.target_port
            ),
            TunnelKind::Remote => format!(
                "{}:{} → localhost → {}:{}",
                self.alias, self.bind_port, self.target_host, self.target_port
            ),
            TunnelKind::Dynamic => format!("SOCKS5 localhost:{} → {}", self.bind_port, self.alias),
        }
    }
}

/// Instantane des compteurs, serialisable pour l'interface.
#[derive(Debug, Clone, Serialize)]
pub struct TunnelSnapshot {
    pub bound_port: u16,
    pub active: u64,
    pub total: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
    /// La connexion SSH porteuse est-elle encore debout ?
    pub alive: bool,
    pub last_error: Option<String>,
}

/// Un tunnel ouvert. Se ferme via [`Tunnel::close`] ou a la destruction.
pub struct Tunnel {
    def: TunnelDef,
    bound_port: u16,
    counters: Arc<ForwardCounters>,
    last_error: Arc<std::sync::Mutex<Option<String>>>,
    session: Arc<AvashSession>,
    /// Boucle d'acceptation (`-L`, `-D`). Aucune en `-R` : c'est le serveur
    /// qui nous ouvre les canaux.
    acceptor: Option<tokio::task::JoinHandle<()>>,
}

impl Tunnel {
    /// Ouvre le tunnel sur une session fraichement authentifiee.
    pub async fn open(session: AvashSession, def: TunnelDef) -> Result<Self> {
        def.validate()?;
        let session = Arc::new(session);
        let counters = Arc::new(ForwardCounters::default());
        let last_error = Arc::new(std::sync::Mutex::new(None));

        let (bound_port, acceptor) = match def.kind {
            TunnelKind::Remote => {
                let port = session
                    .remote_forward(
                        "localhost",
                        def.bind_port,
                        &def.target_host,
                        def.target_port,
                        counters.clone(),
                    )
                    .await?;
                (port, None)
            }
            TunnelKind::Local | TunnelKind::Dynamic => {
                // Loopback seulement : exposer le tunnel a tout le reseau
                // local serait une surprise dangereuse pour l'utilisateur.
                let listener = TcpListener::bind(("127.0.0.1", def.bind_port))
                    .await
                    .with_context(|| {
                        format!("Impossible d'écouter sur le port local {}", def.bind_port)
                    })?;
                let port = listener.local_addr()?.port();
                let task = tokio::spawn(accept_loop(
                    listener,
                    session.clone(),
                    def.clone(),
                    counters.clone(),
                    last_error.clone(),
                ));
                (port, Some(task))
            }
        };

        Ok(Self {
            def,
            bound_port,
            counters,
            last_error,
            session,
            acceptor,
        })
    }

    #[must_use]
    pub fn def(&self) -> &TunnelDef {
        &self.def
    }

    /// Port reellement ecoute (utile quand la definition demandait 0).
    #[must_use]
    pub fn bound_port(&self) -> u16 {
        self.bound_port
    }

    #[must_use]
    pub fn snapshot(&self) -> TunnelSnapshot {
        let alive =
            !self.session.is_closed() && self.acceptor.as_ref().is_none_or(|t| !t.is_finished());
        TunnelSnapshot {
            bound_port: self.bound_port,
            active: self.counters.active.load(Ordering::Relaxed),
            total: self.counters.total.load(Ordering::Relaxed),
            bytes_up: self.counters.bytes_up.load(Ordering::Relaxed),
            bytes_down: self.counters.bytes_down.load(Ordering::Relaxed),
            alive,
            last_error: self.last_error.lock().unwrap().clone(),
        }
    }

    /// Ferme le tunnel et sa connexion SSH.
    pub async fn close(mut self) {
        if let Some(task) = self.acceptor.take() {
            task.abort();
        }
        if self.def.kind == TunnelKind::Remote {
            let _ = self
                .session
                .cancel_remote_forward("localhost", self.bound_port)
                .await;
        }
        let _ = self.session.disconnect().await;
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        if let Some(task) = self.acceptor.take() {
            task.abort();
        }
    }
}

/// Accepte les connexions locales et les relaie dans un canal SSH chacune.
async fn accept_loop(
    listener: TcpListener,
    session: Arc<AvashSession>,
    def: TunnelDef,
    counters: Arc<ForwardCounters>,
    last_error: Arc<std::sync::Mutex<Option<String>>>,
) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                *last_error.lock().unwrap() = Some(format!("Écoute interrompue : {e}"));
                break;
            }
        };
        let session = session.clone();
        let def = def.clone();
        let counters = counters.clone();
        let last_error = last_error.clone();
        tokio::spawn(async move {
            if let Err(e) = relay_one(stream, peer, &session, &def, &counters).await {
                *last_error.lock().unwrap() = Some(e.to_string());
            }
        });
    }
}

/// Relaie une connexion locale : negociation SOCKS si besoin, puis canal SSH.
async fn relay_one(
    mut stream: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
    session: &AvashSession,
    def: &TunnelDef,
    counters: &ForwardCounters,
) -> Result<()> {
    let (host, port) = match def.kind {
        TunnelKind::Local => (def.target_host.clone(), def.target_port),
        TunnelKind::Dynamic => socks5_handshake(&mut stream).await?,
        TunnelKind::Remote => unreachable!("un tunnel distant n'a pas de boucle d'acceptation"),
    };
    let channel = match session.open_direct_tcpip(&host, port, peer).await {
        Ok(c) => c,
        Err(e) => {
            if def.kind == TunnelKind::Dynamic {
                // 0x05 : connexion refusee. Le client SOCKS affiche une
                // erreur claire au lieu d'attendre.
                let _ = stream.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await;
            }
            return Err(e);
        }
    };
    if def.kind == TunnelKind::Dynamic {
        // Succes, adresse liee sans importance pour un client CONNECT.
        stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    }
    let mut remote = channel.into_stream();
    counters.relay(&mut stream, &mut remote).await;
    Ok(())
}

/// Negocie une requete SOCKS5 `CONNECT` (RFC 1928) sans authentification.
///
/// Rend la destination demandee ; la reponse finale (succes ou refus) est
/// envoyee par l'appelant une fois le canal SSH tente.
pub async fn socks5_handshake<S>(s: &mut S) -> Result<(String, u16)>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // Salutation : version, methodes proposees.
    let mut head = [0u8; 2];
    s.read_exact(&mut head).await?;
    if head[0] != 5 {
        bail!(
            "Client SOCKS version {} : seul SOCKS5 est pris en charge",
            head[0]
        );
    }
    let mut methods = vec![0u8; usize::from(head[1])];
    s.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        s.write_all(&[5, 0xFF]).await?;
        bail!("Le client SOCKS exige une authentification");
    }
    s.write_all(&[5, 0]).await?;

    // Requete : version, commande, reserve, type d'adresse.
    let mut req = [0u8; 4];
    s.read_exact(&mut req).await?;
    if req[1] != 1 {
        s.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        bail!(
            "Commande SOCKS {} non prise en charge (seul CONNECT l'est)",
            req[1]
        );
    }
    let host = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            s.read_exact(&mut a).await?;
            std::net::Ipv4Addr::from(a).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await?;
            let mut name = vec![0u8; usize::from(len[0])];
            s.read_exact(&mut name).await?;
            String::from_utf8(name).map_err(|_| anyhow!("Nom d'hôte SOCKS illisible"))?
        }
        4 => {
            let mut a = [0u8; 16];
            s.read_exact(&mut a).await?;
            std::net::Ipv6Addr::from(a).to_string()
        }
        other => {
            s.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            bail!("Type d'adresse SOCKS {other} inconnu");
        }
    };
    let mut p = [0u8; 2];
    s.read_exact(&mut p).await?;
    Ok((host, u16::from_be_bytes(p)))
}

// ---------- Persistance des definitions ----------

/// `~/.config/avash/tunnels.yaml`
#[must_use]
pub fn defs_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("avash")
        .join("tunnels.yaml")
}

pub fn load_defs() -> Result<Vec<TunnelDef>> {
    load_defs_from(&defs_path())
}

pub fn save_defs(defs: &[TunnelDef]) -> Result<()> {
    save_defs_to(&defs_path(), defs)
}

/// Un fichier absent n'est pas une erreur : c'est l'etat initial.
pub fn load_defs_from(path: &Path) -> Result<Vec<TunnelDef>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("Lecture de {}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_yaml::from_str(&text).with_context(|| format!("{} est illisible", path.display()))
}

/// Ecriture atomique : un plantage en pleine ecriture ne laisse pas un
/// fichier tronque qui perdrait tous les tunnels.
pub fn save_defs_to(path: &Path, defs: &[TunnelDef]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, serde_yaml::to_string(defs)?)?;
    std::fs::rename(&tmp, path)?;
    crate::restreindre_au_proprietaire(path);
    Ok(())
}

/// Ajoute ou remplace (meme `id`) une definition.
pub fn upsert_def_in(path: &Path, def: TunnelDef) -> Result<Vec<TunnelDef>> {
    def.validate()?;
    let mut defs = load_defs_from(path)?;
    match defs.iter_mut().find(|d| d.id == def.id) {
        Some(slot) => *slot = def,
        None => defs.push(def),
    }
    save_defs_to(path, &defs)?;
    Ok(defs)
}

pub fn remove_def_in(path: &Path, id: &str) -> Result<Vec<TunnelDef>> {
    let mut defs = load_defs_from(path)?;
    defs.retain(|d| d.id != id);
    save_defs_to(path, &defs)?;
    Ok(defs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(kind: TunnelKind) -> TunnelDef {
        TunnelDef::new("prod", kind, 8080, "db.interne", 5432, "")
    }

    fn temp_file() -> PathBuf {
        std::env::temp_dir().join(format!(
            "avash-tunnels-{}-{:?}.yaml",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    #[test]
    fn deux_definitions_ont_des_identifiants_distincts() {
        assert_ne!(def(TunnelKind::Local).id, def(TunnelKind::Local).id);
    }

    #[test]
    fn validation_refuse_une_destination_vide_sauf_en_socks() {
        let mut d = def(TunnelKind::Local);
        d.target_host = "  ".into();
        assert!(d.validate().is_err());
        let mut d = def(TunnelKind::Dynamic);
        d.target_host = String::new();
        d.target_port = 0;
        assert!(d.validate().is_ok(), "SOCKS n'a pas de destination fixe");
    }

    #[test]
    fn validation_refuse_un_port_d_ecoute_a_zero() {
        for kind in [TunnelKind::Local, TunnelKind::Remote, TunnelKind::Dynamic] {
            let mut d = def(kind);
            d.bind_port = 0;
            assert!(d.validate().is_err(), "{kind:?}");
        }
    }

    #[test]
    fn description_suit_le_sens_du_trafic() {
        assert_eq!(
            def(TunnelKind::Local).describe(),
            "localhost:8080 → prod → db.interne:5432"
        );
        assert_eq!(
            def(TunnelKind::Remote).describe(),
            "prod:8080 → localhost → db.interne:5432"
        );
        assert_eq!(
            def(TunnelKind::Dynamic).describe(),
            "SOCKS5 localhost:8080 → prod"
        );
    }

    #[test]
    fn fichier_absent_donne_une_liste_vide() {
        let p = temp_file();
        let _ = std::fs::remove_file(&p);
        assert!(load_defs_from(&p).unwrap().is_empty());
    }

    #[test]
    fn upsert_puis_remove_font_l_aller_retour_yaml() {
        let p = temp_file();
        let _ = std::fs::remove_file(&p);
        let a = def(TunnelKind::Local);
        let mut b = def(TunnelKind::Dynamic);
        b.name = "proxy bureau".into();
        upsert_def_in(&p, a.clone()).unwrap();
        let defs = upsert_def_in(&p, b.clone()).unwrap();
        assert_eq!(defs, vec![a.clone(), b.clone()]);

        // Remplacement par id : pas de doublon.
        let mut a2 = a.clone();
        a2.bind_port = 9090;
        let defs = upsert_def_in(&p, a2.clone()).unwrap();
        assert_eq!(defs, vec![a2.clone(), b.clone()]);
        assert_eq!(load_defs_from(&p).unwrap(), defs, "relecture identique");

        let defs = remove_def_in(&p, &a2.id).unwrap();
        assert_eq!(defs, vec![b]);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn upsert_refuse_une_definition_invalide_sans_toucher_au_fichier() {
        let p = temp_file();
        let _ = std::fs::remove_file(&p);
        let mut d = def(TunnelKind::Local);
        d.target_port = 0;
        assert!(upsert_def_in(&p, d).is_err());
        assert!(!p.exists(), "rien ne doit etre ecrit");
    }

    #[test]
    fn yaml_ancien_sans_champs_optionnels_reste_lisible() {
        // Compatibilite : un fichier ecrit par une version sans `name` ni
        // destination (SOCKS) doit encore se charger.
        let p = temp_file();
        std::fs::write(
            &p,
            "- id: t-1\n  alias: prod\n  kind: dynamic\n  bind_port: 1080\n",
        )
        .unwrap();
        let defs = load_defs_from(&p).unwrap();
        assert_eq!(defs[0].kind, TunnelKind::Dynamic);
        assert_eq!(defs[0].target_host, "");
        let _ = std::fs::remove_file(&p);
    }

    #[tokio::test]
    async fn socks5_connect_par_nom_de_domaine() {
        let (mut client, mut server) = tokio::io::duplex(256);
        let task = tokio::spawn(async move { socks5_handshake(&mut server).await });
        client.write_all(&[5, 1, 0]).await.unwrap();
        let mut rep = [0u8; 2];
        client.read_exact(&mut rep).await.unwrap();
        assert_eq!(rep, [5, 0]);
        let mut req = vec![5, 1, 0, 3, 11];
        req.extend_from_slice(b"example.org");
        req.extend_from_slice(&443u16.to_be_bytes());
        client.write_all(&req).await.unwrap();
        let (host, port) = task.await.unwrap().unwrap();
        assert_eq!((host.as_str(), port), ("example.org", 443));
    }

    #[tokio::test]
    async fn socks5_connect_par_ipv4() {
        let (mut client, mut server) = tokio::io::duplex(256);
        let task = tokio::spawn(async move { socks5_handshake(&mut server).await });
        client.write_all(&[5, 1, 0]).await.unwrap();
        let mut rep = [0u8; 2];
        client.read_exact(&mut rep).await.unwrap();
        client
            .write_all(&[5, 1, 0, 1, 10, 0, 0, 7, 0x1F, 0x90])
            .await
            .unwrap();
        let (host, port) = task.await.unwrap().unwrap();
        assert_eq!((host.as_str(), port), ("10.0.0.7", 8080));
    }

    #[tokio::test]
    async fn socks5_refuse_un_client_exigeant_une_authentification() {
        let (mut client, mut server) = tokio::io::duplex(256);
        let task = tokio::spawn(async move { socks5_handshake(&mut server).await });
        // Seule methode proposee : 0x02 (user/pass).
        client.write_all(&[5, 1, 2]).await.unwrap();
        let mut rep = [0u8; 2];
        client.read_exact(&mut rep).await.unwrap();
        assert_eq!(rep, [5, 0xFF], "aucune methode acceptable");
        assert!(task.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn socks4_est_refuse() {
        let (mut client, mut server) = tokio::io::duplex(256);
        let task = tokio::spawn(async move { socks5_handshake(&mut server).await });
        client
            .write_all(&[4, 1, 0, 80, 1, 2, 3, 4, 0])
            .await
            .unwrap();
        assert!(task.await.unwrap().is_err());
    }
}
