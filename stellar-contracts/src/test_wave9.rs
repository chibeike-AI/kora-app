/// Wave 9 issue tests: #61 verify_evidence auth, #62 dispute-paused transfers,
/// #63 indexed pet disputes, #64 grooming interval_days validation.
use crate::*;
use soroban_sdk::{
    testutils::Address as _,
    Address, BytesN, Env, String,
};

// -------------------------------------------------------
// Shared helpers
// -------------------------------------------------------

fn register_pet_helper(client: &KoraContractClient, env: &Env, owner: &Address) -> u64 {
    client.register_pet(
        owner,
        &String::from_str(env, "Fluffy"),
        &String::from_str(env, "2020-01-01"),
        &Gender::Female,
        &Species::Cat,
        &String::from_str(env, "Persian"),
        &String::from_str(env, "Domestic Cat"),
        &4u32,
        &None,
        &PrivacyLevel::Public,
    )
}

fn make_hash(env: &Env, seed: u8) -> BytesN<32> {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    BytesN::from_array(env, &bytes)
}

// -------------------------------------------------------
// Issue #61 — verify_evidence auth and state persistence
// -------------------------------------------------------

/// Sets up a dispute with one piece of evidence in EvidencePhase, returns
/// (env, client, admin, arbitrator, dispute_id, evidence_id, hash).
fn setup_dispute_with_evidence() -> (
    Env,
    KoraContractClient<'static>,
    Address,
    Address,
    u64,
    u64,
    BytesN<32>,
) {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(KoraContract, ());
    let client = KoraContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.init_admin(&admin);

    let arbitrator = Address::generate(&env);
    client.assign_arbitrator(&admin, &arbitrator);

    let owner = Address::generate(&env);
    let target = Address::generate(&env);
    let pet_id = register_pet_helper(&client, &env, &owner);

    let dispute_id = client.raise_dispute(
        &pet_id,
        &owner,
        &target,
        &500u64,
        &String::from_str(&env, "Ownership dispute"),
        &String::from_str(&env, "ipfs://initial"),
    );

    // Transition to EvidencePhase so submit_evidence is allowed.
    client.resolve_dispute(&admin, &dispute_id, &DisputeStatus::EvidencePhase);

    let hash = make_hash(&env, 0xab);
    let evidence_id = client.submit_evidence(
        &dispute_id,
        &owner,
        &String::from_str(&env, "ipfs://evidence-cid"),
        &hash,
    );

    (env, client, admin, arbitrator, dispute_id, evidence_id, hash)
}

#[test]
fn test_verify_evidence_arbitrator_succeeds_and_persists() {
    let (env, client, _admin, arbitrator, dispute_id, evidence_id, hash) =
        setup_dispute_with_evidence();

    // Correct arbitrator + correct hash → true, and verification is persisted.
    let result = client.verify_evidence(&dispute_id, &evidence_id, &arbitrator, &hash);
    assert!(result, "correct hash should match");

    let verification = client
        .get_evidence_verification(&dispute_id, &evidence_id)
        .expect("verification record must be stored");
    assert_eq!(verification.verifier, arbitrator);
    // verified_at should be the ledger timestamp (0 in default env)
    assert_eq!(verification.verified_at, env.ledger().timestamp());
}

#[test]
fn test_verify_evidence_wrong_hash_returns_false_but_still_persists() {
    let (env, client, _admin, arbitrator, dispute_id, evidence_id, _hash) =
        setup_dispute_with_evidence();

    let wrong_hash = make_hash(&env, 0xff);
    let result = client.verify_evidence(&dispute_id, &evidence_id, &arbitrator, &wrong_hash);
    assert!(!result, "wrong hash should not match");

    // Even on mismatch the verification attempt is recorded.
    let verification = client.get_evidence_verification(&dispute_id, &evidence_id);
    assert!(verification.is_some(), "verification must be persisted even on mismatch");
}

#[test]
#[should_panic]
fn test_verify_evidence_non_arbitrator_is_rejected() {
    let (_env, client, _admin, _arbitrator, dispute_id, evidence_id, hash) =
        setup_dispute_with_evidence();

    let impostor = Address::generate(&_env);
    // Non-arbitrator must panic with Unauthorized.
    client.verify_evidence(&dispute_id, &evidence_id, &impostor, &hash);
}

#[test]
#[should_panic]
fn test_verify_evidence_missing_evidence_returns_not_found() {
    let (_env, client, _admin, arbitrator, dispute_id, _evidence_id, hash) =
        setup_dispute_with_evidence();

    // evidence_id 9999 does not exist.
    client.verify_evidence(&dispute_id, &9999u64, &arbitrator, &hash);
}

// -------------------------------------------------------
// Issue #62 — disputed PendingTransfer blocks expiry/reclaim
// (full tests live in pet-transfer-adoption/src/test.rs)
// -------------------------------------------------------
// The PendingTransfer dispute guard is tested in the pet-transfer-adoption
// crate's own test suite (see `test_disputed_pending_transfer_*` tests added
// to stellar-contracts/contracts/pet-transfer-adoption/src/test.rs).

// -------------------------------------------------------
// Issue #63 — indexed get_pet_disputes (verify O(1) lookups)
// -------------------------------------------------------

#[test]
fn test_get_pet_disputes_returns_only_pet_disputes() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(KoraContract, ());
    let client = KoraContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.init_admin(&admin);

    let owner1 = Address::generate(&env);
    let target1 = Address::generate(&env);
    let owner2 = Address::generate(&env);
    let target2 = Address::generate(&env);

    let pet1 = register_pet_helper(&client, &env, &owner1);
    let pet2 = register_pet_helper(&client, &env, &owner2);

    // Create disputes for pet1.
    client.raise_dispute(
        &pet1,
        &owner1,
        &target1,
        &100u64,
        &String::from_str(&env, "pet1 dispute 1"),
        &String::from_str(&env, "ipfs://h1"),
    );
    client.raise_dispute(
        &pet1,
        &owner1,
        &target1,
        &200u64,
        &String::from_str(&env, "pet1 dispute 2"),
        &String::from_str(&env, "ipfs://h2"),
    );

    // Create a dispute for pet2 (should not appear in pet1 results).
    client.raise_dispute(
        &pet2,
        &owner2,
        &target2,
        &300u64,
        &String::from_str(&env, "pet2 dispute"),
        &String::from_str(&env, "ipfs://h3"),
    );

    let pet1_disputes = client.get_pet_disputes(&pet1);
    assert_eq!(pet1_disputes.len(), 2, "pet1 must have exactly 2 disputes");

    // All returned disputes must belong to pet1.
    for d in pet1_disputes.iter() {
        assert_eq!(d.pet_id, pet1);
    }

    // pet2 must have exactly 1 dispute, not pet1's.
    let pet2_disputes = client.get_pet_disputes(&pet2);
    assert_eq!(pet2_disputes.len(), 1);
    assert_eq!(pet2_disputes.get(0).unwrap().pet_id, pet2);
}

#[test]
fn test_get_pet_disputes_returns_empty_for_unknown_pet() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(KoraContract, ());
    let client = KoraContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.init_admin(&admin);

    // Pet 9999 has never had a dispute raised.
    let disputes = client.get_pet_disputes(&9999u64);
    assert_eq!(disputes.len(), 0);
}

// -------------------------------------------------------
// Issue #64 — create_grooming_schedule interval_days validation
// -------------------------------------------------------

fn setup_grooming_pet(
    client: &KoraContractClient,
    env: &Env,
) -> (Address, u64) {
    let admin = Address::generate(env);
    client.init_admin(&admin);
    let owner = Address::generate(env);
    let pet_id = client.register_pet(
        &owner,
        &String::from_str(env, "Buddy"),
        &String::from_str(env, "2020-06-01"),
        &Gender::Male,
        &Species::Dog,
        &String::from_str(env, "Labrador"),
        &String::from_str(env, "Labrador Retriever"),
        &30u32,
        &None,
        &PrivacyLevel::Public,
    );
    (owner, pet_id)
}

#[test]
#[should_panic]
fn test_create_grooming_schedule_zero_interval_days_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(KoraContract, ());
    let client = KoraContractClient::new(&env, &contract_id);
    let (_owner, pet_id) = setup_grooming_pet(&client, &env);

    let start = 1_000_000u64;
    let end = start + 365 * 86_400;

    // interval_days == 0 must revert with InvalidInput.
    client.create_grooming_schedule(
        &pet_id,
        &GroomingFrequency::Weekly,
        &0u32,
        &start,
        &end,
        &String::from_str(&env, "Groomer"),
        &String::from_str(&env, "Bath"),
        &1000u64,
    );
}

#[test]
#[should_panic]
fn test_create_grooming_schedule_interval_days_over_365_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(KoraContract, ());
    let client = KoraContractClient::new(&env, &contract_id);
    let (_owner, pet_id) = setup_grooming_pet(&client, &env);

    let start = 1_000_000u64;
    let end = start + 400 * 86_400;

    // interval_days == 366 (> 365) must revert with InvalidInput.
    client.create_grooming_schedule(
        &pet_id,
        &GroomingFrequency::Monthly,
        &366u32,
        &start,
        &end,
        &String::from_str(&env, "Groomer"),
        &String::from_str(&env, "Bath"),
        &1000u64,
    );
}

#[test]
fn test_create_grooming_schedule_valid_interval_days_succeeds() {
    let env = Env::default();
    env.mock_all_auths();
    env.budget().reset_unlimited();
    let contract_id = env.register(KoraContract, ());
    let client = KoraContractClient::new(&env, &contract_id);
    let (_owner, pet_id) = setup_grooming_pet(&client, &env);

    let start = 1_000_000u64;
    let interval_secs = 14u64 * 86_400; // 14 days
    let end = start + interval_secs * 10;

    // interval_days == 14 (valid: 1..=365) must succeed.
    let schedule_id = client.create_grooming_schedule(
        &pet_id,
        &GroomingFrequency::Biweekly,
        &14u32,
        &start,
        &end,
        &String::from_str(&env, "Groomer"),
        &String::from_str(&env, "Bath"),
        &1000u64,
    );
    assert!(schedule_id > 0, "schedule must be created with valid interval");

    // Verify the first 4 grooming slots were generated via the count.
    let count = client.get_grooming_count(&pet_id);
    assert_eq!(count, 4, "4 initial slots must be generated");
}

#[test]
fn test_create_grooming_schedule_boundary_interval_1_day() {
    let env = Env::default();
    env.mock_all_auths();
    env.budget().reset_unlimited();
    let contract_id = env.register(KoraContract, ());
    let client = KoraContractClient::new(&env, &contract_id);
    let (_owner, pet_id) = setup_grooming_pet(&client, &env);

    let start = 1_000_000u64;
    let end = start + 365 * 86_400;

    // interval_days == 1 (minimum valid value) must succeed.
    let schedule_id = client.create_grooming_schedule(
        &pet_id,
        &GroomingFrequency::Weekly,
        &1u32,
        &start,
        &end,
        &String::from_str(&env, "Groomer"),
        &String::from_str(&env, "Bath"),
        &500u64,
    );
    assert!(schedule_id > 0);
}

#[test]
fn test_create_grooming_schedule_boundary_interval_365_days() {
    let env = Env::default();
    env.mock_all_auths();
    env.budget().reset_unlimited();
    let contract_id = env.register(KoraContract, ());
    let client = KoraContractClient::new(&env, &contract_id);
    let (_owner, pet_id) = setup_grooming_pet(&client, &env);

    let start = 1_000_000u64;
    let end = start + 365 * 86_400 * 5;

    // interval_days == 365 (maximum valid value) must succeed.
    let schedule_id = client.create_grooming_schedule(
        &pet_id,
        &GroomingFrequency::Monthly,
        &365u32,
        &start,
        &end,
        &String::from_str(&env, "Groomer"),
        &String::from_str(&env, "Bath"),
        &500u64,
    );
    assert!(schedule_id > 0);
}
