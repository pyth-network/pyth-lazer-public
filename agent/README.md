# pyth-lazer-agent

pyth-lazer-agent is intended to be run by Lazer publishers analogous to [pyth-agent](https://github.com/pyth-network/pyth-agent)
for pythnet publishers. Currently, it retains [the existing Lazer publishing interface](https://github.com/pyth-network/pyth-examples/tree/main/lazer/publisher),
but will batch and sign transactions before publishing them to Lazer.

## Keypair

You will need to generate an ed25519 keypair and provide the pubkey to the Lazer team. `solana-keygen` is the recommended utility.

```bash
solana-keygen new -o /path/to/keypair.json
solana-keygen pubkey /path/to/keypair.json
```

pyth-lazer-agent will need to configure access to this keypair file to sign transactions.

## Build and run

### From cargo

```
# Download the cargo package
cargo install pyth-lazer-agent

# Add .cargo/bin to PATH
export PATH="$PATH:~/.cargo/bin"

# Run the agent
pyth-lazer-agent --help
```

### From source

Please check [rust-toolchain](../rust-toolchain.toml) to see the version of Rust needed to build.
You will also need SSL and CA certificates. `cargo build` should then work as usual.

### Docker

See the included [Dockerfile](Dockerfile) to build an image yourself.

### Container

We also publish images to the [GitHub Container Registry](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry#pulling-container-images).
The latest will be available at `ghcr.io/pyth-network/pyth-lazer-agent:latest`.

## Configure

The agent takes a single `--config` CLI option, pointing at
`config/config.toml` by default. Configuration is currently minimal:

```toml
relayer_urls = ["wss://relayer-0.pyth-lazer.dourolabs.app/v1/transaction", "wss://relayer-1.pyth-lazer.dourolabs.app/v1/transaction"]
# relayer_connections = 2
publish_keypair_path = "/path/to/keypair.json"
authorization_token = "your_token"
listen_address = "0.0.0.0:8910"
publish_interval_duration = "25ms"
enable_update_deduplication = false
# Optional proxy configuration
# proxy_url = "http://proxy.example.com:8080"
# proxy_url = "http://username:password@proxy.example.com:8080"  # With authentication
```

- `relayers_urls`: The Lazer team will provide these.
- `relayer_connections` (optional): How many of the `relayer_urls` to stay connected to at once. Defaults to all. When fewer than the number of URLs, the extras are kept as warm standbys and a connection fails over to the next relayer when one drops.
- `publish_keypair_path`: The keypair file generated with `solana-keygen` or similar.
- `authorization_token`: The Lazer team will provide this or instruct that it can be omitted.
- `listen_address`: The local port the agent will be listening on; can be anything you want.
- `publisher_interval`: The agent will batch and send transaction bundles at this interval. The Lazer team will provide guidance here.
- `enable_update_deduplication`: The agent will deduplicate updates based inside each batch before sending it to Lazer.
- `proxy_url` (optional): HTTP/HTTPS proxy URL for WebSocket connections. Supports Basic authentication via URL credentials (e.g., `http://user:pass@proxy:port`).

## Publishing by CUSIP

A publisher can send an identifier instead of a feed id. An identifier is a `scheme` and a
`value` whose shape depends on the scheme; each scheme carries the fields that name exactly one
feed. Today the only scheme is `cusip`, used by the US Treasury feeds: its value is the CUSIP and
the `quote_currency`, where `%` is the yield feed and `USD` the price feed. The agent routes the
update to every listed feed matching the identifier and drops identifiers that match no feed, so a
publisher can push every CUSIP it quotes and never maintain a feed mapping or handle auction rolls.
The agent reloads the map from the metadata endpoint about once a minute, so a roll takes effect
within a minute or two; until the first load after start, identifier updates are answered with an
error. Only publicly listed feeds are routable; feeds not yet listed need `feed_id`. Existing
`feed_id` updates are unchanged.

```json
{
  "jsonrpc": "2.0",
  "method": "push_update",
  "params": {
    "identifier": {
      "scheme": "cusip",
      "value": { "cusip": "912797NQ4", "quote_currency": "USD" }
    },
    "source_timestamp": 1763400000000000,
    "update": {
      "type": "price",
      "price": 9980000000,
      "best_bid_price": null,
      "best_ask_price": null
    }
  },
  "id": 1
}
```

`push_updates` takes a list of the same params, and `feed_id` and `identifier` may be mixed in one
batch. From Rust, `FeedUpdateParams::by_cusip(cusip, "USD", ..)`, `by_identifier` and `by_feed_id` in
`pyth_lazer_protocol::jrpc` build the params.
