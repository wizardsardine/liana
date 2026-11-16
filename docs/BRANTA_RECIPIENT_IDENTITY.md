# Recipient identity checks

COINCUBE Desktop performs optional, send-only Branta lookups during preparation of
external Spark BOLT11 payments and VAULT Bitcoin/BIP21 requests containing both
`branta_id` and `branta_secret`. No lookup runs on typing, internal-transfer paths,
Liquid, Spark-native addresses/invoices, stablecoin/cross-chain sends, plain
Bitcoin addresses, BOLT12, LNURL or Lightning addresses. Ark and silent-payment
support in the SDK does not enable those payment routes in COINCUBE Desktop.

The setting is local `global_settings.json:recipient_identity_checks`, exposed
in Cube Settings / Payment checks ("Recipient checks"). It is device-wide: it
applies to every Cube on this device. New and older installations default on. Turning it
off invalidates outstanding privacy tickets, cancels their asynchronous work and
hides old identity results across tabs. No lookup or logo client starts while
disabled. Turning it on enables future preparation only. No account synchronization,
check/result/preference telemetry, API key, HMAC secret or payment registration.

Lookups never send plaintext destinations or payment amounts. The SDK (4.0.0)
processes the original payload locally: on-chain requests send only their
`branta_id`, BOLT11 requests send an encrypted token derived from the invoice,
and plain addresses make no request at all. BOLT11 tokens are deterministic and
repeated lookups are linkable. Lookups do not follow redirects, and response
bodies are capped (see *SDK provenance*). Branta
can observe lookup timing and the connecting network address. Only an explicit
"View with Branta" click opens the SDK verification URL; the secret-bearing URL
is neither displayed nor logged. Opening Branta's privacy link is also an explicit
user action, not a lookup or prefetch.

## Where identity appears

- **Spark send**: the review shows a "Recipient identity (Branta)" status row.
  It reads "Identified · {platform}" when a current identity is bound to the
  prepared payment, "Not identified" when checks are on but nothing was
  identified (including every silent failure), and "Checks off" with a
  "Turn on in Settings" link to Payment checks when the setting is disabled.
  "Identified" names who posted the request; it is not a safety verdict.
- **VAULT PSBT review**: when a pending, broadcast-ready PSBT has exactly one
  external (non-change) output with a current identity, a single "Recipient
  identity" summary card shows that output's address and amount with its
  identity card(s). Multi-recipient PSBTs keep the per-output cards instead, so
  each identity stays next to its own output.
- **Broadcast modal**: the identity card for each output appears again before
  broadcast.
- Each identity card reads "Recipient identified" with platform, optional logo
  and description, and a "View with Branta" button.

## Payment correctness

Spark joins its existing payment preparation with a bounded identity lookup;
responses carry a generation, and identity is attached to the exact prepared
handle. Edits, cancellation and navigation invalidate the prior generation.
After the SDK classifies a BOLT11 invoice, Spark preparation receives the bare
invoice with outer whitespace and one case-insensitive `lightning:` wrapper removed.
Invoice bytes/case and checksum validation are preserved; Branta keeps the original
payment request. Other payment routes retain their existing input handling.

VAULT retains BIP21 input separately from the parsed/network-checked address used
for transaction construction. URI amounts populate an empty amount field;
disagreements invalidate the form. Unknown required fields and ambiguous amounts
are rejected. Each original recipient row is matched to a distinct script/amount
output in the actual prepared PSBT. If optional association cannot be made (for
example, recovery/MAX changes after redraft), the valid PSBT proceeds without
identity metadata. Explicit BIP21 requested amounts still constrain the final output
regardless of the privacy preference. Association handles response
reordering. The existing duplicate-address form restriction remains; association
logic itself never collapses equal addresses. Descriptor-owned deposit/change
outputs are excluded before lookup. Existing output and fee checks remain in the
signing/broadcast path. Identity appears beside each output in review and again
inside the final broadcast modal. Identity is not persisted in saved PSBTs.

The three-second lookup deadline includes logos. Batch identity lookups finish before
optional logos begin, so slow images cannot prevent later recipient checks. Logo variants load concurrently;
exhausting their remaining budget preserves the already-bound identity. A VAULT batch shares one deadline
and has at most three outstanding lookups. Unsupported input, empty response,
transport/parsing/decryption error or timeout shows nothing and does not block
sending. Positive identity requires a `match` from the SDK, which returns only payments
bound by decryption, and a returned destination of the same type equal to the
prepared destination. Identity attribution makes no claim of payment safety.

Only a `tampered` verdict from the SDK's `verify` blocks sending and requires
replacing the request. In SDK 4.0.0 it covers an on-chain BIP21 address that does
not match the address decrypted from `branta_id`, and on-chain addresses in one
request (path, `bc=`, `tb=`) that disagree. It is not reachable for BOLT11. VAULT
pays only the path address, so COINCUBE Desktop passes the SDK only the path address,
`branta_id`, `branta_secret` and any `bc=`/`tb=`. Other legs (`pay=`, `lno=`,
`lightning=`, `bolt12`, ...) are dropped first: the SDK would otherwise return
`unknown` for an unbindable leg before decrypting `branta_id`, which would hide a
swapped path address. Tab, CR and LF are removed first, as the wallet's parser and
the SDK both do, and only the path and the forwarded parameters are checked for
spaces, controls or bad encoding, so noise in a `label=` cannot skip the lookup. Identity therefore covers exactly the address being paid. `unknown`, `incomplete`
(Branta unreachable, 5xx or 429), wrong secrets and ordinary failures are silent,
so absence of a block is not a safety claim. The known bad destination stays blocked for the current form despite preference
changes, metadata/encoding changes, or edits to another recipient. Replace or
remove the affected destination to continue. Non-ASCII or ambiguous SDK payloads
are skipped.

## What a hit actually identifies, and how to test one

Branta identifies the **merchant who posted the payment request**, not the wallet
software that generated it. Its network list (`data.json` in
[BrantaOps/branta-network](https://github.com/BrantaOps/branta-network)) has two
sides: `wallets` — send-side consumers that render an identity, which is where
Blitz, Zeus and COINCUBE all sit — and `btcpay` — the instances that publish
payment requests. A wallet being "Branta-supporting" says nothing about whether
the invoices its users generate are registered. A personal invoice from a
Blitz user was never posted to Branta, so it resolves to nothing. Per Branta's
wallet docs, a miss is not an error: "a missing record just means the destination
was never posted to Branta."

Branta's two published Lightning fixtures cover disjoint halves of this pipeline,
and **neither one can demonstrate a success in the send panel**:

| Fixture | Derived-key lookup | Payable? |
|---|---|---|
| zk-lightning | resolves — platform "Peony Lane" | **no** — signed by a node with no channels and no `r` route hints, so the SSP answers `NO_PATH_FOUND` |
| lightning | 404 — registered **plaintext-only**; SDK 4.0.0 has no plaintext lookup mode | yes |

Fixture behavior was recorded against SDK 3.2.1; re-check it after any SDK upgrade.

So the fixture that resolves cannot be prepared, and preparation failure discards
the identity (identity binds to a prepared handle — see *Payment correctness*).
Do not read "nothing rendered" from either fixture as a broken integration.
Covering this path deterministically needs a fake backend whose `prepare_send`
succeeds; observing it live needs an invoice that is both payable and posted to
Branta, i.e. one from a `btcpay` list instance.

## SDK provenance and dependencies

Official Branta 4.0.0 source is vendored from reviewed commit
`6292bbd0c6a720e5c9273a4f4e8b827097db6e5e` (tag `v4.0.0`), with version requirement
`=4.0.0`. Two documented adapters are applied, both hash-pinned in
`vendor/branta/UPSTREAM_SHA256.json` and enforced by the `vendor_integrity` test:
the manifest reuses reqwest `0.12.18` with `rustls-tls` (a resolution fix; see
below for the runtime TLS backend), and `src/v2/client.rs` disables redirects and
caps lookup response bodies. No other SDK source was changed.
See [provenance](../vendor/branta/TENSHU_PROVENANCE.md).

4.0.0 returns one of `match`, `unknown`, `tampered` or `incomplete` from `verify`.
COINCUBE Desktop maps `match` with a bound destination to an identity, `tampered` to a block,
and everything else to silence. Descriptions now come only from encrypted payment
metadata, so older registrations that stored a plaintext description show their
platform without a description.

Upstream reqwest 0.13 uses platform-verifier, whose Apple dependency conflicts
with Breez's exact iOS build-dependency pin even on desktop because Cargo resolves
across targets. No Liquid file or dependency pin was changed.

Declaring `rustls-tls` does not select rustls at runtime. Cargo unifies reqwest
`0.12.18` features across the workspace, and Breez (`breez-sdk-liquid`,
`sdk-common`, `esplora-client` defaults) also enables `default-tls` (native-tls)
and `system-proxy`. With `default-tls` present, reqwest's `TlsBackend::default()`
is native-tls, and neither the SDK lookup client (`reqwest::Client::builder()`,
patched only to disable redirects) nor the logo client calls `use_rustls_tls()`.
Both therefore use the OS TLS stack and trust store (Security.framework on macOS,
including admin/user-added roots; OpenSSL system roots on Linux; SChannel on
Windows), and both honour `HTTP(S)_PROXY`/`ALL_PROXY`/`NO_PROXY` and the macOS/Windows system proxy
settings. Confirm with `cargo tree -e features -i reqwest@0.12.18`.

The lookup client follows no redirects: a 3xx response is treated as `incomplete`,
so a redirect target never receives the lookup token. Lookup response bodies are
capped and an oversized response is `incomplete`. The wrapper also bounds elapsed
time. Logo fetching rejects redirects and validates HTTPS and exact origin for both variants.
Branta Active Storage blob redirect URLs use its same-origin proxy route, avoiding
requests to object storage. Responses are limited to 256 KiB and PNG/JPEG only.
Decoding checks the actual format, caps each dimension at 5000 pixels and decoder
allocation at 100 MiB, and runs one image at a time off the async runtime. Originals
are resized to at most 64×64 before caching (32 images). The image, PNG and
fdeflate dependencies are optimized in development/test builds too; otherwise the
public 4501×4500 fixture exceeds the deadline just in decode/thumbnail work. A decode already running
when its lookup expires may finish locally; cancellation starts no additional network
work and cannot expose a stale identity. Each completed light/dark variant survives
a timeout of the other variant. All logo errors are silent. SVG and other image formats are not loaded.

## Validation

Wrapper tests use a fake `LookupBackend`. SDK 4.0.0 keeps its HTTP client trait
private, so the real `verify` is exercised only on paths that finish before any
request (conflicting on-chain addresses, plain and unbindable destinations). The
vendored SDK's own unit suite runs offline against local mock servers with
`cargo test --manifest-path vendor/branta/Cargo.toml --lib`; its examples and live
integration tests are never run.

References: [SDK](https://github.com/BrantaOps/branta-rust),
[send-side pricing](https://branta.pro/pricing),
[privacy details](https://branta.pro/your-data),
[privacy policy](https://branta.pro/privacy#automatic-collection).
