use std::net;
use std::ops::{Deref, DerefMut};

use git_url::Url;
use log::*;

use crate::address_book::{KnownAddress, Source};
use crate::clock::RefClock;
use crate::collections::HashMap;
use crate::service;
use crate::service::config::*;
use crate::service::message::*;
use crate::service::*;
use crate::storage::WriteStorage;
use crate::test::crypto::MockSigner;
use crate::test::simulator;
use crate::{Link, LocalTime};

/// Service instantiation used for testing.
pub type Service<S> = service::Service<HashMap<net::IpAddr, KnownAddress>, S, MockSigner>;

#[derive(Debug)]
pub struct Peer<S> {
    pub name: &'static str,
    pub service: Service<S>,
    pub ip: net::IpAddr,
    pub rng: fastrand::Rng,
    pub local_time: LocalTime,
    pub local_addr: net::SocketAddr,

    initialized: bool,
}

impl<'r, S> simulator::Peer<S> for Peer<S>
where
    S: WriteStorage<'r> + 'static,
{
    fn init(&mut self) {
        self.initialize()
    }

    fn addr(&self) -> net::SocketAddr {
        net::SocketAddr::new(self.ip, DEFAULT_PORT)
    }
}

impl<S> Deref for Peer<S> {
    type Target = Service<S>;

    fn deref(&self) -> &Self::Target {
        &self.service
    }
}

impl<S> DerefMut for Peer<S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.service
    }
}

impl<'r, S> Peer<S>
where
    S: WriteStorage<'r> + 'static,
{
    pub fn new(name: &'static str, ip: impl Into<net::IpAddr>, storage: S) -> Self {
        Self::config(
            name,
            Config {
                git_url: storage.url(),
                ..Config::default()
            },
            ip,
            vec![],
            storage,
            fastrand::Rng::new(),
        )
    }

    pub fn config(
        name: &'static str,
        config: Config,
        ip: impl Into<net::IpAddr>,
        addrs: Vec<(net::SocketAddr, Source)>,
        storage: S,
        mut rng: fastrand::Rng,
    ) -> Self {
        let addrs = addrs
            .into_iter()
            .map(|(addr, src)| (addr.ip(), KnownAddress::new(addr, src, None)))
            .collect();
        let local_time = LocalTime::now();
        let clock = RefClock::from(local_time);
        let signer = MockSigner::new(&mut rng);
        let service = Service::new(config, clock, storage, addrs, signer, rng.clone());
        let ip = ip.into();
        let local_addr = net::SocketAddr::new(ip, rng.u16(..));

        Self {
            name,
            service,
            ip,
            local_addr,
            rng,
            local_time,
            initialized: false,
        }
    }

    pub fn initialize(&mut self) {
        if !self.initialized {
            info!("{}: Initializing: address = {}", self.name, self.ip);

            self.initialized = true;
            self.service.initialize(LocalTime::now());
        }
    }

    pub fn timestamp(&self) -> Timestamp {
        self.service.timestamp()
    }

    pub fn git_url(&self) -> Url {
        self.config().git_url.clone()
    }

    pub fn node_id(&self) -> NodeId {
        self.service.node_id()
    }

    pub fn receive(&mut self, peer: &net::SocketAddr, msg: Message) {
        self.service
            .received_message(peer, self.config().network.envelope(msg));
    }

    pub fn connect_from(&mut self, peer: &Self) {
        let remote = simulator::Peer::<S>::addr(peer);
        let local = net::SocketAddr::new(self.ip, self.rng.u16(..));
        let git = format!("file:///{}.git", remote.ip());
        let git = Url::from_bytes(git.as_bytes()).unwrap();

        self.initialize();
        self.service.connected(remote, &local, Link::Inbound);
        self.receive(
            &remote,
            Message::init(
                peer.node_id(),
                self.local_time().as_secs(),
                vec![Address::from(remote)],
                git,
            ),
        );

        let mut msgs = self.messages(&remote);
        msgs.find(|m| matches!(m, Message::Initialize { .. }))
            .expect("`initialize` is sent");
        msgs.find(|m| matches!(m, Message::InventoryAnnouncement { .. }))
            .expect("`inventory-announcement` is sent");
    }

    pub fn connect_to(&mut self, peer: &Self) {
        let remote = simulator::Peer::<S>::addr(peer);

        self.initialize();
        self.service.attempted(&remote);
        self.service
            .connected(remote, &self.local_addr, Link::Outbound);

        let mut msgs = self.messages(&remote);
        msgs.find(|m| matches!(m, Message::Initialize { .. }))
            .expect("`initialize` is sent");
        msgs.find(|m| matches!(m, Message::InventoryAnnouncement { .. }))
            .expect("`inventory-announcement` is sent");

        let git = peer.config().git_url.clone();
        self.receive(
            &remote,
            Message::init(
                peer.node_id(),
                self.local_time().as_secs(),
                peer.config().listen.clone(),
                git,
            ),
        );
    }

    /// Drain outgoing messages sent from this peer to the remote address.
    pub fn messages(&mut self, remote: &net::SocketAddr) -> impl Iterator<Item = Message> {
        let mut msgs = Vec::new();

        self.service.outbox().retain(|o| match o {
            service::Io::Write(a, envelopes) if a == remote => {
                msgs.extend(envelopes.iter().map(|e| e.msg.clone()));
                false
            }
            _ => true,
        });

        msgs.into_iter()
    }

    /// Get a draining iterator over the peer's emitted events.
    pub fn events(&mut self) -> impl Iterator<Item = Event> + '_ {
        self.outbox()
            .filter_map(|io| if let Io::Event(e) = io { Some(e) } else { None })
    }

    /// Get a draining iterator over the peer's I/O outbox.
    pub fn outbox(&mut self) -> impl Iterator<Item = Io> + '_ {
        self.service.outbox().drain(..)
    }
}
