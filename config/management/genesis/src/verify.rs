// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use aptos_config::config::{RocksdbConfigs, NO_OP_STORAGE_PRUNER_CONFIG};
use aptos_global_constants::{
    CONSENSUS_KEY, FULLNODE_NETWORK_KEY, OPERATOR_ACCOUNT, OPERATOR_KEY, OWNER_ACCOUNT, OWNER_KEY,
    SAFETY_DATA, VALIDATOR_NETWORK_KEY, WAYPOINT,
};
use aptos_management::{
    config::ConfigPath, error::Error, secure_backend::ValidatorBackend,
    storage::StorageWrapper as Storage,
};
use aptos_state_view::account_with_state_view::AsAccountWithStateView;
use aptos_temppath::TempPath;
use aptos_types::{
    account_address::AccountAddress, account_config, account_view::AccountView,
    network_address::NetworkAddress, on_chain_config::ValidatorSet,
    validator_config::ValidatorConfig, waypoint::Waypoint,
};
use aptos_vm::AptosVM;
use aptosdb::AptosDB;
use executor::db_bootstrapper;
use std::{
    fmt::Write,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};
use storage_interface::{state_view::LatestDbStateCheckpointView, DbReader, DbReaderWriter};
use structopt::StructOpt;

/// Prints the public information within a store
#[derive(Debug, StructOpt)]
pub struct Verify {
    #[structopt(flatten)]
    config: ConfigPath,
    #[structopt(flatten)]
    backend: ValidatorBackend,
    /// If specified, compares the internal state to that of a
    /// provided genesis. Note, that a waypont might diverge from
    /// the provided genesis after execution has begun.
    #[structopt(long, verbatim_doc_comment)]
    genesis_path: Option<PathBuf>,
}

impl Verify {
    pub fn execute(self) -> Result<String, Error> {
        let config = self
            .config
            .load()?
            .override_validator_backend(&self.backend.validator_backend)?;
        let validator_storage = config.validator_backend();

        verify_genesis(validator_storage, self.genesis_path.as_deref())
    }
}

pub fn verify_genesis(
    validator_storage: Storage,
    genesis_path: Option<&Path>,
) -> Result<String, Error> {
    let mut buffer = String::new();

    writeln!(buffer, "Data stored in SecureStorage:").unwrap();
    write_break(&mut buffer);
    writeln!(buffer, "Keys").unwrap();
    write_break(&mut buffer);

    write_bls12381_key(&validator_storage, &mut buffer, CONSENSUS_KEY);
    write_x25519_key(&validator_storage, &mut buffer, FULLNODE_NETWORK_KEY);
    write_ed25519_key(&validator_storage, &mut buffer, OWNER_KEY);
    write_ed25519_key(&validator_storage, &mut buffer, OPERATOR_KEY);
    write_ed25519_key(&validator_storage, &mut buffer, VALIDATOR_NETWORK_KEY);

    write_break(&mut buffer);
    writeln!(buffer, "Data").unwrap();
    write_break(&mut buffer);

    write_string(&validator_storage, &mut buffer, OPERATOR_ACCOUNT);
    write_string(&validator_storage, &mut buffer, OWNER_ACCOUNT);
    write_safety_data(&validator_storage, &mut buffer, SAFETY_DATA);
    write_waypoint(&validator_storage, &mut buffer, WAYPOINT);

    write_break(&mut buffer);

    if let Some(genesis_path) = genesis_path {
        compare_genesis(validator_storage, &mut buffer, genesis_path)?;
    }

    Ok(buffer)
}

fn write_assert(buffer: &mut String, name: &str, value: bool) {
    let value = if value { "match" } else { "MISMATCH" };
    writeln!(buffer, "{} - {}", name, value).unwrap();
}

fn write_break(buffer: &mut String) {
    writeln!(
        buffer,
        "====================================================================================",
    )
    .unwrap();
}

fn write_bls12381_key(storage: &Storage, buffer: &mut String, key: &'static str) {
    let value = storage
        .bls12381_public_from_private(key)
        .map(|v| v.to_string())
        .unwrap_or_else(|e| e.to_string());
    writeln!(buffer, "{} - {}", key, value).unwrap();
}

fn write_ed25519_key(storage: &Storage, buffer: &mut String, key: &'static str) {
    let value = storage
        .ed25519_public_from_private(key)
        .map(|v| v.to_string())
        .unwrap_or_else(|e| e.to_string());
    writeln!(buffer, "{} - {}", key, value).unwrap();
}

fn write_x25519_key(storage: &Storage, buffer: &mut String, key: &'static str) {
    let value = storage
        .x25519_public_from_private(key)
        .map(|v| v.to_string())
        .unwrap_or_else(|e| e.to_string());
    writeln!(buffer, "{} - {}", key, value).unwrap();
}

fn write_string(storage: &Storage, buffer: &mut String, key: &'static str) {
    let value = storage.string(key).unwrap_or_else(|e| e.to_string());
    writeln!(buffer, "{} - {}", key, value).unwrap();
}

fn write_safety_data(storage: &Storage, buffer: &mut String, key: &'static str) {
    let value = storage
        .value::<consensus_types::safety_data::SafetyData>(key)
        .map(|v| v.to_string())
        .unwrap_or_else(|e| e.to_string());
    writeln!(buffer, "{} - {}", key, value).unwrap();
}

fn write_waypoint(storage: &Storage, buffer: &mut String, key: &'static str) {
    let value = storage
        .string(key)
        .map(|value| {
            if value.is_empty() {
                "empty".into()
            } else {
                Waypoint::from_str(&value)
                    .map(|c| c.to_string())
                    .unwrap_or_else(|_| "Invalid waypoint".into())
            }
        })
        .unwrap_or_else(|e| e.to_string());

    writeln!(buffer, "{} - {}", key, value).unwrap();
}

fn compare_genesis(
    storage: Storage,
    buffer: &mut String,
    genesis_path: &Path,
) -> Result<(), Error> {
    // Compute genesis and waypoint and compare to given waypoint
    let db_path = TempPath::new();
    let (db_rw, expected_waypoint) = compute_genesis(genesis_path, db_path.path())?;

    let actual_waypoint = storage.waypoint(WAYPOINT)?;
    write_assert(buffer, WAYPOINT, actual_waypoint == expected_waypoint);

    // Fetch on-chain validator config and compare on-chain keys to local keys
    let validator_account = storage.account_address(OWNER_ACCOUNT)?;
    let validator_config = validator_config(validator_account, db_rw.reader.clone())?;

    let actual_consensus_key = storage.bls12381_public_from_private(CONSENSUS_KEY)?;
    let expected_consensus_key = &validator_config.consensus_public_key;
    write_assert(
        buffer,
        CONSENSUS_KEY,
        &actual_consensus_key == expected_consensus_key,
    );

    let actual_validator_key = storage.x25519_public_from_private(VALIDATOR_NETWORK_KEY)?;
    let actual_fullnode_key = storage.x25519_public_from_private(FULLNODE_NETWORK_KEY)?;

    let network_addrs: Vec<NetworkAddress> = validator_config
        .validator_network_addresses()
        .unwrap_or_default();

    let expected_validator_key = network_addrs
        .get(0)
        .and_then(|addr: &NetworkAddress| addr.find_noise_proto());
    write_assert(
        buffer,
        VALIDATOR_NETWORK_KEY,
        Some(actual_validator_key) == expected_validator_key,
    );

    let expected_fullnode_key = validator_config.fullnode_network_addresses().ok().and_then(
        |addrs: Vec<NetworkAddress>| addrs.get(0).and_then(|addr| addr.find_noise_proto()),
    );
    write_assert(
        buffer,
        FULLNODE_NETWORK_KEY,
        Some(actual_fullnode_key) == expected_fullnode_key,
    );

    Ok(())
}

/// Compute the ledger given a genesis writeset transaction and return access to that ledger and
/// the waypoint for that state.
fn compute_genesis(
    genesis_path: &Path,
    db_path: &Path,
) -> Result<(DbReaderWriter, Waypoint), Error> {
    let aptosdb = AptosDB::open(
        db_path,
        false,
        NO_OP_STORAGE_PRUNER_CONFIG,
        RocksdbConfigs::default(),
    )
    .map_err(|e| Error::UnexpectedError(e.to_string()))?;
    let db_rw = DbReaderWriter::new(aptosdb);

    let mut file = File::open(genesis_path)
        .map_err(|e| Error::UnexpectedError(format!("Unable to open genesis file: {}", e)))?;
    let mut buffer = vec![];
    file.read_to_end(&mut buffer)
        .map_err(|e| Error::UnexpectedError(format!("Unable to read genesis: {}", e)))?;
    let genesis = bcs::from_bytes(&buffer)
        .map_err(|e| Error::UnexpectedError(format!("Unable to parse genesis: {}", e)))?;

    let waypoint = db_bootstrapper::generate_waypoint::<AptosVM>(&db_rw, &genesis)
        .map_err(|e| Error::UnexpectedError(e.to_string()))?;
    db_bootstrapper::maybe_bootstrap::<AptosVM>(&db_rw, &genesis, waypoint)
        .map_err(|e| Error::UnexpectedError(format!("Unable to commit genesis: {}", e)))?;

    Ok((db_rw, waypoint))
}

/// Read from the ledger the validator config from the validator set for the specified account
fn validator_config(
    validator_account: AccountAddress,
    reader: Arc<dyn DbReader>,
) -> Result<ValidatorConfig, Error> {
    let db_state_view = reader
        .latest_state_checkpoint_view()
        .map_err(|e| Error::UnexpectedError(format!("Can't create latest db state view {}", e)))?;
    let address = account_config::validator_set_address();
    let account_state_view = db_state_view.as_account_with_state_view(&address);

    let validator_set: ValidatorSet = account_state_view
        .get_validator_set()
        .map_err(|e| Error::UnexpectedError(format!("ValidatorSet issue {}", e)))?
        .ok_or_else(|| Error::UnexpectedError("ValidatorSet does not exist".into()))?;
    let info = validator_set
        .payload()
        .find(|vi| vi.account_address() == &validator_account)
        .ok_or_else(|| {
            Error::UnexpectedError(format!(
                "Unable to find Validator account {:?}",
                &validator_account
            ))
        })?;
    Ok(info.config().clone())
}