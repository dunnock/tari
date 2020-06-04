use rand::rngs::OsRng;
use std::{convert::identity, path::Path, process, sync::Arc};
use tari_comms::{peer_manager::PeerFeatures, pipeline, pipeline::SinkService, CommsBuilder, CommsNode, NodeIdentity};
use tari_storage::{lmdb_store::LMDBBuilder, LMDBWrapper};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("TODO")]
    Todo,
}

#[tokio_macros::main]
async fn main() {
    env_logger::init();
    if let Err(err) = run().await {
        eprintln!("{error:?}: {error}", error = err);
        process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    Ok(())
}

// async fn setup_node<P: AsRef<Path>>(database_path: P) -> Result<CommsNode, Error> {
//     let datastore = LMDBBuilder::new()
//         .set_path(database_path.as_ref().to_str().unwrap())
//         .set_environment_size(50)
//         .set_max_number_of_databases(1)
//         .add_database("peerdb", lmdb_zero::db::CREATE)
//         .build()
//         .unwrap();
//     let peer_database = datastore.get_handle(&"peerdb").unwrap();
//     let peer_database = LMDBWrapper::new(Arc::new(peer_database));
//
//     let node_identity = Arc::new(NodeIdentity::random(
//         &mut OsRng,
//         "/ip4/127.0.0.1/tcp/0".parse().unwrap(),
//         PeerFeatures::COMMUNICATION_CLIENT,
//     )?);
//
//     let comms_node = CommsBuilder::new()
//         .with_node_identity(node_identity)
//         .with_peer_storage(peer_database)
//         .build()
//         .unwrap()
//         .spawn()
//         .await?;
//
//     Ok(comms_node)
// }
