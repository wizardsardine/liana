# Wallet history on a managed pruned node

COINCUBE Desktop can preserve Connect-discovered Vault history when adopting its managed
regular-Bitcoin Core or Bitcoin Knots node. Pruning removes block bodies, not
transactions already saved in the node's wallet database. A newly imported or
previously offline wallet may still need those bodies to discover older activity.

The Node settings panel exposes three related operations:

- **Import Connect history** retrieves known funding and spending transactions
  from the current wallet engine, requests Merkle-block proofs from the configured
  Esplora providers, validates their transaction membership and active-chain block
  hash, and imports supported transactions with `importprunedfunds`. Existing
  Coincube database history remains intact. A transaction with no usable proof,
  or one Core cannot import (including a spend with no wallet-owned output), has
  only its own block fetched and scanned. A coin whose spend Connect knows queues
  just the spend's block; when Connect's wallet view lacks the spender of an
  output the node reports spent, COINCUBE Desktop asks the Esplora
  `/tx/:txid/outspend/:vout` endpoint and accepts its block only if the hash
  matches the local active chain. The final check asks again and accepts the
  coin once the local wallet holds that spender. Queued blocks are fetched and scanned one at a
  time in height order, followed by a scan of the last 32 blocks. Only a pending
  spend, or a spender that stays unknown, scans every block from the coin's
  height, and the panel says why. Tokens are sent only to their configured
  provider; redirects are disabled.
- **Recover wallet history** accepts an exact block height or a UTC date in
  `YYYY-MM-DD` form. It fetches missing blocks from connected outbound archival
  peers, verifies the block bodies, scans retained batches, and saves progress.
  Date resolution uses median block time with a conservative two-hour allowance
  and 144-block margin; use a height when an exact boundary is required. The scan
  ends at the chain tip captured when the operation starts, followed by spend
  reconciliation. Normal wallet tracking handles subsequent blocks.
- **Retain recent blocks** accepts 1–3650 days, including 180 and 365. Applying a
  window also recovers missing blocks in the requested period. COINCUBE Desktop switches
  the managed node to manual pruning and maintains a rolling timestamp cutoff.
  Choosing a storage-size target in the existing node resource settings disables
  rolling retention after outstanding recoveries have been cancelled/completed.

## Spend reconciliation and switching

A funding inclusion proof establishes confirmation, not current unspentness.
Before declaring recovery complete or permitting the local-node handoff, COINCUBE Desktop
checks funding and spending records against the local active chain and compares
outputs against `gettxout` with mempool spends included. Pending records require
local mempool membership. A mismatch keeps Connect active and leaves a resumable
recovery obligation; it never treats an imported funding transaction alone as
evidence of a spendable balance.

Core wallet records are replayed into Coincube's database on each local daemon
startup, and explicitly after recovery on an already active local backend. Replay
uses saved transaction records, so it works after the scanned blocks are pruned.
Only a successful coherent database update acknowledges its replay request ticket;
an older poll cannot acknowledge a newer request. An unknown-birthday restored
Vault needs a reconciled scan from genesis and an acknowledged local database
replay to retire its full-rescan obligation. A verified completed genesis scan can
permit the Connect-to-local handoff while that obligation remains recorded.
A recent date does not establish complete older history. Descriptor ranges
preserve Core's existing range and include current Coincube indices plus a margin.
Replay maps outputs through index 1000 or the existing wallet lookahead, whichever
is larger. Startup replay logs and skips unmatched owned outputs so ordinary
wallet syncing can continue. An explicit recovery replay remains incomplete for
such outputs and requires restoring the Vault's address indices before it can
claim complete history. These operations use watch-only descriptor wallets.

## Retention, interruption, and disk use

Progress and retention intent are atomic private files under
`<Coincube data directory>/node-history/<network>/`. A recovery is bound to the
network genesis, managed-node instance, endpoint, wallet path and descriptor.
Changed identities or malformed journals are refused. A per-network filesystem
lease serializes block fetching/scanning, reconciliation and pruning across Cubes.
Pruning is limited by the earliest unfinished scan, which for a queue of single
blocks is the lowest unscanned one. Downloaded blocks do not count as scanned
progress. Reorganizations, a wallet reload and a failed final reconciliation
rewind the affected recovery to a full range scan before completion. A node RPC
that misses its deadline does not pause recovery: COINCUBE Desktop keeps polling and
pauses only after 12 consecutive unreachable steps (about a minute) or on any
other error.

**Pause** waits for the current worker; **Resume** rechecks the saved
identity and continues; **Cancel** releases the recovery's retention obligation.
Wallet records already imported/scanned survive cancellation and later pruning.
Temporary recovery restores the previous storage-size pruning target when no
unfinished recoveries remain. A user-selected rolling window stays enabled.

Core can refuse to load an offline wallet whose catch-up blocks were deleted.
COINCUBE Desktop then retains the entire requested range before retrying wallet loading.
If Core still requires older blocks, the recovery pauses and asks for an earlier
start. Managed startup suppresses automatic wallet loading while manual retention
or an unloaded-wallet recovery is recorded, keeping wallet RPC available for
explicit loading. It never deletes a wallet or block file and never runs reindex.

Recovery and rolling retention can consume hundreds of gigabytes. A 512 MiB
free-space guard pauses fetching; it is an emergency reserve, not a size guarantee.
Core prunes whole files and keeps its minimum recent-block reserve, so a date
window is a minimum retention intent rather than an exact disk cap. COINCUBE Desktop must
remain open with a Cube using the node for rolling pruning to advance; while it
is closed the manual-pruning node continues retaining blocks and can grow.
Peer availability, bandwidth, wallet catch-up requirements and disk capacity
determine whether a requested historical range can be recovered.

## Compatibility and verification

These controls apply only to owned, cookie-authenticated, regular-Bitcoin nodes.
External nodes and Bitcoin Blake2b are excluded. The same RPC contract was checked
with Bitcoin Core **29.0** and regular-Bitcoin Knots **29.3.knots20260507**.

Run the isolated two-node contract test with a downloaded release binary:

```sh
python3 tests/pruned_history_regtest.py /absolute/path/to/bitcoind
```

This provider RPC contract test demonstrates that a funding proof alone can produce a stale wallet UTXO, that
spending-only import is rejected, that a fetched pruned block can be scanned
despite an unchanged `pruneheight`, and that wallet records survive a subsequent
pruning request and restart. It does not require that the second pruning request
deletes the fetched block's mixed-age file. It creates private regtest data
directories and stops only its own nodes. GUI unit tests cover proof membership,
provider fallback, identity bounds, filesystem exclusion, failed scan checkpoints,
reorganization, bootstrap progress, stale mempool spend rejection, pruning holds,
and obsolete GUI callbacks. A daemon HTTP integration regression verifies that
Core-only historical funding enters Coincube's coin and transaction APIs even
when its database tip has already reached the current chain tip.

Relevant upstream RPC documentation: [importprunedfunds](https://bitcoincore.org/en/doc/29.0.0/rpc/wallet/importprunedfunds/),
[getblockfrompeer](https://bitcoincore.org/en/doc/29.0.0/rpc/blockchain/getblockfrompeer/),
[rescanblockchain](https://bitcoincore.org/en/doc/29.0.0/rpc/wallet/rescanblockchain/),
and [pruneblockchain](https://bitcoincore.org/en/doc/29.0.0/rpc/blockchain/pruneblockchain/).

### Guided switch and default retention

For a Connect Vault, select **Switch to local node**. When the managed local
wallet is missing history, COINCUBE Desktop shows **Move this Vault to your local node**:
sync the blockchain, bring this Vault's history over, then switch. Step 2 offers
**Start from Connect's history** (recommended; **Import history from Connect**)
or **Start from a date or block**, which also shows the custom rolling retention
window. Step 3's **Switch to local node** stays visible but disabled until the
history checks out. An existing interrupted recovery remains visible so it can
be resumed. **Rescan from a date** is disabled while a recovery runs.
Once reconciliation and a fresh history check succeed, an explicit switch
request is retried through the normal sync and running-scan guards.

The default remains automatic pruning by the node's storage-size target.
Existing targets are preserved; new managed nodes retain COINCUBE Desktop's existing
15,000 MiB target. No rolling day window is selected or applied by default.
Core's upstream default disables pruning, and its minimum automatic target is
550 MiB; neither value is a time-based retention default. After switching,
custom day-based retention remains available under **Advanced node settings**.
