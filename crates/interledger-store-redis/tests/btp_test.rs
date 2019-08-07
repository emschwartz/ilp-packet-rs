mod common;

use common::*;
use interledger_btp::{BtpAccount, BtpStore};
use interledger_http::HttpAccount;
use interledger_service::Account as AccountTrait;

#[test]
fn gets_account_from_btp_token() {
    block_on(test_store().and_then(|(store, context)| {
        store
            .get_account_from_btp_token("other_btp_token")
            .and_then(move |account| {
                assert_eq!(account.id(), 1);
                let _ = context;
                Ok(())
            })
    }))
    .unwrap()
}

#[test]
fn decrypts_outgoing_tokens() {
    block_on(test_store().and_then(|(store, context)| {
        store
            .get_account_from_btp_token("other_btp_token")
            .and_then(move |account| {
                assert_eq!(
                    account.get_http_auth_token().unwrap(),
                    "outgoing_auth_token"
                );
                assert_eq!(
                    account.get_btp_token().unwrap(),
                    b"other_outgoing_btp_token"
                );
                let _ = context;
                Ok(())
            })
    }))
    .unwrap()
}

#[test]
fn errors_on_unknown_btp_token() {
    let result = block_on(test_store().and_then(|(store, context)| {
        store
            .get_account_from_btp_token("unknown_btp_token")
            .then(move |result| {
                let _ = context;
                result
            })
    }));
    assert!(result.is_err());
}
