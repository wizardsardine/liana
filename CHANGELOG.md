# Liana daemon and GUI release notes

## 0.3.1

A patch release for a serious bug fix in the GUI installer.

### Fixes

#### GUI-specific

- Under very specific conditions the GUI installer would not store the mnemonic words corresponding
  to a hot key that was used in the descriptor, nor present it to the user for backup.

## 0.3

A small release which brings some fixes as well as the possibility to use Liana as a "hot wallet".

### Features

- Hot keys: users can now generate and sign with keys that are stored on the device. It is
  recommended to be only used for testing for now.

#### GUI-specific

- It is now possible to use multiple signing device of the same type without having to first connect
  one then the other.

### Fixes

- When used as a daemon the `lianad` process had its PID and logs file mixed up. This is now fixed.
- We fixed the transaction creation sanity check that was overestimating the transaction fee.

#### GUI-specific

- In the installer flow, extended keys are now shared without the `/<0;1>/*` suffix.

## 0.2

The second release of Liana brings various fixes as well as the possibility to use a multisig in
either, or both, of the spending paths.

### Features

- Multisig: we now support descriptors with multiple keys both in the primary (non-timelocked)
  spending path and the recovery (timelocked) path.

#### GUI-specific

- You can now import and update Spend transaction drafts as PSBTs to collaboratively create and sign
  transactions.
- When creating a new descriptor you can now set an alias for each key. Those will be displayed when
  inspecting a transaction's signatories.
- Amounts are now displayed with the sats in bold for better redability.

### Fixes

- We now remove the fixed interpreter and rpath set by GUIX reproducible builds in the `liana-cli`
  ELF binary.
- We now check the `bitcoind` version before trying to import a Miniscript descriptor.
- We now discard unconfirmed incoming payments that were dropped from our mempool.
- **Breaking change**: the first version of Liana mistakenly accepted extended keys without origin
  in descriptors. This meant that unless this extended key was the master extended key of a chain,
  it would not be possible to sign with it (since signing devices need to know the origin). Starting
  from version 2 Liana forces extended keys to contain an origin (of the form `[a1b2c3d4]`) to avoid
  this footgun. This means that existing descriptors might have to be migrated, but it's very likely
  only for test configurations where an xpub wasn't gathered from a signing device (which prepends
  an origin) but generated (probably imported from Coleman's website) and pasted without origin.

#### GUI-specific

- Various wording fixes on the UI.
- Amounts are now updated when moving between steps in the Spend creation flow.
- Coins are now sorted by age when displayed as a list.
- Some flakiness in the connection to a signing device were fixed.
- The descriptor registration on a signing device step in the installer was made clearer.
