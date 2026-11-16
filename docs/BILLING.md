# Connect billing on the desktop

Plan & billing lives in Tenshu (Settings → Plan & billing). The desktop does not price plans itself. It renders `GET /api/v1/connect/plan` and `GET /api/v1/connect/features`, and it sends checkout and plan-change calls only when those responses say purchasing is open. Dollar amounts, entitlements, and campaign labels come from Connect. This page is the desktop contract; the API owns the charge ledger.

Offline, with no plan loaded, the launcher still has a tier: Free allows 1 Cube per network, Pro 3, Estate 10. That fallback is `AccountTier::cube_limit`. Connect's `max_allowed` replaces it as soon as `GET /api/v1/connect/cubes/limits` succeeds.

## Where the code lives

| Piece | Path |
|---|---|
| Wire types | `coincube-gui/src/services/coincube/mod.rs` (`ConnectPlan`, `PlanEntitlements`, `FeaturesResponse`, checkout and charge types) |
| HTTP | `coincube-gui/src/services/coincube/client.rs` |
| Session, checkout phases, plan sequencing | `coincube-gui/src/app/state/connect/account.rs` |
| Plan & billing, Change plan, Review, history | `coincube-gui/src/app/view/connect/billing.rs` |
| Feature-bullet and provenance filtering | `coincube-gui/src/app/view/connect/mod.rs` (`launch_feature_copy`, `plan_provenance_display`) |
| Cube slots on the launcher | `coincube-gui/src/home.rs`, `coincube-gui/src/home/launcher_chrome.rs` |

`PlanTier` is `free`, `pro`, or `estate`. The deserializer accepts `legacy` as Estate, because some older feature-catalog rows still use that name. `PlanStatus` is `active`, `past_due`, or `canceled`.

## What a plan response may omit

Every field on `PlanEntitlements` defaults. An entitlements object that adds or renames a field must still parse. The previous model required six booleans the API had stopped sending, so every account rendered as Free. An absent entitlement reading as off or zero is the safe direction.

These fields are also lenient (`lenient_option`): a value that does not parse, including an empty string or a future enum, becomes `None` instead of failing the whole plan:

- `billingCycle` — absent on Free and on APIs from before billing v2
- `pendingPlan`, `pendingBillingCycle` — a lower tier already paid for, starting when the current period ends
- `paymentMethod` — `bitcoin`, `card` (brand and last four only), or an unknown kind

`planProvenance` is optional server-authored display (`label`, `expiresAt`, `badge`). The desktop does not know campaign names. `plan_provenance_display` keeps the server strings and only replaces launch-banned wording (a stale label becomes a neutral line; a stale or empty badge is dropped). Purchased and free plans omit provenance.

`GET /connect/features` carries `pricingSchemaVersion`. This build fully renders version 2 (`SUPPORTED_PRICING_SCHEMA_VERSION`). A higher version shows a soft update note. Absent is version 0, which is not treated as newer.

## Screens

`plan_billing_ux` switches among four surfaces. Nothing is charged until the user leaves Review.

1. **Plan & billing.** Current plan, lifecycle notice, promo field, feature list, and the latest history rows. Opening it loads the plan and `GET /api/v1/connect/billing/history`.
2. **Change plan.** Monthly or annual, three tier rows, comparison table. Continue sends `ReviewPlanChange`.
3. **Review.** Plan, amount, dates covered, and how to pay. "Create invoice" sends `StartCheckout`. Card entry is a Stripe page in the browser (`POST /api/v1/connect/card/checkout-session`). The app never collects a card number.
4. **Billing history.** One row per charge. `provider` is `btcpay` or `stripe`. Older history rows omit `chargeId` and `state`; those still parse, and `ChargeState::resolve` maps the legacy `status`.

`PlanLifecycle` is derived from the plan plus the clock:

- `past_due` with `graceEndsAt` still in the future is `PastDue`. The paid tier stays on while the card issuer retries.
- `past_due` without a future grace end is `Expired`, whatever tier the payload names. The comparison treats that account as Free (`effective_tier`).
- An active paid plan within 7 days of `renewalAt` (`PLAN_RENEWAL_BANNER_DAYS`) is `RenewalDue`, unless the plan is a card that renews itself (`card_auto_renews`). Those skip the renewal reminder.
- A missing or unparseable `renewalAt` on an active paid plan stays `Active`.

Feature bullets are filtered before display (`launch_feature_copy`):

- Any bullet naming duress is dropped.
- Recovery-alert and duress-alert bullets render as included "Recovery alerts".
- A bullet about policies renders as "Coming soon". Connect-enforced spending policies are not sold from this build.
- A "N cubes per network" bullet is dropped. The Cube count is its own row, from the entitlement.
- A trailing "(coming soon)" marker stays Coming soon.

An omitted `cubeLimit` parses as 0. The comparison must not show "0 per network": it uses the plan's own positive limit, then the offline fallback.

## Paying

**Bitcoin.** `POST /api/v1/connect/checkout` with `{ plan, billingCycle }` returns the invoice (`chargeId`, Lightning invoice, on-chain address, sats, fiat, `checkoutUrl`, optional `expiresAt` and `startsAt`). The desktop polls `GET /api/v1/connect/checkout/{chargeId}`.

`CheckoutPhase` follows the charge. `state` (billing v2) wins over the legacy `status` when it is present and known. The states the UI branches on:

| `ChargeState` | Meaning |
|---|---|
| `unpaid` | Invoice is waiting. |
| `partially_paid` | Some sats arrived; the rest is due on the same invoice. |
| `processing` | Seen, waiting for confirmation. |
| `paid` | Applied. An overpayment still activates, with a support note. |
| `expired` | Nothing received. |
| `needs_support` | Money arrived and cannot be applied (paid late, partly paid then expired, or invalid). The plan is unchanged. The row keeps `invoiceId`. |
| `declined` | Card issuer declined. Nothing was charged. |
| `refunded`, `charged_back` | Paid, then reversed. |
| `unknown` | An unrecognized legacy status. The poll still parses. |

A 4xx while creating the invoice is `Failed { refused: true }`. Retrying the same request would fail again (a plan change already scheduled is the example in code), so the view offers Change plan rather than Try again. After poll errors, `PollFailed` resumes the same invoice. "Check again" does not create a second one. Three consecutive poll errors stop the chain.

**Card.** Offered only when `cardPaymentsEnabled` is true. Absent or unloaded features read as off, and Review shows Card as "Coming soon". Stripe Checkout and the card portal (`POST /api/v1/connect/card/portal-session`, update card only) open in the browser. While that page is open the desktop polls `GET /connect/plan` every 5 seconds (`PollCardPlan`), up to `MAX_BROWSER_POLLS` (360, about 30 minutes), then stops with "Still waiting after 30 minutes".

An active card subscription changes plan through `POST /api/v1/connect/plan/change/preview` and `POST /api/v1/connect/plan/change`, not a new bitcoin invoice, when `card_change_route` is true: cards are enabled, the plan is a started card (a cycle is known, and it is not in card grace), and either auto-renew is on or this is an upgrade. Annual on the same tier counts as an upgrade from monthly. With auto-renew on, a bitcoin purchase of a replacement plan is refused. The preview's `prorationDate` is sent back on the change.

`PlanChangeResult.status`:

- `paid` — upgrade applied now
- `requires_action` — finish at `url` (3-D Secure); the desktop polls the plan
- `scheduled` — downgrade at `effectiveAt`

`DELETE /api/v1/connect/plan/change` undoes a scheduled downgrade. `PUT /api/v1/connect/plan/auto-renew` with `{ "enabled": bool }` is the auto-renew switch. Bitcoin purchases do not auto-renew.

A change whose response is lost, a 5xx, or an unknown status becomes `ChangeUnconfirmed`. The plan may or may not have changed. The screen reloads plan and history and says whether that reload landed (`PlanRefresh::Updated` or `Failed`). It does not claim Plan & billing is current before the reload answers.

Phases that still have an invoice, a browser checkout, or an in-flight change are not `leavable`. Leaving Change plan must not drop them.

`purchasingEnabled` hides every purchase path when it is `false`, so the desktop does not send a checkout the API will reject. Until features have loaded, purchasing is treated as off. Once they have loaded, an absent flag means an older backend with purchasing on.

## Plan fetches

Every plan read goes through `plan_fetch_task` (sign-in, after a promo redeem, opening Plan & billing, after a payment). It bumps `plan_seq`. `PlanReloaded` applies only when the session generation matches and `seq` is greater than `applied_plan_seq` **and** the body parsed. A later failed fetch does not erase an older good plan. A slow sign-in response does not overwrite a newer redeem. Card polls take their sequence number when they are sent, before the 5 second wait, so a reload sent after the poll outranks it. If a poll then sees a newer sequence already issued, it fetches the plan once more so the screen settles on the current one.

`get_connect_plan` returning `Err` is delivered as `None`. A parse failure is the same shape as a transport failure: keep the plan you already have.

Opening Plan & billing also requests history, including when the dashboard was reached by a path that had not loaded it yet. History is tagged with the session generation and dropped if the session changed.

A promo or referral code is `POST /api/v1/connect/campaigns/redeem` with `{ "code" }`. The desktop forwards the text and shows the server message. It does not special-case campaigns. On success it refreshes the plan. A code captured at account creation is redeemed after the session exists, and that redeem owns the plan fetch so a pre-redeem response cannot clobber the granted tier. A bad code does not block sign-in; the error stays on the Plan field.

The launcher copies the loaded tier into `GlobalSettings.account_tier` (Free when no plan is loaded). A tier change clears `server_cube_limit` until `get_cube_limits` returns for the current network.

## Cube slots

`GET /api/v1/connect/cubes/limits?network={network}` returns `{ network, currentCount, maxAllowed }`. The launcher stores `maxAllowed` as `server_cube_limit` for the network it asked about. It fetches on sign-in and again when the user switches network, and it clears the cached limit on a network switch so the previous network's number is not reused. A failed fetch leaves the previous limit in place and logs a warning. With no server limit, `AccountTier::cube_limit` applies.

The limit is **per network**. The count is Cubes on this device plus Cubes registered in Connect that are not on this device. Copy names both, because a bare ratio disagrees with the list the user can see. While this device's catalog for that network cannot be counted, the summary says the local count is unknown. The client total used for the gate then counts only the Connect-only Cubes. That check is advisory: Connect still refuses `POST /api/v1/connect/cubes` over the limit, and the Cube remains on this device.

At the limit, Create Cube and Import or restore both close. Existing Cubes keep working, including when the account is already over the limit after a plan change. Estate has no higher plan, so its copy says to delete a Cube on that network and does not say to upgrade. Other tiers say to upgrade or delete. A registration the API refuses with a message containing `Cube limit reached` (the API's "Cube limit reached for this network") uses the same wording and adds that the Cube is saved on this device only and will sync after a slot frees. Other refusal messages are shown verbatim. Matching is on `error.message`, which is all the client keeps from the envelope.

## Constraints that are easy to break

- Do not make a plan-field parse failure fail `ConnectPlan`. That drops the account to Free.
- Do not show a purchase control while `purchasing_enabled()` is false, and do not treat unloaded features as purchasing-on.
- Do not treat an absent `cardPaymentsEnabled` as on. Card details stay on Stripe's page.
- Do not apply a plan response with an older sequence, a mismatched session generation, or a `None` body over a plan you already show.
- Do not create a second bitcoin invoice when one is `AwaitingPayment`, `PayingInBrowser`, `PartlyPaid`, or `Processing`. "Check again" polls the same `chargeId`.
- Do not tell an Estate account to upgrade. Deleting a Cube on that network is the remedy.
- Do not treat a local Cube that failed to register as unsynced-without-a-reason. `cube_sync_errors` is why the list can say the slot is full.
- A `cubeLimit` of 0 on a catalog row is an omitted field, not a plan that allows no Cubes.

Unit coverage is in `account.rs` (sequencing, lifecycle, checkout phases, wire compatibility), `app/view/connect/mod.rs` (board copy), and `home.rs` (per-network counts, Estate limit copy, remote Cubes).
