# BitBox02 firmware advisory (August 2026) — update your device

On 17 August 2026 BitBox released firmware **9.26.5** ("Dixence") for the
BitBox02, fixing three security issues. Coincube shows a notice next to every
connected BitBox02 running firmware below 9.26.5 so you can find your way here.

Source: [BitBox's release post](https://blog.bitbox.swiss/en/bitbox-08-2026-dixence-update/)
is the authority on what is affected and which firmware fixes it.

## The short version

**Update the firmware. That is the whole job.** BitBox states plainly that
existing wallet seeds are not affected by any of these issues, so — unlike the
[Coldcard advisory](2026-07-coldcard-rng.md) from last month — there is no key
to rotate, no new Vault to build and no funds to move. Plug the device in, open
the BitBoxApp, and update from **Manage device**. The notice in Coincube clears
by itself once the device reports 9.26.5 or later.

BitBox reports no evidence that any of the three issues was ever exploited.

## What was fixed

| Issue | Affected firmware | Fixed in |
|---|---|---|
| Bootloader: malicious firmware could be installed on a genuine device following a successful phishing attack | through `9.26.1` | `9.26.2` ("Oeschinen") |
| Memory corruption: arbitrary code execution on an **uninitialised** device connected to a malicious host | through `9.26.4` | `9.26.5` ("Dixence") |
| Silent Payments: funds could be locked to an unintended payment address | `9.21.0` through `9.26.4` | `9.26.5` ("Dixence") |

Firmware 9.26.5 also carries a set of smaller hardening changes to data
handling, cryptographic operations, input validation and device-state
protection, and fixes an unrelated iOS/iPadOS swapping bug.

Two of the three have carve-outs that depend on which BitBox you own: the
BitBox02 Nova was never affected by the bootloader issue, and the Bitcoin-only
edition was never affected by the memory-corruption issue. Coincube does not
distinguish editions — every BitBox arrives over USB as a plain "BitBox02" —
so it shows one notice below 9.26.5 rather than guessing which issues reach
your particular device. Updating settles all three either way.

## What this means for a Cube

**Your recovery words are unaffected.** No part of this touches how the device
generated your seed, so the key this device holds in your Vault stays exactly as
good as it was.

Three narrower points, in case they apply to you:

- **The memory-corruption issue only concerns a device that has not been set up
  yet.** If you set your BitBox up on a computer you trust, it was never
  reachable. If you set one up on a machine you had reason to distrust and
  before updating, treat that device's seed as suspect and move to a new key —
  the [Coldcard rotation guide](2026-07-coldcard-rng.md#rotating-the-key)
  describes that procedure, and it is the same one.
- **The bootloader issue required you to be phished first** — specifically, into
  installing a counterfeit BitBoxApp and unlocking the device for it. If you
  believe that happened to you, the same applies: the seed on that device should
  be replaced. Firmware from 9.26.2 onward closes the hole regardless.
- **Silent Payments do not reach your Vault.** Coincube never builds a
  silent-payment send with a BitBox key. If the same device signs in other
  wallets that do support them, that is where the issue lives, and updating
  fixes it there too.

A Cube is a multisig wallet, so a single key cannot move funds on its own. That
is worth knowing, but it is not the reason to relax here — the reason is that
this incident does not reach your keys at all.

Coincube's behaviour with a BitBox02 is unchanged: connecting, registering the
descriptor, signing PSBTs and importing xpubs all work exactly as before,
whatever the advisory says about a given device.

## Updating

1. Open the **BitBoxApp** you already have and use its update link, or download
   it from [bitbox.swiss/download](https://bitbox.swiss/download).
2. Plug in the BitBox02 and unlock it.
3. Go to **Manage device** and run the firmware update. Confirm on the device.
4. Check that it now reports **9.26.5** or later.

Next time you connect it to Coincube the advisory notice will be gone. You do
not need to re-register your Vault descriptor, and nothing about the Cube
changes.

### Get the app from the right place

Security announcements attract phishing, and this one describes an attack that
*began* with a counterfeit BitBoxApp — so the way you get the update matters as
much as getting it:

- Use the update link inside the BitBoxApp you already have, or type
  `bitbox.swiss/download` yourself. Do not follow a download link out of an
  email.
- If you want to check for yourself,
  [verify the app signature](https://bitbox.swiss/download) before installing.
- **BitBox will never ask for your recovery words**, and neither will Coincube.
  No genuine firmware update needs them typed anywhere but on the device
  itself. Anyone asking is stealing from you.

## Related

- [Signing devices](../SIGNING_DEVICES.md) — firmware requirements per device.
- [Coldcard firmware advisory (July 2026)](2026-07-coldcard-rng.md) — the
  previous advisory, and the rotation procedure referenced above.
- [BitBox — 08.2026 Dixence update](https://blog.bitbox.swiss/en/bitbox-08-2026-dixence-update/)
  — the vendor's own account of what was fixed.
