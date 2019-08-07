#!/bin/bash

printf "Building interledger.rs... (this may take a little while)\n"
cargo build --bins

# Create the logs directory if it doesn't already exist
mkdir -p logs

# Start Redis
redis-server --version > /dev/null || echo "Uh oh! You need to install redis-server before running this example"
redis-server &> logs/redis.log &

# Turn on debug logging for all of the interledger.rs components and tower-web
export RUST_LOG=tower-web,interledger/.*

# Start both nodes
cargo run --package interledger -- node --config ./node-a/config.yml &> logs/node-a.log &
cargo run --package interledger -- node --config ./node-b/config.yml &> logs/node-b.log &

sleep 1

printf "The Interledger.rs nodes are up and running!\n\n"
