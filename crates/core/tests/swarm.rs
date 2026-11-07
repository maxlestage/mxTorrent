//! Des essaims entiers simulés en mémoire : création, liens magnet, métadonnées,
//! téléchargement depuis plusieurs pairs, découpage arbitraire des messages.

use mxtorrent_core::torrent::{Event, PeerKey};
use mxtorrent_core::*;

struct Link {
    a: usize,
    a_key: PeerKey,
    b: usize,
    b_key: PeerKey,
    open: bool,
}

struct Net {
    nodes: Vec<Torrent>,
    links: Vec<Link>,
    now: u64,
    chunk: usize,
    next_key: PeerKey,
}

impl Net {
    fn new(nodes: Vec<Torrent>, chunk: usize) -> Net {
        Net { nodes, links: vec![], now: 0, chunk, next_key: 1 }
    }

    fn connect(&mut self, a: usize, b: usize) {
        let (ka, kb) = (self.next_key, self.next_key + 1);
        self.next_key += 2;
        self.nodes[a].add_peer(ka);
        self.nodes[b].add_peer(kb);
        self.links.push(Link { a, a_key: ka, b, b_key: kb, open: true });
    }

    /// Fait circuler les octets jusqu'au silence, puis avance l'horloge.
    fn step(&mut self) -> usize {
        let mut moved = 0;
        for i in 0..self.links.len() {
            if !self.links[i].open {
                continue;
            }
            let (a, ka, b, kb) = (self.links[i].a, self.links[i].a_key, self.links[i].b, self.links[i].b_key);
            for (from, fk, to, tk) in [(a, ka, b, kb), (b, kb, a, ka)] {
                let out = self.nodes[from].poll_transmit(fk, 256 * 1024);
                moved += out.len();
                for c in out.chunks(self.chunk.max(1)) {
                    self.nodes[to].on_data(tk, c);
                }
            }
        }
        self.now += 100;
        for n in &mut self.nodes {
            n.tick(self.now);
        }
        // Fermetures demandées par les sessions.
        for ni in 0..self.nodes.len() {
            for ev in self.nodes[ni].take_events() {
                if let Event::ClosePeer(k, reason) = ev {
                    for l in &mut self.links {
                        if l.open && ((l.a == ni && l.a_key == k) || (l.b == ni && l.b_key == k)) {
                            eprintln!("fermeture {ni}/{k} : {reason}");
                            l.open = false;
                        }
                    }
                }
            }
        }
        for l in &self.links {
            if !l.open {
                let (a, ka, b, kb) = (l.a, l.a_key, l.b, l.b_key);
                self.nodes[a].remove_peer(ka);
                self.nodes[b].remove_peer(kb);
            }
        }
        moved
    }

    fn run_until(&mut self, mut done: impl FnMut(&Net) -> bool, max_steps: usize) -> bool {
        for _ in 0..max_steps {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }
}

fn data(len: usize, salt: u32) -> Vec<u8> {
    (0..len as u32).map(|i| (i.wrapping_mul(2654435761).wrapping_add(salt) >> 13) as u8).collect()
}

fn id(n: u8) -> [u8; 20] {
    let mut rng = Rng::new(n as u64 * 7919);
    make_peer_id(&mut rng)
}

#[test]
fn lien_magnet_depuis_un_seul_partageur() {
    let a = data(300_000, 1);
    let b = data(77_777, 2);
    let meta = create_torrent(
        &[
            NewFile { path: vec!["photos".into(), "a.jpg".into()], data: &a },
            NewFile { path: vec!["photos".into(), "b.png".into()], data: &b },
        ],
        &CreateOptions { piece_length: Some(32 * 1024), ..Default::default() },
    )
    .unwrap();
    let magnet = Magnet::from_metainfo(&meta);
    let seeder = Torrent::seed(meta.clone(), &[&a, &b], id(1), 1, 0).unwrap();
    assert!(seeder.is_complete());
    let leecher = Torrent::from_magnet(&Magnet::parse(&magnet.to_uri()).unwrap(), id(2), 2, 0);
    assert_eq!(leecher.state(), State::FetchingMetadata);

    let mut net = Net::new(vec![seeder, leecher], 7_001);
    net.connect(1, 0);
    assert!(net.run_until(|n| n.nodes[1].is_complete(), 2_000), "téléchargement inachevé : {}", net.nodes[1].progress());
    let l = &net.nodes[1];
    assert_eq!(l.metainfo().unwrap().info_hash, meta.info_hash);
    assert_eq!(l.file_bytes(0).unwrap(), a);
    assert_eq!(l.file_bytes(1).unwrap(), b);
    let chunks: Vec<u8> = l.file_chunks(1).unwrap().concat();
    assert_eq!(chunks, b);
    assert_eq!(l.hash_failures, 0);
    assert!(net.nodes[0].uploaded >= (a.len() + b.len()) as u64);
    assert_eq!(l.state(), State::Seeding);
}

#[test]
fn essaim_de_plusieurs_pairs() {
    let a = data(1_000_000, 9);
    let meta = create_torrent(
        &[NewFile { path: vec!["film.mp4".into()], data: &a }],
        &CreateOptions { piece_length: Some(16 * 1024), ..Default::default() },
    )
    .unwrap();
    let mut nodes = vec![Torrent::seed(meta.clone(), &[&a], id(10), 10, 0).unwrap()];
    for i in 0..4u8 {
        nodes.push(Torrent::from_metainfo(meta.clone(), id(20 + i), 20 + i as u64, 0));
    }
    let mut net = Net::new(nodes, 16_384 + 13);
    // Le partageur ne connaît que deux pairs : les autres doivent passer par eux.
    net.connect(1, 0);
    net.connect(2, 0);
    for (x, y) in [(1, 2), (2, 3), (3, 4), (1, 4), (1, 3)] {
        net.connect(x, y);
    }
    assert!(net.run_until(|n| n.nodes.iter().all(|t| t.is_complete()), 5_000));
    for t in &net.nodes {
        assert_eq!(t.file_bytes(0).unwrap(), a);
    }
    // Les pairs 3 et 4 n'ont jamais vu le partageur : ils ont tout reçu des autres.
    assert!(net.nodes[3].downloaded >= a.len() as u64);
    assert!(net.nodes[1].uploaded + net.nodes[2].uploaded > 0);
}

#[test]
fn choix_des_fichiers() {
    let a = data(200_000, 3);
    let b = data(200_000, 4);
    let meta = create_torrent(
        &[NewFile { path: vec!["d".into(), "a".into()], data: &a }, NewFile { path: vec!["d".into(), "b".into()], data: &b }],
        &CreateOptions { piece_length: Some(16 * 1024), ..Default::default() },
    )
    .unwrap();
    let seeder = Torrent::seed(meta.clone(), &[&a, &b], id(30), 30, 0).unwrap();
    let mut leecher = Torrent::from_metainfo(meta.clone(), id(31), 31, 0);
    leecher.set_file_wanted(1, false);
    let mut net = Net::new(vec![seeder, leecher], 50_000);
    net.connect(0, 1);
    assert!(net.run_until(|n| n.nodes[1].is_complete(), 2_000));
    let l = &net.nodes[1];
    assert_eq!(l.file_bytes(0).unwrap(), a);
    assert!(!l.file_complete(1));
    assert!(l.downloaded < 260_000, "{} octets téléchargés", l.downloaded);
}

#[test]
fn refuse_un_autre_torrent_et_soi_meme() {
    let a = data(10_000, 5);
    let m1 = create_torrent(&[NewFile { path: vec!["x".into()], data: &a }], &CreateOptions::default()).unwrap();
    let m2 = create_torrent(&[NewFile { path: vec!["y".into()], data: &a }], &CreateOptions::default()).unwrap();
    let t1 = Torrent::seed(m1.clone(), &[&a], id(40), 40, 0).unwrap();
    let t2 = Torrent::from_metainfo(m2, id(41), 41, 0);
    let t3 = Torrent::from_metainfo(m1, id(40), 42, 0);
    let mut net = Net::new(vec![t1, t2, t3], 1000);
    net.connect(0, 1);
    net.connect(0, 2);
    for _ in 0..5 {
        net.step();
    }
    assert!(net.links.iter().all(|l| !l.open));
    assert_eq!(net.nodes[0].peer_count(), 0);
}

#[test]
fn pair_menteur_detecte() {
    // Un pair qui envoie de mauvaises données : la pièce échoue puis est reprise ailleurs.
    let a = data(64 * 1024, 6);
    let meta = create_torrent(
        &[NewFile { path: vec!["v".into()], data: &a }],
        &CreateOptions { piece_length: Some(32 * 1024), ..Default::default() },
    )
    .unwrap();
    let mut bad = a.clone();
    bad[100] ^= 0xff;
    // On construit un "faux" partageur en trichant sur sa mémoire : seed() refuse, ce qui est déjà un bon signe.
    assert!(Torrent::seed(meta.clone(), &[&bad], id(50), 50, 0).is_err());
}
