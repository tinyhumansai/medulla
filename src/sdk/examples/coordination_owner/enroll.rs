//! Enrollment: mint one orchestrator/host pair and write both `node.json`
//! files (protocol §7).
//!
//! This is the harness's stand-in for the backend's invite/enroll endpoints
//! (§7.2) *and* for the human who carries the pair key from the orchestrator to
//! the host (§7.1) — the one step of enrollment a script cannot perform. The
//! backend's half of the material (the two forwarder keys) is printed for the
//! mock forwarder's node table; the pair key is not printed anywhere.

use std::path::Path;

use medulla_link::keys::{self, ForwarderKey, NodeId, NodeState, PairKey, Role};
use sha2::{Digest, Sha256};

use crate::{decode_hex, encode_hex, Args};

/// Mint an orchestrator/host pair and write both `node.json` files (§7).
///
/// The pair key is generated here, on the orchestrator, exactly as §7.1 says —
/// and written straight into the host's identity file rather than typed in by a
/// human, which is the one step of enrollment a harness cannot perform.
pub fn enroll(args: &Args, owner_dir: &Path) -> Result<(), String> {
    let host_dir = args
        .host_state_dir
        .clone()
        .ok_or("--enroll needs --host-state-dir")?;
    let forwarder = args
        .forwarder
        .clone()
        .ok_or("--enroll needs --forwarder <host:port>")?;

    let material = match args.seed.as_deref() {
        Some(seed) => Material::from_seed(seed)?,
        None => Material::random(),
    };

    write_identity(
        owner_dir,
        &material,
        Role::Orchestrator,
        forwarder.clone(),
        material.owner_id,
        material.host_id,
        &material.owner_key,
    )?;
    write_identity(
        &host_dir,
        &material,
        Role::Host,
        forwarder,
        material.host_id,
        material.owner_id,
        &material.host_key,
    )?;

    // The forwarder keys are the backend's half of enrollment, so they are what
    // the mock forwarder needs. The pair key stays between the two endpoints.
    println!("OWNER_NODE_ID={}", material.owner_id);
    println!(
        "OWNER_FORWARDER_KEY={}",
        encode_hex(material.owner_key.as_bytes())
    );
    println!("HOST_NODE_ID={}", material.host_id);
    println!(
        "HOST_FORWARDER_KEY={}",
        encode_hex(material.host_key.as_bytes())
    );
    Ok(())
}

/// The key material one enrollment mints.
struct Material {
    owner_id: NodeId,
    host_id: NodeId,
    owner_key: ForwarderKey,
    host_key: ForwarderKey,
    pair_key: PairKey,
}

impl Material {
    /// Fresh random material — the normal case.
    fn random() -> Self {
        Material {
            owner_id: NodeId::generate(),
            host_id: NodeId::generate(),
            owner_key: ForwarderKey::generate(),
            host_key: ForwarderKey::generate(),
            pair_key: PairKey::generate(),
        }
    }

    /// Material derived from a 64-character hex seed, so a run is reproducible.
    ///
    /// Each field is a separate SHA-256 over the seed and a label, so learning
    /// one tells an attacker nothing about the others — a test fixture is still
    /// key material.
    fn from_seed(seed: &str) -> Result<Self, String> {
        let seed = decode_hex(seed.trim(), 32)?;
        let derive = |label: &str, len: usize| -> Vec<u8> {
            let mut hasher = Sha256::new();
            hasher.update(label.as_bytes());
            hasher.update(&seed);
            hasher.finalize()[..len].to_vec()
        };
        let id =
            |label: &str| -> NodeId { NodeId(derive(label, 16).try_into().expect("16 bytes")) };
        let key = |label: &str| -> ForwarderKey {
            ForwarderKey(derive(label, 32).try_into().expect("32 bytes"))
        };
        Ok(Material {
            owner_id: id("medulla-e2e owner-node"),
            host_id: id("medulla-e2e host-node"),
            owner_key: key("medulla-e2e owner-forwarder"),
            host_key: key("medulla-e2e host-forwarder"),
            pair_key: PairKey::from_bytes(derive("medulla-e2e pair", 16).try_into().expect("16")),
        })
    }
}

/// Write one endpoint's `node.json`, creating the directory if needed.
fn write_identity(
    dir: &Path,
    material: &Material,
    role: Role,
    forwarder_endpoint: String,
    node_id: NodeId,
    peer_node_id: NodeId,
    forwarder_key: &ForwarderKey,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    keys::acquire_or_create(dir, || NodeState {
        version: 1,
        node_id,
        role,
        pair_key: material.pair_key.clone(),
        forwarder_key: forwarder_key.clone(),
        forwarder_endpoint,
        peer_node_id,
        peers: Vec::new(),
        seq_reservation: 1,
    })
    .map_err(|e| format!("could not enroll {}: {e}", dir.display()))?;
    Ok(())
}
