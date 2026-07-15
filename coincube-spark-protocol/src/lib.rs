//! JSON-RPC-ish protocol spoken between `coincube-gui` and `coincube-spark-bridge`.
//!
//! Both sides link this crate; the bridge binary depends on
//! `breez-sdk-spark`, `coincube-gui` does not. The separation exists because
//! the Liquid SDK's dependency graph (`rusqlite`/`libsqlite3-sys`,
//! `tokio_with_wasm`, `reqwest`) is incompatible with the Spark SDK's graph
//! at the `links = "sqlite3"` level, so the two SDKs can't live in the same
//! binary. Running Spark in a sibling process isolates them permanently.
//!
//! The wire format is one JSON object per line (newline-delimited JSON):
//!
//! - [`Request`] messages flow gui → bridge.
//! - [`Response`] messages flow bridge → gui, correlated by `id`.
//! - [`Event`] messages flow bridge → gui unsolicited (SDK event stream).
//!
//! Framing choice: line-delimited JSON is cheap, easy to debug by hand, and
//! works over anonymous stdio pipes without needing length prefixes or
//! framing protocols. The envelope enums are tagged with `serde(tag = ...)`
//! so `{"kind": "get_info", ...}` round-trips through `serde_json::from_str`.

use serde::{Deserialize, Serialize};

/// A request from the gui to the bridge. `id` is echoed back in the matching
/// [`Response`] so the client can correlate concurrent in-flight calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub method: Method,
}

/// The RPC methods the bridge understands.
///
/// Phase 2 shipped Init / GetInfo / ListPayments / Shutdown. Phase 4c
/// added the Send + Receive write path (`prepare_send_payment`,
/// `send_payment`, `receive_payment`). Phase 4e adds:
/// - [`Method::ParseInput`] — generic input classifier so the gui can
///   route BOLT11/on-chain inputs to [`Method::PrepareSend`] and
///   LNURL/Lightning-address inputs to [`Method::PrepareLnurlPay`].
/// - [`Method::PrepareLnurlPay`] — the LNURL analog of `PrepareSend`.
///   Internally the bridge parses, fetches the invoice from the LNURL
///   callback, and wraps the resulting BOLT11 prepare response. The
///   returned handle is routed by [`Method::SendPayment`] — same RPC
///   name as the regular send, handled transparently based on which
///   pending map the handle lives in.
///
/// On-chain claim lifecycle, fee tier picker, and pending-prepare TTL
/// cleanup remain deferred to Phase 4f.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Method {
    /// Connect to Spark mainnet with the given credentials and storage
    /// directory. Must be the first request — all others return
    /// [`ErrorKind::NotConnected`] until init succeeds.
    Init(InitParams),
    /// Fetch wallet info (balance, pubkey).
    GetInfo(GetInfoParams),
    /// Fetch recent payments with optional limit/offset pagination.
    ListPayments(ListPaymentsParams),
    /// Classify an arbitrary destination string — BOLT11, BIP21,
    /// on-chain address, LNURL, Lightning Address, etc. Used by the
    /// Send panel to decide between [`Method::PrepareSend`] (regular
    /// SDK prepare) and [`Method::PrepareLnurlPay`] (LNURL code path).
    ParseInput(ParseInputParams),
    /// Parse a BOLT11 invoice / BIP21 URI / on-chain address and return
    /// a [`PrepareSendOk`] preview + opaque `handle`. The bridge stores
    /// the full SDK prepare response keyed by that handle so the gui
    /// can echo it back in [`Method::SendPayment`] without re-sending
    /// the whole complex prepare struct over JSON-RPC.
    PrepareSend(PrepareSendParams),
    /// Prepare an LNURL-pay / Lightning-address send. The bridge
    /// internally parses the input, fetches the invoice from the LNURL
    /// callback, and wraps the resulting SDK `PrepareLnurlPayResponse`
    /// in an opaque handle keyed into a separate pending map. The
    /// returned [`PrepareSendOk`] is shape-compatible with regular
    /// prepares so the gui's state machine stays uniform — the only
    /// difference is the `method` string, which comes back as
    /// `"LnurlPay"` / `"LightningAddress"`.
    PrepareLnurlPay(PrepareLnurlPayParams),
    /// Execute a previously-prepared send, identified by the opaque
    /// handle returned by either [`Method::PrepareSend`] or
    /// [`Method::PrepareLnurlPay`]. The bridge routes the handle to
    /// `sdk.send_payment` or `sdk.lnurl_pay` based on which pending
    /// map contains it. Handles that have already been consumed or
    /// have expired return [`ErrorKind::BadRequest`].
    SendPayment(SendPaymentParams),
    /// Generate a BOLT11 Lightning invoice to receive funds.
    ReceiveBolt11(ReceiveBolt11Params),
    /// Generate an on-chain Bitcoin deposit address. Spark's on-chain
    /// receive model requires a separate `claim_deposit` call once the
    /// incoming tx has confirmed — that's the Phase 4f
    /// [`Method::ListUnclaimedDeposits`] / [`Method::ClaimDeposit`] flow
    /// below. The `receive_onchain` RPC just generates the address;
    /// the gui watches for incoming deposits via
    /// [`Event::DepositsChanged`] and lets the user claim mature ones.
    ReceiveOnchain(ReceiveOnchainParams),
    /// Generate a static, reusable Spark address for native
    /// Spark-to-Spark transfers. Unlike Lightning/on-chain receive
    /// there is no amount or invoice — the address is identity-bound
    /// and token-agnostic. Maps to the SDK's
    /// `ReceivePaymentMethod::SparkAddress` (no params, fee 0). The
    /// reused [`OkPayload::ReceivePayment`] carries the address as
    /// `payment_request`.
    ReceiveSpark,
    /// Phase 4f: enumerate on-chain deposits the SDK has noticed but
    /// not yet claimed. Returns a [`Vec<DepositInfo>`] the gui renders
    /// as a "pending deposits" card in the Receive panel.
    ListUnclaimedDeposits,
    /// Phase 4f: claim a specific deposit (txid + vout) into the
    /// Spark wallet. The SDK's `claim_deposit` succeeds only if the
    /// deposit is `is_mature == true`; immature claims return
    /// [`ErrorKind::Sdk`] with the SDK's error string.
    ClaimDeposit(ClaimDepositParams),
    /// Phase 6: fetch the runtime user settings (Stable Balance
    /// state, private mode). Boolean-only view of the SDK's
    /// `UserSettings` struct; the panel exposes Stable Balance to
    /// the user as an on/off toggle without mentioning USDB.
    GetUserSettings,
    /// Phase 6: enable or disable Stable Balance. The bridge
    /// translates this into the SDK's
    /// `UpdateUserSettingsRequest { stable_balance_active_label:
    /// Some(Set { label: "USDB" }) | Some(Unset) }` call. The
    /// `"USDB"` label is the integrator-defined handle configured
    /// at init time in [`coincube-spark-bridge`]; it's internal
    /// plumbing and never surfaces in the gui.
    SetStableBalance(SetStableBalanceParams),
    /// Phase 4g: check whether a Lightning Address username is
    /// available on the configured LNURL server. UX nicety during
    /// typing — the authoritative uniqueness check is still done
    /// server-side by our API on reservation.
    CheckLightningAddressAvailable(CheckLightningAddressAvailableParams),
    /// Phase 4g: register a Lightning Address `<username>@<lnurl_domain>`
    /// with the Breez-hosted LNURL server. The SDK persists the
    /// address in local storage and binds it to this wallet's Spark
    /// leaf so incoming LNURL-pay payments settle server-side.
    RegisterLightningAddress(RegisterLightningAddressParams),
    /// Phase 4g: fetch the Lightning Address currently bound to this
    /// wallet (from the SDK's local cache, falling back to a
    /// server-side recovery when the cache is empty). Returns
    /// `None` when no address is registered.
    GetLightningAddress,
    /// Phase 4g: unregister the Lightning Address bound to this
    /// wallet. Idempotent — returns Ok even when no address is
    /// registered.
    DeleteLightningAddress,
    /// Classify a raw destination string as a cross-chain address and list
    /// the routes that can reach it. Returns [`CrossChainRoutesOk`], whose
    /// `address` field is `None` when the input isn't a cross-chain address
    /// at all (an ordinary BOLT11/Spark destination) — that's not an error,
    /// it's how the gui's Send panel decides which flow to run.
    ///
    /// The SDK only offers cross-chain for USD-stable assets (USDT/USDC), and
    /// only on mainnet.
    GetCrossChainRoutes(GetCrossChainRoutesParams),
    /// Prepare a cross-chain send along a route chosen from
    /// [`Method::GetCrossChainRoutes`]. Distinct from [`Method::PrepareSend`]
    /// because the SDK models it as a different `PaymentRequest` variant: it
    /// carries an explicitly-selected route rather than a string for the SDK
    /// to classify. Returns a [`PrepareSendOk`] whose `cross_chain` field is
    /// populated with the quote (amount out, fees, provider, expiry), and an
    /// opaque handle routed by the same [`Method::SendPayment`].
    PrepareCrossChain(PrepareCrossChainParams),
    /// Gracefully disconnect and exit the bridge.
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitParams {
    pub api_key: String,
    pub network: Network,
    /// BIP-39 mnemonic (space-separated words).
    pub mnemonic: String,
    pub mnemonic_passphrase: Option<String>,
    pub storage_dir: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Network {
    Mainnet,
    Regtest,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetInfoParams {
    #[serde(default)]
    pub ensure_synced: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListPaymentsParams {
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareSendParams {
    /// User-supplied destination — a BOLT11 invoice, BIP21 URI, or
    /// on-chain Bitcoin address. LNURL / Lightning Address destinations
    /// are NOT supported in Phase 4c — they go through a different SDK
    /// code path (`prepare_lnurl_pay`) that lands in Phase 4d.
    pub input: String,
    /// Amount override in sats. Required for amountless BOLT11 invoices
    /// and for on-chain sends; ignored otherwise. The SDK's underlying
    /// field is `u128` (tokens can have larger precision than sats);
    /// the bridge up-casts and validates.
    #[serde(default)]
    pub amount_sat: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendPaymentParams {
    /// Opaque handle returned from a prior [`Method::PrepareSend`]. The
    /// bridge looks this up in its pending-prepare map to recover the
    /// full `breez_sdk_spark::PrepareSendPaymentResponse`. Single-use —
    /// a successful or failed send consumes the entry.
    pub prepare_handle: String,
    /// Caller-supplied idempotency key. When set, the SDK short-circuits a
    /// repeat send with the same key instead of paying twice, which makes a
    /// retry after an ambiguous failure safe.
    ///
    /// **It does not cover every leg.** Spark's token transfer
    /// (`transfer_tokens`) has no idempotency hook, so a cross-chain send
    /// whose *source* asset is a token gets no such protection — see
    /// [`CrossChainQuote::retry_safe`]. Never blind-retry one of those; check
    /// the payment state first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCrossChainRoutesParams {
    /// Raw destination string as typed/pasted by the user. May be a bare
    /// address (`0xabc…`, a Solana base58 key, a `T…` Tron address) or a
    /// canonical URI (`ethereum:0xabc…?chain=base&asset=usdc`).
    pub input: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareCrossChainParams {
    /// The destination, echoed back **whole** from
    /// [`CrossChainRoutesOk::address`].
    ///
    /// The full [`CrossChainAddress`] rather than just the address string,
    /// because the bridge has to rebuild the SDK's `CrossChainAddressDetails`
    /// to re-resolve the route — and the extra fields (`contract_address`,
    /// `chain_id`) only exist when the user pasted a URI. Sending the bare
    /// address back would force the bridge to re-parse it, silently dropping
    /// them, and re-resolve against a *broader* route set than the one the user
    /// actually chose from. That happens to still work today only because the
    /// SDK's route filter is monotone (an absent contract filter matches
    /// everything), which is an accident of its implementation, not a promise.
    /// Round-tripping the details keeps the prepare resolving against exactly
    /// the destination the routes were offered for.
    pub destination: CrossChainAddress,
    /// The route the user picked, echoed back from [`CrossChainRoute`].
    pub route: CrossChainRoute,
    /// Amount to send, in sats. v1 always funds cross-chain sends from the
    /// BTC balance (pay-time conversion); sending *from* Stable Balance is a
    /// follow-up.
    pub amount_sat: u64,
    /// Slippage tolerance in basis points. Must be in `10..=500`; `None` uses
    /// the SDK default of 100 bps (1%). Exposed in the gui only behind an
    /// advanced disclosure — normal users should never have to think in bps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_slippage_bps: Option<u32>,
}

/// A cross-chain destination the SDK recognised. Mirrors
/// `breez_sdk_spark::CrossChainAddressDetails`, stringified to keep this crate
/// free of SDK type dependencies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossChainAddress {
    /// The bare recipient address, with any URI wrapper stripped.
    pub address: String,
    /// Address family: `"evm"`, `"solana"` or `"tron"`. The gui shows this to
    /// the user for confirmation — a Tron address does not look obviously
    /// Tron to a human, and sending USDT to the wrong chain loses it.
    pub family: String,
    /// Token contract / mint address, when the input was a URI that named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_address: Option<String>,
    /// EIP-681 chain id, when the input carried an `@<chain_id>` suffix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<u64>,
    /// Amount named by the URI, in the **destination asset's** base units.
    ///
    /// Deliberately not used to prefill the send amount, and it would be a bug
    /// to start: v1 funds cross-chain sends from the BTC balance and quotes
    /// *amount-in sats*, so this figure is in the wrong denomination entirely
    /// (10 USDC is not 10 sats). Honouring it needs an exact-out send, which
    /// the SDK only offers on routes flagged `exact_out_eligible` — a follow-up.
    /// Carried through so that follow-up has it, and so the gui can show the
    /// requested amount for confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<u128>,
}

/// One way to get funds to a cross-chain destination. Mirrors
/// `breez_sdk_spark::CrossChainRoutePair`; round-tripped back to the bridge
/// verbatim on [`Method::PrepareCrossChain`], which re-resolves it against the
/// SDK's live route list rather than trusting these fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossChainRoute {
    /// `"orchestra"` (Spark transfer → bridge) or `"boltz"` (Lightning
    /// reverse swap). Surfaced to the user as the route's provider.
    pub provider: String,
    /// Destination chain, e.g. `"base"`, `"solana"`, `"tron"`.
    pub chain: String,
    /// Stable chain identifier (EVM `chainId` as a decimal string). `None` for
    /// chains that don't expose one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    /// Destination asset symbol, e.g. `"USDC"`, `"USDT"`.
    pub asset: String,
    /// Token contract / mint on the destination chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_address: Option<String>,
    /// Decimal places of the destination asset — needed to render
    /// [`CrossChainQuote::estimated_out`] as a human amount.
    pub decimals: u8,
    /// Whether this route can be funded from the BTC balance. v1 only offers
    /// routes where this holds; token-funded routes are a follow-up.
    pub btc_source_supported: bool,
}

/// Result of [`Method::GetCrossChainRoutes`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossChainRoutesOk {
    /// The parsed destination, or `None` when the input isn't a cross-chain
    /// address. `None` is the ordinary "this is a Lightning invoice, not an
    /// EVM address" case, not an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<CrossChainAddress>,
    /// Routes that can reach it, BTC-fundable ones only. Empty with a `Some`
    /// address means the address parsed but nothing can currently reach it —
    /// the gui must say so rather than silently offering a normal send.
    pub routes: Vec<CrossChainRoute>,
}

/// The quote attached to a prepared cross-chain send.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossChainQuote {
    /// The route this quote is for.
    pub route: CrossChainRoute,
    /// Estimated amount the recipient receives, in the destination asset's
    /// base units (scale by [`CrossChainRoute::decimals`]).
    pub estimated_out: u128,
    /// Total user-visible fee, in the destination asset's base units. Covers
    /// provider spread, bridge/gas, and DEX slippage.
    pub fee_amount: u128,
    /// Sats debited from the wallet to move funds to the provider. Denominated
    /// in sats, unlike the two fields above — don't add them together.
    pub source_transfer_fee_sats: u64,
    /// ISO8601 timestamp after which the quote is void. The gui counts down to
    /// this and re-quotes on expiry rather than sending against a stale rate.
    pub expires_at: String,
    /// Whether a failed send can be safely retried with the same idempotency
    /// key.
    ///
    /// `false` means the send's source leg is a Spark *token* transfer, which
    /// has no idempotency hook in the SDK — a blind retry could pay twice. The
    /// gui must check payment state before offering retry on one of these.
    /// True for the v1 BTC-funded path.
    pub retry_safe: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReceiveBolt11Params {
    /// Amount in sats. `None` generates an amountless invoice.
    #[serde(default)]
    pub amount_sat: Option<u64>,
    /// Invoice description shown to the payer.
    #[serde(default)]
    pub description: String,
    /// Invoice expiry in seconds. `None` uses the SDK default.
    #[serde(default)]
    pub expiry_secs: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReceiveOnchainParams {
    /// If `true`, force the bridge to return a fresh address instead
    /// of a cached one. `None` defers to the SDK default (reuse last).
    #[serde(default)]
    pub new_address: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseInputParams {
    /// The raw destination string to classify.
    pub input: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimDepositParams {
    /// Deposit transaction id to claim.
    pub txid: String,
    /// Output index of the deposit within `txid`.
    pub vout: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SetStableBalanceParams {
    /// `true` to activate Stable Balance, `false` to deactivate.
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckLightningAddressAvailableParams {
    /// The username portion of `<username>@<lnurl_domain>`.
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterLightningAddressParams {
    /// The username portion of `<username>@<lnurl_domain>`.
    pub username: String,
    /// Payer-visible description. `None` tells the SDK to default
    /// to `"Pay to <username>@<domain>"`.
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareLnurlPayParams {
    /// The LNURL-pay or Lightning-address destination. The bridge
    /// re-parses this internally via `BreezSdk::parse` to recover the
    /// `LnurlPayRequestDetails` — we don't serialize the full details
    /// struct over JSON-RPC, the bridge just re-does the work (cheap,
    /// and keeps the protocol surface tiny).
    pub input: String,
    /// Amount in sats. LNURL-pay always requires an explicit amount
    /// between the server's `min_sendable` and `max_sendable`. The
    /// gui validates against the range it got from an earlier
    /// [`Method::ParseInput`] call.
    pub amount_sat: u64,
    /// Optional comment to attach to the payment. Only surfaces to
    /// the payee if the LNURL server declared `comment_allowed > 0`
    /// in its metadata.
    #[serde(default)]
    pub comment: Option<String>,
}

/// A response envelope. Exactly one of `ok` / `err` is populated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(flatten)]
    pub result: ResponseResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseResult {
    Ok(OkPayload),
    Err(ErrorPayload),
}

/// The success payload shape for each method.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum OkPayload {
    Init {},
    GetInfo(GetInfoOk),
    ListPayments(ListPaymentsOk),
    ParseInput(ParseInputOk),
    /// Shared between [`Method::PrepareSend`] and
    /// [`Method::PrepareLnurlPay`] — both bridge paths return
    /// shape-compatible previews so the gui state machine can treat
    /// them uniformly.
    PrepareSend(PrepareSendOk),
    GetCrossChainRoutes(CrossChainRoutesOk),
    SendPayment(SendPaymentOk),
    ReceivePayment(ReceivePaymentOk),
    ListUnclaimedDeposits(ListUnclaimedDepositsOk),
    ClaimDeposit(ClaimDepositOk),
    GetUserSettings(GetUserSettingsOk),
    SetStableBalance {},
    CheckLightningAddressAvailable(CheckLightningAddressAvailableOk),
    RegisterLightningAddress(RegisterLightningAddressOk),
    GetLightningAddress(GetLightningAddressOk),
    DeleteLightningAddress {},
    Shutdown {},
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInfoOk {
    pub balance_sats: u64,
    pub identity_pubkey: String,
    /// Stable Balance USDB holding when the SDK reports a non-zero
    /// USDB token balance. `None` when the user has no USDB.
    /// Carries enough metadata for the gui to render the holding
    /// without hardcoding decimals or ticker.
    #[serde(default)]
    pub stable_balance: Option<StableBalanceSnapshot>,
}

/// Snapshot of the wallet's USDB (Stable Balance) holding. Mirrors the
/// fields the gui needs to fold the holding into the unified portfolio
/// total, the same way the Liquid panel folds USDt into the L-BTC
/// total.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StableBalanceSnapshot {
    /// USDB amount in token base units (`balance / 10^decimals` is
    /// the dollar value).
    pub balance: u64,
    /// Token base-units precision (USDB ships at 6 decimals today).
    pub decimals: u32,
    /// Display ticker (e.g. "USDB"). Plumbing only — the gui still
    /// renders the feature as "Stable Balance".
    pub ticker: String,
}

/// Compact payment summary used by Phase 2. The full Spark `Payment` shape is
/// not mirrored here yet — only the fields we need to prove end-to-end
/// connectivity and render a minimum list. Later phases add the richer
/// domain mapping when the UI actually consumes the data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListPaymentsOk {
    pub payments: Vec<PaymentSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentSummary {
    /// Payment id / tx id as reported by the Spark SDK.
    pub id: String,
    /// Amount in satoshis (direction-signed). Zero for `method=token`
    /// payments — the real amount is in `token_amount` because Spark's
    /// `Payment.amount` is overloaded ("satoshis or token base units"
    /// depending on method).
    pub amount_sat: i64,
    /// Fees paid in satoshis (non-signed). Zero for `method=token`
    /// payments — token fees are tracked separately on the SDK side
    /// and aren't surfaced here yet.
    #[serde(default)]
    pub fees_sat: u64,
    /// Token amount in base units when `method=token`; `None` for
    /// Bitcoin-side payments. Always positive — direction is in
    /// [`PaymentSummary::direction`].
    #[serde(default)]
    pub token_amount: Option<u64>,
    /// Decimals for [`PaymentSummary::token_amount`]. `None` for
    /// non-token payments.
    #[serde(default)]
    pub token_decimals: Option<u32>,
    /// Display ticker for [`PaymentSummary::token_amount`] (e.g.
    /// "USDB"). `None` for non-token payments.
    #[serde(default)]
    pub token_ticker: Option<String>,
    /// Unix timestamp in seconds.
    pub timestamp: u64,
    pub status: String,
    pub direction: String,
    /// Payment method: `lightning`, `spark`, `deposit`, `withdraw`,
    /// `token`, or `unknown`. Mirrors [`PaymentMethod`] on the SDK
    /// side. The gui uses this to pick the right asset icon for
    /// each transaction row in the Overview list.
    #[serde(default)]
    pub method: String,
    /// Optional human description extracted from the payment details
    /// (invoice memo for Lightning, etc.). Empty for on-chain /
    /// Spark-native transfers.
    #[serde(default)]
    pub description: Option<String>,
}

/// High-level classification of a user-supplied destination string,
/// returned by [`Method::ParseInput`]. The Send panel branches on
/// [`ParseInputKind`] to decide whether to route to
/// [`Method::PrepareSend`] (regular SDK prepare) or
/// [`Method::PrepareLnurlPay`] (LNURL code path).
///
/// The SDK's `InputType` enum is much richer than this; Phase 4e only
/// surfaces the fields the gui actually branches on, which keeps the
/// protocol crate decoupled from the SDK's extensive type tree. Later
/// phases can extend the payload as panels start needing more detail.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseInputKind {
    /// A BOLT11 Lightning invoice. Use [`Method::PrepareSend`].
    Bolt11Invoice,
    /// A plain Bitcoin on-chain address or BIP21 URI. Use
    /// [`Method::PrepareSend`].
    BitcoinAddress,
    /// A raw LNURL-pay string (typically `lnurl1...`). Use
    /// [`Method::PrepareLnurlPay`].
    LnurlPay,
    /// A Lightning address in the form `user@domain`. Resolves to an
    /// LNURL-pay flow internally. Use [`Method::PrepareLnurlPay`].
    LightningAddress,
    /// A static Spark address (identity-bound, reusable). Native
    /// Spark-to-Spark transfer. Use [`Method::PrepareSend`] — the
    /// SDK auto-detects and routes it as a Spark send.
    SparkAddress,
    /// A Spark invoice (single-use, may carry an amount / token /
    /// expiry). Native Spark-to-Spark transfer. Use
    /// [`Method::PrepareSend`].
    SparkInvoice,
    /// Anything else the SDK could parse (BOLT12, Bolt12 offer,
    /// silent payment, etc.). Phase 4e falls through to
    /// [`Method::PrepareSend`] which handles the ones the SDK
    /// supports; the gui shows a "not supported" error for the rest.
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseInputOk {
    /// Which high-level category the input parsed into.
    pub kind: ParseInputKind,
    /// Pre-specified amount if the input carries one (BOLT11 with
    /// amount, BIP21 with amount). `None` for amountless invoices,
    /// plain on-chain addresses, and LNURL/Lightning-address inputs
    /// (which use `lnurl_min_sendable_sat` / `lnurl_max_sendable_sat`
    /// as a range instead).
    #[serde(default)]
    pub amount_sat: Option<u64>,
    /// For LNURL / Lightning Address inputs: the server's minimum
    /// acceptable payment in sats. `None` for non-LNURL inputs.
    #[serde(default)]
    pub lnurl_min_sendable_sat: Option<u64>,
    /// For LNURL / Lightning Address inputs: the server's maximum
    /// acceptable payment in sats. `None` for non-LNURL inputs.
    #[serde(default)]
    pub lnurl_max_sendable_sat: Option<u64>,
    /// For LNURL / Lightning Address inputs: max comment length
    /// the server accepts. `0` means the server doesn't accept a
    /// comment. Defaults to `0` for non-LNURL inputs.
    #[serde(default)]
    pub lnurl_comment_allowed: u16,
    /// For Lightning Address inputs: the `user@domain` string the
    /// user typed. Surfaced for display; the bridge re-parses on
    /// prepare so the gui doesn't need to round-trip it.
    #[serde(default)]
    pub lnurl_address: Option<String>,
}

/// Preview returned by [`Method::PrepareSend`]. The `handle` is a bridge
/// session token that the gui echoes back on the subsequent
/// [`Method::SendPayment`] call — the bridge stores the full SDK
/// `PrepareSendPaymentResponse` (which is a complex nested struct)
/// internally under that key so it doesn't have to round-trip over
/// JSON-RPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareSendOk {
    /// Opaque handle to pass into `send_payment`. Single-use.
    pub handle: String,
    /// Display amount in sats. The SDK's underlying field is `u128`
    /// (Spark tokens can exceed sat precision); for Bitcoin sends the
    /// value fits in u64. The bridge saturates at `u64::MAX` if the
    /// amount somehow exceeds that.
    pub amount_sat: u64,
    /// Estimated fee in sats (also saturating u64).
    pub fee_sat: u64,
    /// High-level send-method tag for display. Mirrors the variant
    /// names of `breez_sdk_spark::SendPaymentMethod` — one of
    /// "BitcoinAddress", "Bolt11Invoice", "SparkAddress",
    /// "SparkInvoice", "CrossChainAddress". Stringified to keep the
    /// protocol crate free of SDK type deps.
    pub method: String,
    /// The cross-chain quote, present only for a [`Method::PrepareCrossChain`]
    /// prepare (`method == "CrossChainAddress"`).
    ///
    /// `amount_sat` / `fee_sat` above can't carry a cross-chain quote on their
    /// own: the recipient amount and the headline fee are denominated in the
    /// *destination* asset (USDC/USDT base units), not sats, so reporting them
    /// through sats-typed fields would misprice the send by orders of
    /// magnitude. Everything asset-denominated lives here instead.
    ///
    /// **Boxed on purpose.** A `CrossChainQuote` is large and `None` on every
    /// ordinary send (Lightning, on-chain, Spark) — which is the overwhelming
    /// majority. Inlining it made `PrepareSendOk` the fat variant of
    /// `OkPayload`, pushing `ResponseResult` past clippy's `large_enum_variant`
    /// threshold and bloating every small response. The box keeps the common
    /// path pointer-sized and only allocates when a quote is actually present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_chain: Option<Box<CrossChainQuote>>,
}

/// Result of a successful [`Method::SendPayment`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendPaymentOk {
    /// Payment id from `breez_sdk_spark::Payment::id` — the caller
    /// can feed this into a follow-up [`Method::ListPayments`] to
    /// display the new row.
    pub payment_id: String,
    /// Final amount sent (sats).
    pub amount_sat: u64,
    /// Final fee paid (sats).
    pub fee_sat: u64,
}

/// Result of a successful [`Method::ReceiveBolt11`] or
/// [`Method::ReceiveOnchain`]. For Lightning the `payment_request` is
/// a BOLT11 invoice string; for on-chain it's a BIP21 URI or bare
/// Bitcoin address (whatever the SDK returns — the gui treats it as
/// an opaque string to copy / render as a QR code).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceivePaymentOk {
    pub payment_request: String,
    /// Fee the SDK expects the receiver to pay to settle this
    /// incoming payment. 0 for most Bitcoin on-chain addresses.
    pub fee_sat: u64,
}

/// One entry in the unclaimed-deposits list returned by
/// [`Method::ListUnclaimedDeposits`].
///
/// Mirrors a subset of `breez_sdk_spark::DepositInfo` — the gui only
/// needs the txid/vout (to call [`Method::ClaimDeposit`]), the amount
/// (for display), the maturity flag (to gate the Claim button), and
/// any claim error string (to surface failures from previous attempts).
/// `refund_tx` / `refund_tx_id` are deferred until a Phase 4g+ adds
/// the refund flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepositInfo {
    pub txid: String,
    pub vout: u32,
    pub amount_sat: u64,
    /// `true` once the deposit has enough confirmations to be claimed.
    /// The gui shows "Pending confirmation" when `false` and a "Claim"
    /// button when `true`.
    pub is_mature: bool,
    /// If the SDK has previously tried to claim this deposit and
    /// failed, this carries the SDK's error string. Surfaced in the
    /// UI as a per-row warning.
    pub claim_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListUnclaimedDepositsOk {
    pub deposits: Vec<DepositInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimDepositOk {
    /// Payment id of the resulting Spark wallet transfer.
    pub payment_id: String,
    /// Amount claimed in sats. Mirrors the deposit's `amount_sat`
    /// minus any internal fees the SDK deducted.
    pub amount_sat: u64,
}

/// Phase 6: boolean-flattened view of the SDK's `UserSettings`. The
/// gui only ever cares whether Stable Balance is active — it never
/// renders the USDB label. Private mode is surfaced here as a
/// preview field for a future "privacy" toggle but is not yet wired
/// into the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetUserSettingsOk {
    /// `true` when the SDK reports an active Stable Balance token.
    pub stable_balance_active: bool,
    /// Mirrors `UserSettings::spark_private_mode_enabled`.
    pub private_mode_enabled: bool,
}

/// Phase 4g: Lightning Address record as reported by the Breez-hosted
/// LNURL server via the Spark SDK. Flat mirror of the SDK's
/// `LightningAddressInfo` (with the nested `LnurlInfo { url, bech32 }`
/// inlined) so the JSON wire format is self-describing and the gui
/// doesn't have to know about `LnurlInfo`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightningAddressInfo {
    /// Full `<username>@<lnurl_domain>` string.
    pub lightning_address: String,
    /// Username portion — the part to the left of `@`.
    pub username: String,
    /// Payer-visible description. `None` if the caller didn't
    /// supply one and the SDK's default wasn't retained in the
    /// local cache.
    #[serde(default)]
    pub description: Option<String>,
    /// `https://<lnurl_domain>/.well-known/lnurlp/<username>` — the
    /// LUD-06 callback URL the payer's wallet resolves to.
    pub lnurl_url: String,
    /// Bech32-encoded `lnurl1...` form of `lnurl_url` — what a QR
    /// code scanner would read.
    pub lnurl_bech32: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckLightningAddressAvailableOk {
    /// `true` when the LNURL server considers the username free.
    pub available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterLightningAddressOk {
    #[serde(flatten)]
    pub info: LightningAddressInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetLightningAddressOk {
    /// `None` when no address is currently registered on this wallet.
    pub info: Option<LightningAddressInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The bridge has not received an `init` yet.
    NotConnected,
    /// `init` was called after a previous successful init.
    AlreadyConnected,
    /// The underlying Spark SDK returned an error.
    Sdk,
    /// The request envelope failed to parse.
    BadRequest,
    /// The bridge is shutting down and cannot accept new work.
    ShuttingDown,
}

impl Response {
    pub fn ok(id: u64, payload: OkPayload) -> Self {
        Self {
            id,
            result: ResponseResult::Ok(payload),
        }
    }

    pub fn err(id: u64, kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            id,
            result: ResponseResult::Err(ErrorPayload {
                kind,
                message: message.into(),
            }),
        }
    }
}

/// Unsolicited bridge → gui event.
///
/// The bridge subscribes to the Spark SDK's `EventListener` stream via
/// `add_event_listener` and translates each `SdkEvent` variant into one
/// of these envelopes before writing it to stdout as a [`Frame::Event`].
/// The gui's [`crate::Frame`] reader task forwards received events into
/// an in-process broadcast channel; panels subscribe via
/// `SparkBackend::event_subscription()` and react in `update()`.
///
/// Phase 4d shipped `Synced` + the three `Payment*` variants. Phase 4f
/// adds:
/// - `bolt11: Option<String>` on `PaymentSucceeded` so the Receive
///   panel can correlate against a specific generated invoice instead
///   of advancing on any incoming payment.
/// - [`Event::DepositsChanged`] — fires when the SDK detects new,
///   newly-mature, or claimed on-chain deposits. The gui's Receive
///   panel reloads its `list_unclaimed_deposits` view in response.
///
/// Optimization and lightning-address-changed events from the SDK
/// remain deferred until a panel needs them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "payload", rename_all = "snake_case")]
pub enum Event {
    /// SDK completed an internal sync tick. The gui uses this as a
    /// cheap "refresh balance" trigger.
    Synced,
    /// A payment finalized successfully. `amount_sat` is positive for
    /// incoming, negative for outgoing (mirrors the [`PaymentSummary`]
    /// convention). `bolt11` is the BOLT11 invoice string for
    /// Lightning payments — `None` for on-chain / Spark-native /
    /// non-Lightning payments.
    PaymentSucceeded {
        id: String,
        amount_sat: i64,
        #[serde(default)]
        bolt11: Option<String>,
    },
    /// A payment entered the pending state (broadcast but not yet
    /// confirmed, or lightning htlc in flight).
    PaymentPending { id: String, amount_sat: i64 },
    /// A payment failed permanently.
    PaymentFailed { id: String, amount_sat: i64 },
    /// One or more on-chain deposits changed state — newly observed,
    /// newly mature, or claimed. The gui reloads its
    /// [`Method::ListUnclaimedDeposits`] result in response. We
    /// collapse the SDK's three event types (`UnclaimedDeposits`,
    /// `NewDeposits`, `ClaimedDeposits`) into a single coalesced
    /// signal because the gui treats them all as "refresh the list."
    DepositsChanged,
    /// Phase 4g: the Lightning Address bound to this wallet changed
    /// on the LNURL server — either a register (payload `Some`),
    /// an unregister (payload `None`), or a cross-device sync via
    /// realtime-sync. The gui refreshes its settings view in
    /// response; a `None` state it didn't initiate triggers
    /// auto-re-register from the DB-reserved username.
    LightningAddressChanged { info: Option<LightningAddressInfo> },
}

/// The top-level message envelope written/read on the wire. We use a single
/// outer discriminator so one `serde_json::from_str` call works for all
/// three directions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    Request(Request),
    Response(Response),
    Event(Event),
}
