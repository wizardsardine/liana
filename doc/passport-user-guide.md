# Using Passport with Liana

Liana supports Passport Core and Passport Prime as air-gapped signers. The
workflow uses animated QR codes or microSD and never requires USB, copying an
xpub, or editing a descriptor by hand.

## Add a Passport key

1. On Passport, export a Liana account for the correct Bitcoin network and
   account number. Choose QR or microSD.
2. In Liana's wallet installer, choose **Passport** for the policy key slot.
3. Scan the `crypto-account` QR, or import the exported descriptor-key file.
4. Compare the complete master fingerprint, BIP48 origin, network, and account
   number before confirming.

Repeat this for each Passport used by the policy. The same account key may be
used in mutually exclusive immediate and recovery paths; Liana determines the
threshold and timelock from the completed wallet policy.

## Register the wallet policy

After the complete descriptor is built, Liana shows its eight-character
**Policy checksum**. This checksum—not the wallet name—is the identity users
must compare.

1. In the installer's registration step, select each Passport and scan the
   animated policy QR or export the policy JSON to microSD. Alternatively,
   finish installation and open **Settings → Wallet → Air-gapped signers →
   Register policy**.
2. Review the policy and exact checksum on Passport, then confirm it.
3. Return to Liana and select **Done**. Liana records the registration for the
   current descriptor.

A descriptor change makes the registration stale and requires registration
again.

The registration states mean:

- **Not registered** — Liana has no confirmation that the current policy was
  registered on this signer.
- **Registration completed** — the current policy was registered on Passport.

If the descriptor changes, Liana clears the completed state and requires the
policy to be registered again.

## Verify a receive address

1. Reveal a receive address in Liana and select **Verify**.
2. Select the configured Passport.
3. Show the request as an animated QR or save it to microSD.
4. Passport looks up the registered policy, independently derives the selected
   branch and index, and displays the complete address.
5. Compare the address and return Passport's confirmation by QR or microSD.

Liana accepts the confirmation only when the network, policy identity,
checksum, branch, index, address, and Passport fingerprint all match.

## Sign a transaction

1. Create and review the transaction normally in Liana.
2. Select **Sign**, then the Passport required by the active spending path.
3. Show the `crypto-psbt` animated QR or export the binary PSBT to microSD.
4. Review and sign on Passport.
5. Scan the returned `crypto-psbt` QR or import `signed.psbt`.

If Liana reports that a PSBT exceeds Passport's QR limit, choose microSD on the
same exchange screen. This is a transport-size fallback; it does not change the
transaction or wallet policy.

Liana rejects a returned PSBT if the unsigned transaction changed, an existing
signature disappeared, a new signature is invalid or belongs to an unexpected
key, or the selected Passport did not add a signature. Signer-side metadata
normalization is discarded: Liana keeps its original non-signature PSBT fields
and merges only verified signatures. For multisig wallets, repeat the same flow
with additional signers until the chosen path is complete, then finalize and
broadcast normally.

## Camera and privacy

Liana asks for camera access only while a scanner is open, never writes frames
to disk, and releases the camera on success, cancellation, timeout, or error.
Animated QR codes reveal the public wallet policy or transaction details to
anyone who can see them. Use microSD in environments where displaying those
details is inappropriate.

If camera access is denied or unavailable, use the microSD actions on the same
screen. If a Passport is replaced or restored with a different seed or
passphrase, import its account again and verify the full master fingerprint
and BIP48 xpub before registering the policy. A restored Passport with the same
seed and passphrase can reuse the public account key, but the wallet policy must
still be present on that device; re-register it if Passport reports it missing.
