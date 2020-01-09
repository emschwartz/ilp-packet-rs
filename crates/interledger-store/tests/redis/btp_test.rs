use super::fixtures::*;
use super::redis_helpers::TestContext;
use super::store_helpers::*;
use futures::compat::Future01CompatExt;
use futures::future::Future;
use futures01::future::Future as Future01;
use interledger_api::NodeStore;
use interledger_btp::{BtpAccount, BtpStore};
use interledger_http::HttpAccount;
use interledger_packet::Address;
use interledger_service::{Account as AccountTrait, Username};
use interledger_store::{account::Account, redis::RedisStore};
use secrecy::{ExposeSecret, SecretString};
use std::str::FromStr;

#[tokio::test]
async fn gets_account_from_btp_auth() {
    let (store, _context, _) = test_store().await.unwrap();
    let account = store
        .get_account_from_btp_auth(&Username::from_str("bob").unwrap(), "other_btp_token")
        .await
        .unwrap();
    assert_eq!(
        *account.ilp_address(),
        Address::from_str("example.alice.user1.bob").unwrap()
    );
}

#[tokio::test]
async fn decrypts_outgoing_tokens_btp() {
    let (store, _context, _) = test_store().await.unwrap();
    let account = store
        .get_account_from_btp_auth(&Username::from_str("bob").unwrap(), "other_btp_token")
        .await
        .unwrap();

    // the account is created on Dylan's connector
    assert_eq!(
        account.get_http_auth_token().unwrap().expose_secret(),
        "outgoing_auth_token",
    );
    assert_eq!(
        &account.get_ilp_over_btp_outgoing_token().unwrap(),
        b"btp_token"
    );
}

#[tokio::test]
async fn errors_on_unknown_user_or_wrong_btp_token() {
    let (store, _context, _) = test_store().await.unwrap();
    let result = store
        .get_account_from_btp_auth(&Username::from_str("asdf").unwrap(), "other_btp_token")
        .await;
    assert!(result.is_err());

    let result = store
        .get_account_from_btp_auth(&Username::from_str("bob").unwrap(), "wrong_token")
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn duplicate_btp_incoming_auth_works() {
    let mut charlie = ACCOUNT_DETAILS_2.clone();
    charlie.ilp_over_btp_incoming_token = Some(SecretString::new("btp_token".to_string()));
    let (store, _context, accs) = test_store().await.unwrap();
    let alice = accs[0].clone();
    let alice_id = alice.id();
    let charlie = store.insert_account(charlie).await.unwrap();
    let charlie_id = charlie.id();
    assert_ne!(alice_id, charlie_id);
    let result = futures::future::join_all(vec![
        store.get_account_from_btp_auth(&Username::from_str("alice").unwrap(), "btp_token"),
        store.get_account_from_btp_auth(&Username::from_str("charlie").unwrap(), "btp_token"),
    ])
    .await;
    let accs: Vec<_> = result.into_iter().map(|r| r.unwrap()).collect();
    assert_ne!(accs[0].id(), accs[1].id());
    assert_eq!(accs[0].id(), alice_id);
    assert_eq!(accs[1].id(), charlie_id);
}
