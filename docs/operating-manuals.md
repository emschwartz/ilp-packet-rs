# Interledger.rs Operating Manuals

## Initial Set up

### Top Level Commands
Currently we have 2 top level commands.

```bash
# Interledger Node
cargo run -- node

# Ethereum Ledger Settlement Engine
cargo run --package interledger-settlement-engines -- ethereum-ledger
```

### Types of Parameters

Please use `--help` option to see what kind of parameters are available. For example,

```bash
# shows the top level command help
cargo run -- --help

# shows the subcommand level help of `node` command
cargo run -- node --help
```

### Specifying Parameters

Interledger.rs commands such as `node` and `ethereum-ledger` accept configuration options in the following ways:

1. Command line arguments
1. Configuration files
1. Standard In (stdin)
1. Environment variables

The priority is: Command line arguments < Configuration files < STDIN < Environment variables.

```bash #
# 1.
# Passing by command line arguments.
# --{parameter name} {value}
cargo run -- node --ilp_address example.alice

# 2.
# Passing by a configuration file in JSON, TOML, YAML format.
# Note that the first argument after subcommands such as `node` is considered as a configuration file.
cargo run -- node config.yml

# 3.
# Passing from STDIN in JSON, TOML, YAML format.
some_command | cargo run -- node

# 4.
# passing as environment variables
# {parameter name (typically in capital)}={value}
# note that the parameter names MUST begin with a prefix of "ILP_" e.g. ILP_SECRET_SEED
ILP_ADDRESS=example.alice \
ILP_OTHER_PARAMETER=other_value \
cargo run -- node
```

You can specify these 4 at the same time.

```bash
config_cmd | ILP_ADDRESS=example.alice \
cargo run -- node alice.yaml \
--admin_auth_token 26931aa8c117726b2c25c9be2c52ca24d26eda5782fe9a39984db7dc602dcf0c
```
