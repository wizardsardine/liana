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

/// Serde codec that carries a `u128` on the wire as a decimal **string**.
///
/// Two independent reasons force this for the cross-chain token-amount fields:
///
/// 1. [`Request`] and [`Response`] both use `#[serde(flatten)]`, and serde's
///    flatten path buffers values through `FlatMapDeserializer`, which does not
///    implement `deserialize_u128`. A bare `u128` field anywhere beneath a
///    flattened envelope therefore fails to parse with the exact error
///    "u128 is not supported" (serde issue #1183). Every response is flattened
///    at `Response.result`, so this hits all of them.
/// 2. A JSON number above 2^53 silently loses precision in many parsers. An
///    18-decimal token amount can exceed that, so a bare number would be unsafe
///    on the wire even without (1).
///
/// The bridge and gui both link this crate, so annotating the field with
/// `#[serde(with = "u128_dec")]` keeps their serialize/deserialize in lockstep.
mod u128_dec {
    use serde::{de, Deserializer, Serializer};
    use std::fmt;

    pub fn serialize<S: Serializer>(value: &u128, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }

    /// Accepts a `u128` from a decimal string (what we emit) or a bare JSON
    /// integer (tolerated so an older number-format frame still parses). Goes
    /// through `deserialize_any` rather than `deserialize_u128` — the latter is
    /// the method serde's flatten / `Content` replay path leaves unimplemented,
    /// which is the whole reason a bare `u128` failed here.
    pub(super) struct U128Visitor;

    impl<'de> de::Visitor<'de> for U128Visitor {
        type Value = u128;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a u128 as a decimal string or a JSON integer")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<u128, E> {
            v.parse().map_err(de::Error::custom)
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<u128, E> {
            Ok(u128::from(v))
        }

        fn visit_u128<E: de::Error>(self, v: u128) -> Result<u128, E> {
            Ok(v)
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<u128, E> {
            // Our own wording, not `TryFromIntError`'s. std reworded that
            // Display in 2026 ("out of range integral type conversion
            // attempted" → "number too small to fit in target type"), which
            // silently broke the test asserting on it. Naming the value and
            // the target type is also more use in a log than std's sentence.
            u128::try_from(v).map_err(|_| E::custom(format_args!("u128 out of range: {v}")))
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
        deserializer.deserialize_any(U128Visitor)
    }
}

/// [`u128_dec`] for `Option<u128>`. Pairs with `#[serde(default,
/// skip_serializing_if = "Option::is_none")]`: `None` is omitted from the wire,
/// `Some` is a decimal string.
mod opt_u128_dec {
    use super::u128_dec::U128Visitor;
    use serde::{de, Deserializer, Serializer};
    use std::fmt;

    pub fn serialize<S: Serializer>(
        value: &Option<u128>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(v) => serializer.serialize_str(&v.to_string()),
            None => serializer.serialize_none(),
        }
    }

    struct OptU128Visitor;

    impl<'de> de::Visitor<'de> for OptU128Visitor {
        type Value = Option<u128>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an optional u128 as a decimal string or a JSON integer")
        }

        fn visit_none<E: de::Error>(self) -> Result<Option<u128>, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Option<u128>, E> {
            Ok(None)
        }

        fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Option<u128>, D::Error> {
            d.deserialize_any(U128Visitor).map(Some)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<u128>, D::Error> {
        deserializer.deserialize_option(OptU128Visitor)
    }
}

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
    ///
    /// Since S2b [`GetUserSettingsOk::stable_balance_active`] reports
    /// bridge-driven auto-convert ([`Method::GetAutoConvert`]). The SDK's
    /// own Stable Balance is never configured.
    GetUserSettings,
    /// Phase 6: enable or disable Stable Balance.
    ///
    /// Since S2b, a legacy alias of [`Method::SetAutoConvert`], answered
    /// with [`OkPayload::SetStableBalance`]. The bridge no longer touches the
    /// SDK's Stable Balance, which stays unconfigured: turning that off
    /// (`StableBalanceActiveLabel::Unset`) converts the whole USDB balance
    /// back to bitcoin.
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
    /// Read the bridge-driven auto-convert setting and its circuit breaker.
    /// Returns [`OkPayload::AutoConvert`].
    ///
    /// Auto-convert is the bridge's own BTC → USDB conversion of incoming
    /// bitcoin, saved per Cube. It replaces the SDK's Stable Balance, which
    /// the bridge leaves unconfigured; [`Method::SetStableBalance`] is a
    /// legacy alias of [`Method::SetAutoConvert`].
    GetAutoConvert,
    /// Turn bridge-driven auto-convert on or off. Returns the resulting
    /// [`AutoConvertState`] in [`OkPayload::AutoConvert`]. Turning it on also
    /// resets a paused breaker.
    ///
    /// Neither direction moves funds: on converts only bitcoin received
    /// afterwards, and off leaves the USDB balance as USDB. The change is
    /// saved before it is answered, so an error answer means nothing changed.
    SetAutoConvert(SetAutoConvertParams),
    /// Minimum amounts for a conversion out of `from`. Returns
    /// [`OkPayload::GetConversionLimits`]. Lets the Swap form state the
    /// minimum before the user asks for an estimate.
    GetConversionLimits(GetConversionLimitsParams),
    /// Estimate a BTC ⇄ USDB swap and hold the prepared self-transfer under an
    /// opaque handle. Returns [`OkPayload::PrepareSwap`]. The estimate is not
    /// a locked price: the slippage cap is the only protection.
    PrepareSwap(PrepareSwapParams),
    /// Execute a swap prepared by [`Method::PrepareSwap`]. Moves funds, so the
    /// gui treats a missing answer as an unknown outcome. Returns
    /// [`OkPayload::Swap`].
    ///
    /// Only a refusal before submission is a definite failure:
    /// [`ErrorKind::BadRequest`] or [`ErrorKind::ShuttingDown`]. They are
    /// definite because the bridge sends them only for a swap it hasn't
    /// started executing. The bridge answers `BadRequest` for every refusal
    /// it makes before asking the SDK to execute (S3b). A bridge that
    /// answers a swap it is already executing must not use either kind.
    ///
    /// The gui treats [`ErrorKind::NotConnected`] as an unknown outcome. Its
    /// client makes that answer up for every request still in flight when
    /// the bridge dies. The bridge itself refuses a swap before `init` with
    /// `BadRequest`.
    ///
    /// An [`ErrorKind::Sdk`] error can come after the SDK has sent the input
    /// to the pool: at `a38117d` it gives up waiting for the output after
    /// 30 s and returns an error, and the output can still arrive. So the gui
    /// treats an `Sdk` error as an unknown outcome too, and reconciles from
    /// Activity before it offers another swap. A retry can't be made safe
    /// with an idempotency key, because the bridge drops the key for any
    /// conversion leg.
    ///
    /// After an `Sdk` error on a swap out of USDB, auto-convert counts no
    /// bitcoin received until its switch next changes, since the swap's
    /// output may arrive as an ordinary receive (S3b). The bridge saves this
    /// hold before sending, so it also holds after a bridge restart during
    /// the swap, and refuses the swap with `BadRequest` if it can't be
    /// saved. [`AutoConvertState::held_after_swap`] shows it, and
    /// [`Event::AutoConvertStateChanged`] announces it.
    Swap(SwapParams),
    /// [`Method::PrepareSend`] with an explicit balance to pay from. Returns
    /// [`OkPayload::PrepareSendFrom`]; the handle executes through the
    /// ordinary [`Method::SendPayment`].
    ///
    /// A separate method rather than a new field on [`PrepareSendParams`] so a
    /// stale bridge fails closed: one that predates this method can't parse
    /// the request and drops it, where it would silently ignore an unknown
    /// `pay_from` field and prepare a bitcoin send instead.
    ///
    /// A Spark invoice that requests the wallet's USDB token, paid from
    /// [`WalletAsset::Usdb`], is paid straight from the USDB balance: the
    /// invoice must name its amount, `amount_sat` is ignored, and the
    /// preview carries [`PrepareSendFromOk::token_payment`]. Paid from
    /// bitcoin, it is refused with [`ErrorKind::BadRequest`].
    ///
    /// Refuses a Spark invoice that requests any other token, with
    /// [`ErrorKind::BadRequest`] and before preparing anything (and refuses
    /// a token leg the ordinary prepare returns, as from a BIP-21 URI
    /// carrying a token invoice). Paying one spends that token directly,
    /// which neither `pay_from` describes. So every other token leg this
    /// method prepares is a conversion; see [`PrepareSendFromOk::conversion`].
    PrepareSendFrom(PrepareSendFromParams),
    /// [`Method::PrepareLnurlPay`] with an explicit balance to pay from: the
    /// LNURL / Lightning-address twin of [`Method::PrepareSendFrom`], and a
    /// separate method for the same reason. Returns
    /// [`OkPayload::PrepareSendFrom`]; the handle executes through the
    /// ordinary [`Method::SendPayment`].
    PrepareLnurlPayFrom(PrepareLnurlPayFromParams),
    /// Send USDB from the USDB balance to a plain Spark address, for an
    /// amount in USDB base units: the address-shaped twin of the USDB
    /// invoice that [`Method::PrepareSendFrom`] pays. Returns
    /// [`OkPayload::PrepareSendFrom`] shaped as that one is, with
    /// [`PrepareSendFromOk::token_payment`] carrying the amount and fee and
    /// `pay_from` [`WalletAsset::Usdb`]; the handle executes through the
    /// ordinary [`Method::SendPayment`].
    ///
    /// Refuses, with [`ErrorKind::BadRequest`], an input that isn't a Spark
    /// address and a zero amount. A separate method because the amount is
    /// not sats: a bridge that predates it can't parse the request and drops
    /// it, where a field on [`PrepareSendFromParams`] would be ignored and the
    /// base units sent as sats.
    PrepareUsdbSend(PrepareUsdbSendParams),
    /// Gracefully disconnect and exit the bridge.
    Shutdown,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct InitParams {
    pub api_key: String,
    pub network: Network,
    /// BIP-39 mnemonic (space-separated words).
    pub mnemonic: String,
    pub mnemonic_passphrase: Option<String>,
    pub storage_dir: String,
}

// Hand-written so a `debug!("{:?}", params)` anywhere in the bridge handshake
// cannot write a seed phrase (or the API key) into `<datadir>/logs/`. The
// derived impl printed both in full. Nothing does that today; this is one
// stray log line away from being a disclosure, and the log file outlives the
// process.
impl std::fmt::Debug for InitParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InitParams")
            .field("api_key", &"<redacted>")
            .field("network", &self.network)
            .field("mnemonic", &"<redacted>")
            .field(
                "mnemonic_passphrase",
                &self.mnemonic_passphrase.as_ref().map(|_| "<redacted>"),
            )
            .field("storage_dir", &self.storage_dir)
            .finish()
    }
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
    /// Take the fee out of `amount_sat` instead of adding it on top, so the
    /// wallet spends exactly `amount_sat`: pass the whole balance to drain it.
    /// The recipient gets `amount_sat - fee_sat`. Omitted on the wire when
    /// false, so older bridges see the request they always did.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fees_included: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendPaymentParams {
    /// Opaque handle returned from a prior [`Method::PrepareSend`]. The
    /// bridge looks this up in its pending-prepare map to recover the
    /// full `breez_sdk_spark::PrepareSendPaymentResponse`. Single-use —
    /// a successful or failed send consumes the entry.
    pub prepare_handle: String,
    /// Caller-supplied idempotency key. On the plain bitcoin rails (Lightning /
    /// on-chain / Spark) the SDK honours it: a repeat send with the same key
    /// short-circuits instead of paying twice, so a retry after an ambiguous
    /// failure is safe.
    ///
    /// **It does not cover a token or conversion leg.** The SDK rejects an
    /// idempotency key on any send that has one, so the bridge drops the key
    /// there — and that is *every* cross-chain (USDt/USDC) send, BTC-funded ones
    /// included, because they carry an AMM conversion. Do not infer idempotency
    /// for a cross-chain retry from this field: that safety instead comes from
    /// re-sending the *same* prepared quote (the provider dedups on its swap id),
    /// gated by [`CrossChainQuote::retry_safe`] and the panel's retry policy.
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "opt_u128_dec"
    )]
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
    /// The amount bounds the provider publishes for funding this route from
    /// bitcoin. `None` when it publishes none. A route can enforce a tighter
    /// bound than it publishes, so these are a floor for the gui's own check,
    /// not a promise that an amount inside them will quote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btc_limits: Option<CrossChainRouteLimits>,
}

impl CrossChainRoute {
    /// Whether `other` names the same route: provider, chain, asset and token.
    ///
    /// Leaves out [`Self::btc_limits`]. Those are what the provider publishes
    /// right now — a USD floor moves with the bitcoin price — so a route looked
    /// up on Details and re-resolved at prepare can carry different bounds and
    /// still be the route the user chose. Matching on `==` would turn a price
    /// tick into "no longer offered".
    pub fn same_route_as(&self, other: &Self) -> bool {
        self.provider == other.provider
            && self.chain == other.chain
            && self.chain_id == other.chain_id
            && self.asset == other.asset
            && self.contract_address == other.contract_address
            && self.decimals == other.decimals
            && self.btc_source_supported == other.btc_source_supported
    }
}

/// Mirrors `breez_sdk_spark::CrossChainRouteLimits` for the bitcoin-funded
/// side of a route. Either group may be absent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossChainRouteLimits {
    /// Smallest amount accepted, in sats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_sats: Option<u64>,
    /// Largest amount accepted, in sats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_sats: Option<u64>,
    /// Smallest order value accepted, in USD cents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_usd_cents: Option<u64>,
    /// Largest order value accepted, in USD cents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_usd_cents: Option<u64>,
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
    #[serde(with = "u128_dec")]
    pub estimated_out: u128,
    /// Total user-visible fee, in the destination asset's base units. Covers
    /// provider spread, bridge/gas, and DEX slippage.
    #[serde(with = "u128_dec")]
    pub fee_amount: u128,
    /// Sats debited from the wallet to move funds to the provider. Denominated
    /// in sats, unlike the two fields above — don't add them together.
    pub source_transfer_fee_sats: u64,
    /// Timestamp after which the quote is void — the gui counts down to it and
    /// re-quotes on expiry rather than sending against a stale rate. Format is
    /// provider-dependent and passed through from the SDK verbatim: Orchestra
    /// sends RFC3339, Boltz sends a bare Unix epoch seconds string. The gui's
    /// `quote_countdown` parses both.
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
    /// `true` to turn auto-convert on, `false` to turn it off (see
    /// [`Method::SetStableBalance`]).
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SetAutoConvertParams {
    /// `true` to convert incoming bitcoin to USDB, `false` to stop. Turning it
    /// off leaves the existing USDB balance as USDB.
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct GetConversionLimitsParams {
    /// The asset being converted out of. The other asset is the target.
    pub from: WalletAsset,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareSwapParams {
    /// How much to convert, and out of which asset.
    pub amount: SwapAmount,
    /// Slippage cap in basis points. An explicit cap must be in `10..=500`,
    /// the same range as [`PrepareCrossChainParams::max_slippage_bps`]; the
    /// bridge refuses anything outside it with [`ErrorKind::BadRequest`]
    /// (S3b). `None` uses [`CONVERSION_MAX_SLIPPAGE_BPS`]. The cap is shown
    /// read-only, so gui callers send `None` until it becomes editable.
    ///
    /// The quote echoes the cap applied, and the gui client refuses a quote
    /// whose cap isn't the one asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_slippage_bps: Option<u32>,
}

/// The amount a swap converts. Each variant names the direction exactly once,
/// so a request can't name one direction and denominate the amount in the
/// other: bitcoin converts to USDB and USDB to bitcoin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SwapAmount {
    /// Convert `amount_in`, in the asset converted out of.
    ///
    /// Exact for USDB, unless the quote reports an
    /// [`ConversionQuote::amount_adjustment`]. For bitcoin it is a target: the
    /// SDK takes an exact input only out of a token, so the bridge converts
    /// the sats into a USDB target, and the pool pads the sats by up to the
    /// slippage cap. The quote's `amount_in` can then differ from the request
    /// with no adjustment. Review always shows [`ConversionQuote::amount_in`],
    /// never the figure the user typed.
    Exact { amount_in: AssetAmount },
    /// Convert all of `from`. The bridge works out the figure: the whole
    /// token balance for USDB, and the spendable sats less fees for bitcoin.
    /// [`ConversionQuote::amount_in`] reports what it settled on.
    Max { from: WalletAsset },
}

impl SwapAmount {
    /// The asset converted out of.
    pub fn from_asset(&self) -> WalletAsset {
        match self {
            Self::Exact { amount_in } => amount_in.asset,
            Self::Max { from } => *from,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapParams {
    /// Opaque handle from [`PrepareSwapOk::handle`]. Single-use.
    pub prepare_handle: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareSendFromParams {
    /// Same as [`PrepareSendParams::input`], except for a Spark invoice
    /// requesting a token (see [`Method::PrepareSendFrom`]).
    pub input: String,
    /// Same as [`PrepareSendParams::amount_sat`].
    #[serde(default)]
    pub amount_sat: Option<u64>,
    /// Which balance pays. [`WalletAsset::Bitcoin`] keeps today's
    /// [`Method::PrepareSend`] behaviour, including its USDB shortfall
    /// conversion. [`WalletAsset::Usdb`] converts the full amount from USDB.
    pub pay_from: WalletAsset,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareLnurlPayFromParams {
    /// Same as [`PrepareLnurlPayParams::input`].
    pub input: String,
    /// Same as [`PrepareLnurlPayParams::amount_sat`].
    pub amount_sat: u64,
    /// Same as [`PrepareLnurlPayParams::comment`].
    #[serde(default)]
    pub comment: Option<String>,
    /// Which balance pays, as in [`PrepareSendFromParams::pay_from`].
    pub pay_from: WalletAsset,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareUsdbSendParams {
    /// A Spark address. Anything else is refused.
    pub input: String,
    /// What the recipient gets, in USDB base units. A decimal string on the
    /// wire.
    #[serde(with = "u128_dec")]
    pub amount: u128,
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
    /// Shared between [`Method::GetAutoConvert`] and
    /// [`Method::SetAutoConvert`]: both report the state after the call.
    AutoConvert(AutoConvertState),
    GetConversionLimits(ConversionLimits),
    PrepareSwap(PrepareSwapOk),
    Swap(SwapOk),
    PrepareSendFrom(PrepareSendFromOk),
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
    /// The USDB balance at full precision, reported separately from
    /// [`Self::balance_sats`] so the gui can show it as its own row.
    ///
    /// `None` means no USDB balance to show. From S1b on, the bridge reports
    /// only a nonzero balance of the configured USDB token whose metadata
    /// names that token, so `None` from it means the wallet holds none. A
    /// bridge older than S1b leaves the field out, so its `None` says nothing
    /// about the holding; the gui can't tell the two apart and shows no USDB
    /// row for either. Treat `Some` with a zero balance the same as `None`.
    /// Never fall back to [`Self::stable_balance`], which isn't checked
    /// against the token's metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usdb_balance: Option<UsdbBalanceSnapshot>,
    /// Sats that left [`Self::balance_sats`] and wait to be recovered
    /// on-chain (SDK 0.28.0+). The gui shows the figure; it can't drive the
    /// recovery yet. `0` from a bridge before 0.28.0, and left off the wire
    /// when `0`, so an older gui sees the frame it always did.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub recoverable_funds_sats: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// The wallet's USDB holding at the SDK's full `u128` precision.
///
/// [`StableBalanceSnapshot`] clamps the balance to `u64`. This keeps the
/// SDK's width and names the token, so the gui never has to assume which
/// token the figure is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsdbBalanceSnapshot {
    /// The SDK's token identifier (`btkn1…`).
    pub token_identifier: String,
    /// Balance in token base units (`balance / 10^decimals` is the dollar
    /// value).
    #[serde(with = "u128_dec")]
    pub balance: u128,
    /// Token base-units precision.
    pub decimals: u32,
    /// Display ticker, e.g. "USDB".
    pub ticker: String,
}

/// A balance the Spark wallet holds.
///
/// Not named `SparkAsset`: the bridge already imports the SDK's
/// `breez_sdk_spark::SparkAsset`, which has a different shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalletAsset {
    /// Amounts are in sats.
    Bitcoin,
    /// Amounts are in USDB token base units.
    Usdb,
}

/// An amount with its denomination attached, so a sats figure can't be read
/// as token base units or the other way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetAmount {
    pub asset: WalletAsset,
    /// Sats for [`WalletAsset::Bitcoin`], token base units for
    /// [`WalletAsset::Usdb`]. A decimal string on the wire.
    #[serde(with = "u128_dec")]
    pub amount: u128,
}

/// Bridge-driven auto-convert, as reported by [`Method::GetAutoConvert`],
/// [`Method::SetAutoConvert`] and [`Event::AutoConvertStateChanged`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoConvertState {
    /// Whether incoming bitcoin is converted to USDB.
    pub enabled: bool,
    /// The circuit breaker on repeated conversion failures. When it trips,
    /// the bridge also turns auto-convert off, so `enabled` is `false`.
    pub breaker: AutoConvertBreaker,
    /// True while auto-convert is on but counts no bitcoin received, because
    /// a swap out of USDB may still deliver its bitcoin as an ordinary
    /// receive: its send ended in an error, or the bridge stopped while
    /// sending it. Sats already pending still convert. Turning auto-convert
    /// off ends it. Always `false` while off.
    ///
    /// Added in S3b and left out of the wire while `false`, so every other
    /// state reads as before. A bridge from before it never sends it, which
    /// reads as `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held_after_swap: bool,
}

/// Circuit breaker on auto-convert failures. Tagged so a pause can't be
/// represented without its reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AutoConvertBreaker {
    /// Conversions run when auto-convert is on.
    Ready,
    /// The bridge paused auto-convert after `failures` consecutive failed
    /// conversions. `reason` is the last failure's text, for the gui to show.
    Paused { failures: u32, reason: String },
}

/// Minimum amounts for a conversion. Each is `None` when the SDK sets no
/// minimum on that side.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConversionLimits {
    /// Least that can be converted, in the asset converted out of. Out of
    /// bitcoin the bridge reports the least its sizing can convert, about
    /// 0.2% above the pool's own minimum (S3b).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_from: Option<AssetAmount>,
    /// Least that can be received, in the asset converted into.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_to: Option<AssetAmount>,
}

/// Why the SDK changed the requested conversion amount. Mirrors
/// `breez_sdk_spark::AmountAdjustmentReason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmountAdjustment {
    /// Raised to the minimum conversion amount.
    FlooredToMinLimit,
    /// Raised to convert the whole token balance, so no balance below the
    /// minimum is left behind.
    IncreasedToAvoidDust,
}

/// The SDK's estimate for a conversion, every figure with its denomination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionQuote {
    /// What is converted, and the figure Review shows: never the one the user
    /// typed. For [`SwapAmount::Max`] it is the figure the bridge settled on.
    /// Out of USDB it differs from an exact request only with an
    /// [`Self::amount_adjustment`]. Out of bitcoin it can differ by up to
    /// about the slippage cap with no adjustment; see [`SwapAmount::Exact`].
    pub amount_in: AssetAmount,
    /// Estimated amount received. The least the conversion will deliver
    /// depends on the SDK's path (`flashnet.rs` at `a38117d`):
    ///
    /// - a swap, which is an exact input in both directions (S3b). This is
    ///   not a floor: the SDK prices an exact input again when it is sent and
    ///   signs that price less [`Self::max_slippage_bps`] as the minimum. Out
    ///   of USDB this figure already has the cap taken off the simulated
    ///   output; out of bitcoin it is the simulated output. The bridge
    ///   refuses a swap whose price moved against the user by more than the
    ///   cap since this estimate, so the least it delivers is about this
    ///   figure less the cap out of USDB, and less twice the cap out of
    ///   bitcoin;
    /// - toward a target (a pay-from conversion): this is the simulated
    ///   output, and the minimum is the target, about this figure less
    ///   [`Self::max_slippage_bps`].
    ///
    /// Below the minimum the conversion fails and is refunded.
    pub amount_out: AssetAmount,
    /// Conversion fee. The SDK denominates it in the asset converted out of.
    pub fee: AssetAmount,
    /// Set when the SDK changed the requested amount.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount_adjustment: Option<AmountAdjustment>,
    /// The slippage cap applied, in basis points.
    pub max_slippage_bps: u32,
}

/// Result of [`Method::PrepareSwap`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareSwapOk {
    /// Opaque handle to pass to [`Method::Swap`]. Single-use, and evicted by
    /// the bridge's prepare TTL like a send handle.
    pub handle: String,
    pub quote: ConversionQuote,
    pub limits: ConversionLimits,
}

/// Where a swap is in its lifecycle. Mirrors `breez_sdk_spark::ConversionStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapStatus {
    /// Submitted and not finished.
    Pending,
    Completed,
    /// Failed, e.g. the SDK's initial send was rejected.
    Failed,
    /// Failed, and the SDK still has to refund the input.
    RefundNeeded,
    /// Failed, and the input was refunded.
    Refunded,
}

/// Result of [`Method::Swap`].
///
/// Only this answer says the swap was submitted. An error answer is a
/// definite failure only for the refusals before submission listed on
/// [`Method::Swap`]: [`ErrorKind::BadRequest`] and
/// [`ErrorKind::ShuttingDown`], which the bridge sends only for a swap it
/// hasn't started. An [`ErrorKind::Sdk`] or [`ErrorKind::NotConnected`]
/// error leaves the outcome unknown, to be reconciled from Activity:
/// `NotConnected` because the gui's client makes it up for requests still in
/// flight when the bridge dies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapOk {
    /// The [`PaymentSummary::id`] of the swap's Activity row, which is also
    /// the id later [`Event::SwapStatusChanged`] events carry. One id, one
    /// Activity item.
    ///
    /// The SDK's self-transfer `send_payment` returns the conversion's
    /// receive payment and hides the send leg from `list_payments` as its
    /// child, so the bridge reports that receive payment's id. Read from the
    /// SDK source at `a38117d`; not yet run.
    pub swap_id: String,
    /// The swap's status as the SDK answered it, worked out as
    /// [`PaymentConversionSummary::status`] is. Usually `Completed`: the SDK
    /// answers a self-transfer once the converted funds have arrived. It is
    /// `Pending` when the SDK answers before it has stored the receive with
    /// its conversion information; the bridge then follows the swap and
    /// emits [`Event::SwapStatusChanged`].
    pub status: SwapStatus,
}

/// Result of [`Method::PrepareSendFrom`], [`Method::PrepareLnurlPayFrom`] and
/// [`Method::PrepareUsdbSend`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareSendFromOk {
    /// The ordinary send preview. Its handle executes through
    /// [`Method::SendPayment`].
    pub prepare: PrepareSendOk,
    /// The balance the bridge prepared the send from. The gui checks it
    /// matches the request.
    pub pay_from: WalletAsset,
    /// The USDB → bitcoin conversion funding the send. The bridge fills it in
    /// whenever the SDK's prepare carries a conversion estimate, whichever
    /// balance pays, so the gui can show it before the user confirms:
    ///
    /// - paying from USDB always attaches one; paying from bitcoin attaches
    ///   one only to cover a shortfall;
    /// - apart from a USDB invoice paid from USDB ([`Self::token_payment`]),
    ///   a conversion is the only token leg these methods prepare, because
    ///   [`Method::PrepareSendFrom`] refuses every other token invoice and
    ///   LNURL has none. So `prepare.has_token_leg` is exactly
    ///   `Some(conversion.is_some() || token_payment.is_some())`.
    ///
    /// The gui client rejects a response that breaks either rule, including
    /// a token leg with no conversion: Review would show a bitcoin payment
    /// while a token moves, and couldn't tell it from a dropped quote.
    ///
    /// Boxed for the same reason as [`PrepareSendOk::cross_chain`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversion: Option<Box<ConversionQuote>>,
    /// The token the send itself moves, when it is not bitcoin: a Spark
    /// invoice that asks for USDB ([`ParseInputOk::usdb_invoice`]), or a
    /// [`Method::PrepareUsdbSend`] to a Spark address, paid straight from the
    /// USDB balance with no conversion.
    ///
    /// For such a send `prepare.amount_sat` and `prepare.fee_sat` are `0`:
    /// the SDK's figures are token base units, and this carries them with
    /// their denomination instead. `prepare.has_token_leg` is `Some(true)`
    /// and [`Self::conversion`] is `None`. The gui client accepts a token leg
    /// without a conversion only in that shape, paying from USDB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_payment: Option<TokenPayment>,
}

/// A send that moves a token directly, as [`PrepareSendFromOk::token_payment`]
/// reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenPayment {
    /// What the recipient is paid: the invoice's own amount, or the one
    /// asked for when sending to a Spark address.
    pub amount: AssetAmount,
    /// The SDK's fee for the transfer, in the same token.
    pub fee: AssetAmount,
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
    /// Identifiers and destination for the payment's detail view. Empty
    /// when the bridge predates them, so an old bridge still decodes.
    #[serde(default)]
    pub details: PaymentDetailSummary,
}

/// The parts of a Spark `PaymentDetails` that Activity shows. The HTLC
/// preimage is left out: the SDK redacts it from logs, and this type is
/// logged with `Debug`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentDetailSummary {
    /// Lightning HTLC payment hash.
    #[serde(default)]
    pub payment_hash: Option<String>,
    /// BOLT11/BOLT12 invoice, or the Spark invoice a transfer fulfilled.
    #[serde(default)]
    pub invoice: Option<String>,
    /// Lightning address of an LNURL-pay send.
    #[serde(default)]
    pub lightning_address: Option<String>,
    /// Bitcoin txid of a deposit or withdrawal, or a token transaction hash.
    #[serde(default)]
    pub tx_id: Option<String>,
    /// The BTC ⇄ USDB conversion behind this payment. Set on a swap's or an
    /// auto-convert's row, on a refund of one, and on a send funded by a
    /// conversion; a cross-chain provider's conversion isn't one. `None` on
    /// every other row, and from a bridge older than S3b.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversion: Option<PaymentConversionSummary>,
}

/// A payment's conversion, as Activity shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentConversionSummary {
    /// The SDK's `conversion_details.status` when the payment has them,
    /// otherwise the row's own AMM `conversion_info` status, otherwise
    /// `Pending`. A self-transfer's row has no `conversion_details` at
    /// `a38117d`, so a swap's status comes from its AMM information.
    /// `Completed` only once the payment itself is completed; a completed
    /// conversion on a payment that failed is `Failed`, since nothing
    /// arrived. Updated in place by [`Event::SwapStatusChanged`].
    pub status: SwapStatus,
    /// What the BTC ⇄ USDB step converted. `None` until the SDK fills in the
    /// steps, which it does later from child payments, or when there is no
    /// BTC ⇄ USDB step. The SDK builds steps only for a payment with
    /// `conversion_details`, so a swap's own row has none: Activity shows its
    /// own amount.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amounts: Option<ConversionAmounts>,
}

/// Both sides of a BTC ⇄ USDB conversion, each with its denomination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionAmounts {
    /// Converted out of.
    pub from: AssetAmount,
    /// Received.
    pub to: AssetAmount,
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
    /// For a Spark invoice: whether it asks to be paid in the wallet's USDB
    /// token. Such an invoice is paid from the USDB balance, with no amount
    /// from the gui: the invoice names its own, in token base units, which
    /// the sats-typed [`Self::amount_sat`] can't carry. `false` for every
    /// other input, including an invoice asking for any other token, which
    /// the bridge refuses.
    ///
    /// `false` from a bridge older than the field, so the gui sends the
    /// invoice down the ordinary path, where the bridge refuses it.
    #[serde(default)]
    pub usdb_invoice: bool,
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
    /// Whether the prepared payment carries a **token-transfer leg** — a direct
    /// token send, or an AMM conversion feeding the send.
    ///
    /// This exists because the bridge *drops the caller's idempotency key* on
    /// any send with such a leg: the SDK rejects the two together (see
    /// `execute_regular_send`, and `orchestrate_send` in the SDK). A retry of
    /// one of these is therefore unguarded, and the gui has to know that to
    /// pick the right [retry guard](../../coincube-gui/src/app/state/spark/send.rs).
    ///
    /// The gui cannot infer it from [`Self::cross_chain`]. Stable Balance can
    /// auto-attach a token→BTC conversion to an *ordinary sats send* whenever
    /// the sat balance can't cover amount + fee — no cross-chain quote in
    /// sight. Before this field existed, the gui reported "protected by an
    /// idempotency key" for exactly those sends while the bridge had already
    /// dropped the key.
    ///
    /// **`None` means "the bridge didn't say", not "no token leg."** A plain
    /// `bool` would collapse those two, and `serde`'s default would resolve the
    /// silence to `false` — the *unguarded* reading — reintroducing the exact
    /// double-pay this field exists to prevent. The bridge is a separate cargo
    /// workspace ([`exclude`d from the root one](../../Cargo.toml)) built by its
    /// own `cargo build --manifest-path` invocation, and the gui will happily
    /// spawn whatever binary it finds in `coincube-spark-bridge/target/` or at
    /// `COINCUBE_SPARK_BRIDGE_PATH`. Rebuilding only the gui after touching this
    /// crate leaves a bridge behind that has never heard of this field. Callers
    /// must treat `None` as "assume a token leg" — see `retry_guard_for`.
    #[serde(default)]
    pub has_token_leg: Option<bool>,
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
    /// Payment id of the resulting Spark wallet transfer. `None` when the
    /// deposit was claimed before maturity (SDK 0.25.0+): that transfer
    /// settles asynchronously, and the gui sees it land through
    /// `PaymentSucceeded` / `DepositsChanged` like any other incoming
    /// transfer. Which case applies follows from the deposit's maturity and
    /// the fee ceiling, not from anything the caller asked for.
    #[serde(default)]
    pub payment_id: Option<String>,
    /// Amount claimed in sats — the deposit's `amount_sat` minus any
    /// internal fees the SDK deducted. `None` in the asynchronous case
    /// above; the caller still knows the deposit's own amount.
    #[serde(default)]
    pub amount_sat: Option<u64>,
    /// Set when nothing was claimed yet (SDK 0.28.0+). Not a failure: the
    /// SDK keeps claiming the deposit on its own, early as confirmations
    /// accrue or else at maturity. `payment_id` and `amount_sat` are `None`
    /// with it. A bridge before 0.28.0 never sets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred: Option<ClaimDeferral>,
}

/// Why a deposit claim was deferred rather than made. Mirrors the SDK's
/// `ClaimDeferredReason`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ClaimDeferral {
    /// Claiming now costs more than the fee ceiling allows. Unlike depth,
    /// this does not clear on its own: the deposit is claimed at maturity.
    MaxFeeExceeded {
        required_fee_sats: u64,
        max_fee_sats: u64,
    },
    /// The provider offers no early claim at the deposit's current depth.
    /// The next confirmation may bring one within reach.
    NoEarlyClaimAvailable,
    /// The provider refused the early claim or could not be reached.
    ProviderDeclined { message: String },
}

/// Phase 6: boolean-flattened view of the SDK's `UserSettings`. The
/// gui only ever cares whether Stable Balance is active — it never
/// renders the USDB label. Private mode is surfaced here as a
/// preview field for a future "privacy" toggle but is not yet wired
/// into the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetUserSettingsOk {
    /// `true` when bridge-driven auto-convert is on. Before S2b this was the
    /// SDK's Stable Balance; the gui built before S2b reconciles its per-Cube
    /// Stable Balance preference against it, which moves that preference
    /// onto auto-convert.
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
/// The token a [`Event::PaymentSucceeded`] moved, so its amount (in the
/// token's base units) can be read as that token. A token payment the SDK
/// reported without its metadata still carries one, with an empty
/// `identifier`/`ticker` and `decimals` 0: it is a token, just not one the
/// gui can name, so it is never read as sats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentToken {
    /// The Spark token identifier (`btkn1…`); empty when unknown.
    pub identifier: String,
    /// Display ticker (e.g. "USDB"); empty when unknown.
    pub ticker: String,
    /// Decimals of the token's base units.
    pub decimals: u32,
    /// Whether this is the wallet's USDB, by its identifier (the bridge
    /// knows it; a ticker can be claimed by any token).
    #[serde(default)]
    pub usdb: bool,
}

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
    /// non-Lightning payments. For a token payment (`token` is `Some`),
    /// `amount_sat` is not sats: it carries the token's base units, as
    /// Spark's `Payment.amount` does.
    PaymentSucceeded {
        id: String,
        amount_sat: i64,
        #[serde(default)]
        bolt11: Option<String>,
        /// The token a token payment moved (e.g. USDB); `None` for a
        /// bitcoin payment and from a bridge that predates the field.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<PaymentToken>,
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
    /// The bridge switched auto-convert off on its own after
    /// [`STABLE_BALANCE_PAUSE_THRESHOLD`] consecutive failed conversions.
    /// Sent with [`Event::AutoConvertStateChanged`], for the gui built
    /// before S2b: it records "off" for the Cube on this event, so its next
    /// reconcile doesn't turn auto-convert back on. `reason` is the last
    /// failure's text (e.g. `Pool has no liquidity`) for the gui to show;
    /// `failures` is how many failed in a row.
    StableBalancePaused { reason: String, failures: u32 },
    /// Bridge-driven auto-convert changed: switched on or off, its breaker
    /// tripped, or it holds receives after a swap
    /// ([`AutoConvertState::held_after_swap`]). Settings re-renders from
    /// `state`.
    ///
    /// Emitted from S2b on. A gui built before this variant (#952) can't
    /// parse it, and its reader stops on an unparseable line, so such a gui
    /// must not run a bridge from S2b on, including one picked through
    /// `COINCUBE_SPARK_BRIDGE_PATH`. Release builds ship both halves from
    /// one build, so they always match.
    AutoConvertStateChanged { state: AutoConvertState },
    /// A swap from [`Method::Swap`] moved to `status`. `swap_id` is
    /// [`SwapOk::swap_id`], the [`PaymentSummary::id`] of the swap's Activity
    /// row, so the gui updates that row's
    /// [`PaymentDetailSummary::conversion`]. Emitted from S3b on, only for a
    /// swap whose [`SwapOk::status`] wasn't finished: the bridge re-reads it
    /// on each sync until it is, and only within the session that ran it.
    /// In a rare race it can arrive just before the [`SwapOk`] it follows.
    /// The same caution as [`Event::AutoConvertStateChanged`] applies.
    SwapStatusChanged { swap_id: String, status: SwapStatus },
    /// Details of an already stored payment changed after it settled, e.g.
    /// a cross-chain receive's conversion info arriving late (SDK 0.28.0+).
    /// The gui re-reads its payment lists; nothing is announced, since the
    /// payment's own state event already was. The same caution as
    /// [`Event::AutoConvertStateChanged`] applies.
    PaymentUpdated { id: String },
}

/// How many consecutive failed auto-convert conversions the bridge
/// tolerates before it pauses auto-convert and emits
/// [`Event::AutoConvertStateChanged`] and [`Event::StableBalancePaused`].
/// Enough to skip a one-off AMM hiccup, few enough that a broken pool
/// doesn't fail on every receive. Lives in the protocol crate so the gui
/// can name the same number in its copy.
pub const STABLE_BALANCE_PAUSE_THRESHOLD: u32 = 3;

/// Slippage cap, in basis points, on every BTC ⇄ USDB conversion the bridge
/// requests: 50 bps is 0.5%. Lives here so the bridge's requests and the
/// gui's copy name the same number.
pub const CONVERSION_MAX_SLIPPAGE_BPS: u32 = 50;

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

#[cfg(test)]
mod tests {
    #[test]
    fn a_route_is_the_same_route_when_only_its_limits_moved() {
        let route = CrossChainRoute {
            provider: "orchestra".to_string(),
            chain: "base".to_string(),
            chain_id: Some("8453".to_string()),
            asset: "USDC".to_string(),
            contract_address: Some("0xusdc".to_string()),
            decimals: 6,
            btc_source_supported: true,
            btc_limits: Some(CrossChainRouteLimits {
                min_usd_cents: Some(500),
                min_sats: Some(4_500),
                ..Default::default()
            }),
        };
        let repriced = CrossChainRoute {
            btc_limits: Some(CrossChainRouteLimits {
                min_usd_cents: Some(500),
                min_sats: Some(4_700),
                ..Default::default()
            }),
            ..route.clone()
        };
        assert!(route.same_route_as(&repriced));
        assert!(route.same_route_as(&CrossChainRoute {
            btc_limits: None,
            ..route.clone()
        }));
        assert!(!route.same_route_as(&CrossChainRoute {
            chain: "arbitrum".to_string(),
            chain_id: Some("42161".to_string()),
            ..route.clone()
        }));
        assert!(!route.same_route_as(&CrossChainRoute {
            asset: "USDT".to_string(),
            ..route.clone()
        }));
    }

    use super::*;
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    /// The exact `prepare_send` response (USDT on Solana) that used to fail to
    /// parse with "u128 is not supported": its `estimated_out` / `fee_amount`
    /// are `u128`, nested under the flattened [`Response`] envelope. Kept verbatim
    /// from the bug report — bare integers, the pre-fix on-wire form — so this
    /// also proves the deserializer still tolerates that shape.
    const CROSS_CHAIN_PREPARE_FRAME: &str = r#"{"type":"response","id":79,"ok":{"kind":"prepare_send","data":{"handle":"97f19c3d-1a09-4240-ab15-61571e6ceaba","amount_sat":10173,"fee_sat":43,"method":"CrossChainAddress","cross_chain":{"route":{"provider":"boltz","chain":"Solana","asset":"USDT","decimals":6,"btc_source_supported":true},"estimated_out":6499079,"fee_amount":102531,"source_transfer_fee_sats":43,"expires_at":"1784180130","retry_safe":true}}}}"#;

    fn prepare_ok_from_frame(frame: &str) -> PrepareSendOk {
        let parsed: Frame = serde_json::from_str(frame).expect("frame must parse");
        let Frame::Response(resp) = parsed else {
            panic!("expected a Response frame");
        };
        let ResponseResult::Ok(OkPayload::PrepareSend(prep)) = resp.result else {
            panic!("expected an Ok(PrepareSend) payload");
        };
        prep
    }

    #[test]
    fn cross_chain_prepare_frame_with_integer_amounts_parses() {
        // Before the fix this returned Err("u128 is not supported").
        let prep = prepare_ok_from_frame(CROSS_CHAIN_PREPARE_FRAME);
        // Predates `has_token_leg`, so it also pins that field's `serde(default)`:
        // an older frame must still parse rather than fail the whole send, and
        // must land on `None` — "the bridge didn't say" — never on `Some(false)`,
        // which is the reading that would let the gui offer a blind retry of a
        // send whose idempotency key the bridge had dropped.
        assert_eq!(prep.has_token_leg, None);
        let quote = prep.cross_chain.expect("cross_chain quote present");
        assert_eq!(quote.estimated_out, 6_499_079);
        assert_eq!(quote.fee_amount, 102_531);
        assert_eq!(quote.source_transfer_fee_sats, 43);
    }

    #[test]
    fn cross_chain_quote_round_trips_u128_beyond_u64() {
        // A value past u64::MAX proves we keep the full u128 range (and that a
        // bare JSON number would have been unsafe — this can't be one).
        let big = u128::from(u64::MAX) + 1; // 18446744073709551616
        let frame = Frame::Response(Response {
            id: 79,
            result: ResponseResult::Ok(OkPayload::PrepareSend(PrepareSendOk {
                handle: "h".into(),
                amount_sat: 10173,
                fee_sat: 43,
                method: "CrossChainAddress".into(),
                cross_chain: Some(Box::new(CrossChainQuote {
                    route: CrossChainRoute {
                        provider: "boltz".into(),
                        chain: "Solana".into(),
                        chain_id: None,
                        asset: "USDT".into(),
                        contract_address: None,
                        decimals: 6,
                        btc_source_supported: true,
                        btc_limits: None,
                    },
                    estimated_out: big,
                    fee_amount: 102_531,
                    source_transfer_fee_sats: 43,
                    expires_at: "1784180130".into(),
                    retry_safe: true,
                })),
                has_token_leg: Some(true),
            })),
        });

        let json = serde_json::to_string(&frame).expect("serialize");
        // On the wire the u128 fields are decimal strings, never bare numbers.
        assert!(
            json.contains(r#""estimated_out":"18446744073709551616""#),
            "u128 must serialize as a string: {json}"
        );

        let quote = prepare_ok_from_frame(&json)
            .cross_chain
            .expect("quote survives round-trip");
        assert_eq!(quote.estimated_out, big);
        assert_eq!(quote.fee_amount, 102_531);
    }

    #[test]
    fn cross_chain_address_optional_amount_round_trips_and_omits_none() {
        // `amount: Option<u128>` under the flattened GetCrossChainRoutes payload.
        let big = u128::from(u64::MAX) + 5;
        let present = Frame::Response(Response {
            id: 1,
            result: ResponseResult::Ok(OkPayload::GetCrossChainRoutes(CrossChainRoutesOk {
                address: Some(CrossChainAddress {
                    address: "0xabc".into(),
                    family: "evm".into(),
                    contract_address: None,
                    chain_id: None,
                    amount: Some(big),
                }),
                routes: vec![],
            })),
        });
        let json = serde_json::to_string(&present).expect("serialize");
        assert!(
            json.contains(r#""amount":"18446744073709551620""#),
            "{json}"
        );

        let parsed: Frame = serde_json::from_str(&json).expect("parse");
        let Frame::Response(Response {
            result: ResponseResult::Ok(OkPayload::GetCrossChainRoutes(ok)),
            ..
        }) = parsed
        else {
            panic!("expected GetCrossChainRoutes");
        };
        assert_eq!(ok.address.and_then(|a| a.amount), Some(big));

        // A `None` amount must be omitted entirely, not emitted as null/"".
        let absent = CrossChainAddress {
            address: "0xabc".into(),
            family: "evm".into(),
            contract_address: None,
            chain_id: None,
            amount: None,
        };
        let json = serde_json::to_string(&absent).expect("serialize");
        assert!(
            !json.contains("amount"),
            "None amount must be omitted: {json}"
        );
    }

    #[test]
    fn request_frames_use_snake_case_method_tags_and_expected_params() {
        let set_stable = Frame::Request(Request {
            id: 7,
            method: Method::SetStableBalance(SetStableBalanceParams { enabled: true }),
        });
        assert_eq!(
            serde_json::to_value(set_stable).expect("serialize"),
            json!({
                "type": "request",
                "id": 7,
                "method": "set_stable_balance",
                "params": { "enabled": true }
            })
        );

        let receive_spark = Frame::Request(Request {
            id: 8,
            method: Method::ReceiveSpark,
        });
        assert_eq!(
            serde_json::to_value(receive_spark).expect("serialize"),
            json!({
                "type": "request",
                "id": 8,
                "method": "receive_spark"
            })
        );
    }

    #[test]
    fn response_helpers_emit_ok_and_error_envelopes() {
        assert_eq!(
            serde_json::to_value(Frame::Response(Response::ok(
                11,
                OkPayload::SetStableBalance {}
            )))
            .expect("serialize ok"),
            json!({
                "type": "response",
                "id": 11,
                "ok": {
                    "kind": "set_stable_balance",
                    "data": {}
                }
            })
        );

        assert_eq!(
            serde_json::to_value(Frame::Response(Response::err(
                12,
                ErrorKind::BadRequest,
                "bad frame"
            )))
            .expect("serialize err"),
            json!({
                "type": "response",
                "id": 12,
                "err": {
                    "kind": "bad_request",
                    "message": "bad frame"
                }
            })
        );
    }

    #[test]
    fn events_default_missing_optional_payload_fields() {
        let parsed: Frame = serde_json::from_value(json!({
            "type": "event",
            "event": "payment_succeeded",
            "payload": {
                "id": "payment-1",
                "amount_sat": 42
            }
        }))
        .expect("event parses");
        let Frame::Event(Event::PaymentSucceeded {
            id,
            amount_sat,
            bolt11,
            token,
        }) = parsed
        else {
            panic!("expected payment succeeded event");
        };
        assert_eq!(id, "payment-1");
        assert_eq!(amount_sat, 42);
        assert_eq!(bolt11, None);
        assert_eq!(token, None);

        assert_eq!(
            serde_json::to_value(Frame::Event(Event::DepositsChanged)).expect("serialize"),
            json!({
                "type": "event",
                "event": "deposits_changed"
            })
        );
    }

    #[test]
    fn stable_balance_paused_event_round_trips() {
        let frame = Frame::Event(Event::StableBalancePaused {
            reason: "Pool has no liquidity".to_string(),
            failures: STABLE_BALANCE_PAUSE_THRESHOLD,
        });
        let value = serde_json::to_value(&frame).expect("serialize");
        assert_eq!(
            value,
            json!({
                "type": "event",
                "event": "stable_balance_paused",
                "payload": { "reason": "Pool has no liquidity", "failures": 3 }
            })
        );

        let Frame::Event(Event::StableBalancePaused { reason, failures }) =
            serde_json::from_value(value).expect("deserialize")
        else {
            panic!("expected stable balance paused event");
        };
        assert_eq!(reason, "Pool has no liquidity");
        assert_eq!(failures, 3);
    }

    #[test]
    fn payment_summary_defaults_keep_old_list_payments_frames_parsing() {
        let payment: PaymentSummary = serde_json::from_value(json!({
            "id": "payment-1",
            "amount_sat": 21,
            "timestamp": 1_700_000_000u64,
            "status": "completed",
            "direction": "receive"
        }))
        .expect("legacy payment summary parses");

        assert_eq!(payment.fees_sat, 0);
        assert_eq!(payment.token_amount, None);
        assert_eq!(payment.token_decimals, None);
        assert_eq!(payment.token_ticker, None);
        assert_eq!(payment.method, "");
        assert_eq!(payment.description, None);
        assert_eq!(payment.details, PaymentDetailSummary::default());
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct OptionalAmountProbe {
        #[serde(with = "super::opt_u128_dec")]
        amount: Option<u128>,
    }

    #[test]
    fn optional_u128_codec_accepts_null_and_serializes_none_as_null() {
        let parsed: OptionalAmountProbe =
            serde_json::from_value(json!({ "amount": null })).expect("null parses as None");
        assert_eq!(parsed, OptionalAmountProbe { amount: None });

        let encoded =
            serde_json::to_value(OptionalAmountProbe { amount: None }).expect("serialize None");
        assert_eq!(encoded, json!({ "amount": null }));
    }

    #[test]
    fn u128_codecs_reject_non_numeric_json_with_useful_errors() {
        let err = serde_json::from_value::<Frame>(json!({
            "type": "response",
            "id": 1,
            "ok": {
                "kind": "prepare_send",
                "data": {
                    "handle": "h",
                    "amount_sat": 1,
                    "fee_sat": 0,
                    "method": "CrossChainAddress",
                    "cross_chain": {
                        "route": {
                            "provider": "boltz",
                            "chain": "Solana",
                            "asset": "USDT",
                            "decimals": 6,
                            "btc_source_supported": true
                        },
                        "estimated_out": true,
                        "fee_amount": "1",
                        "source_transfer_fee_sats": 0,
                        "expires_at": "1784180130",
                        "retry_safe": true
                    }
                }
            }
        }))
        .expect_err("bool is not a u128");
        assert!(
            err.to_string()
                .contains("a u128 as a decimal string or a JSON integer"),
            "{err}"
        );

        let err = serde_json::from_value::<OptionalAmountProbe>(json!({ "amount": false }))
            .expect_err("bool is not an optional u128");
        assert!(
            err.to_string()
                .contains("a u128 as a decimal string or a JSON integer"),
            "{err}"
        );
    }

    #[test]
    fn u128_codec_accepts_positive_signed_integers_and_rejects_negative() {
        let frame = json!({
            "type": "response",
            "id": 79,
            "ok": {
                "kind": "prepare_send",
                "data": {
                    "handle": "h",
                    "amount_sat": 10173,
                    "fee_sat": 43,
                    "method": "CrossChainAddress",
                    "cross_chain": {
                        "route": {
                            "provider": "boltz",
                            "chain": "Solana",
                            "asset": "USDT",
                            "decimals": 6,
                            "btc_source_supported": true
                        },
                        "estimated_out": 42,
                        "fee_amount": -1,
                        "source_transfer_fee_sats": 43,
                        "expires_at": "1784180130",
                        "retry_safe": true
                    }
                }
            }
        });
        let err = serde_json::from_value::<Frame>(frame).expect_err("negative u128 rejected");
        // Asserts on the message `U128Visitor` builds itself, the way the
        // sibling test above asserts on its `expecting()` string. Asserting on
        // a std error's Display made this test hostage to std's wording.
        assert!(err.to_string().contains("u128 out of range: -1"), "{err}");
    }

    // ---- P0 (#950): USDB balance, auto-convert, swap, pay-from ----

    /// Past `u64::MAX`, so a figure that went through a `u64` or a JSON number
    /// anywhere on the path would not survive.
    const BIG: u128 = u64::MAX as u128 + 7; // 18446744073709551622
    const BIG_STR: &str = "18446744073709551622";

    fn usdb(amount: u128) -> AssetAmount {
        AssetAmount {
            asset: WalletAsset::Usdb,
            amount,
        }
    }

    fn sats(amount: u128) -> AssetAmount {
        AssetAmount {
            asset: WalletAsset::Bitcoin,
            amount,
        }
    }

    fn quote() -> ConversionQuote {
        ConversionQuote {
            amount_in: usdb(BIG),
            amount_out: sats(15_003),
            fee: usdb(1_200),
            amount_adjustment: Some(AmountAdjustment::IncreasedToAvoidDust),
            max_slippage_bps: CONVERSION_MAX_SLIPPAGE_BPS,
        }
    }

    fn legacy_prepare() -> PrepareSendOk {
        PrepareSendOk {
            handle: "h".into(),
            amount_sat: 15_000,
            fee_sat: 3,
            method: "Bolt11Invoice".into(),
            cross_chain: None,
            has_token_leg: Some(true),
        }
    }

    fn request_json(id: u64, method: Method) -> serde_json::Value {
        serde_json::to_value(Frame::Request(Request { id, method })).expect("serialize")
    }

    fn response_json(id: u64, payload: OkPayload) -> serde_json::Value {
        serde_json::to_value(Frame::Response(Response::ok(id, payload))).expect("serialize")
    }

    /// Serialize, then parse the line back the way the reader does, through
    /// the flattened envelope.
    fn ok_round_trip(payload: OkPayload) -> OkPayload {
        let line =
            serde_json::to_string(&Frame::Response(Response::ok(1, payload))).expect("serialize");
        let Frame::Response(Response {
            result: ResponseResult::Ok(payload),
            ..
        }) = serde_json::from_str(&line).expect("parse")
        else {
            panic!("expected an ok response: {line}");
        };
        payload
    }

    fn method_round_trip(method: Method) -> Method {
        let line =
            serde_json::to_string(&Frame::Request(Request { id: 1, method })).expect("serialize");
        let Frame::Request(Request { method, .. }) = serde_json::from_str(&line).expect("parse")
        else {
            panic!("expected a request: {line}");
        };
        method
    }

    fn event_round_trip(event: Event) -> Event {
        let line = serde_json::to_string(&Frame::Event(event)).expect("serialize");
        let Frame::Event(event) = serde_json::from_str(&line).expect("parse") else {
            panic!("expected an event: {line}");
        };
        event
    }

    #[test]
    fn new_request_frames_have_exact_wire_shapes() {
        assert_eq!(
            request_json(1, Method::GetAutoConvert),
            json!({ "type": "request", "id": 1, "method": "get_auto_convert" })
        );
        assert_eq!(
            request_json(
                2,
                Method::SetAutoConvert(SetAutoConvertParams { enabled: false })
            ),
            json!({
                "type": "request", "id": 2, "method": "set_auto_convert",
                "params": { "enabled": false }
            })
        );
        assert_eq!(
            request_json(
                3,
                Method::GetConversionLimits(GetConversionLimitsParams {
                    from: WalletAsset::Usdb
                })
            ),
            json!({
                "type": "request", "id": 3, "method": "get_conversion_limits",
                "params": { "from": "usdb" }
            })
        );
        assert_eq!(
            request_json(
                4,
                Method::PrepareSwap(PrepareSwapParams {
                    amount: SwapAmount::Exact {
                        amount_in: usdb(BIG)
                    },
                    max_slippage_bps: None,
                })
            ),
            json!({
                "type": "request", "id": 4, "method": "prepare_swap",
                "params": {
                    "amount": { "kind": "exact", "amount_in": { "asset": "usdb", "amount": BIG_STR } }
                }
            })
        );
        assert_eq!(
            request_json(
                5,
                Method::PrepareSwap(PrepareSwapParams {
                    amount: SwapAmount::Exact {
                        amount_in: sats(25_000)
                    },
                    max_slippage_bps: Some(CONVERSION_MAX_SLIPPAGE_BPS),
                })
            ),
            json!({
                "type": "request", "id": 5, "method": "prepare_swap",
                "params": {
                    "amount": {
                        "kind": "exact", "amount_in": { "asset": "bitcoin", "amount": "25000" }
                    },
                    "max_slippage_bps": 50
                }
            })
        );
        assert_eq!(
            request_json(
                8,
                Method::PrepareSwap(PrepareSwapParams {
                    amount: SwapAmount::Max {
                        from: WalletAsset::Usdb
                    },
                    max_slippage_bps: None,
                })
            ),
            json!({
                "type": "request", "id": 8, "method": "prepare_swap",
                "params": { "amount": { "kind": "max", "from": "usdb" } }
            })
        );
        assert_eq!(
            request_json(
                6,
                Method::Swap(SwapParams {
                    prepare_handle: "swap-h".into()
                })
            ),
            json!({
                "type": "request", "id": 6, "method": "swap",
                "params": { "prepare_handle": "swap-h" }
            })
        );
        assert_eq!(
            request_json(
                7,
                Method::PrepareSendFrom(PrepareSendFromParams {
                    input: "lnbc1".into(),
                    amount_sat: Some(15_000),
                    pay_from: WalletAsset::Usdb,
                })
            ),
            json!({
                "type": "request", "id": 7, "method": "prepare_send_from",
                "params": { "input": "lnbc1", "amount_sat": 15000, "pay_from": "usdb" }
            })
        );
        assert_eq!(
            request_json(
                10,
                Method::PrepareLnurlPayFrom(PrepareLnurlPayFromParams {
                    input: "shop@example.test".into(),
                    amount_sat: 15_000,
                    comment: Some("thanks".into()),
                    pay_from: WalletAsset::Usdb,
                })
            ),
            json!({
                "type": "request", "id": 10, "method": "prepare_lnurl_pay_from",
                "params": {
                    "input": "shop@example.test", "amount_sat": 15000,
                    "comment": "thanks", "pay_from": "usdb"
                }
            })
        );
        assert_eq!(
            request_json(
                11,
                Method::PrepareUsdbSend(PrepareUsdbSendParams {
                    input: "spark1".into(),
                    amount: 12_500_000,
                })
            ),
            json!({
                "type": "request", "id": 11, "method": "prepare_usdb_send",
                "params": { "input": "spark1", "amount": "12500000" }
            })
        );
    }

    #[test]
    fn new_request_frames_round_trip() {
        let Method::PrepareSwap(p) = method_round_trip(Method::PrepareSwap(PrepareSwapParams {
            amount: SwapAmount::Exact {
                amount_in: usdb(BIG),
            },
            max_slippage_bps: Some(75),
        })) else {
            panic!("expected prepare_swap");
        };
        assert_eq!(
            p.amount,
            SwapAmount::Exact {
                amount_in: usdb(BIG)
            }
        );
        assert_eq!(p.amount.from_asset(), WalletAsset::Usdb);
        assert_eq!(p.max_slippage_bps, Some(75));

        let Method::PrepareSwap(p) = method_round_trip(Method::PrepareSwap(PrepareSwapParams {
            amount: SwapAmount::Max {
                from: WalletAsset::Bitcoin,
            },
            max_slippage_bps: None,
        })) else {
            panic!("expected prepare_swap");
        };
        assert_eq!(
            p.amount,
            SwapAmount::Max {
                from: WalletAsset::Bitcoin
            }
        );
        assert_eq!(p.amount.from_asset(), WalletAsset::Bitcoin);

        let Method::PrepareLnurlPayFrom(p) =
            method_round_trip(Method::PrepareLnurlPayFrom(PrepareLnurlPayFromParams {
                input: "shop@example.test".into(),
                amount_sat: 21,
                comment: None,
                pay_from: WalletAsset::Bitcoin,
            }))
        else {
            panic!("expected prepare_lnurl_pay_from");
        };
        assert_eq!(p.input, "shop@example.test");
        assert_eq!(p.amount_sat, 21);
        assert_eq!(p.comment, None);
        assert_eq!(p.pay_from, WalletAsset::Bitcoin);

        let Method::PrepareUsdbSend(p) =
            method_round_trip(Method::PrepareUsdbSend(PrepareUsdbSendParams {
                input: "spark1".into(),
                amount: BIG,
            }))
        else {
            panic!("expected prepare_usdb_send");
        };
        assert_eq!(p.input, "spark1");
        assert_eq!(p.amount, BIG);

        let Method::PrepareSendFrom(p) =
            method_round_trip(Method::PrepareSendFrom(PrepareSendFromParams {
                input: "sp1".into(),
                amount_sat: None,
                pay_from: WalletAsset::Bitcoin,
            }))
        else {
            panic!("expected prepare_send_from");
        };
        assert_eq!(p.input, "sp1");
        assert_eq!(p.amount_sat, None);
        assert_eq!(p.pay_from, WalletAsset::Bitcoin);

        let Method::SetAutoConvert(p) =
            method_round_trip(Method::SetAutoConvert(SetAutoConvertParams {
                enabled: true,
            }))
        else {
            panic!("expected set_auto_convert");
        };
        assert!(p.enabled);

        let Method::GetConversionLimits(p) =
            method_round_trip(Method::GetConversionLimits(GetConversionLimitsParams {
                from: WalletAsset::Bitcoin,
            }))
        else {
            panic!("expected get_conversion_limits");
        };
        assert_eq!(p.from, WalletAsset::Bitcoin);

        let Method::Swap(p) = method_round_trip(Method::Swap(SwapParams {
            prepare_handle: "swap-h".into(),
        })) else {
            panic!("expected swap");
        };
        assert_eq!(p.prepare_handle, "swap-h");

        assert!(matches!(
            method_round_trip(Method::GetAutoConvert),
            Method::GetAutoConvert
        ));
    }

    #[test]
    fn prepare_swap_slippage_defaults_to_none_when_absent() {
        let parsed: Frame = serde_json::from_value(json!({
            "type": "request", "id": 4, "method": "prepare_swap",
            "params": { "amount": { "kind": "max", "from": "bitcoin" } }
        }))
        .expect("parses");
        let Frame::Request(Request {
            method: Method::PrepareSwap(p),
            ..
        }) = parsed
        else {
            panic!("expected prepare_swap");
        };
        assert_eq!(p.max_slippage_bps, None);
    }

    /// Why pay-from is a method and not a field. A bridge that predates it
    /// drops an unknown method (`unknown_method_is_rejected_instead_of_silently_accepted`
    /// in `tests/wire_contract.rs`), but it would ignore an unknown field and
    /// prepare a bitcoin send.
    #[test]
    fn prepare_send_silently_ignores_an_unknown_pay_from_field() {
        let parsed: Frame = serde_json::from_value(json!({
            "type": "request", "id": 1, "method": "prepare_send",
            "params": { "input": "lnbc1", "amount_sat": 15000, "pay_from": "usdb" }
        }))
        .expect("an unknown field is ignored");
        let Frame::Request(Request {
            method: Method::PrepareSend(p),
            ..
        }) = parsed
        else {
            panic!("expected prepare_send");
        };
        assert_eq!(p.input, "lnbc1");
        assert_eq!(p.amount_sat, Some(15_000));
    }

    /// `prepare_send` is untouched by P0: same tag, same fields, nothing new.
    #[test]
    fn legacy_prepare_send_wire_shapes_are_unchanged() {
        assert_eq!(
            request_json(
                9,
                Method::PrepareSend(PrepareSendParams {
                    input: "lnbc1".into(),
                    amount_sat: Some(15_000),
                    fees_included: false,
                })
            ),
            json!({
                "type": "request", "id": 9, "method": "prepare_send",
                "params": { "input": "lnbc1", "amount_sat": 15000 }
            })
        );
        assert_eq!(
            request_json(
                10,
                Method::PrepareSend(PrepareSendParams {
                    input: "bc1q".into(),
                    amount_sat: Some(15_000),
                    fees_included: true,
                })
            ),
            json!({
                "type": "request", "id": 10, "method": "prepare_send",
                "params": { "input": "bc1q", "amount_sat": 15000, "fees_included": true }
            })
        );
        assert_eq!(
            response_json(9, OkPayload::PrepareSend(legacy_prepare())),
            json!({
                "type": "response", "id": 9,
                "ok": {
                    "kind": "prepare_send",
                    "data": {
                        "handle": "h", "amount_sat": 15000, "fee_sat": 3,
                        "method": "Bolt11Invoice", "has_token_leg": true
                    }
                }
            })
        );
    }

    #[test]
    fn get_info_usdb_balance_is_additive_and_keeps_full_precision() {
        // An older bridge's frame: no `usdb_balance`.
        let old: GetInfoOk = serde_json::from_value(json!({
            "balance_sats": 21,
            "identity_pubkey": "02ab",
            "stable_balance": { "balance": 5, "decimals": 6, "ticker": "USDB" }
        }))
        .expect("old get_info parses");
        assert_eq!(old.usdb_balance, None);
        assert_eq!(old.stable_balance.map(|s| s.balance), Some(5));

        // `None` is omitted, so an older gui sees exactly today's frame.
        let none = GetInfoOk {
            balance_sats: 21,
            identity_pubkey: "02ab".into(),
            stable_balance: None,
            usdb_balance: None,
            recoverable_funds_sats: 0,
        };
        assert_eq!(
            response_json(1, OkPayload::GetInfo(none)),
            json!({
                "type": "response", "id": 1,
                "ok": {
                    "kind": "get_info",
                    "data": { "balance_sats": 21, "identity_pubkey": "02ab", "stable_balance": null }
                }
            })
        );

        let snapshot = UsdbBalanceSnapshot {
            token_identifier: "btkn1usdb".into(),
            balance: BIG,
            decimals: 6,
            ticker: "USDB".into(),
        };
        let some = GetInfoOk {
            balance_sats: 21,
            identity_pubkey: "02ab".into(),
            stable_balance: None,
            usdb_balance: Some(snapshot.clone()),
            recoverable_funds_sats: 0,
        };
        assert_eq!(
            response_json(1, OkPayload::GetInfo(some.clone()))["ok"]["data"]["usdb_balance"],
            json!({
                "token_identifier": "btkn1usdb", "balance": BIG_STR,
                "decimals": 6, "ticker": "USDB"
            })
        );
        let OkPayload::GetInfo(back) = ok_round_trip(OkPayload::GetInfo(some)) else {
            panic!("expected get_info");
        };
        assert_eq!(back.usdb_balance, Some(snapshot));
    }

    /// 0.28.0: recoverable funds travel only when there are some, and an
    /// older bridge's frame reads as none.
    #[test]
    fn get_info_recoverable_funds_are_additive() {
        let info = |recoverable_funds_sats| GetInfoOk {
            balance_sats: 21,
            identity_pubkey: "02ab".into(),
            stable_balance: None,
            usdb_balance: None,
            recoverable_funds_sats,
        };
        let data = |ok| response_json(1, OkPayload::GetInfo(ok))["ok"]["data"].clone();
        assert!(data(info(0)).get("recoverable_funds_sats").is_none());
        assert_eq!(data(info(330))["recoverable_funds_sats"], 330);
        let OkPayload::GetInfo(back) = ok_round_trip(OkPayload::GetInfo(info(330))) else {
            panic!("expected get_info");
        };
        assert_eq!(back.recoverable_funds_sats, 330);
    }

    /// 0.28.0: a deferred claim names its reason; a settled claim's frame is
    /// what it always was, and an older bridge's frame reads as not deferred.
    #[test]
    fn claim_deposit_deferral_is_additive_and_tagged() {
        let settled = ClaimDepositOk {
            payment_id: Some("p".into()),
            amount_sat: Some(5_000),
            deferred: None,
        };
        assert_eq!(
            response_json(1, OkPayload::ClaimDeposit(settled))["ok"]["data"],
            json!({ "payment_id": "p", "amount_sat": 5000 })
        );
        let deferred = ClaimDepositOk {
            payment_id: None,
            amount_sat: None,
            deferred: Some(ClaimDeferral::MaxFeeExceeded {
                required_fee_sats: 1_200,
                max_fee_sats: 800,
            }),
        };
        assert_eq!(
            response_json(1, OkPayload::ClaimDeposit(deferred.clone()))["ok"]["data"]["deferred"],
            json!({ "reason": "max_fee_exceeded", "required_fee_sats": 1200, "max_fee_sats": 800 })
        );
        let OkPayload::ClaimDeposit(back) = ok_round_trip(OkPayload::ClaimDeposit(deferred)) else {
            panic!("expected claim_deposit");
        };
        assert_eq!(
            back.deferred,
            Some(ClaimDeferral::MaxFeeExceeded {
                required_fee_sats: 1_200,
                max_fee_sats: 800
            })
        );
        let old: ClaimDepositOk =
            serde_json::from_value(json!({ "payment_id": null, "amount_sat": null }))
                .expect("old claim_deposit parses");
        assert_eq!(old.deferred, None);
    }

    #[test]
    fn auto_convert_payloads_have_exact_wire_shapes_and_round_trip() {
        let ready = AutoConvertState {
            enabled: true,
            breaker: AutoConvertBreaker::Ready,
            held_after_swap: false,
        };
        assert_eq!(
            response_json(1, OkPayload::AutoConvert(ready.clone())),
            json!({
                "type": "response", "id": 1,
                "ok": {
                    "kind": "auto_convert",
                    "data": { "enabled": true, "breaker": { "status": "ready" } }
                }
            })
        );
        let OkPayload::AutoConvert(back) = ok_round_trip(OkPayload::AutoConvert(ready.clone()))
        else {
            panic!("expected auto_convert");
        };
        assert_eq!(back, ready);

        let paused = AutoConvertState {
            enabled: false,
            breaker: AutoConvertBreaker::Paused {
                failures: STABLE_BALANCE_PAUSE_THRESHOLD,
                reason: "Pool has no liquidity".into(),
            },
            held_after_swap: false,
        };
        assert_eq!(
            serde_json::to_value(&paused).expect("serialize"),
            json!({
                "enabled": false,
                "breaker": { "status": "paused", "failures": 3, "reason": "Pool has no liquidity" }
            })
        );
        let OkPayload::AutoConvert(back) = ok_round_trip(OkPayload::AutoConvert(paused.clone()))
        else {
            panic!("expected auto_convert");
        };
        assert_eq!(back, paused);
    }

    /// The hold after a swap is on the wire only while set, so every other
    /// state reads as before. A state from a bridge without it reads as not
    /// held, and a gui without it ignores it.
    #[test]
    fn the_hold_after_a_swap_is_on_the_wire_only_while_set() {
        let held = AutoConvertState {
            enabled: true,
            breaker: AutoConvertBreaker::Ready,
            held_after_swap: true,
        };
        assert_eq!(
            response_json(1, OkPayload::AutoConvert(held.clone())),
            json!({
                "type": "response", "id": 1,
                "ok": {
                    "kind": "auto_convert",
                    "data": {
                        "enabled": true,
                        "breaker": { "status": "ready" },
                        "held_after_swap": true
                    }
                }
            })
        );
        let OkPayload::AutoConvert(back) = ok_round_trip(OkPayload::AutoConvert(held.clone()))
        else {
            panic!("expected auto_convert");
        };
        assert_eq!(back, held);
        let Event::AutoConvertStateChanged { state: back } =
            event_round_trip(Event::AutoConvertStateChanged {
                state: held.clone(),
            })
        else {
            panic!("expected auto_convert_state_changed");
        };
        assert_eq!(back, held);

        let not_held = AutoConvertState {
            held_after_swap: false,
            ..held.clone()
        };
        let wire = serde_json::to_value(&not_held).expect("serialize");
        assert_eq!(
            wire,
            json!({ "enabled": true, "breaker": { "status": "ready" } })
        );
        let from_an_older_bridge: AutoConvertState = serde_json::from_value(wire).expect("parse");
        assert_eq!(from_an_older_bridge, not_held);

        /// [`AutoConvertState`] as a gui from before the hold has it.
        #[derive(Debug, PartialEq, Deserialize)]
        struct Before {
            enabled: bool,
            breaker: AutoConvertBreaker,
        }
        let in_an_older_gui: Before =
            serde_json::from_value(serde_json::to_value(&held).expect("serialize"))
                .expect("an older gui ignores the field");
        assert_eq!(
            in_an_older_gui,
            Before {
                enabled: true,
                breaker: AutoConvertBreaker::Ready
            }
        );
    }

    #[test]
    fn a_paused_breaker_without_a_reason_does_not_parse() {
        let err = serde_json::from_value::<AutoConvertState>(json!({
            "enabled": false,
            "breaker": { "status": "paused", "failures": 3 }
        }))
        .expect_err("a pause needs its reason");
        assert!(err.to_string().contains("reason"), "{err}");
    }

    #[test]
    fn conversion_limits_have_exact_wire_shapes_and_round_trip() {
        let limits = ConversionLimits {
            min_from: Some(usdb(BIG)),
            min_to: Some(sats(1_000)),
        };
        assert_eq!(
            response_json(1, OkPayload::GetConversionLimits(limits.clone())),
            json!({
                "type": "response", "id": 1,
                "ok": {
                    "kind": "get_conversion_limits",
                    "data": {
                        "min_from": { "asset": "usdb", "amount": BIG_STR },
                        "min_to": { "asset": "bitcoin", "amount": "1000" }
                    }
                }
            })
        );
        let OkPayload::GetConversionLimits(back) =
            ok_round_trip(OkPayload::GetConversionLimits(limits.clone()))
        else {
            panic!("expected get_conversion_limits");
        };
        assert_eq!(back, limits);

        // No minimum on either side: both omitted, and an empty object parses.
        assert_eq!(
            serde_json::to_value(ConversionLimits::default()).expect("serialize"),
            json!({})
        );
        let empty: ConversionLimits = serde_json::from_value(json!({})).expect("parses");
        assert_eq!(empty, ConversionLimits::default());
    }

    #[test]
    fn prepare_swap_ok_has_exact_wire_shape_and_round_trips() {
        let ok = PrepareSwapOk {
            handle: "swap-h".into(),
            quote: quote(),
            limits: ConversionLimits {
                min_from: Some(usdb(1_000_000)),
                min_to: None,
            },
        };
        assert_eq!(
            response_json(1, OkPayload::PrepareSwap(ok.clone())),
            json!({
                "type": "response", "id": 1,
                "ok": {
                    "kind": "prepare_swap",
                    "data": {
                        "handle": "swap-h",
                        "quote": {
                            "amount_in": { "asset": "usdb", "amount": BIG_STR },
                            "amount_out": { "asset": "bitcoin", "amount": "15003" },
                            "fee": { "asset": "usdb", "amount": "1200" },
                            "amount_adjustment": "increased_to_avoid_dust",
                            "max_slippage_bps": 50
                        },
                        "limits": { "min_from": { "asset": "usdb", "amount": "1000000" } }
                    }
                }
            })
        );
        let OkPayload::PrepareSwap(back) = ok_round_trip(OkPayload::PrepareSwap(ok.clone())) else {
            panic!("expected prepare_swap");
        };
        assert_eq!(back.handle, ok.handle);
        assert_eq!(back.quote, ok.quote);
        assert_eq!(back.limits, ok.limits);

        // No adjustment: omitted on the wire, `None` when absent.
        let mut plain = quote();
        plain.amount_adjustment = None;
        let value = serde_json::to_value(&plain).expect("serialize");
        assert!(value.get("amount_adjustment").is_none(), "{value}");
        let back: ConversionQuote = serde_json::from_value(value).expect("parse");
        assert_eq!(back.amount_adjustment, None);

        assert_eq!(
            serde_json::to_value(AmountAdjustment::FlooredToMinLimit).expect("serialize"),
            json!("floored_to_min_limit")
        );
    }

    #[test]
    fn swap_ok_and_every_status_have_exact_wire_shapes() {
        for (status, tag) in [
            (SwapStatus::Pending, "pending"),
            (SwapStatus::Completed, "completed"),
            (SwapStatus::Failed, "failed"),
            (SwapStatus::RefundNeeded, "refund_needed"),
            (SwapStatus::Refunded, "refunded"),
        ] {
            let ok = SwapOk {
                swap_id: "swap-1".into(),
                status,
            };
            assert_eq!(
                response_json(1, OkPayload::Swap(ok)),
                json!({
                    "type": "response", "id": 1,
                    "ok": { "kind": "swap", "data": { "swap_id": "swap-1", "status": tag } }
                })
            );
            let OkPayload::Swap(back) = ok_round_trip(OkPayload::Swap(SwapOk {
                swap_id: "swap-1".into(),
                status,
            })) else {
                panic!("expected swap");
            };
            assert_eq!(back.status, status);
        }
    }

    #[test]
    fn prepare_send_from_ok_has_exact_wire_shape_and_round_trips() {
        let ok = PrepareSendFromOk {
            prepare: legacy_prepare(),
            pay_from: WalletAsset::Usdb,
            conversion: Some(Box::new(quote())),
            token_payment: None,
        };
        assert_eq!(
            response_json(1, OkPayload::PrepareSendFrom(ok)),
            json!({
                "type": "response", "id": 1,
                "ok": {
                    "kind": "prepare_send_from",
                    "data": {
                        "prepare": {
                            "handle": "h", "amount_sat": 15000, "fee_sat": 3,
                            "method": "Bolt11Invoice", "has_token_leg": true
                        },
                        "pay_from": "usdb",
                        "conversion": {
                            "amount_in": { "asset": "usdb", "amount": BIG_STR },
                            "amount_out": { "asset": "bitcoin", "amount": "15003" },
                            "fee": { "asset": "usdb", "amount": "1200" },
                            "amount_adjustment": "increased_to_avoid_dust",
                            "max_slippage_bps": 50
                        }
                    }
                }
            })
        );

        let OkPayload::PrepareSendFrom(back) =
            ok_round_trip(OkPayload::PrepareSendFrom(PrepareSendFromOk {
                prepare: legacy_prepare(),
                pay_from: WalletAsset::Usdb,
                conversion: Some(Box::new(quote())),
                token_payment: None,
            }))
        else {
            panic!("expected prepare_send_from");
        };
        assert_eq!(back.pay_from, WalletAsset::Usdb);
        assert_eq!(back.prepare.handle, "h");
        assert_eq!(back.prepare.has_token_leg, Some(true));
        assert_eq!(back.conversion.as_deref(), Some(&quote()));

        // Paying from bitcoin with no shortfall conversion: omitted, `None`.
        let value = serde_json::to_value(PrepareSendFromOk {
            prepare: PrepareSendOk {
                has_token_leg: Some(false),
                ..legacy_prepare()
            },
            pay_from: WalletAsset::Bitcoin,
            conversion: None,
            token_payment: None,
        })
        .expect("serialize");
        assert!(value.get("conversion").is_none(), "{value}");
        assert!(value.get("token_payment").is_none(), "{value}");
        let back: PrepareSendFromOk = serde_json::from_value(value).expect("parse");
        assert!(back.conversion.is_none());
        assert!(back.token_payment.is_none());
    }

    /// A USDB invoice paid from USDB carries its USDB figures, denominated,
    /// beside zero sats; a bridge older than the field reads as `None`.
    #[test]
    fn a_token_payment_has_exact_wire_shape_and_round_trips() {
        let usdb = |amount| AssetAmount {
            asset: WalletAsset::Usdb,
            amount,
        };
        let token_payment = TokenPayment {
            amount: usdb(2_500_000),
            fee: usdb(1_200),
        };
        let ok = PrepareSendFromOk {
            prepare: PrepareSendOk {
                amount_sat: 0,
                fee_sat: 0,
                method: "SparkInvoice".into(),
                ..legacy_prepare()
            },
            pay_from: WalletAsset::Usdb,
            conversion: None,
            token_payment: Some(token_payment),
        };
        let value = serde_json::to_value(&ok).expect("serialize");
        assert_eq!(
            value["token_payment"],
            json!({
                "amount": { "asset": "usdb", "amount": "2500000" },
                "fee": { "asset": "usdb", "amount": "1200" }
            })
        );
        let back: PrepareSendFromOk = serde_json::from_value(value.clone()).expect("parse");
        assert_eq!(back.token_payment, Some(token_payment));

        let mut legacy = value;
        legacy
            .as_object_mut()
            .expect("object")
            .remove("token_payment");
        let back: PrepareSendFromOk = serde_json::from_value(legacy).expect("parse");
        assert!(back.token_payment.is_none());

        // `usdb_invoice` defaults to false from an older bridge.
        let parsed: ParseInputOk =
            serde_json::from_value(json!({ "kind": "spark_invoice" })).expect("parse");
        assert!(!parsed.usdb_invoice);
    }

    #[test]
    fn new_events_have_exact_wire_shapes_and_round_trip() {
        let state = AutoConvertState {
            enabled: false,
            breaker: AutoConvertBreaker::Paused {
                failures: 3,
                reason: "Pool has no liquidity".into(),
            },
            held_after_swap: false,
        };
        assert_eq!(
            serde_json::to_value(Frame::Event(Event::AutoConvertStateChanged {
                state: state.clone()
            }))
            .expect("serialize"),
            json!({
                "type": "event",
                "event": "auto_convert_state_changed",
                "payload": {
                    "state": {
                        "enabled": false,
                        "breaker": {
                            "status": "paused", "failures": 3,
                            "reason": "Pool has no liquidity"
                        }
                    }
                }
            })
        );
        let Event::AutoConvertStateChanged { state: back } =
            event_round_trip(Event::AutoConvertStateChanged {
                state: state.clone(),
            })
        else {
            panic!("expected auto_convert_state_changed");
        };
        assert_eq!(back, state);

        assert_eq!(
            serde_json::to_value(Frame::Event(Event::SwapStatusChanged {
                swap_id: "swap-1".into(),
                status: SwapStatus::Refunded,
            }))
            .expect("serialize"),
            json!({
                "type": "event",
                "event": "swap_status_changed",
                "payload": { "swap_id": "swap-1", "status": "refunded" }
            })
        );
        let Event::SwapStatusChanged { swap_id, status } =
            event_round_trip(Event::SwapStatusChanged {
                swap_id: "swap-1".into(),
                status: SwapStatus::RefundNeeded,
            })
        else {
            panic!("expected swap_status_changed");
        };
        assert_eq!(swap_id, "swap-1");
        assert_eq!(status, SwapStatus::RefundNeeded);
    }

    #[test]
    fn asset_amounts_reject_a_negative_or_non_numeric_amount() {
        for bad in [json!(-1), json!("12.5"), json!("abc"), json!(true)] {
            assert!(
                serde_json::from_value::<AssetAmount>(json!({ "asset": "usdb", "amount": bad }))
                    .is_err(),
                "{bad} must not parse as an amount"
            );
        }
        assert!(
            serde_json::from_value::<AssetAmount>(json!({ "asset": "liquid", "amount": "1" }))
                .is_err(),
            "an unknown asset must not parse"
        );
    }

    #[test]
    fn a_swap_amount_needs_its_direction() {
        for bad in [
            json!({ "kind": "exact" }),
            json!({ "kind": "max" }),
            json!({ "kind": "max", "from": "liquid" }),
            json!({ "kind": "all", "from": "usdb" }),
            // The shape before `SwapAmount`: no `kind`.
            json!({ "asset": "usdb", "amount": "1" }),
        ] {
            assert!(
                serde_json::from_value::<SwapAmount>(bad.clone()).is_err(),
                "{bad} must not parse as a swap amount"
            );
        }
        assert!(
            serde_json::from_value::<PrepareSwapParams>(json!({
                "amount_in": { "asset": "usdb", "amount": "1" }
            }))
            .is_err(),
            "a swap request needs `amount`"
        );
    }

    #[test]
    fn prepare_lnurl_pay_from_comment_defaults_to_none_and_pay_from_is_required() {
        let p: PrepareLnurlPayFromParams = serde_json::from_value(json!({
            "input": "shop@example.test", "amount_sat": 21, "pay_from": "usdb"
        }))
        .expect("parses without a comment");
        assert_eq!(p.comment, None);
        assert_eq!(p.pay_from, WalletAsset::Usdb);

        // A legacy `prepare_lnurl_pay` body: without `pay_from` it is not a
        // pay-from request, and must not default to either balance.
        assert!(serde_json::from_value::<PrepareLnurlPayFromParams>(json!({
            "input": "shop@example.test", "amount_sat": 21
        }))
        .is_err());
        assert!(serde_json::from_value::<PrepareSendFromParams>(json!({
            "input": "lnbc1", "amount_sat": 21
        }))
        .is_err());
    }

    #[test]
    fn payment_detail_conversion_is_additive_and_keeps_denominations() {
        // An older bridge's details: no `conversion`.
        let old: PaymentDetailSummary = serde_json::from_value(json!({
            "payment_hash": null, "invoice": null,
            "lightning_address": null, "tx_id": "ab"
        }))
        .expect("old details parse");
        assert_eq!(old.conversion, None);
        assert_eq!(old.tx_id.as_deref(), Some("ab"));

        // `None` is omitted, so an older gui sees exactly today's object.
        assert_eq!(
            serde_json::to_value(PaymentDetailSummary::default()).expect("serialize"),
            json!({
                "payment_hash": null, "invoice": null,
                "lightning_address": null, "tx_id": null
            })
        );

        let swap = PaymentDetailSummary {
            conversion: Some(PaymentConversionSummary {
                status: SwapStatus::Completed,
                amounts: Some(ConversionAmounts {
                    from: usdb(BIG),
                    to: sats(15_003),
                }),
            }),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&swap).expect("serialize")["conversion"],
            json!({
                "status": "completed",
                "amounts": {
                    "from": { "asset": "usdb", "amount": BIG_STR },
                    "to": { "asset": "bitcoin", "amount": "15003" }
                }
            })
        );
        let back: PaymentDetailSummary =
            serde_json::from_value(serde_json::to_value(&swap).expect("serialize")).expect("parse");
        assert_eq!(back, swap);

        // Status before the SDK fills in the steps: `amounts` omitted.
        let pending = PaymentConversionSummary {
            status: SwapStatus::Pending,
            amounts: None,
        };
        assert_eq!(
            serde_json::to_value(&pending).expect("serialize"),
            json!({ "status": "pending" })
        );
        let back: PaymentConversionSummary =
            serde_json::from_value(json!({ "status": "pending" })).expect("parse");
        assert_eq!(back, pending);

        // It reaches the gui through the list payload, nested in a row.
        let row: PaymentSummary = serde_json::from_value(json!({
            "id": "swap-1", "amount_sat": 15003, "timestamp": 1,
            "status": "completed", "direction": "receive",
            "details": { "conversion": { "status": "refunded" } }
        }))
        .expect("row parses");
        assert_eq!(
            row.details.conversion.map(|c| c.status),
            Some(SwapStatus::Refunded)
        );
    }
}
