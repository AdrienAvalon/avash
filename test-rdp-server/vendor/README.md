# Correctif porté sur ironrdp-server

Un seul paquet est copié ici, `ironrdp-server` 0.13.0, avec un seul ajout,
dans `src/server.rs`, `src/builder.rs` et `src/lib.rs`. Tout le reste est
identique à l'amont ; `diff -r` avec la version de crates.io ne doit signaler
que ces trois fichiers, plus le `rustfmt.toml` décrit plus bas.

## Ce qui manquait

Le serveur de test doit jouer le côté serveur de la redirection de lecteur
(canal statique `rdpdr`, [MS-RDPEFS]) pour que la suite bout en bout vérifie
que le lecteur annoncé par le sidecar est bien servi. Or `ironrdp-server`
n'offre **aucun point d'attache** pour un canal statique arbitraire :
`RdpServer::attach_channels` (privé) attache en dur cliprdr, rdpsnd et
drdynvc par `Acceptor::attach_static_channel`, publique mais hors de portée
une fois le serveur construit. La réception, elle, est déjà générique :
`handle_x224` route tout `SendDataRequest` d'un canal statique connu vers son
`process()` et renvoie la réponse. Il ne manquait que l'attache.

## L'ajout

- `SvcServerFactory`, un trait de fabrique (`build_svc() -> Box<dyn
  SvcServerProcessor>`), appelé **à chaque connexion** comme les fabriques de
  cliprdr et de rdpsnd : un canal porte l'état d'une session et ne survit pas
  au client.
- `RdpServerBuilder::with_static_channel_factory(Option<Box<dyn
  SvcServerFactory>>)`, et l'attache dans `attach_channels`, juste après
  rdpsnd. L'acceptor apparie les canaux attachés aux noms que le client
  demande dans ses données GCC : sans demande de `rdpdr`, le canal n'est pas
  joint et reste inerte.
- `BoxedSvc`, une enveloppe privée qui délègue `SvcProcessor` à la boîte.
  `attach_static_channel` exige un type concret dont il tire le `TypeId` qui
  indexe le canal dans `StaticChannelSet` ; une boîte de trait n'en a pas
  d'utile. Conséquence assumée : **un seul canal** passe par ce chemin (une
  seconde fabrique remplacerait la première sous le même `TypeId`), d'où une
  option plutôt qu'une liste.

Le canal lui-même (poignée de main, scénario, décodeurs des réponses du
client) vit dans `test-rdp-server/src/rdpdr/`, pas ici.

## `rustfmt.toml`

IronRDP formate à 120 colonnes avec le style de l'édition 2024 ; un rustfmt
lancé depuis avash (largeur par défaut, édition 2021) reformatait tout le
paquet et retriait ses imports, 629 lignes changées avant la première ligne
utile. Le fichier fige le style de l'amont pour que le diff reste lisible.

## À retirer quand l'amont l'offrira

Ce n'est pas un défaut, c'est une absence : dès qu'une version publiée
d'`ironrdp-server` permet d'attacher un canal statique par son builder,
supprimer ce répertoire, la section `[patch.crates-io]` de
`test-rdp-server/Cargo.toml`, et brancher `FabriqueRdpdr` sur l'API amont.

[MS-RDPEFS]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpefs/34d9de58-b2b5-40b6-b970-f82d4603bdb5
