#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    interledger_ccp::fuzz_update_request(data);
});
