# Bitcoin Worldview Modes

Bitcoin Worldview is an opt-in presentation preference for Tenshu. The redesign's
navigation, layouts, forms, typography, financial hierarchy and wallet workflows
provide the common design. The modes vary the illustrations, decorative accents
and approved display copy.

## Modes

| Mode | Tagline | SHA-256 mainnet display | BLAKE2b mainnet display |
| --- | --- | --- | --- |
| Samurai | The Way of the Bitcoin | Bitcoin (BTC) | Bitcoin BLAKE2b (BTCB2) |
| Suitcoiner | The Market Has Spoken | Bitcoin (BTC) | LukeCoin (BTCB2) |
| Luke | No Spam Shall Pass | Spamcoin (BTC) | Bitcoin (XBT) |

- **Samurai** is the default. Preserve the existing Sumi-e artwork, sizing, colors
  and typography. Keep ordinary financial copy neutral.
- **Suitcoiner** uses Gilded Wall Street art. Draw from Art Deco, banknote engraving,
  vintage securities certificates and classical architecture. Use midnight navy,
  antique gold and warm ivory with restrained ornamentation.
- **Luke** uses the Purist's Woodcut. Draw from medieval printed manuscripts,
  monastic architecture, fortresses and crosshatching. Use parchment, black ink
  and restrained oxblood. Use symbolic imagery for the satire.

Keep humor in preferences, idle or empty states and optional post-success copy.
Keep confirmations, failures, security warnings and recovery instructions neutral.

## Device preference

Place **Bitcoin Worldview** in **App preferences → Appearance**, reachable from
the Launcher and inside a Cube, with the existing “This device” scope. Offer one
choice per mode that shows its tagline. Under the choices, show what the selected
mode changes: its network names from the chain presentation registry (BLAKE2b
only where that network is switched on) and example wording from the voice
catalog, for Samurai as for the other modes.

Persist the choice in installation-wide `global_settings.json` with the stable
values `samurai`, `suitcoiner` and `luke`. A missing value or an unrecognized
string selects Samurai. Reuse the locked single-field preference writer and
publish the accepted value after its save receipt. A failed save leaves the
previous choice active. The selection takes effect without restarting.

Worldview and System/Light/Dark are independent. As presentation consumers are
added, read the accepted mode at render time or refresh their presentation caches.
Retain pane, route, focus, draft, balance privacy and selected-chain state. A
worldview edit must never run the global reload that reconciles fork availability
and Split flows.

The **Artwork** row follows Bitcoin Worldview. Each mode saves its own
`existing` or `scene` choice under the named fields of `artwork` in
`global_settings.json`. The first option is the default when a mode has no
saved choice.

| Mode | First option (default) | Second option |
| --- | --- | --- |
| Samurai | Kage | Mountain Watch |
| Suitcoiner | Gilded Exchange | Gilded Wall Street |
| Luke | The Stone Crossing | Purist’s Woodcut |

Full scenes cover the 160-point Opening and 240-point empty activity slots,
plus Sent, Received, Restore complete, Recovery Kit protection and removal,
and the PIN prompt. Opening and empty activity share a scene. The six other
slots each have their own scene.

## Localization

Language is independent of worldview and color scheme. All language and worldview
combinations must work. Spanish is the first additional locale to validate.

- Give new user-facing strings stable message IDs with English fallback. Keep the
  setting name, help, taglines, humor and technical label templates in the
  presentation copy catalog. Translate full messages and named placeholders so
  Spanish can reorder words and punctuation naturally. Review Spanish satire for
  tone as well as accuracy.
- Initially treat Samurai, Suitcoiner, Luke and the chain aliases as proper names.
  Keep tickers and canonical chain identifiers stable. Translate field labels and
  network qualifiers, for example `Red: Bitcoin BLAKE2b · Red principal`.
- Select locale independently of the saved worldview enum. Keep translated copy
  out of preference keys, widget IDs, input parsing and routing.
- Validate Spanish across all three modes, both color schemes and English
  fallback. Check minimum window width, high zoom, focus, accent marks and
  wrapping of full taglines and technical network labels. Format numbers and
  currencies according to the app's locale policy when available, without
  changing their values, rounding rules, units or amount parsing in this feature.

The desktop language selector is a device preference under **App preferences →
Appearance**. It defaults to English and uses the accepted settings receipt
independently of worldview. The Spanish catalog includes Samurai's desktop
navigation, setup, wallet screens, settings, bundled quotes, and app-authored
error guidance. Translate messages at presentation time so the accepted locale
also applies to deferred views and errors received earlier. Preserve canonical
network names, addresses, amounts, and support codes. Diagnostics returned by
external services, devices, or libraries may appear in their original language.
Voice lines use the accepted language independently of the selected worldview.

## Chain identity and financial safety

Keep canonical `coincube_core::chain::ChainId` as the source of truth. The GUI
presentation registry maps `(WorldviewMode, ChainId)` to mainnet display aliases
only on approved surfaces. Preserve existing technical names for Testnet,
Testnet4, Signet, Regtest and BLAKE2b Testnet4.

Derive a mode-independent technical network label from the exact chain held by the
operation. Show it in the action area for signing, broadcast, replay and Split
review, address verification, network selection for a wallet operation, and import
or restore. For example, Luke may show `Bitcoin (XBT)` as secondary copy beside
`Bitcoin BLAKE2b · Mainnet`. Bitcoin SHA-256 and BLAKE2b share address encodings,
so an address or the projected `bitcoin::Network` cannot identify the chain.

Keep aliases out of chain selection and admission, addresses, transaction
construction and signing, PSBTs, descriptors, replay checks, broadcast targets,
fees, price symbols, Connect/API/RPC payloads, exports and wallet persistence.
Keep asset and amount arithmetic on the existing typed paths. Use the existing
wallet and chain data as input to presentation.

## Voice and humor

Samurai uses calm, direct language and is the default when a view has no
accepted worldview. Its voice lines are stoic: quiet discipline and elegant
simplicity, with no satire, market or ideology jokes, so they suit every user
as the neutral voice. Samurai speaks on the same passive surfaces as the other
modes, uses one chain-neutral line for an empty Vault, and stays out of the
consensus exchange in the picker. Suitcoiner treats market consensus and institutional
adoption with absolute confidence. Luke treats validation, sovereignty, and
the ledger with austere conviction. Deliver the satire sincerely. A short line
beside a clear label is usually enough.

Keep the English and Spanish pairs together in
[`coincube-ui/src/worldview_copy.rs`](../../coincube-ui/src/worldview_copy.rs).
The launcher welcome, overview, creation, empty history, synchronization,
payment success, and Appearance screens draw from that registry. The picker
lets Suitcoiner say “Consensus by market capitalization.” and Luke answer
“Consensus is not a popularity contest.” A switch message follows an accepted
device preference save. Rivalry Easter eggs are reserved for a later pass.

On a visible, synchronized Vault with a known zero balance, the active typed
mainnet selects the joke. Suitcoiner calls an empty BLAKE2b Vault
“institutionally compliant.” Luke is pleased when a SHA-256 Vault has no
Spamcoin. Samurai's empty-Vault line names neither chain. A hidden, loading, stale, or
unscanned balance and any test network suppress these lines in every mode.

Keep seed phrases, recovery instructions, error messages, fees, addresses,
confirmation statuses, and technical settings neutral. Payment captions are
secondary to the existing outcome and amount. Broadcasting does not establish
confirmation. Mode aliases are confined to the existing passive display surfaces.

## Artwork and accents

- Reuse the existing assets under `coincube-ui/static/images`. The shared
  `component/quote_display.rs` maps Kage and Cube art to first-launch, loading,
  empty and celebration contexts. `image.rs` supplies the castle mark, sealed
  Cube art for Restore and the recovery illustrations used by Backup and
  Recovery Kit. Preserve these files and their current Samurai bindings.
- Bundle artwork under `coincube-ui/static/images/worldview`. The existing
  sets keep their original screen illustrations, with dedicated send and
  receive artwork for Bitcoin, Lightning, Note, and Spark. The full-scene
  sets also have a separate scene for each of those payment directions and
  methods. Generic payment contexts use the corresponding Sent or Received
  artwork. Liquid-native payments and Liquid empty activity use the
  existing presentation.
- Mascot quote illustrations follow the selected artwork set, including on
  the startup error screen. Kage, the Broker, and the armored Warden are
  available in their existing artwork sets. Each full-scene set has its own
  portrait in the scene's style. Quote choice stays stable during a mode switch.
- Use restrained decorative accents in both color schemes. Preserve established
  warning, error, success, focus and chain-identification semantics. Keep asset
  symbols, user avatars, the interactive Launcher Cube and the app mark outside
  the decorative registry.

## Delivery and validation

1. Add this spec and the device preference. Test defaults, old and future
   settings, save receipts, reloading, failed saves and keyboard interaction.
2. Classify label call sites and add the presentation registry and canonical
   confirmation labels. Test all seven chain IDs in every mode. Compare fixed
   transaction fixtures across modes for selected inputs, outputs, fees, PSBTs,
   signing, replay eligibility and broadcast routing, including the twin-chain
   case with shared address encodings.
3. Add alternate illustrations and accents once the settings and identity
   boundaries are verified. Compare Samurai with the redesign baseline and
   verify live updates, fallback, contrast, focus and zoom on the supported
   desktop platforms. Validate Spanish against the same layouts when the
   locale is available.

Keychain remains a separate mobile signing surface. Its prompts retain their
canonical chain identity. Review phone-signing handoffs from Tenshu in each
worldview.
