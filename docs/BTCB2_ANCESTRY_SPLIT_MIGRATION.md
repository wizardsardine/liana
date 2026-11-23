# BTCB2 split migration: chain-specific inputs

Status: superseded as a migration plan. This records the input-ancestry
migration of the paired Claim and two-step Split flows. Robert's decision of
2026-10-29 (company-brain
`decisions/2026-10-29-coincube-desktop-btcb2-split-protection-in-coin-control.md`)
replaced both with one-sided **Split coins**. The live flow is Tools › Split
coins (`coincube-gui/src/app/state/vault/split_hub.rs` and
`coincube-gui/src/services/split_coins.rs`). Coin control keeps the banner and
sends its Split buttons to that tab. S7a removed the Claim wizard, the
foreign-wallet Split and the launch gate from the GUI; S7b removed the paired
engine, the OP_RETURN poison, `coincubed`'s `poison_broadcast`, the Claim
observation assessment and the regtest bridges and consensus modules that
drove them. What this document says about the input-ancestry engine
(`coincube_core::claim_ancestry`, its proofs and bounds) still describes code
Split coins uses; what it says about Claim and Split intents, paired
signing, staged submission, the 6-confirmation gate between sides and the
acceptance gate describes removed code and is kept as history only.

## Current desktop flow

The step is derived from the Vault each time the tab is drawn: get a split
coin, review and sign, wait out confirmations, then done or the coins still
left. Waiting for a split coin can take an hour, so the flow is a tab, not a
dialog. The panel keeps the user's choices (coins left out, split-key versus
signature), the address a split coin is sent to, the fee rate, and a build
in flight. Everything else is read back from the Vault.

The fee rate is Connect's six-block (`"6"`) Esplora estimate for this chain
only (`split_coins::fee_rate`). A missing quote, a rate outside
`1..=MAX_FEERATE`, or a source for the other chain is unavailable. The
Bitcoin read has to be a fresh Connect response. There is no local guess.
An unavailable rate is retried on its own three times, five seconds
apart, while the review still says it is getting the rate. After that the
review says the rate is unavailable and offers Try again. A fee answer from
an older fetch is ignored: each fetch has a generation, and a superseded one
does not change the rate or start another retry.

A saved ancestry path is not a split key. `chain-proofs.json`, next to the
Vault's daemon data, loads as `ProofEntry::Saved`: the links fit, and a fresh
check still has to show the chain holds them. A check that cannot finish
(Connect down, a read that times out) does not erase that path. It becomes
`SavedRetry`, is written back, stays "checking", and is tried again after
`FAILED_RETRY` (30 minutes). A finished answer replaces the path, including a
completed search that found none. A failed check that was not holding a path
is not written, so the next launch retries it. A verified path from this
session is not replaced by a later miss or failure.

## Decision and safety invariant

Replace the 90-byte OP_RETURN with ordinary Bitcoin and BTCB2 inputs whose
ancestry proves they exist on only one chain. A user deposits a small amount of
post-fork Bitcoin and a small amount of post-fork BTCB2 into fresh addresses of
the **source wallet being split**. These deposits pay the respective network
fees; they are not sent to COINCUBE. The app must positively verify each
deposit's chain-specific ancestry, ownership, confirmation depth and current
unspent status. A post-fork timestamp, a missing transaction on the other
indexer or an exchange withdrawal label is not proof.

For every shared pre-fork outpoint `S`, construct two ordinary transactions:

1. On Bitcoin, spend **all** selected shared outpoints `S` together with a
   verified Bitcoin-only input `B` into a fresh address of the same source
   wallet. BTCB2 cannot validate this transaction because `B` is absent there.
2. On BTCB2, spend the **same** shared outpoints `S` together with a verified
   BTCB2-only input `F`. Claim returns the proceeds to a fresh address of its
   paired BTCB2 Vault; Split sends them to a fresh address of the target BTCB2
   Cube. Bitcoin cannot validate this transaction because `F` is absent there.

Both transactions use normal outputs and ordinary `SIGHASH_ALL` PSBTs. Every
selected input must be signed by the wallet's normal threshold. Neither
transaction may be offered for signing until *both* chain-specific inputs are
verified and reserved. This avoids relying on the six-confirmation Bitcoin
step-one observation as the only protection for the BTCB2 spend if Bitcoin
later reorganizes. Keep six confirmations and reorg monitoring for the
user-visible sequence and accounting. A reorg or spent marker invalidates
review and requires rebuilding; it must never silently switch to OP_RETURN or
a replayable spend.

## Existing pieces and required work

1. **Proof and inventory.** Reuse `claim_ancestry`, its bounded coinbase path,
   retained proof and `claim_observation/http/ancestry` for Bitcoin. The
   canonical two-chain coinbase comparison now accepts either chain as the
   dependency root, with an explicit root-chain label. Bounded BTCB2 path
   discovery and proof refresh are wired, but the BTCB2 candidate inventory
   and live spend admission still need to be wired. Generalize
   the authenticated observation collector to BTCB2 without weakening its
   post-fork, BIP34 height, coinbase maturity, canonical block, tip-bracketing
   or current-UTXO checks. At the dependency's root height, read the canonical
   coinbase on **both** chains and require distinct txids; the same-height
   coinbase could otherwise have been copied across the fork. The currently displayed Split
   `bitcoin_only_post_fork` candidates are hints only. Add symmetrical
   BTCB2-only candidates for display (now present) and promote neither to
   proof without the ancestry collector. A paired read-only funding collector
   now refreshes both root paths, selected-transaction confirmations,
   canonical block mappings, maturity, unspent status and two-chain tips under
   one session. Claim and Split admission bind its result to the source wallet,
   reserved deposits and exact saved pair before signing.
   The foreign inventory now retains previous transactions for covered,
   post-fork funding candidates. Review-only validators compare those bytes,
   retained paths, session, source constructions and two current tips for
   Split and Claim; Split also binds the candidates' block assignments.
   A further read-only collection now checks every shared input's confirmed
   transaction against the same canonical pre-fork block on both chains and
   requires an explicit current unspent response on each, under the paired
   funding session and stable tips. It also authenticates the shared output
   bytes by txid and checks their scripts and amounts against both PSBTs' UTXO
   metadata. Claim and Split review validators bind
   that exact shared set to both unsigned transactions and their version-10
   Claim or version-11 Split journal. Signer admission still needs to consume
   this fresh evidence with wallet ownership, reservations and a new preflight;
   no candidate or validator result is cached spend authority.
   The version-10 Claim journal now requires its recorded fee rates to match
   the exact unsigned PSBT fees, and the version-11 Split observation check
   compares the funding coins' branch and index to the saved derivations.
   A bounded paired search now uses one session, deadline and request budget
   for Bitcoin and BTCB2 candidate ancestry plus the shared-input refresh;
   it returns no one-sided result. For foreign Split, scan-derived funding and
   shared coins are rechecked against the source descriptor before the search.
   The two-chain observation collector and retained funding proof now treat
   every Connect session notification as revocation, including one that
   repeats the same numeric generation. Reads from either side cannot be
   combined across that boundary.
   Claim signer and recovery checks use the same unread-notification rule.
   After an explicit panel revocation, a newly recovered Claim acknowledges
   the new generation only on its own receiver; in-flight task clones stay
   revoked. A GUI regression covers recovery after a same-value notification.
   Selected coins are matched to live paths and confirmation blocks. A saved
   Split deposit reservation now restricts the search to the two exact
   external receive indices shown to the user. The search reloads and compares
   the released reservation under the current Connect identity before
   returning a read-only result from a completed paired scan. The production
   Split panel calls it for a read-only deposit check; it cannot authorize signing.
   A separate read-only Split signer prerequisite now restricts a fresh
   paired proof to the exact two outpoints in the retained PSBT pair. It
   reloads the source and target address reservation, verifies the current
   target Vault binding, reconstructs and compares both complete PSBTs, then
   refreshes both tips, proof age and Connect generation. The production
   Connect implementation can run this against a new two-chain source scan,
   reject a changed fee quote and create or reopen only the exact version-11
   intent under the saved journal lock. Split signer and file-import dispatch
   call this operation before handling an exact PSBT; it does not by itself
   reconcile a full source wallet or permit a send.
   Claim now has an equivalent read-only paired search that binds the selected
   Bitcoin and BTCB2 funding outputs and every shared output to its Vault
   descriptor. A Cube-specific entry point to the bounded two-chain scanner
   now walks its known receive/change descriptors, joins scan-time shared and
   chain-specific UTXOs, authenticates every candidate against the Vault and
   feeds that inventory to the paired search. Its history-gap result is not a
   complete wallet accounting: the production route must reconcile the full
   wallet UTXO set and refresh spendability before admission. A review-only
   Claim pair constructor builds the exact ordinary Bitcoin and BTCB2 PSBTs
   from that paired result, current shared wallet coins, previous transactions
   and separately reserved change indices. It refuses a changed shared set or
   output before returning the pair. `ClaimInventory::construct_pair_for_review`
   now supplies previous transactions from the same paired scan, rejecting a
   missing or conflicting transaction before delegating to that constructor.
   The production Claim deposit check invokes paired discovery for read-only
   status, but cannot construct or display the pair until both change
   destinations are durably reserved; it does not admit it to signers.
   The Claim funding journal now has a version-3 record for two distinct
   change indices. A reservation service asks the Bitcoin and BTCB2 wallet
   backends to advance their own change branches, checks their explicit chain
   identities and descriptor, then writes both indices under the Claim lock.
   A partial failure burns any already allocated index. Version-10 Claim
   creation and restart require the exact saved pair. Coincubed now exposes a
   local RPC reservation that checks the requested chain ID and descriptor
   before atomically advancing the persisted branch. The generic socket is
   intentionally disabled for BTCB2 desktop daemons; the production Claim
   wizard instead borrows the currently open paired fork daemon in process.
   It checks the exact Cube, descriptor, local data root, account, provider
   and token before a deposit check, and stores only a weak reference plus the
   fork tab's revocable session generation. Closing or changing that tab
   invalidates an in-flight reservation or preview, including a result waiting
   to be delivered to the Bitcoin UI.
   The binding also carries a weak reference to the exact paired BTCB2 wallet.
   Deposit review refuses a dropped wallet, a changed chain ID or a changed
   descriptor; this prepares the separate BTCB2 signer without granting it
   signing authority. After both proofs pass, it asks both daemons for durable change indices and
   constructs a read-only exact PSBT preview. The separate legacy
   remote-wallet backend still cannot atomically reserve change indices.
   The preview is not signer admission and is discarded after 30 seconds.
2. **Owned Claim transactions.** `create_ancestry_self_transfer` makes a
   one-output Bitcoin self-transfer with `B`.
   `create_ancestry_fork_sweep_with_marker` now constructs and exactly
   reconstructs the ordinary `S + F` fork transaction, requiring the
   designated `F` input. These builders establish structure and wallet
   ownership, not live chain exclusivity. Production discovery and signer
   dispatch remain gated in `claim_ancestry_gate`. Add a separately typed
   BTCB2 live proof and preserve exact unsigned-PSBT and finalized witness
   checks. The version-10 Claim journal now retains both *structural* paths,
   marker outpoints and exact unsigned transactions; paired restore rebuilds
   both against current owned coins while keeping signing and submission
   unavailable. An exact-pair create-or-reopen boundary now allows a future
   explicit signer transition to retry only the same unsigned transactions,
   raw paths, fee rates and destination. Deposit previews do not write this
   intent because fee quotes can change before the user chooses to sign.
   Next bind the live two-chain collector and reservations to this intent. On
   restart, recollect live proofs before sign, broadcast or
   completion. Keep legacy reconstruction and verification only for recorded
   intents and possibly broadcast transactions. Remove them after journal
   migration and reconciliation are complete.
3. **Foreign Split transactions.** The existing `foreign_split::SplitStep1`
   always writes OP_RETURN output 0 and treats every input as a shared claim.
   A separately typed ancestry construction now builds `S + B` with one
   ordinary destination output and an explicit `claimed_prevouts = S`.
   Its BTCB2 counterpart builds `S + F` and authenticates `F` against the
   foreign source descriptor even though its destination is the target Cube.
   Both ordinary Split builders now refuse a marker whose value is smaller
   than that chain's computed worst-case transaction fee. Claim's ordinary
   Bitcoin and BTCB2 builders apply the same fee-cover check to their selected
   chain-specific inputs, including when reconstructing recorded transactions.
   A review-only size estimator matches the exact constructed sizes across all
   supported foreign source shapes. The Split deposit screen now rescans the
   saved source and reads current fees on both chains before showing each
   minimum; it refuses the instructions when either quote is unavailable.
   This estimate must be recalculated from actual inputs at review. Claim's
   deposit screen now uses the Vault descriptor's maximum signed size for all
   scanned shared inputs plus one funding input, reads fresh fees on each
   chain, and shows separate minimum deposits. It refuses to release an
   address reservation when either quote is unavailable. The actual fee must
   still be recalculated from the constructed pair at review.
   Both rebuild exact PSBTs from fresh coins and the persisted fee rate,
   rejecting changed scripts, amounts, input sets, destinations and fees.
   These are structural builders only; the old OP_RETURN types remain for
   recorded legacy transactions. A version-11 Split journal now retains both
   unsigned constructions, funding paths, fee rates and target identity, and
   restart can rebuild both from current source coins. It remains unchecked;
   reconstruction does not restore chain proof, reservations or signer access.
   Split also has an exact-pair create-or-reopen boundary, refusing a changed
   transaction, funding path, source or target on retry.
   A read-only paired construction now takes the qualified funding result and
   builds both ordinary PSBTs together, then validates them against the paired
   observation. It requires the exact source receive index and target script
   supplied by their owners, and refuses if scan tips have changed. Journal
   creation now also requires the saved pre-deposit record and checks that the
   Bitcoin funding index, BTCB2 funding index and Bitcoin return index match
   its three distinct source addresses and Connect identity. The journal also
   requires the exact BTCB2 target descriptor, receive index and script from
   the saved target reservation; a different destination is refused. It still does
   not grant live proof or signer access.
   The Connect preflight reader now offers a paired operation for finalized
   ordinary transactions. It requires one common input set with exactly one
   distinct funding input on each chain and positive, fresh policy acceptance
   for both exact signed transactions under one Connect generation; one-sided
   acceptance returns no paired evidence. The coordinator still must bind this
   read to both typed live ancestry proofs, wallet reservations and finalized
   signature checks before submission. Keep the unified signature fallback
   separate and labeled with its actual replay properties.
4. **Both desktop wizards.** Lead with the two deposits and show separate
   Bitcoin and BTCB2 receive addresses, exact chain labels, expected minimum
   funding for each fee and independent confirmation/proof progress. Clearly
   say to send only Bitcoin to the Bitcoin address and only BTCB2 to the BTCB2
   address. Do not reuse an address across chains as evidence of exclusivity.
   After both checks pass, show each step's shared coins, marker input, fee,
   full destination address and signing threshold before the usual signer
   screen. The final review must show the exact destination and broadcast
   action. If a deposit is too small, unconfirmed, spent, entangled, outside
   scanner coverage or cannot be traced to a chain-specific coinbase, explain
   that state without enabling Sign. Do not call the deposit itself a split.
   The foreign scan now identifies three distinct receive indices proved unused
   within both chains' covered ranges: Bitcoin deposit, BTCB2 deposit, and
   Bitcoin self-transfer return. These are read-only hints, not reservations;
   the wizard must durably reserve the exact addresses and recheck their
   history before displaying them. A scan with room for only one fresh address
   cannot start the two-deposit flow. An immutable private pre-deposit record
   now persists the source descriptor, target Cube, Connect account/provider
   and those three indices under the Split journal lock. Restart reloads the
   exact same addresses and refuses a changed selection or identity. This
   record alone does not prove freshness or authorize a spend; the wizard still
   needs to recheck both chains' address history before showing it. A read-only
   pre-display check now reloads that record, binds it to the current Connect
   identity, and checks all three addresses on both chains under one timeout;
   any used address or failed read refuses the instructions. The production
   Split's production "Start split" now stores this reservation under the
   source digest, performs all six two-chain address-history reads off the UI
   thread, and shows distinct Bitcoin and BTCB2 deposit addresses only when
   the check succeeds. It then durably marks the same reservation released;
   a later retry with a matching source, target and Connect identity reuses
   those exact addresses after fresh authenticated reads on both chains even
   when deposits have created history, while a
   first display still refuses any already-used address. The old route picker
   is bypassed for fresh production
   splits and cannot open a retired signer path. The paired Split funding
   screen now gives both deposit addresses adjacent copy controls with the
   same clipboard confirmation as Claim. The paired Split funding
   search now filters out unrelated post-fork outputs at other receive
   indices. The production Split deposit screen now offers a read-only
   "Check deposits" action: it rescans the saved source descriptors on both
   chains, reloads the exact released reservation, reports whether each
   reserved address has a confirmed post-fork candidate, and attempts the
   bounded paired ancestry proof. Once proof succeeds, it builds both ordinary
   PSBTs at current per-chain fee rates and derives a read-only coin,
   destination and fee preview from their exact inputs and outputs. It discards
   the proof and transactions after displaying the preview and cannot open
   Sign. It refreshes both chain tips and verifies the proof age after building
   the preview, refusing a changed chain view or expired observation. Before
   the read-only proof search, the deposit check now excludes
   individual coins at either reserved address that are below that chain's
   current estimated fee. This lets a later sufficient deposit be considered
   without rotating the reserved address; the exact builder still enforces
   the fee bound. The check also distinguishes a shared ancestry root,
   reorganization and unavailable fresh reads in its retry guidance.
   Successful Split previews and Claim proof statuses expire after 30 seconds
   in the desktop, with older timers unable to clear newer checks. The user
   must check both deposits again for a current display. This display limit
   does not replace live preflight at signing and broadcast.
   Both panels now retain the exact ordinary PSBT pair with the selected
   funding proof for that short review window, and suppress a verified coin
   review if it no longer matches the retained pair. Expiry, cancellation or
   session revocation discards the pair. This is transient handoff state only;
   the signer must recollect proof and wallet state before each dispatch.
   Exact per-chain proof failure states, proof refresh and ordinary
   signer admission remain to be wired. Claim now independently reserves
   two distinct receive indices from its paired Cube scan in a private journal
   bound to both Cubes, the Vault descriptor and the Connect identity. Its
   pre-display check reloads that exact record and requires fresh unused
   address history on both chains for both addresses. A preparation step also
   checks the Bitcoin Vault descriptor and advances its receive index through
   both reserved addresses before returning the deposit instructions, so its
   daemon can discover incoming funds. The production Claim preconditions now
   offer a bounded two-chain Vault scan, persist that reservation and prepare
   the Bitcoin daemon's receive range before displaying the two copyable
   deposit addresses. Stale results and session revocations withdraw them;
   this stage cannot sign. A separate "Check deposits" action now reloads the
   same reservation, reconciles a new paired scan with the full Bitcoin Vault
   UTXO set, and runs the bounded two-chain ancestry collector; it displays
   only a transient status and does not retain spend authority. Its status
   distinguishes candidate presence at the Bitcoin and BTCB2 reserved addresses
   from a successful paired ancestry proof. The read-only Claim check now
   refreshes both fee quotes and filters each reserved address's candidates
   by that chain's current estimated fee, so a later sufficient deposit can
   be considered after a smaller one. An unreleased reservation cannot
   enter discovery. Ordinary transaction admission remains. Claim now durably marks
   its saved reservation released only after both addresses were checked
   unused and the Bitcoin daemon accepted their receive indices. Reopening
   that same Claim reservation checks identity and wallet state again, then
   displays the exact addresses after fresh authenticated history reads even
   if deposits have since made their
   history nonempty; it never chooses replacements on retry.
   Coincubed `getinfo` now reports an explicit chain ID. The desktop accepts
   an authenticated `chain_id` on a legacy remote-wallet record, checks its
   address network, and keeps an omitted field unknown. The Claim deposit probe and
   read-only checks now admit a Connect-backed Bitcoin Vault when its wallet
   record positively identifies the Bitcoin chain, matches the Vault
   descriptor and reports a ready wallet state; they no longer require the old
   embedded-daemon submission adapter. That remote wallet API does not yet
   supply `chain_id`, so Claim address preparation through it still refuses.
   COINCUBE | Connect instead uses local Coincubed with an authenticated
   Esplora backend and has a positive chain ID. The preparation path advances
   a remote-wallet-backed Bitcoin Vault's receive branch with durable one-at-a-time new-address
   requests, verifies every returned derivation, then reads back both exact
   reserved addresses and repeats the two-chain unused-history check before
   releasing the journal record. It refuses an
   advance of more than 256 indices in one attempt. Connect must return the
   positive identity
   before those deposits are usable. Creation of a version-10
   paired Claim intent now requires that saved record and checks both funding
   derivations against its exact external indices under the journal lock. The
   paired Claim discovery now searches only funding outputs at the two
   reserved receive indices, so a different post-fork Vault coin cannot
   silently replace a planned deposit. Before that search it reads the
   Bitcoin Vault daemon's unfiltered coin set and compares every current
   pre-fork coin, amount, derivation, address and confirmation height to the
   paired scan. It also requires the reserved Bitcoin funding output to be
   present and unspent in that wallet. A missing coin, incomplete address
   coverage, unsynced wallet or changed spend state refuses review. The Claim
   daemon comparison now uses the explicit `getinfo` chain identity, so an
   external Coincubed daemon can pass the same positive check as an embedded
   one; an older or unbound remote response cannot.
   The paired BTCB2 daemon now undergoes the same complete-wallet
   reconciliation before Claim preview: its explicit chain ID, descriptor,
   sync and address coverage must agree, every shared coin must still be
   owned and unspent at its BTCB2 block height, and the reserved BTCB2
   funding coin must match the fork scan. After reserving both change
   branches and constructing the preview PSBTs, Claim rereads both complete
   wallet coin sets; a spend or new shared coin during construction withdraws
   the preview. A reusable paired snapshot operation also verifies that the
   collected proof still matches both exact transactions, refreshes both chain
   tips after the wallet reads, and checks the proof's age again before the
   preview is labeled verified. Signer admission must recollect the live proof
   and rerun this operation under the saved reservation; a preview result is
   never signing authority.
   A separate signer-prerequisite operation now reloads that reservation,
   selects the exact funding outpoints from the retained pair, recollects
   both-chain ancestry and shared-input evidence, rebuilds both PSBTs with
   the saved change indices and rates, compares all PSBT fields, rereads both
   wallets and tips, then checks the journal and Connect generation again.
   A production preparation function can invoke it after a new bounded
   two-chain Vault scan, require the current fee quotes to match the retained
   pair, verify the currently open BTCB2 Vault and create or reopen only the
   exact version-10 intent. The dormant Claim signer transition now calls it
   before opening either PSBT; each device dispatch and file export/import
   repeats it. It does not authorize a send.
   Claim and Split requalification now return distinct typed results that
   retain the exact checked PSBT pair. Their journal finalizers require that
   result, so a raw observation alone cannot be used as a substitute for the
   complete-wallet or foreign-source and reservation checks. Claim also has a
   ten-second, one-use signer permit bound to the exact current PSBT, chain and
   Connect generation; the ordinary picker can consume it while keeping its
   own broadcast actions closed. The paired Claim panel consumes this permit
   for its signer requests. Its entry action is behind the production
   acceptance gate while submission remains incomplete.
   Split's requalification now also retains the exact source coin set, session,
   tips and PSBTs long enough to issue the same one-use permit after checking
   both constructions and the current partial signatures. A new core combiner
   authenticates independent Bitcoin and BTCB2 Split signer returns for every
   supported source descriptor, rejects altered returns and only reports the
   pair complete when both thresholds are met. Neither capability is yet
   connected to the Split signer UI or a submission action. The production
   Split adapter now returns that typed fresh check after recording the exact
   version-11 intent, rather than discarding it; a UI caller must still repeat
   the adapter call for each signer dispatch and consume the one-use permit.
   The check carries the production Connect generation receiver itself, so a
   future UI caller cannot substitute a synthetic session when asking for it.
   The paired Split PSBT file importer now uses the same strict core combiner,
   so two valid returns for the same key with conflicting signatures cannot
   be merged by silently keeping whichever file was read first.
   The production Split adapter now also has a finalization boundary for both
   signed ordinary PSBTs: it rescans and requalifies the exact pair, checks
   each signer return, reopens the version-11 journal, verifies both complete
   signed transactions against the live proof and atomically records both
   witnesses. The Split panel now opens a paired file-signing stage only after
   a fresh requalification creates or reopens the exact intent. Each unsigned
   file export repeats that check and consumes a short-lived, one-use permit;
   each imported signer file is combined under another fresh paired scan.
   Connected-device signing now has distinct Bitcoin and BTCB2 actions: the
   panel builds its listing from the source policy, repeats the same live
   requalification, consumes a one-use permit before opening the device, and
   sends returned PSBTs through the same strict combiner as files. Once both
   sides are complete, the panel invokes the finalization boundary and
   reopens read-only recovery. Separate Bitcoin and BTCB2 broadcast reviews
   now repeat paired preflight and use the durable submission coordinator;
   production submission remains gated, and device support still needs
   independent hardware smoke tests.
   The production Split session check now rejects an unread Connect session
   notification even if it carries the same numeric generation; signer and
   reservation calls cannot continue under that replaced session.
   The production Split and Claim signer entries and their buttons are now
   closed by the shared ancestry acceptance gate; a verified deposit preview
   alone cannot expose spendable PSBTs. Both staged paired submission methods
   also check that gate before a preflight read or possible-send journal write.
   The signing review repeats the source threshold and the exact output
   amount beside each full destination, so a signer can compare both sides
   without relying on the earlier deposit preview.
   The Cube launcher now discovers a funding-only Split record, which has no
   transaction intent yet. Reopening under Connect validates its source
   digest, target Cube and account, then repeats both-chain address checks
   before showing the same deposit addresses. A resumed deposit screen can
   run the same read-only "Check deposits" scan.
   New Split deposit records also bind the authenticated target BTCB2 Vault
   ID, descriptor and reserved receive script under the same journal lock.
   Reopening or checking deposits rejects a different loaded target Vault.
   An unsigned version-11 Split intent now reopens at its saved deposit
   addresses after rechecking the original source, target Vault, Connect
   account, both-chain address histories and fee quotes. A recorded signed
   pair still opens read-only recovery; reopening never restores signer
   authority from the journal.
   After Bitcoin confirms, Claim now reconstructs the spent Bitcoin-only
   input from its retained path and the shared inputs from the current BTCB2
   scan. The collector's canonical Bitcoin blocks must match the BTCB2
   confirming blocks and all recovered scripts must belong to the Claim Vault.
   Its stage-two proof entry rebuilds both exact version-10 PSBTs and verifies
   both saved signed witnesses before using the post-Bitcoin observation.
   This remains a gated evidence path, not a signer or broadcast action.
   The target daemon must positively identify as BTCB2 when reserving its
   destination; mainnet address encoding alone cannot distinguish the two
   chains. An older Connect wallet response without an authenticated chain
   ID is therefore refused until Connect supplies it.
   The Claim wizard now calls the paired transaction builder for a read-only
   preview once the BTCB2 Cube is open and both wallet change branches have
   been reserved. Claim and Split previews now show the source wallet's
   signing threshold, each verified deposit's current confirmation count, the
   full destination and exact output value on each chain. They do not hand
   the pair to signers.
5. **Retire the old route now.** New builds, signing, first submission and
   resubmission of the 90-byte OP_RETURN transaction are disabled before the
   replacement goes live. The old one-sided Claim signer, Bitcoin handoff and
   completion path are now test-fixture-only or permanently denied in
   production, independent of the paired ancestry gate. New OP_RETURN Claim
   and Split journal creation, along with the old seed-signed one-chain Split
   fallback, are refused by the production workflow service even if a caller
   bypasses the desktop route. Existing records retain their recovery readers.
   Reopening an unsubmitted legacy one-chain Split draft now refuses its seed
   signer and directs the user to start the two-deposit flow; a possibly sent
   record still enters read-only reconciliation.
   The daemon's old exact-byte OP_RETURN, one-sided ancestry Claim, Split and
   unified-sweep transport methods now return `RetiredRoute` in production
   before acquiring a backend or consuming a one-use gate. Their unit
   fixtures remain executable; the ordinary paired ancestry transport is
   separate.
   Enabling the paired gate later
   cannot turn the old path back on. Managed Bitcoin node setup and startup no
   longer add the old claim's 100-byte `datacarriersize` relay override;
   operator-specified values in existing configurations still round-trip.
   The active-fork Connect anchor and the desktop ancestry and fork-only
   collectors no longer require RDTS metadata; historical OP_RETURN record
   interpretation still retains its own RDTS checks. Next remove `split_poison`
   payload building, `Poison::OpReturn` production selection, OP_RETURN review
   details and the transport path for
   newly created claims. Migrate or explicitly refuse any persisted,
   unbroadcast OP_RETURN intent; a possibly broadcast old intent must remain
   recoverable and reconcilable by exact txid. No historical journal is
   silently reinterpreted as ancestry. Update the in-repo workflow and safety
   docs, which currently describe the ancestry path as unsupported.

## Wizard coin review

Use the Send screen's coin-row hierarchy as a visual reference: a compact
label, status badge and right-aligned amount, with the full outpoint available
on expansion. Do **not** reuse its coin-selection checkbox or its "Expires in"
badge. The split builder determines the required inputs; the review is
read-only and must be generated from the exact PSBT inputs being signed.

- Show the shared pre-fork coins together, with a subtotal and a clear
  "Before fork · present on both chains" label. Every shared coin in this
  attempt appears in both transaction reviews.
- Show a Bitcoin funding coin under the Bitcoin transaction and a BTCB2
  funding coin under the BTCB2 transaction. Before proof, label each
  "Post-fork candidate · checking"; only a fresh, complete two-chain proof
  earns "Bitcoin-only · verified" or "BTCB2-only · verified". Amount,
  confirmations and outpoint remain visible. A missing or invalid marker
  keeps Sign unavailable.
- At each signing review, display a compact equation such as "2 shared coins
  + 1 Bitcoin-only coin → your source wallet" (and the corresponding BTCB2
  version with its actual destination), followed by the full address and fee.
  State that these inputs must be spent together to separate the chains.
  There is no "select all" action, unchecked row, or way to omit the marker.
- Keep unrelated post-fork coins outside the transaction review. If useful,
  show a separate "Not part of this split" count, without implying that all
  post-fork coins are automatically safe or that the split covers unrelated
  wallet activity.

The reusable read-only `CoinReview`/`CoinPairReview` model now derives shared
and funding rows, subtotals, destination and fees from the exact unsigned
PSBTs. It refuses missing amounts, duplicate or omitted inputs, mismatched
shared coins across chains (including different scripts at the same amount)
and OP_RETURN outputs. The recorded Claim step-one
review uses its Bitcoin side and labels the marker as an unverified candidate.
The Split and Claim deposit stages now consume the paired model only after fresh
two-chain proof, source ownership, bound reservations and construction of
both ordinary PSBTs. The model alone
cannot enable signing. Both deposit previews now render the same read-only coin
rows with full scrollable outpoints, right-aligned amounts and a confirmation
badge for each verified chain-specific input; no selection control is shown.
Both signing stages also show the shared-coin count plus the required
chain-specific input for each exact PSBT, immediately before its destination
and fee details. The headless two-chain wizard cases assert those labels.

Build this as a reusable *read-only* coin summary for Claim and foreign Split,
using the existing amount formatting and badge components. Keep the normal
Send selector unchanged. Test the row mapping against the actual unsigned PSBT
and re-render it after any fee, tip or proof change so stale input labels cannot
survive a rebuilt transaction.

## Acceptance checks before enabling signing

- Core unit tests prove exact `S + B` and `S + F` input sets, one owned output,
  no OP_RETURN, descriptor ownership, fee bounds and ordinary hardware PSBT
  metadata across all supported source shapes. Missing or duplicate markers,
  unproven paths and mixed-chain inputs must refuse.
- A two-chain regtest uses distinct post-fork coinbase ancestors, signs both
  transactions with ordinary ECDSA, accepts each on its intended chain and
  rejects the exact signed bytes on the other chain. Repeat after restart,
  stale observations, marker spend, shallow/deep reorg and step-order changes.

The paired Claim consensus vector passes against the pinned Knots and electrs
two-chain harness, including BTCB2-first broadcast and one-block and six-block
reorgs of either confirmation. Two more vectors spend only the Bitcoin or
BTCB2 funding input after pair signing and confirm that the saved transaction
on that chain is rejected while the other chain's input remains unspent. A
collector regression also refuses a fresh paired proof when that Bitcoin
funding output has become spent. The disposable two-chain Claim harness now
checks either spent funding output through the live authenticated Connect/Esplora
bridge after both indexers observe the spend, and refuses a stale outspend
response. Coordinator restart and full wizard-level marker-spend admission
cases remain.
   The foreign Split bridge now builds and finalizes an ordinary `S+B`/`S+F`
   pair from distinct coinbase dependencies. All five supported foreign source
   shapes pass the offline signed-PSBT vectors and the pinned two-chain regtest.
   Core also has typed pair finalizers for both Claim and Split. They require
   the same shared input set, distinct chain-specific funding inputs, ordinary
   outputs and valid signatures on both exact PSBTs before returning a pair.
   Both pairs now also have pre-signing validators for incomplete PSBTs. They
   bind the two constructions and reject changes to transaction outputs,
   prevout metadata or sighash policy after a signer round trip. These checks
   do not replace live two-chain proof at each signer dispatch.
   The desktop Split PSBT file service now has a paired importer for ordinary
   `S+B` and `S+F` constructions. It cryptographically checks each returned
   partial signature against its exact side, combines multisig returns, and
   marks the pair complete only when both sides finalize. It still needs the
   production signer UI and fresh admission at every export or device call.
   Claim now has the equivalent typed paired signing-result combiner. It
   authenticates every Bitcoin and BTCB2 signature against the exact owned
   construction, refuses changed metadata or finalized signer returns, merges
   partial signatures and reports completion only when both ordinary
   transactions finalize. The dormant Claim signer panel now checks each
   returned PSBT with the combiner before merging, requalifies both wallets
   and chains after both sides complete, and atomically records the finalized
   witnesses. Late signer results are scoped to the exact panel session.
   A focused ordinary-picker test now sends a local request through each
   chain's exact one-use permit, then refuses a second request, a revoked
   session and a changed PSBT. It does not prove the full wizard, physical
   device approval or restart.
   This grants no spend authority or broadcast action.
   Restart of an unsigned version-10 intent now reloads its fixed deposit
   addresses and current fee quote, then requires a new deposit check before
   reopening the exact pair for signing. A signed intent still opens the
   per-chain recovery view.
   The version-10 Claim and version-11 Split journals now expose a review-only
   path that first validates a fresh paired funding collection against the
   exact saved intent and then invokes the corresponding pair finalizer.
   Connect now has separate exact-byte, one-attempt Bitcoin and BTCB2 mainnet
   broadcast transports for either typed pair. Each side has its own
   transaction-bound, expiring gate and uncertain outcome. The Claim and Split
   wizards now have a gated first-Bitcoin broadcast review. It reconstructs
   the exact recorded signed pair from a new scan, repeats live two-chain
   requalification, shows both finalized transactions and fees, and requires
   an explicit confirmation. Confirmation repeats the entire check and
   compares the displayed bytes and fees before paired Connect preflight,
   durable possible-attempt recording and the one-use Bitcoin send. The
   production gate remains closed; this route is not yet available to users.
   The separate BTCB2 broadcast review is also wired. It requires a fresh
   six-confirmation Bitcoin observation, repeats the paired scan and signer
   requalification, and compares the newly verified signed bytes and fees to
   the displayed review before its one-use BTCB2 submission.
   Gate constructors now accept the finalized Claim or Split pair directly
   and bind separately to its Bitcoin or BTCB2 witness; the coordinator must
   still issue them only after the corresponding durable possible-attempt
   record and one-use user approval. A successful possible-attempt journal
   write now returns a non-restorable, one-use token that can create only the
   matching finalized pair's chain gate. No token is returned on a failed
   write, and a reopened journal still has no transport authority.
   Paired Connect preflight also accepts the typed, signature-verified Claim
   or Split pair directly, observes both exact witnesses under one session,
   and refuses non-mainnet pairs; it remains read-only evidence.
   Version-10 Claim and version-11 Split intents can now atomically retain the
   two finalized signed transactions, including each txid and wtxid, before
   any network attempt. Journal validation requires their unsigned forms to
   match both saved templates exactly. Restart restores the bytes for
   reconciliation but leaves the controller unchecked and does not permit
   signing or broadcast. Separate possible-submission records now bind each
   chain's exact txid, wtxid and short-lived Connect preflight before any
   transport call. The initial Bitcoin record requires both signed witnesses
   to pass paired preflight. The later fork record requires a new preflight of
   the exact BTCB2 witness; the Bitcoin transaction may be mined by then.
   Typed Claim and Split preflight readers now make only that BTCB2 request
   after the Bitcoin step and refuse a rejected policy result. They do not
   themselves establish Bitcoin confirmation, live ancestry or spend authority.
   These records cannot be cleared or repeated through the journal; the fork
   record is sequenced after the Bitcoin record. Neither record
   establishes upstream acceptance or unlocks a send. The paired coordinator
   must still require a fresh six-confirmation Bitcoin observation before
   recording the BTCB2 attempt. A read-only recovery collector now checks
   each possible attempt independently against fresh transaction status,
   exact signed witness bytes, canonical block mapping and stable chain tip.
   Absent, stale, mismatched or unavailable observations never permit an
   automatic retry. The Bitcoin Claim wizard now reopens a version-10 journal
   through a separate recovery screen, checks each recorded possible
   send against its own chain, and shows exact transaction IDs and independent
   statuses. It does not re-enable signing or retry an absent transaction.
   The foreign Split wizard now also diverts version-11 journals away from
   the legacy step-one/step-two restart routes. Its recovery screen
   checks the exact saved possible sends independently under the current
   Connect identity. Both screens can offer the separate gated first-send
   review only while the journal records no Bitcoin attempt; an attempted
   transaction remains status-only. The
   fork-attempt journal now also requires an opaque,
   short-lived Bitcoin six-confirmation result bound to the exact signed
   witness and session; the coordinator must still recollect live two-chain
   ancestry and obtain explicit approval before either send. Acceptance on
   one chain never implies acceptance on the other.
   The initial collector requires the shared and Bitcoin-only inputs to be
   unspent on Bitcoin. That condition becomes false once the exact Bitcoin
   transaction confirms, so the fork stage needs a distinct collector that
   verifies the canonical Bitcoin spend and requires the shared and BTCB2-only
   inputs to remain unspent on BTCB2. Reusing the initial collector would make
   a successful second step impossible.
   A separate read-only collector now requalifies both ancestry roots, checks
   the exact Bitcoin witness in a canonical block at six confirmations,
   requires the Bitcoin funding and shared outputs spent there, and requires
   the BTCB2 funding and shared outputs unspent under stable two-chain tips.
   Its distinct result type cannot enter the initial signer validator. It
   still needs wallet ownership, reservation, finalized fork signature and
   fresh policy checks in the stage-two coordinator.
   The post-Bitcoin proof boundary now also requires the reconstructed BTCB2
   PSBT: it compares every shared and BTCB2-only input's saved amount and
   script with the newly observed source transaction, and requires that PSBT's
   unsigned transaction to match the exact signed journal record. This does
   not itself reconcile the current source wallet or its reservation. A
   separate Claim wallet snapshot check now compares the full BTCB2 Vault coin
   set with a fresh paired scan after Bitcoin spends: every claimed pre-fork
   output must be absent from Bitcoin's scanned UTXOs, present and unspent in
   the BTCB2 Vault, and the exact reserved BTCB2 funding output must still be
   confirmed and unspent. That snapshot alone is not spend authority.
   A Claim stage-two requalification now joins the read-only parts: it reloads
   the saved funding and change reservation, compares the confirmed Bitcoin
   witness to the supplied construction, recollects both ancestry roots and
   current input states, validates the BTCB2 PSBT prevouts, reads the complete
   BTCB2 Vault, and brackets the check with current two-chain tips. It returns
   expiring observations only. The coordinator still must bind the exact
   signed journal, currently open paired Vault, finalized BTCB2 signature,
   fresh policy result and explicit review before sending.
   Foreign Split now has an equivalent read-only post-Bitcoin source-scan
   recheck: the exact shared set must have left Bitcoin while remaining
   confirmed and unspent on BTCB2, and the exact reserved BTCB2 funding coin
   must remain there with its source descriptor derivation. It also binds the
   scan generation and both current tips. It remains a separate read-only
   check rather than spend authority.
   A separate Split stage-two requalification now joins those read-only
   checks: it reloads the exact released source and target reservation,
   compares the confirmed Bitcoin witness to the supplied exact Bitcoin
   construction,
   recollects the canonical post-Bitcoin input evidence, checks the BTCB2
   PSBT prevouts, and compares the refreshed tips with the new source scan.
   It returns expiring observations only. The coordinator still needs to
   bind the pair to the signed journal, validate the finalized BTCB2 witness
   and live target Vault, obtain fresh BTCB2 policy, journal the attempt, and
   enforce explicit broadcast review.
   This requalification now also recovers the exact source-owned BTCB2 input
   transactions from the fresh post-Bitcoin scan. Each shared input's block
   must equal the collector's canonical pre-fork Bitcoin block, and the BTCB2
   funding input must match the scan's joined candidate. The recovered coins
   remain construction data; no signer or send path consumes them yet. A
   separate version-11 journal reader can now reconstruct only the recorded
   BTCB2 PSBT from such coins, since the Bitcoin inputs have left the current
   UTXO scan. The stage-two collector also retains the spent Bitcoin-only
   deposit's exact previous transaction and canonical confirming block; after
   validating the signed pair, it binds that transaction to the saved path and
   source derivation for whole-pair reconstruction. Neither path grants spend
   permission; the coordinator must still join the signed journal, current
   reservations, finalized BTCB2 witness and policy before review.
   The Split stage-two proof entry now requires that joined step: the expiring
   requalification rebuilds both exact version-11 constructions from the
   recovered coins and retained paths, then cryptographically reverifies the
   saved Bitcoin and BTCB2 witnesses. Only that reverified pair can accompany
   the post-Bitcoin proof to a future review. The production wizard still does
   not call this entry or expose a broadcast action.
   The version-10 Claim and version-11 Split journal proof entries now accept
   those distinct requalification result types rather than a raw
   post-Bitcoin observation. Their shared private check binds the saved exact
   signed pair, canonical six-confirmation token, current context and BTCB2
   PSBT. The resulting proof still expires and cannot enter transport by itself.
   The Claim and Split pair finalizers now issue an opaque, controller-bound
   initial proof only after validating the exact signed pair against the saved
   intent and fresh two-chain collection. Recording a possible Bitcoin send
   requires that proof, matching preflight tip hashes, context and witnesses,
   and the original observation-age limit (capped at 30 seconds). A separate
   post-Bitcoin proof is issued only after the stage-two collector agrees with
   the saved witnesses, exact canonical Bitcoin confirmation block, input set
   and current two-chain tips. The six-confirmation token now retains the
   Bitcoin tip under which its block was read. Recording the possible BTCB2
   send requires this new proof, fresh fork policy and the *same* confirmation
   block and Bitcoin tip token, so a changed chain view between collection and
   submission forces a new read.
   Neither proof survives restart or grants transport access by itself.
   These artifacts are cryptographic evidence only; the coordinator still must
   bind them to the saved intent, fresh two-chain proof, reservations and paired
   Connect preflight before either transaction can be submitted.
   Core can now reverify both retained signed Claim or Split witnesses as one
   typed ordinary pair against freshly rebuilt constructions after restart.
   A workflow reader checks the exact saved unsigned templates and context
   before returning that read-only pair. Neither path restores live proof or
   send authority; the stage-two coordinator must still invoke it after
   rebuilding source-owned inputs and refreshing both chains.
   A staged first-send review now holds the reverified typed pair, an opaque
   fresh two-chain proof, the exact Bitcoin and BTCB2 witnesses and their
   fees. On explicit confirmation it reads positive Connect policy for both
   exact witnesses from the same provider, persists the Bitcoin possible-send
   record, then enters a one-use exact-byte gate. Once the record exists,
   cancellation, timeout or any transport error is uncertain and never
   automatically retried. A separate staged BTCB2 review requires the saved
   Bitcoin attempt, the exact signed pair, a current six-confirmation Bitcoin
   token and an opaque post-Bitcoin two-chain proof. On explicit confirmation
   it rereads BTCB2 policy, journals the possible BTCB2 send and enters a
   separate one-use exact-byte gate. Either review refuses a Connect origin
   that differs from the authenticated proof provider before recording an
   attempt. The first-Bitcoin review and confirmation are now wired in both
   wizards behind the closed acceptance gate. The post-Bitcoin coordinator
   must still join fresh source and target checks to its BTCB2 review, and
   both paths need GUI and end-to-end tests.
   Paired preflight and both one-use submission boundaries now also revoke on
   any Connect session notification, including a same-value generation
   refresh. An interrupted attempt after its durable possible-send record is
   reported as uncertain and is never retried automatically.
   Initial Claim and Split requalification now each have a typed handoff that
   rechecks the exact saved signatures and prepares a first-send review under
   the same live controller. The desktop invokes that handoff only after a
   new paired scan and shows an explicit broadcast review; loading signed
   bytes from disk alone remains read-only.
   An unattempted signed journal now exposes only its two saved funding
   outpoints and fee rates for that fresh scan. The hint is unavailable after
   either possible-send record; it does not restore proof or send authority.
   Split can use those exact outpoints to reconstruct its saved ordinary pair
   from a new two-chain source scan. Reconstruction remains unchecked; the
   wizard must repeat live ancestry, reservation and policy checks before a
   broadcast review.
   The production Split adapter now wraps that reconstruction with journal
   identity and Connect-generation checks, releasing its journal lock during
   the network scan and rechecking the saved plan when it reopens. It is not
   alone broadcast authority.
   Claim has the matching structural restore from a new paired Vault scan:
   it selects the exact saved Bitcoin and BTCB2 funding outpoints, joins each
   with the freshly scanned shared set, and requires both saved unsigned
   templates to reconstruct. Full wallet reconciliation and live proof still
   have to follow.
   Its production adapter also reopens the exact journal before and after the
   network scan, checks the current paired BTCB2 Vault and Connect session,
   then returns only the structurally rebuilt pair for a later live review.
   Both production adapters can now take that rebuilt pair through a fresh
   exact-pair requalification and saved-witness re-verification under one live
   controller, producing a short-lived first-send review. Both wizards call
   the review on request and repeat it on confirmation; only the latter may
   run paired preflight and a one-use Bitcoin broadcast. The production gate
   is still closed.
   The corresponding stage-two Claim and Split handoffs now also join their
   recovered exact signed pair, new post-Bitcoin source check and canonical
   six-confirmation token to a one-use BTCB2 review in the same controller.
   The raw re-verification and proof producers are private to these handoffs;
   both wizards now have separate gated BTCB2 review and confirmation actions.
   Each action reconstructs the exact saved pair from a new two-chain scan,
   repeats post-Bitcoin wallet and ancestry checks, and independently checks
   six canonical confirmations for the recorded Bitcoin transaction. The
   confirmation action compares the newly verified signed bytes and fees to
   those shown in the review before the paired preflight and one-use BTCB2
   broadcast. Recovery screens only offer this review after a current
   six-confirmation status read; that display state grants no send authority.
   These paths are still unavailable while the production acceptance gate is
   closed, pending the remaining end-to-end and hardware acceptance checks.
   The Claim and Split signed-pair recovery screens now also display the exact
   one-output destination and amount decoded from each retained signed
   transaction, including immediately after Claim signs. These are read-only
   transaction facts; they do not restore approval or requalification.
The latter mines separate post-fork coinbases into the foreign source, tests
both signed transactions against each node, confirms BTCB2 first, and checks
that the BTCB2 transaction cannot replay on Bitcoin after a Bitcoin
confirmation is reorganized out. All five supported source shapes also reject
the opposite transaction after a six-block reorg of either chain.
The Split-only fixture needs the two pinned Knots binaries, not electrs.
The later paired GUI fixture covers signed-pair restart, stale observations
and spent funding inputs in the disposable two-chain environment.
The pinned BLAKE2b-aware electrs can panic while the disposable harness moves
the node tip across a 100-block coinbase reorg. The harness now stops it and
reopens the **same** index database after the new tip settles. Both chain cases
pass: the indexer reaches the replacement tip and the desktop collector then
refuses the obsolete funding proof. This establishes manual same-database
recovery in that fixture, not deployed automatic recovery from a deep reorg.
Keep the mainnet gate closed until that operational recovery is exercised
against the deployed node and indexer builds.
- Exercise Claim and Split through their actual GUI intents and signed-PSBT
  round trips. Include BitBox and Ledger integration smoke checks on real
  devices before a mainnet release; software-only tests cannot prove a device
  will display and approve the PSBT.
  The headless Claim fixture asserts that both retired one-sided routes refuse
  before construction and drives the new paired Claim wizard through deposit
  checking, signing, recovery and separate two-chain submission reviews.
  Its disposable Connect bridge now routes preflight and submission to the
  selected Bitcoin or BTCB2 node, with a local routing test for each chain.
  The paired funding fixture also asks the live collector to refuse a
  same-value Connect session notification and to resume only through a fresh
  collector before testing a spent marker on either chain.
  The headless paired Claim wizard now reserves separate deposit addresses,
  mines mature chain-specific funding coins to both addresses, checks both
  deposits through the actual panel, and imports both software-signed PSBTs
  through its signer dispatch before durably recording the exact pair. Its
  disposable bridge supplies separate fresh six-block fee quotes because a
  quiet regtest mempool cannot estimate a mainnet fee; production continues to
  refuse deposit preparation when either chain has no usable quote. The
  fixture configures the disposable fork boundary explicitly so the actual
  ancestry collector can qualify those funding coins; candidate presence
  alone never opens signing. The
  fixture also asks the broadcast review to reject a reported spent Bitcoin
  funding output, asserts the exact outpoint was read, and reopens a signed
  pair in a new panel process without restoring submission authority. The
  test acceptance gate opens only inside the disposable test-binary child; the
  production gate remains closed. The pinned two-chain CI run passed 106
  tests with one intentionally deselected on commit `a41ec0e8`, including
  this Claim wizard path, signed-pair restart, spent-input refusal and
  same-database indexer recovery after deep reorg.
  The headless paired Split wizard now reserves separate chain-specific
  addresses, checks mature deposits through the production Connect adapter,
  refuses to sign while the production gate is closed, and imports ordinary
  software-signed PSBTs for both sides. A new panel process reopens the exact
  signed pair without submission authority; the fixture checks that each
  signed transaction enters its own chain's mempool and is refused by the
  opposite chain. The pinned two-chain CI run passed this Split case and the
  paired Claim case (106 passed, 1 deselected overall) on commit `52147c26`.
  A Split signer-admission regression for a spent Bitcoin funding marker also
  passed in that pinned run. Test-only Claim and Split fixtures now drive
  separate Bitcoin and BTCB2 broadcast reviews across process restarts,
  including six Bitcoin confirmations, exact-byte node submission and
  per-chain attempt journaling. The pinned two-chain CI run on `2ecc15e9`
  passed both staged-broadcast fixtures (106 passed, 1 intentionally
  deselected overall), including separate process restarts, six canonical
  Bitcoin confirmations, exact-byte submission to each node and the matching
  per-chain attempt records.
  The `38ef5e59` pinned harness passed 106 tests with one intentionally
  deselected, including headless rendered-element assertions for both wizards:
  exact deposit addresses, reviewed input outpoints and amounts, return
  destinations, fees, recorded transaction IDs and broadcast labels. These
  assertions do not replace visual and accessibility review. Deployed
  recovery, realistic Connect scan latency, physical device round trips and
  independent review remain outstanding.
- Local live-testing update (2026-10-19): the owner explicitly requested
  testing with actual funds. Launching Tenshu with `--enable-btcb2-split`
  enables paired Claim and Split signing and both broadcast reviews for that
  process only, using the normal wallet directory. This is an opt-in for live
  testing, not evidence that hardware acceptance is complete. It does not
  persist to subsequent launches or enable the retired OP_RETURN route.
  Two-chain ancestry, confirmation, ownership, unspent-input, fee, session,
  exact-transaction and separate broadcast-confirmation checks remain active.
- Superseded (2026-10-29, Robert's instruction): the paired Claim and
  foreign-wallet Split wizards are removed from the desktop in favour of the
  per-chain Split coins feature, and its launch gate is gone. There is no
  `--enable-btcb2-split` flag; Split coins (ancestry checks and building a
  split) is available whenever Bitcoin Blake2b is on for the Cube, the same
  condition that shows the Chains column and Tools › Split coins. This
  replaces the #547 acceptance gate below for the desktop.
- Keep the default production gate closed until these checks and an independent review
  pass. Then enable discovery, signing and durable completion together. Confirm
  Connect serves the block, transaction, UTXO and tip endpoints needed for
  both chain-specific proof collectors under one account session. Measure the
  bounded address scan through the production Connect proxy with a realistic
  Vault history: it repeats fresh reads within batches of at most eight
  addresses under a 30-second deadline. A passing loopback regtest is not
  proof that a remote user's deposit check will finish in time.

This is a wallet migration, not just a copy change. A user's existing shared
coins should remain untouched until both independent marker proofs are ready.
