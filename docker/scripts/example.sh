#!/usr/bin/env bash
# Walkthrough on the docker network: confidential transfers, issuance, reissuance, Esplora,
# a Simplicity contract.
set -euo pipefail

w() { wharfedale-wallet --rpc "http://$1:18443" "${@:2}"; }

echo "== waiting for the chain"
w node0 wait-height --height 2 --timeout 180 >/dev/null
w node0 rpc getblockchaininfo

mkdir -p /tmp/wallets
w node0 keygen --out /tmp/wallets/alice.json >/dev/null
w node0 keygen --out /tmp/wallets/bob.json >/dev/null
ALICE=$(w node0 address --key /tmp/wallets/alice.json)
BOB=$(w node0 address --key /tmp/wallets/bob.json)
echo "alice: $ALICE"
echo "bob:   $BOB"

echo "== faucet → alice 5000000, confidential (submitted to node0)"
TX=$(w node0 send --key /net/faucet.json --to "$ALICE" --amount 5000000)
echo "txid $TX, confirmed at height $(w node3 wait-tx --txid "$TX")"

echo "== alice → bob 1000000, confidential (submitted to node2)"
PAY=$(w node2 send --key /tmp/wallets/alice.json --to "$BOB" --amount 1000000)
echo "txid $PAY, confirmed at height $(w node1 wait-tx --txid "$PAY")"

echo "== balances (unblinded by each wallet)"
echo "alice: $(w node0 balance --key /tmp/wallets/alice.json)"
echo "bob:   $(w node0 balance --key /tmp/wallets/bob.json)"

echo "== the payment as anyone sees it (Esplora /tx): commitments, no amounts"
w node3 rest "tx/$PAY" | grep -E '"(valuecommitment|assetcommitment|value|asset|scriptpubkey_address)"'

echo "== alice issues 1000 units with 1 reissuance token"
OUT=$(w node0 issue --key /tmp/wallets/alice.json --amount 1000 --tokens 1)
ISS=$(echo "$OUT" | sed -n 1p); ASSET=$(echo "$OUT" | sed -n 2p)
echo "asset $ASSET, confirmed at height $(w node0 wait-tx --txid "$ISS")"
for _ in $(seq 1 60); do w node0 rest "asset/$ASSET" >/dev/null 2>&1 && break; sleep 0.5; done

echo "== alice reissues 500"
RE=$(w node0 reissue --key /tmp/wallets/alice.json --asset "$ASSET" --amount 500)
echo "confirmed at height $(w node0 wait-tx --txid "$RE")"
sleep 1
echo "alice: $(w node0 balance --key /tmp/wallets/alice.json)"

echo "== Esplora asset record"
w node1 rest "asset/$ASSET"

echo "== bob's p2pk Simplicity contract (examples/contracts/p2pk.simf)"
C=/usr/local/share/wharfedale/contracts
echo "{\"OWNER\": \"0x$(w node0 pubkey --key /tmp/wallets/bob.json)\"}" > /tmp/wallets/p2pk.args
echo "{\"SIG\": \"sign:/tmp/wallets/bob.json\"}" > /tmp/wallets/bob.wit
P2PK=(--program "$C/p2pk.simf" --args /tmp/wallets/p2pk.args)
CADDR=$(w node0 contract-address "${P2PK[@]}")
echo "cmr $(w node0 contract-address "${P2PK[@]}" --cmr), address $CADDR"
TX=$(w node0 send --key /net/faucet.json --to "$CADDR" --amount 300000)
echo "faucet → contract 300000, confirmed at height $(w node0 wait-tx --txid "$TX")"
echo "contract: $(w node0 contract-balance "${P2PK[@]}")"

echo "== bob spends the contract to alice (witness: his signature over sig_all_hash)"
CS=$(w node1 contract-spend "${P2PK[@]}" --witness /tmp/wallets/bob.wit --to "$ALICE" --amount 200000)
echo "txid $CS, confirmed at height $(w node2 wait-tx --txid "$CS")"
echo "contract: $(w node0 contract-balance "${P2PK[@]}")"
echo "alice:    $(w node0 balance --key /tmp/wallets/alice.json)"
