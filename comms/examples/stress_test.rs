use bytes::{Buf, Bytes, BytesMut};
use futures::{
    channel::{mpsc, mpsc::SendError},
    stream::Fuse,
    SinkExt,
    StreamExt,
};
use log::*;
use rand::{rngs::OsRng, RngCore};
use std::{
    env,
    io,
    path::Path,
    process,
    sync::Arc,
    time::{Duration, Instant},
};
use tari_comms::{
    connectivity::{ConnectivityError, ConnectivityEvent, ConnectivityRequester},
    framing,
    framing::CanonicalFraming,
    multiaddr::Multiaddr,
    peer_manager::{NodeId, NodeIdentity, NodeIdentityError, Peer, PeerFeatures, PeerManagerError},
    protocol::{ProtocolEvent, ProtocolNotification, Protocols},
    tor,
    tor::TorIdentity,
    types::CommsPublicKey,
    CommsBuilder,
    CommsBuilderError,
    CommsNode,
    Substream,
};
use tari_crypto::tari_utilities::{
    hex::Hex,
    message_format::{MessageFormat, MessageFormatError},
};
use tari_storage::{lmdb_store::LMDBBuilder, LMDBWrapper};
use tempfile::Builder;
use thiserror::Error;
use tokio::{task, task::JoinHandle, time};

const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;
const TOR_CONTROL_PORT_ADDR: &str = "/ip4/127.0.0.1/tcp/9051";
const NUM_MESSAGES: usize = 1_000_000;
const MSG_SIZE: usize = 256;

static STRESS_PROTOCOL: Bytes = Bytes::from_static(b"stress-test/1.0");

// Tor example for tari_comms.
//
// _Note:_ A running tor proxy with `ControlPort` set is required for this example to work.

#[derive(Debug, Error)]
enum Error {
    #[error("NodeIdentityError: {0}")]
    NodeIdentityError(#[from] NodeIdentityError),
    #[error("HiddenServiceBuilderError: {0}")]
    HiddenServiceBuilderError(#[from] tor::HiddenServiceBuilderError),
    #[error("CommsBuilderError: {0}")]
    CommsBuilderError(#[from] CommsBuilderError),
    #[error("PeerManagerError: {0}")]
    PeerManagerError(#[from] PeerManagerError),
    #[error("Connectivity error: {0}")]
    ConnectivityError(#[from] ConnectivityError),
    #[error("Message format error: {0}")]
    MessageFormatError(#[from] MessageFormatError),
    #[error("Failed to send message")]
    SendError(#[from] SendError),
    #[error("JoinError: {0}")]
    JoinError(#[from] task::JoinError),
    #[error("Example did not exit cleanly")]
    WaitTimeout(#[from] time::Elapsed),
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
}

#[tokio_macros::main]
async fn main() {
    env_logger::init();
    if let Err(err) = run().await {
        println!("{error:?}: {error}", error = err);
        process::exit(1);
    }
}

fn save_tor_identity<P: AsRef<Path>>(identity: &TorIdentity, path: P) -> Result<(), Error> {
    let json = identity.to_json()?;
    std::fs::write(path, json).map_err(Into::into)
}

fn load_tor_identity<P: AsRef<Path>>(path: P) -> Option<tor::TorIdentity> {
    if path.as_ref().exists() {
        let contents = std::fs::read_to_string(path).unwrap();
        Some(tor::TorIdentity::from_json(&contents).unwrap())
    } else {
        None
    }
}

fn remove_arg(args: &mut Vec<String>, item: &str) -> Option<usize> {
    if let Some(pos) = args.iter().position(|s| s == item) {
        args.remove(pos);
        Some(pos)
    } else {
        None
    }
}

async fn run() -> Result<(), Error> {
    let args_iter = env::args().skip(1);
    let mut args = args_iter.collect::<Vec<_>>();

    let is_tcp = remove_arg(&mut args, "--tcp").is_some();

    let mut tor_identity_path = None;
    if let Some(pos) = remove_arg(&mut args, "--tor-identity") {
        tor_identity_path = Some(args.remove(pos));
    }

    let mut peer = None;
    if let Some(pos) = remove_arg(&mut args, "--peer") {
        peer = Some(args.remove(pos));
    }

    let mut port = 9098;
    if let Some(pos) = remove_arg(&mut args, "--port") {
        port = args.remove(pos).parse().expect("Unable to parse port");
    }

    println!("Initializing...",);

    let tor_identity = tor_identity_path.as_ref().and_then(load_tor_identity);

    let temp_dir = Builder::new().prefix("stress-test").tempdir().unwrap();
    let (comms_node, protocol_notif) = setup_node(temp_dir.as_ref(), port, tor_identity, is_tcp).await?;
    if let Some(tor_identity_path) = tor_identity_path.as_ref() {
        save_tor_identity(comms_node.hidden_service().unwrap().tor_identity(), tor_identity_path)?;
    }

    if let Some(mut peer) = peer.as_ref().map(|s| s.splitn(2, "::").collect::<Vec<_>>()) {
        let pk = CommsPublicKey::from_hex(peer.remove(0)).unwrap();
        let node_id = NodeId::from_key(&pk).unwrap();
        let address = peer.remove(0).parse::<Multiaddr>().unwrap();
        comms_node
            .peer_manager()
            .add_peer(Peer::new(
                pk,
                node_id.clone(),
                vec![address.clone()].into(),
                Default::default(),
                Default::default(),
                &[],
            ))
            .await?;
        println!("Connecting to {}", address,);

        let start = Instant::now();
        comms_node.connectivity().dial_peer(node_id).await?;
        println!("Connected in {:.2?}", start.elapsed());
    }

    println!("Stress test server started!");
    let handle = start_service(&comms_node, protocol_notif);
    tokio::signal::ctrl_c().await.expect("ctrl-c failed");

    println!("Stress test service is shutting down...");
    comms_node.shutdown().await;

    println!("Done. Waiting for service to exit");
    time::timeout(Duration::from_secs(5), handle).await???;

    Ok(())
}

fn start_service(
    comms_node: &CommsNode,
    protocol_notif: mpsc::Receiver<ProtocolNotification<Substream>>,
) -> JoinHandle<Result<(), Error>>
{
    let node_identity = comms_node.node_identity();

    println!(
        "Node credentials are '{}::{}' (local_listening_addr='{}')",
        node_identity.public_key().to_hex(),
        node_identity.public_address(),
        comms_node.listening_address(),
    );

    let connectivity = comms_node.connectivity();

    let handle = task::spawn(async move {
        let mut server = StressTestService::new(connectivity, protocol_notif);
        server.start().await
    });

    handle
}

async fn setup_node(
    database_path: &Path,
    port: u16,
    tor_identity: Option<TorIdentity>,
    is_tcp: bool,
) -> Result<(CommsNode, mpsc::Receiver<ProtocolNotification<Substream>>), Error>
{
    let datastore = LMDBBuilder::new()
        .set_path(database_path.to_str().unwrap())
        .set_environment_size(50)
        .set_max_number_of_databases(1)
        .add_database("peerdb", lmdb_zero::db::CREATE)
        .build()
        .unwrap();
    let peer_database = datastore.get_handle(&"peerdb").unwrap();
    let peer_database = LMDBWrapper::new(Arc::new(peer_database));

    let mut protocols = Protocols::new();
    let (proto_notif_tx, proto_notif_rx) = mpsc::channel(1);
    protocols.add(&[STRESS_PROTOCOL.clone()], proto_notif_tx);

    let node_identity = Arc::new(NodeIdentity::random(
        &mut OsRng,
        format!("/ip4/127.0.0.1/tcp/{}", port).parse().unwrap(),
        PeerFeatures::COMMUNICATION_CLIENT,
    )?);

    let public_address = node_identity.public_address();

    let builder = CommsBuilder::new()
        .with_protocols(protocols)
        .with_node_identity(node_identity)
        .with_peer_storage(peer_database);

    let comms_node = if is_tcp {
        builder
            .allow_test_addresses()
            .with_listener_address(public_address)
            .build()?
            .spawn()
            .await?
    } else {
        let mut hs_builder = tor::HiddenServiceBuilder::new()
            .with_port_mapping(port)
            .with_control_server_address(TOR_CONTROL_PORT_ADDR.parse().unwrap());

        if let Some(tor_identity) = tor_identity {
            hs_builder = hs_builder.with_tor_identity(tor_identity);
        }

        let tor_hidden_service = hs_builder.finish().await?;

        println!(
            "Tor hidden service created with address '{}'",
            tor_hidden_service.get_onion_address()
        );

        builder
            .configure_from_hidden_service(tor_hidden_service)
            .build()?
            .spawn()
            .await?
    };

    Ok((comms_node, proto_notif_rx))
}

pub struct StressTestService {
    connectivity: ConnectivityRequester,
    protocol_notif: Fuse<mpsc::Receiver<ProtocolNotification<Substream>>>,
}

impl StressTestService {
    pub fn new(
        connectivity: ConnectivityRequester,
        protocol_notif: mpsc::Receiver<ProtocolNotification<Substream>>,
    ) -> Self
    {
        Self {
            connectivity,
            protocol_notif: protocol_notif.fuse(),
        }
    }

    async fn start(&mut self) -> Result<(), Error> {
        let mut events = self.connectivity.subscribe_event_stream().fuse();

        loop {
            futures::select! {
                event = events.select_next_some() => {
                    if let Ok(event) = event {
                        self.handle_connectivity_event(&*event).await;
                    }
                },
                notif = self.protocol_notif.select_next_some() => {
                    self.handle_protocol_notification(notif).await;
                },
                complete => {
                    info!("StressTestService is shutting down");
                    break;
                }

            }
        }

        Ok(())
    }

    async fn handle_connectivity_event(&mut self, event: &ConnectivityEvent) {
        use ConnectivityEvent::*;
        match event {
            PeerConnected(conn) => {
                println!(
                    "Peer `{}` connected. Starting stress protocol",
                    conn.peer_node_id().short_str()
                );

                if conn.direction().is_inbound() {
                    let framed = conn
                        .clone()
                        .open_framed_substream(&STRESS_PROTOCOL, MAX_FRAME_SIZE)
                        .await
                        .unwrap();
                    task::spawn(start_initiator_protocol(framed));
                }
            },
            _ => {},
        }
    }

    async fn handle_protocol_notification(&mut self, notification: ProtocolNotification<Substream>) {
        match notification.event {
            ProtocolEvent::NewInboundSubstream(node_id, substream) => {
                println!(
                    "Peer `{}` initiated `{}` protocol",
                    node_id,
                    String::from_utf8_lossy(&notification.protocol)
                );
                let framed = framing::canonical(substream, MAX_FRAME_SIZE);
                task::spawn(start_responder_protocol(framed));
            },
        }
    }
}

async fn start_initiator_protocol(mut framed: CanonicalFraming<Substream>) -> Result<(), Error> {
    let mut counter = 0u32;

    println!(
        "Sending {} messages ({} MiB)",
        NUM_MESSAGES,
        NUM_MESSAGES * MSG_SIZE / 1024 / 1024
    );
    let start = Instant::now();
    for _ in 0..NUM_MESSAGES {
        framed.send(generate_message(counter)).await?;
        counter += 1;
    }
    framed.close().await?;
    println!("Done in {:.2?}. Closing substream.", start.elapsed());

    Ok(())
}

async fn start_responder_protocol(mut framed: CanonicalFraming<Substream>) -> Result<(), Error> {
    let mut received = vec![];
    let start = Instant::now();

    while let Some(Ok(mut msg)) = framed.next().await {
        let mut buf = [0u8; 4];
        msg.copy_to_slice(&mut buf);
        let msg_num = u32::from_be_bytes(buf);
        received.push(msg_num);
    }

    println!("Received {} messages in {:.2?}", received.len(), start.elapsed());

    println!("Checking that all messages are accounted for");
    let dropped = (0..NUM_MESSAGES)
        .filter(|i| !received.contains(&(*i as u32)))
        .collect::<Vec<_>>();
    if dropped.is_empty() {
        println!("All messages received");
    } else {
        println!(
            "{} messages arrived! But {} were dropped",
            NUM_MESSAGES - dropped.len(),
            dropped.len()
        );
        println!("{:?}", dropped);
    }

    Ok(())
}

fn generate_message(n: u32) -> Bytes {
    let counter_bytes = n.to_be_bytes();
    let mut bytes = BytesMut::with_capacity(MSG_SIZE);
    bytes.resize(MSG_SIZE, 0);
    bytes[..4].copy_from_slice(&counter_bytes);
    OsRng.fill_bytes(&mut bytes[8..MSG_SIZE]);
    bytes.freeze()
}
