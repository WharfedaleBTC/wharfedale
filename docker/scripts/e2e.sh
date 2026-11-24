#!/usr/bin/env bash
# End-to-end test on the docker network. Exits non-zero on the first failed check.
set -euo pipefail

w() { wharfedale-wallet --rpc "http://$1:18443" "${@:2}"; }
fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok - $*"; }
retry() { for _ in $(seq 1 120); do "$@" >/dev/null 2>&1 && return 0; sleep 0.5; done; return 1; }

w node0 wait-height --height 0 --timeout 180 >/dev/null
pass "node0 answers"

H0=$(w node0 rpc getblockcount)
sleep 3
[ "$(w node0 rpc getblockcount)" = "$H0" ] || fail "the chain grew with nothing to include"
pass "no empty blocks: an idle chain stays at height $H0"

FEE_ASSET=$(w node0 rpc getblockchaininfo | sed -n 's/.*"fee_asset": "\([0-9a-f]*\)".*/\1/p')
[ -n "$FEE_ASSET" ] || fail "no fee_asset in getblockchaininfo"

D=$(mktemp -d)
w node0 keygen --out "$D/alice.json" >/dev/null
w node0 keygen --out "$D/bob.json" >/dev/null
ALICE=$(w node0 address --key "$D/alice.json")
BOB=$(w node0 address --key "$D/bob.json")
case "$ALICE" in wfc1p*) ;; *) fail "expected a confidential address, got $ALICE" ;; esac

FUND=$(w node0 send --key /net/faucet.json --to "$ALICE" --amount 5000000)
w node3 wait-tx --txid "$FUND" --timeout 60 >/dev/null || fail "faucet tx not confirmed on node3"
pass "faucet → alice (confidential) confirmed: gossip node0 → proposer, block seen by node3"

PAY=$(w node2 send --key "$D/alice.json" --to "$BOB" --amount 1000000 --fee 1000)
w node1 wait-tx --txid "$PAY" --timeout 60 >/dev/null || fail "alice tx not confirmed on node1"
pass "alice → bob (confidential input and outputs) confirmed"

A=$(w node1 balance --key "$D/alice.json" --asset "$FEE_ASSET")
B=$(w node3 balance --key "$D/bob.json" --asset "$FEE_ASSET")
[ "$A" = "3999000" ] || fail "alice balance $A, expected 3999000"
[ "$B" = "1000000" ] || fail "bob balance $B, expected 1000000"
pass "unblinded balances: alice $A, bob $B"

w node0 rest "tx/$PAY" | grep -q '"valuecommitment"' || fail "payment amounts are not committed"
pass "payment shows value commitments on chain"

OUT=$(w node0 issue --key "$D/alice.json" --amount 1000 --tokens 1)
ISS=$(echo "$OUT" | sed -n 1p); ASSET=$(echo "$OUT" | sed -n 2p); TOKEN=$(echo "$OUT" | sed -n 3p)
w node0 wait-tx --txid "$ISS" --timeout 60 >/dev/null || fail "issuance not confirmed"
[ "$(w node0 balance --key "$D/alice.json" --asset "$ASSET")" = "1000" ] || fail "issued amount"
[ "$(w node0 balance --key "$D/alice.json" --asset "$TOKEN")" = "1" ] || fail "reissuance token"
pass "issued asset $ASSET (1000) with 1 reissuance token"

retry w node2 rest "asset/$ASSET" || fail "asset not indexed on node2"
RE=$(w node2 reissue --key "$D/alice.json" --asset "$ASSET" --amount 500)
w node0 wait-tx --txid "$RE" --timeout 60 >/dev/null || fail "reissuance not confirmed"
[ "$(w node0 balance --key "$D/alice.json" --asset "$ASSET")" = "1500" ] || fail "reissued amount"
pass "reissued 500 with the token"

retry sh -c "wharfedale-wallet --rpc http://node1:18443 rest asset/$ASSET | grep -q '\"issued_amount\": 1500'" \
  || fail "esplora asset record not updated"
retry sh -c "wharfedale-wallet --rpc http://node3:18443 rest tx/$FUND/outspends | grep -q '\"spent\": true'" \
  || fail "esplora outspends"
pass "esplora: asset record and outspends"

if w node0 send --key "$D/bob.json" --to "$ALICE" --amount 2000000 >/dev/null 2>&1; then
  fail "overspend was accepted"
fi
pass "overspend refused"

C=/usr/local/share/wharfedale/contracts
BOB_PK=$(w node0 pubkey --key "$D/bob.json")
echo "{\"OWNER\": \"0x$BOB_PK\"}" > "$D/p2pk.args"
echo "{\"SIG\": \"sign:$D/bob.json\"}" > "$D/bob.wit"
echo "{\"SIG\": \"sign:$D/alice.json\"}" > "$D/alice.wit"
P2PK=(--program "$C/p2pk.simf" --args "$D/p2pk.args")
CADDR=$(w node0 contract-address "${P2PK[@]}")
case "$CADDR" in wf1p*) ;; *) fail "expected an explicit contract address, got $CADDR" ;; esac
LOCK=$(w node1 send --key /net/faucet.json --to "$CADDR" --amount 300000)
w node2 wait-tx --txid "$LOCK" --timeout 60 >/dev/null || fail "contract funding not confirmed"
[ "$(w node0 contract-balance "${P2PK[@]}" --asset "$FEE_ASSET")" = "300000" ] || fail "contract balance"
pass "300000 locked to bob's p2pk contract"

if w node3 contract-spend "${P2PK[@]}" --witness "$D/alice.wit" --to "$ALICE" --amount 200000 >/dev/null 2>&1; then
  fail "contract spent with alice's signature"
fi
pass "contract refuses alice's signature"

A0=$(w node0 balance --key "$D/alice.json" --asset "$FEE_ASSET")
CS=$(w node3 contract-spend "${P2PK[@]}" --witness "$D/bob.wit" --to "$ALICE" --amount 200000)
w node0 wait-tx --txid "$CS" --timeout 60 >/dev/null || fail "contract spend not confirmed"
[ "$(w node0 contract-balance "${P2PK[@]}" --asset "$FEE_ASSET")" = "99000" ] || fail "contract change"
[ "$(w node0 balance --key "$D/alice.json" --asset "$FEE_ASSET")" = "$((A0 + 200000))" ] || fail "alice did not receive"
pass "bob's signature spends the contract (Simplicity executed by every validator)"

AFTER=$(( $(w node0 rpc getblockcount) + 3 ))
echo "{\"OWNER\": \"0x$BOB_PK\", \"AFTER\": \"$AFTER\"}" > "$D/timelock.args"
TL=(--program "$C/timelock.simf" --args "$D/timelock.args")
TLOCK=$(w node0 send --key /net/faucet.json --to "$(w node0 contract-address "${TL[@]}")" --amount 50000)
w node0 wait-tx --txid "$TLOCK" --timeout 60 >/dev/null || fail "timelock funding not confirmed"
if w node0 contract-spend "${TL[@]}" --witness "$D/bob.wit" --to "$BOB" --amount 10000 --lock-time $((AFTER - 1)) >/dev/null 2>&1; then
  fail "timelock spent before its height"
fi
# Blocks only come with transactions: small payments carry the chain to $AFTER.
while [ "$(w node0 rpc getblockcount)" -lt "$AFTER" ]; do
  FILL=$(w node0 send --key /net/faucet.json --to "$ALICE" --amount 1000)
  w node0 wait-tx --txid "$FILL" --timeout 60 >/dev/null || fail "filler payment not confirmed"
done
TS=$(w node0 contract-spend "${TL[@]}" --witness "$D/bob.wit" --to "$BOB" --amount 10000 --lock-time "$AFTER")
w node1 wait-tx --txid "$TS" --timeout 60 >/dev/null || fail "timelock spend not confirmed"
pass "timelock contract: refused with lock time $((AFTER - 1)), spent with $AFTER"

FT=$(w rpc0 send --key /net/faucet.json --to "$BOB" --amount 70000)
w node3 wait-tx --txid "$FT" --timeout 60 >/dev/null || fail "transaction submitted to the full node not confirmed"
pass "full node rpc0 relays a transaction the validators confirm"

H=$(w node0 rpc getblockcount)
for n in node1 node2 node3 rpc0; do w "$n" wait-height --height "$H" --timeout 30 >/dev/null; done
REF=$(w node0 rpc getblockhash "$H")
for n in node1 node2 node3 rpc0; do
  [ "$(w "$n" rpc getblockhash "$H")" = "$REF" ] || fail "$n disagrees at height $H"
done
pass "all 4 validators and the full node agree on block $H"

echo "PASS"
