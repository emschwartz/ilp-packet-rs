cargo run --bin ilp-cli -- --node http://127.0.0.1:7770 accounts delete alice --auth hi_alice
cargo run --bin ilp-cli -- --node http://127.0.0.1:7770 accounts delete bob --auth hi_alice
cargo run --bin ilp-cli -- --node http://127.0.0.1:8770 accounts delete alice --auth hi_bob
cargo run --bin ilp-cli -- --node http://127.0.0.1:8770 accounts delete bob --auth hi_bob