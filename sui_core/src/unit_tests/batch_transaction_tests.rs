// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use bcs;

use authority_tests::{
    get_genesis_package_by_module, init_state_with_ids, send_and_confirm_transaction,
};
use move_binary_format::file_format;
use move_core_types::{account_address::AccountAddress, ident_str};
use sui_adapter::genesis;
use sui_types::{
    crypto::{get_key_pair, Signature},
    messages::Transaction,
};

#[tokio::test]
async fn test_batch_transaction_ok() -> anyhow::Result<()> {
    // This test tests a sucecssful normal batch transaction.
    // This batch transaction contains 100 transfers, and 100 Move calls.
    let (sender, sender_key) = get_key_pair();
    let (recipient, _) = get_key_pair();
    const N: usize = 100;
    const TOTAL: usize = N + 1;
    let all_ids = (0..TOTAL).map(|_| ObjectID::random()).collect::<Vec<_>>();
    let authority_state =
        init_state_with_ids([sender; TOTAL].into_iter().zip(all_ids.clone().into_iter())).await;
    let mut transactions = vec![];
    for obj_id in all_ids.iter().take(N) {
        transactions.push(SingleTransactionKind::Transfer(Transfer {
            recipient,
            object_ref: authority_state
                .get_object(obj_id)
                .await?
                .unwrap()
                .compute_object_reference(),
        }));
    }
    let genesis_package_objects = genesis::clone_genesis_packages();
    let package_object_ref =
        get_genesis_package_by_module(&genesis_package_objects, "ObjectBasics");
    for _ in 0..N {
        transactions.push(SingleTransactionKind::Call(MoveCall {
            package: package_object_ref,
            module: ident_str!("ObjectBasics").to_owned(),
            function: ident_str!("create").to_owned(),
            type_arguments: vec![],
            object_arguments: vec![],
            shared_object_arguments: vec![],
            pure_arguments: vec![
                16u64.to_le_bytes().to_vec(),
                bcs::to_bytes(&AccountAddress::from(sender)).unwrap(),
            ],
            gas_budget: 500,
        }));
    }
    let data = TransactionData::new(
        TransactionKind::Batch(transactions),
        sender,
        authority_state
            .get_object(&all_ids[N])
            .await?
            .unwrap()
            .compute_object_reference(),
    );
    let signature = Signature::new(&data, &sender_key);
    let tx = Transaction::new(data, signature);
    let response = send_and_confirm_transaction(&authority_state, tx).await?;
    let effects = response.signed_effects.unwrap().effects;
    assert!(effects.status.is_ok());
    assert_eq!((effects.created.len(), effects.mutated.len()), (N, N + 1),);
    assert!(effects
        .created
        .iter()
        .all(|(_, owner)| owner == &Owner::AddressOwner(sender)));
    // N of the objects should now be owned by recipient.
    assert_eq!(
        effects
            .mutated
            .iter()
            .filter(|(_, owner)| owner == &Owner::AddressOwner(recipient))
            .count(),
        N
    );

    Ok(())
}

#[tokio::test]
async fn test_batch_transaction_last_one_fail() -> anyhow::Result<()> {
    // This test tests the case where the last transaction in a batch transaction would fail to execute.
    // We make sure that the entire batch is rolled back, and only gas is charged.
    let (sender, sender_key) = get_key_pair();
    let (recipient, _) = get_key_pair();
    const N: usize = 100;
    const TOTAL: usize = N + 1;
    let all_ids = (0..TOTAL).map(|_| ObjectID::random()).collect::<Vec<_>>();
    let authority_state =
        init_state_with_ids([sender; TOTAL].into_iter().zip(all_ids.clone().into_iter())).await;
    let mut transactions = vec![];
    for obj_id in all_ids.iter().take(N) {
        transactions.push(SingleTransactionKind::Transfer(Transfer {
            recipient,
            object_ref: authority_state
                .get_object(obj_id)
                .await?
                .unwrap()
                .compute_object_reference(),
        }));
    }
    let genesis_package_objects = genesis::clone_genesis_packages();
    let package_object_ref =
        get_genesis_package_by_module(&genesis_package_objects, "ObjectBasics");
    transactions.push(SingleTransactionKind::Call(MoveCall {
        package: package_object_ref,
        module: ident_str!("ObjectBasics").to_owned(),
        function: ident_str!("create").to_owned(),
        type_arguments: vec![],
        object_arguments: vec![],
        shared_object_arguments: vec![],
        pure_arguments: vec![],
        gas_budget: 500,
    }));
    let data = TransactionData::new(
        TransactionKind::Batch(transactions),
        sender,
        authority_state
            .get_object(&all_ids[N])
            .await?
            .unwrap()
            .compute_object_reference(),
    );
    let signature = Signature::new(&data, &sender_key);
    let tx = Transaction::new(data, signature);
    let response = send_and_confirm_transaction(&authority_state, tx).await?;
    let effects = response.signed_effects.unwrap().effects;
    assert!(effects.status.is_err());
    assert_eq!((effects.created.len(), effects.mutated.len()), (0, N + 1));

    Ok(())
}

#[tokio::test]
async fn test_batch_contains_publish() -> anyhow::Result<()> {
    // Test that a batch transaction containing publish will fail.
    let (sender, sender_key) = get_key_pair();
    let gas_object_id = ObjectID::random();
    let authority_state = init_state_with_ids([(sender, gas_object_id)]).await;
    let module = file_format::empty_module();
    let mut module_bytes = Vec::new();
    module.serialize(&mut module_bytes).unwrap();
    let module_bytes = vec![module_bytes];
    let transactions = vec![SingleTransactionKind::Publish(MoveModulePublish {
        modules: module_bytes,
        gas_budget: 10000,
    })];
    let data = TransactionData::new(
        TransactionKind::Batch(transactions),
        sender,
        authority_state
            .get_object(&gas_object_id)
            .await?
            .unwrap()
            .compute_object_reference(),
    );
    let signature = Signature::new(&data, &sender_key);
    let tx = Transaction::new(data, signature);
    let response = send_and_confirm_transaction(&authority_state, tx).await;
    assert!(matches!(
        response.unwrap_err(),
        SuiError::InvalidBatchTransaction { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn test_batch_insufficient_gas_balance() -> anyhow::Result<()> {
    // This test creates 100 Move call transactions batch, each with a budget of 5000.
    // However we provide a gas coin with only 49999 balance.
    let (sender, sender_key) = get_key_pair();
    let authority_state = init_state_with_ids([]).await;
    let gas_object_id = ObjectID::random();
    let gas_object = Object::with_id_owner_gas_for_testing(
        gas_object_id,
        SequenceNumber::new(),
        sender,
        49999, // We need 50000
    );
    authority_state.insert_object(gas_object.clone()).await;

    let genesis_package_objects = genesis::clone_genesis_packages();
    let package_object_ref =
        get_genesis_package_by_module(&genesis_package_objects, "ObjectBasics");
    const N: usize = 100;
    let mut transactions = vec![];
    for _ in 0..N {
        transactions.push(SingleTransactionKind::Call(MoveCall {
            package: package_object_ref,
            module: ident_str!("ObjectBasics").to_owned(),
            function: ident_str!("create").to_owned(),
            type_arguments: vec![],
            object_arguments: vec![],
            shared_object_arguments: vec![],
            pure_arguments: vec![
                16u64.to_le_bytes().to_vec(),
                bcs::to_bytes(&AccountAddress::from(sender)).unwrap(),
            ],
            gas_budget: 500,
        }));
    }
    let data = TransactionData::new(
        TransactionKind::Batch(transactions),
        sender,
        gas_object.compute_object_reference(),
    );
    let signature = Signature::new(&data, &sender_key);
    let tx = Transaction::new(data, signature);
    let response = send_and_confirm_transaction(&authority_state, tx).await;
    assert!(matches!(
        response.unwrap_err(),
        SuiError::InsufficientGas { .. }
    ));

    Ok(())
}